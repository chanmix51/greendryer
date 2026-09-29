//! Fixed-capacity history of probe samples from the cabinet.

use heapless::HistoryBuf;

/// One climate sample kept until the rack overwrites it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbeMeasure {
    /// Temperature in degrees Celsius.
    pub temp: f32,
    /// Relative humidity in percent.
    pub hygr: u8,
    /// Vapor-pressure deficit in kPa.
    pub vpd: f32,
    /// Timestamp of the sample.
    pub time: i32,
}

impl ProbeMeasure {
    /// Build a sample from its timestamp, temperature, and relative humidity.
    ///
    /// `temp` is degrees Celsius and `hygr` is relative humidity in percent.
    /// Vapor-pressure deficit, in kPa, is `es(T) * (1 - HR / 100)`, with
    /// saturation vapor pressure `es(T) = 0.6108 * exp(17.27 * T / (T + 237.3))`.
    pub fn new(time: i32, temp: f32, hygr: u8) -> Self {
        Self {
            temp,
            hygr,
            vpd: vapor_pressure_deficit(temp, hygr),
            time,
        }
    }
}

/// Vapor-pressure deficit of one sample, in kPa.
fn vapor_pressure_deficit(temp: f32, hygr: u8) -> f32 {
    let saturation = 0.6108 * libm::expf(17.27 * temp / (temp + 237.3));
    saturation * (1.0 - f32::from(hygr) / 100.0)
}

/// A probe read that could not be stored as a sample.
///
/// A failed write, a failed read, and a bad CRC are the same failure for the
/// rack: the slot becomes `Err(ProbeError)` and the other slots stay put.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeError;

/// One rack slot: empty, a valid sample, or a failed read.
pub type SampleSlot = Option<Result<ProbeMeasure, ProbeError>>;

/// History of the last `N` probe results.
///
/// Backed by a [`HistoryBuf`] (the heapless hist buffer). Every slot starts
/// as `None`. A push fills the next slot and leaves the others in place.
/// Once the rack is full, the next push overwrites the oldest slot.
pub struct SampleRack<const N: usize> {
    slots: HistoryBuf<SampleSlot, N>,
}

impl<const N: usize> SampleRack<N> {
    /// An empty rack: every slot is `None`.
    pub fn new() -> Self {
        const {
            assert!(N > 0, "a sample rack needs at least one slot");
        }
        Self {
            slots: HistoryBuf::new_with(None),
        }
    }

    /// Store `sample` in the next slot.
    ///
    /// Slots that already hold a sample stay as they are. After `N` pushes,
    /// the next push overwrites the oldest slot.
    pub fn push(&mut self, sample: Result<ProbeMeasure, ProbeError>) {
        self.slots.write(Some(sample));
    }

    /// Slots in write order. Index 0 is the first slot filled.
    pub fn slots(&self) -> &[SampleSlot] {
        self.slots.as_slice()
    }

    /// At least one slot is still `None`.
    pub fn is_incomplete(&self) -> bool {
        self.slots().iter().any(Option::is_none)
    }

    /// Every slot holds `Ok`.
    pub fn is_healthy(&self) -> bool {
        self.slots().iter().all(|slot| matches!(slot, Some(Ok(_))))
    }

    /// At least one slot is `Err` and at least one slot is not.
    pub fn is_sick(&self) -> bool {
        let mut errors = 0;
        let mut others = 0;
        for slot in self.slots() {
            if matches!(slot, Some(Err(_))) {
                errors += 1;
            } else {
                others += 1;
            }
        }
        errors > 0 && others > 0
    }

    /// Every slot is `Err`.
    pub fn is_dead(&self) -> bool {
        self.slots().iter().all(|slot| matches!(slot, Some(Err(_))))
    }

    /// True only when all `N` slots are valid samples and each satisfies `pred`.
    ///
    /// A missing slot or a failed read rejects the threshold.
    pub fn meets(&self, mut pred: impl FnMut(&ProbeMeasure) -> bool) -> bool {
        self.slots().iter().all(|slot| match slot {
            Some(Ok(sample)) => pred(sample),
            _ => false,
        })
    }

    /// One mark per slot, separated by a single space.
    ///
    /// `*` is a valid sample, `?` is missing, and `!` is a failed read.
    /// The string length is `2N - 1`. `L` must be at least that long.
    pub fn glyph_line<const L: usize>(&self) -> heapless::String<L> {
        let mut line = heapless::String::new();
        for (index, slot) in self.slots().iter().enumerate() {
            if index > 0 {
                line.push(' ')
                    .expect("glyph line capacity must be at least 2N - 1");
            }
            let mark = match slot {
                Some(Ok(_)) => '*',
                None => '?',
                Some(Err(_)) => '!',
            };
            line.push(mark)
                .expect("glyph line capacity must be at least 2N - 1");
        }
        line
    }
}

impl<const N: usize> Default for SampleRack<N> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{ProbeError, ProbeMeasure, SampleRack};

    const N: usize = 3;

    fn measure(temp: f32, hygr: u8) -> ProbeMeasure {
        ProbeMeasure::new(1, temp, hygr)
    }

    fn assert_vpd(actual: f32, expected: f32) {
        let difference = (actual - expected).abs();
        assert!(
            difference < 1.0e-4,
            "VPD {actual} differs from {expected} by {difference}"
        );
    }

    #[test]
    fn probe_measure_stores_time_temperature_and_humidity() {
        let sample = ProbeMeasure::new(42, 19.0, 73);

        assert_eq!(sample.time, 42);
        assert_eq!(sample.temp, 19.0);
        assert_eq!(sample.hygr, 73);
    }

    #[test]
    fn probe_measure_derives_vpd_from_temperature_and_humidity() {
        assert_vpd(ProbeMeasure::new(1, 20.0, 50).vpd, 1.1691406355);
        assert_vpd(ProbeMeasure::new(2, 19.0, 73).vpd, 0.5932961974);
        assert_vpd(ProbeMeasure::new(3, 18.0, 0).vpd, 2.0639892027);
        assert_eq!(ProbeMeasure::new(4, 15.0, 100).vpd, 0.0);
    }

    #[test]
    fn probe_measure_computes_each_sample_on_its_own() {
        let cool = ProbeMeasure::new(10, 15.0, 40);
        let warm = ProbeMeasure::new(11, 25.0, 80);
        let averaged = ProbeMeasure::new(12, 20.0, 60);

        assert_vpd(cool.vpd, 1.0232077393);
        assert_vpd(warm.vpd, 0.6335555435);
        assert_vpd(averaged.vpd, 0.9353125084);

        let mean_of_deficits = (cool.vpd + warm.vpd) / 2.0;
        assert!((mean_of_deficits - averaged.vpd).abs() > 1.0e-3);
    }

    fn ok(temp: f32, hygr: u8) -> Result<ProbeMeasure, ProbeError> {
        Ok(measure(temp, hygr))
    }

    #[test]
    fn empty_rack_is_incomplete_with_every_slot_none() {
        let rack = SampleRack::<N>::new();

        assert!(rack.is_incomplete());
        assert!(!rack.is_healthy());
        assert!(!rack.is_sick());
        assert!(!rack.is_dead());
        assert_eq!(rack.slots().len(), N);
        assert!(rack.slots().iter().all(Option::is_none));
        assert_eq!(rack.glyph_line::<{ 2 * N - 1 }>().as_str(), "? ? ?");
    }

    #[test]
    fn push_ok_fills_the_next_slot_and_leaves_the_others() {
        let mut rack = SampleRack::<N>::new();
        let first = ok(18.0, 60);
        let second = ok(18.5, 61);

        rack.push(first);
        assert_eq!(rack.slots(), &[Some(first), None, None]);

        rack.push(second);
        assert_eq!(rack.slots(), &[Some(first), Some(second), None]);
    }

    #[test]
    fn push_err_fills_the_next_slot_and_leaves_the_others() {
        let mut rack = SampleRack::<N>::new();
        let first = ok(18.0, 60);

        rack.push(first);
        rack.push(Err(ProbeError));

        assert_eq!(rack.slots(), &[Some(first), Some(Err(ProbeError)), None]);
    }

    #[test]
    fn push_past_capacity_overwrites_only_the_oldest_slot() {
        let mut rack = SampleRack::<N>::new();
        let first = ok(18.0, 50);
        let failed = Err(ProbeError);
        let third = ok(17.0, 55);
        let newest = ok(16.0, 40);

        rack.push(first);
        rack.push(failed);
        rack.push(third);
        assert_eq!(rack.slots(), &[Some(first), Some(failed), Some(third)]);

        rack.push(newest);
        assert_eq!(rack.slots(), &[Some(newest), Some(failed), Some(third)]);
    }

    #[test]
    fn healthy_only_when_every_slot_is_ok() {
        let mut partial = SampleRack::<N>::new();
        partial.push(ok(18.0, 60));
        partial.push(ok(18.0, 60));
        assert!(!partial.is_healthy());

        let mut mixed = SampleRack::<N>::new();
        mixed.push(ok(18.0, 60));
        mixed.push(Err(ProbeError));
        mixed.push(ok(18.0, 60));
        assert!(!mixed.is_healthy());

        let mut healthy = SampleRack::<N>::new();
        healthy.push(ok(18.0, 60));
        healthy.push(ok(19.0, 61));
        healthy.push(ok(17.5, 59));
        assert!(healthy.is_healthy());
        assert!(!healthy.is_sick());
        assert!(!healthy.is_dead());
        assert!(!healthy.is_incomplete());
    }

    #[test]
    fn sick_when_one_error_and_one_other_slot() {
        let mut with_ok = SampleRack::<N>::new();
        with_ok.push(ok(18.0, 60));
        with_ok.push(Err(ProbeError));
        with_ok.push(ok(18.0, 62));
        assert!(with_ok.is_sick());
        assert!(!with_ok.is_healthy());
        assert!(!with_ok.is_dead());
        assert!(!with_ok.is_incomplete());

        let mut with_empty = SampleRack::<N>::new();
        with_empty.push(Err(ProbeError));
        assert!(with_empty.is_sick());
        assert!(with_empty.is_incomplete());
        assert!(!with_empty.is_dead());

        let mut only_ok_and_empty = SampleRack::<N>::new();
        only_ok_and_empty.push(ok(18.0, 60));
        assert!(!only_ok_and_empty.is_sick());
    }

    #[test]
    fn dead_when_every_slot_is_err() {
        let mut rack = SampleRack::<N>::new();
        rack.push(Err(ProbeError));
        rack.push(Err(ProbeError));
        assert!(!rack.is_dead());

        rack.push(Err(ProbeError));
        assert!(rack.is_dead());
        assert!(!rack.is_sick());
        assert!(!rack.is_healthy());
        assert!(!rack.is_incomplete());
    }

    #[test]
    fn single_error_slot_is_dead_not_sick() {
        let mut rack = SampleRack::<1>::new();
        assert!(rack.is_incomplete());
        assert!(!rack.is_sick());

        rack.push(Err(ProbeError));
        assert!(rack.is_dead());
        assert!(!rack.is_sick());
        assert!(!rack.is_healthy());
    }

    #[test]
    fn threshold_is_met_only_when_every_slot_satisfies_it() {
        let hot = ok(20.0, 50);
        let cool = ok(19.0, 50);

        let mut met = SampleRack::<N>::new();
        met.push(hot);
        met.push(hot);
        met.push(hot);
        assert!(met.meets(|sample| sample.temp > 19.0));

        let mut missed = SampleRack::<N>::new();
        missed.push(hot);
        missed.push(hot);
        missed.push(cool);
        assert!(!missed.meets(|sample| sample.temp > 19.0));
    }

    #[test]
    fn missing_or_failed_slot_rejects_the_threshold() {
        let humid = ok(18.0, 72);

        let mut incomplete = SampleRack::<N>::new();
        incomplete.push(humid);
        incomplete.push(humid);
        assert!(!incomplete.meets(|sample| sample.hygr >= 70));

        let mut sick = SampleRack::<N>::new();
        sick.push(humid);
        sick.push(humid);
        sick.push(Err(ProbeError));
        assert!(!sick.meets(|sample| sample.hygr >= 70));
    }

    #[test]
    fn glyph_line_marks_each_slot_with_single_spaces() {
        let mut rack = SampleRack::<N>::new();
        assert_eq!(rack.glyph_line::<{ 2 * N - 1 }>().as_str(), "? ? ?");
        assert_eq!(rack.glyph_line::<{ 2 * N - 1 }>().len(), 2 * N - 1);

        rack.push(ok(18.0, 60));
        assert_eq!(rack.glyph_line::<{ 2 * N - 1 }>().as_str(), "* ? ?");

        rack.push(Err(ProbeError));
        assert_eq!(rack.glyph_line::<{ 2 * N - 1 }>().as_str(), "* ! ?");

        rack.push(ok(18.0, 60));
        assert_eq!(rack.glyph_line::<{ 2 * N - 1 }>().as_str(), "* ! *");
        assert_eq!(rack.glyph_line::<{ 2 * N - 1 }>().len(), 2 * N - 1);

        let one = SampleRack::<1>::new();
        assert_eq!(one.glyph_line::<1>().as_str(), "?");
        assert_eq!(one.glyph_line::<1>().len(), 1);
    }
}
