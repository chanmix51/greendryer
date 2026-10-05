//! Cabinet policies for the vegetal material.
//!
//! Each policy is a struct that implements [`ApplicationMode`]. The actor
//! calls `init` when the policy changes and `exec` on every sample. The
//! decision reads the sample rack, compressor rest, and the time already
//! spent in the mode. It does not read a probe, a pin, or a panel.
//!
//! `exec` may switch [`AppMode`](crate::AppMode). It does not call the next
//! policy. The actor sees the new mode, calls that policy's `init`, then
//! `exec`, and repeats while the mode keeps changing. One sample may cross
//! several modes. The longest chains are three modes, such as `WAIT_PROBE`
//! → `NOMINAL` → `COMPRESSOR_WAIT`, `COMPRESSOR_WAIT` → `NOMINAL` →
//! `VPD_REGULATION`, and a deadline recovery that continues out of `NOMINAL`.
//! A dead rack is switched to [`WaitProbeMode`] by the
//! actor before `exec`, so a fridge that was on starts compressor rest on
//! that sample. `WAIT_PROBE` shows the glyph line, not that rest.
//!
//! `now_s` is the current whole second. [`AppState`](crate::AppState) stores
//! the mode-entry timestamp and the last fridge on-to-off edge. It does not
//! store a clock, so each call receives the time.

mod compressor_wait;
mod failure_stop;
mod hr_enforcing;
mod nominal;
mod temp_enforcing;
mod vpd_regulation;
mod wait_probe;

pub use compressor_wait::CompressorWaitMode;
pub use failure_stop::FailureStopMode;
pub use hr_enforcing::HrEnforcingMode;
pub use nominal::NominalMode;
pub use temp_enforcing::TempEnforcingMode;
pub use vpd_regulation::VpdRegulationMode;
pub use wait_probe::WaitProbeMode;

use core::fmt::Write;

use crate::{AppState, CompressorRestError};

/// Relative humidity, in percent, at or above which `HR_ENFORCING` starts.
pub const HR_EMERGENCY_PERCENT: u8 = 70;

/// Humidity threshold, in percent, shared by two rules.
///
/// All N samples at or below this leave `HR_ENFORCING`. All N samples at or
/// above this latch the dehumidifier on for the rest of `TEMP_ENFORCING`.
pub const HR_CLEAR_PERCENT: u8 = 65;

/// Temperature, in degrees Celsius, above which cooling may start.
pub const TEMP_START_C: f32 = 19.0;

/// Temperature, in degrees Celsius, at or below which cooling is finished.
pub const TEMP_DONE_C: f32 = 15.0;

/// Vapor-pressure deficit, in kPa, above which the cabinet is too dry to leave closed.
pub const VPD_HIGH_KPA: f32 = 0.8;

/// Vapor-pressure deficit, in kPa, under which the dehumidifier must run.
pub const VPD_LOW_KPA: f32 = 0.6;

/// Seconds from entry after which `TEMP_ENFORCING` recovers or stops.
///
/// A healthy rack under 19°C and under 0.8 kPa returns to `NOMINAL` and logs
/// a warning. Any other healthy rack becomes `FAILURE_STOP`.
pub const TEMP_ENFORCING_LIMIT_S: u32 = 20 * 60;

/// Seconds from entry after which `HR_ENFORCING` recovers or stops.
///
/// A healthy rack under 70% humidity and above 0.6 kPa returns to `NOMINAL`
/// and logs a warning. Any other healthy rack becomes `FAILURE_STOP`.
pub const HR_ENFORCING_LIMIT_S: u32 = 30 * 60;

/// A mode could not update the published cabinet state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppModError {
    /// The fridge was commanded on while compressor rest was still active.
    CompressorRest,
    /// The status line did not fit in the published buffer.
    StatusLine,
    /// One sample switched policies more times than the actor follows.
    Cascade,
}

impl From<CompressorRestError> for AppModError {
    fn from(_error: CompressorRestError) -> Self {
        Self::CompressorRest
    }
}

impl From<heapless::CapacityError> for AppModError {
    fn from(_error: heapless::CapacityError) -> Self {
        Self::StatusLine
    }
}

impl From<core::fmt::Error> for AppModError {
    fn from(_error: core::fmt::Error) -> Self {
        Self::StatusLine
    }
}

/// One cabinet policy.
///
/// `init` runs on entry. `exec` runs once per sample after that.
pub trait ApplicationMode<const N: usize, const L: usize> {
    /// Apply the relays this policy holds from the moment it is entered.
    fn init(state: &mut AppState<N, L>, now_s: u32) -> Result<Self, AppModError>
    where
        Self: Sized;

    /// Apply this policy's rules to the current samples.
    fn exec(&self, state: &mut AppState<N, L>, now_s: u32) -> Result<(), AppModError>;
}

/// Publish one mark per rack slot.
pub(crate) fn publish_glyphs<const N: usize, const L: usize>(
    state: &mut AppState<N, L>,
) -> Result<(), AppModError> {
    let glyphs = state.sample_rack().glyph_line::<L>();
    state.set_status_line(glyphs.as_str())?;
    Ok(())
}

/// Sick or incomplete racks freeze the caller.
///
/// Publishes the glyph line and reports that `exec` must return without
/// changing the mode or the relays. A dead rack is not frozen here.
pub(crate) fn held_for_probe_gaps<const N: usize, const L: usize>(
    state: &mut AppState<N, L>,
) -> Result<bool, AppModError> {
    if !(state.sample_rack().is_sick() || state.sample_rack().is_incomplete()) {
        return Ok(false);
    }
    publish_glyphs(state)?;
    Ok(true)
}

/// `true` when sample rules must not run.
///
/// Sick and incomplete racks publish the glyph line. A dead rack publishes
/// nothing: the actor enters [`WaitProbeMode`] before `exec`.
pub(crate) fn block_sample_rules<const N: usize, const L: usize>(
    state: &mut AppState<N, L>,
) -> Result<bool, AppModError> {
    Ok(held_for_probe_gaps(state)? || state.sample_rack().is_dead())
}

pub(crate) fn command_relays<const N: usize, const L: usize>(
    state: &mut AppState<N, L>,
    fridge_on: bool,
    dehumidifier_on: bool,
    now_s: u32,
) -> Result<(), AppModError> {
    state.set_relays(fridge_on, dehumidifier_on, now_s)?;
    Ok(())
}

/// Turn the fridge off and leave the dehumidifier as it is.
///
/// An on-to-off edge starts compressor rest. Commanding it off while it is
/// already off does not move that edge.
pub(crate) fn stop_fridge<const N: usize, const L: usize>(
    state: &mut AppState<N, L>,
    now_s: u32,
) -> Result<(), AppModError> {
    let dehumidifier_on = state.relay_bank().sw2();
    command_relays(state, false, dehumidifier_on, now_s)
}

pub(crate) fn publish_elapsed<const N: usize, const L: usize>(
    state: &mut AppState<N, L>,
    now_s: u32,
) -> Result<(), AppModError> {
    let elapsed = state.mode_elapsed_s(now_s);
    let mut line = heapless::String::<L>::new();
    write!(line, "{elapsed}s")?;
    state.set_status_line(line.as_str())?;
    Ok(())
}

/// Clear the status line.
pub(crate) fn clear_status<const N: usize, const L: usize>(
    state: &mut AppState<N, L>,
) -> Result<(), AppModError> {
    state.set_status_line("")?;
    Ok(())
}

/// `reste {S}s`, with S the whole seconds of compressor rest still remaining.
pub(crate) fn publish_rest<const N: usize, const L: usize>(
    state: &mut AppState<N, L>,
    now_s: u32,
) -> Result<(), AppModError> {
    let remaining = state.rest_remaining_s(now_s);
    let mut line = heapless::String::<L>::new();
    write!(line, "reste {remaining}s")?;
    state.set_status_line(line.as_str())?;
    Ok(())
}

/// Both actuators off, then an empty status line.
pub(crate) fn settle_nominal<const N: usize, const L: usize>(
    state: &mut AppState<N, L>,
    now_s: u32,
) -> Result<(), AppModError> {
    command_relays(state, false, false, now_s)?;
    clear_status(state)
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::{AppMode, AppState, ProbeError, ProbeMeasure};

    use super::{
        ApplicationMode, HR_CLEAR_PERCENT, HR_EMERGENCY_PERCENT, TEMP_START_C, VPD_HIGH_KPA,
        VPD_LOW_KPA,
    };

    pub const N: usize = 3;
    pub const L: usize = 17;

    pub fn fresh() -> AppState<N, L> {
        AppState::new()
    }

    pub fn hot_dry() -> ProbeMeasure {
        let sample = ProbeMeasure::new(0, 20.0, 50);
        assert!(sample.temp > TEMP_START_C);
        assert!(sample.vpd > VPD_HIGH_KPA);
        assert!(sample.hygr < HR_EMERGENCY_PERCENT);
        sample
    }

    pub fn hot_humid() -> ProbeMeasure {
        // Hot enough, and dry enough in vapor-pressure deficit, that cooling
        // would also start. Humidity is tested first.
        let sample = ProbeMeasure::new(0, 25.0, 70);
        assert!(sample.hygr >= HR_EMERGENCY_PERCENT);
        assert!(sample.temp > TEMP_START_C && sample.vpd > VPD_HIGH_KPA);
        sample
    }

    pub fn in_band() -> ProbeMeasure {
        let sample = ProbeMeasure::new(0, 20.0, 66);
        assert!(sample.hygr < HR_EMERGENCY_PERCENT);
        assert!(sample.hygr > HR_CLEAR_PERCENT);
        assert!(!(sample.temp > TEMP_START_C && sample.vpd > VPD_HIGH_KPA));
        assert!(sample.vpd >= VPD_LOW_KPA);
        sample
    }

    pub fn cool_dry() -> ProbeMeasure {
        let sample = ProbeMeasure::new(0, 18.0, 40);
        assert!(sample.temp <= TEMP_START_C);
        assert!(sample.vpd > VPD_HIGH_KPA);
        assert!(sample.hygr < HR_EMERGENCY_PERCENT);
        sample
    }

    pub fn low_vpd() -> ProbeMeasure {
        let sample = ProbeMeasure::new(0, 15.0, 68);
        assert!(sample.hygr < HR_EMERGENCY_PERCENT);
        assert!(sample.temp <= TEMP_START_C);
        assert!(sample.vpd < VPD_LOW_KPA);
        sample
    }

    pub fn cooled() -> ProbeMeasure {
        let sample = ProbeMeasure::new(0, 15.0, 50);
        assert!(sample.temp <= 15.0);
        assert!(sample.hygr < HR_EMERGENCY_PERCENT);
        assert!(sample.vpd >= VPD_LOW_KPA);
        sample
    }

    /// Humidity under 70% and above 65%, VPD inside (0.6, 0.8), temperature under 19°C.
    ///
    /// A deadline recovery from either enforcing mode returns to `NOMINAL` and stays there.
    pub fn deadline_recovered() -> ProbeMeasure {
        let sample = ProbeMeasure::new(0, 18.0, 66);
        assert!(sample.hygr > HR_CLEAR_PERCENT);
        assert!(sample.hygr < HR_EMERGENCY_PERCENT);
        assert!(sample.vpd > VPD_LOW_KPA);
        assert!(sample.vpd < VPD_HIGH_KPA);
        assert!(sample.temp < TEMP_START_C);
        assert!(sample.temp > 15.0);
        sample
    }

    /// Still hot and dry, with humidity under 70% and above 65%.
    ///
    /// An `HR_ENFORCING` deadline recovery continues into cooling.
    pub fn recovered_hot_dry() -> ProbeMeasure {
        let sample = ProbeMeasure::new(0, 25.0, 66);
        assert!(sample.hygr > HR_CLEAR_PERCENT);
        assert!(sample.hygr < HR_EMERGENCY_PERCENT);
        assert!(sample.temp > TEMP_START_C);
        assert!(sample.vpd > VPD_HIGH_KPA);
        sample
    }

    /// Above 15°C, under 19°C, VPD under 0.6, humidity under 70% and above 65%.
    ///
    /// A `TEMP_ENFORCING` deadline recovery continues into `VPD_REGULATION`.
    pub fn recovered_low_vpd() -> ProbeMeasure {
        let sample = ProbeMeasure::new(0, 16.2, 68);
        assert!(sample.temp > 15.0);
        assert!(sample.temp < TEMP_START_C);
        assert!(sample.hygr > HR_CLEAR_PERCENT);
        assert!(sample.hygr < HR_EMERGENCY_PERCENT);
        assert!(sample.vpd < VPD_LOW_KPA);
        sample
    }

    /// Install a logger and drop warnings recorded on this thread.
    pub fn capture_warnings() {
        install_warning_log();
        WARNINGS.with(|warnings| warnings.borrow_mut().clear());
    }

    /// The warnings recorded on this thread since [`capture_warnings`].
    pub fn assert_warnings(expected: &[&str]) {
        let got = WARNINGS.with(|warnings| warnings.borrow().clone());
        let texts: std::vec::Vec<&str> = got.iter().map(|line| line.as_str()).collect();
        assert_eq!(texts.as_slice(), expected);
    }

    pub fn fill(state: &mut AppState<N, L>, sample: ProbeMeasure) {
        for _ in 0..N {
            state.sample_rack_mut().push(Ok(sample));
        }
    }

    pub fn fill_with_one_difference(
        state: &mut AppState<N, L>,
        majority: ProbeMeasure,
        outlier: ProbeMeasure,
    ) {
        for _ in 0..N - 1 {
            state.sample_rack_mut().push(Ok(majority));
        }
        state.sample_rack_mut().push(Ok(outlier));
    }

    pub fn kill(state: &mut AppState<N, L>) {
        for _ in 0..N {
            state.sample_rack_mut().push(Err(ProbeError));
        }
        assert!(state.sample_rack().is_dead());
    }

    /// Switch to `mode` at `now_s` and run its `init`.
    pub fn enter<M>(state: &mut AppState<N, L>, mode: AppMode, now_s: u32) -> M
    where
        M: ApplicationMode<N, L>,
    {
        if state.app_mode() == mode {
            let other = if mode == AppMode::Nominal {
                AppMode::WaitProbe
            } else {
                AppMode::Nominal
            };
            state.switch_mode(other, now_s.wrapping_sub(1));
        }
        state.switch_mode(mode, now_s);
        M::init(state, now_s).expect("mode entry")
    }

    /// Start compressor rest with a fridge on-to-off edge at `at_s`.
    pub fn start_rest(state: &mut AppState<N, L>, at_s: u32) {
        state
            .set_relays(true, false, at_s.saturating_sub(1))
            .unwrap();
        state.set_relays(false, false, at_s).unwrap();
        assert_eq!(state.compressor_last_stop_s(), Some(at_s));
        assert!(state.rest_active(at_s));
    }

    pub fn assert_spares_off(state: &AppState<N, L>) {
        assert!(!state.relay_bank().sw3());
        assert!(!state.relay_bank().sw4());
    }

    std::thread_local! {
        static WARNINGS: std::cell::RefCell<std::vec::Vec<heapless::String<64>>> =
            const { std::cell::RefCell::new(std::vec::Vec::new()) };
    }

    struct WarningLog;

    impl log::Log for WarningLog {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.level() <= log::Level::Warn
        }

        fn log(&self, record: &log::Record<'_>) {
            if record.level() == log::Level::Warn {
                let mut line = heapless::String::new();
                let _ = core::fmt::Write::write_fmt(&mut line, *record.args());
                WARNINGS.with(|warnings| warnings.borrow_mut().push(line));
            }
        }

        fn flush(&self) {}
    }

    fn install_warning_log() {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        static LOGGER: WarningLog = WarningLog;
        INSTALL.call_once(|| {
            log::set_logger(&LOGGER).expect("install the test warning log");
            log::set_max_level(log::LevelFilter::Warn);
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppMode, ProbeMeasure};

    use super::test_support;
    use super::{
        ApplicationMode, FailureStopMode, HrEnforcingMode, NominalMode, TEMP_ENFORCING_LIMIT_S,
        TempEnforcingMode, VpdRegulationMode, WaitProbeMode,
    };

    #[test]
    fn a_hot_dry_rack_reaches_temp_enforcing_on_the_sample_that_completes_it() {
        let mut state = test_support::fresh();
        let waiting = WaitProbeMode::init(&mut state, 0).unwrap();
        waiting.exec(&mut state, 500).unwrap();
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert!(!state.relay_bank().sw1());
        assert!(!state.rest_active(500));

        test_support::fill(&mut state, test_support::hot_dry());
        waiting.exec(&mut state, 500).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.mode_since_s(), 500);

        let nominal = NominalMode::init(&mut state, 500).unwrap();
        nominal.exec(&mut state, 500).unwrap();
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_eq!(state.mode_since_s(), 500);

        let cooling = TempEnforcingMode::init(&mut state, 500).unwrap();
        assert!(state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        cooling
            .exec(&mut state, 500 + TEMP_ENFORCING_LIMIT_S - 1)
            .unwrap();
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);

        cooling
            .exec(&mut state, 500 + TEMP_ENFORCING_LIMIT_S)
            .unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_eq!(state.mode_since_s(), 500 + TEMP_ENFORCING_LIMIT_S);
    }

    #[test]
    fn humidity_outranks_cooling_on_the_sample_that_completes_the_rack() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::hot_humid());

        let waiting = WaitProbeMode::init(&mut state, 10).unwrap();
        waiting.exec(&mut state, 10).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);

        let nominal = NominalMode::init(&mut state, 10).unwrap();
        nominal.exec(&mut state, 10).unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_ne!(state.app_mode(), AppMode::TempEnforcing);

        let drying = HrEnforcingMode::init(&mut state, 10).unwrap();
        assert!(!state.relay_bank().sw1());
        assert!(state.relay_bank().sw2());
        drying.exec(&mut state, 10).unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
    }

    #[test]
    fn a_dead_rack_returns_to_nominal_and_does_not_resume_the_interrupted_mode() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::hot_dry());
        let _cooling =
            test_support::enter::<TempEnforcingMode>(&mut state, AppMode::TempEnforcing, 20);
        assert!(state.relay_bank().sw1());

        test_support::kill(&mut state);
        let now = 40;
        state.switch_mode(AppMode::WaitProbe, now);
        let waiting = WaitProbeMode::init(&mut state, now).unwrap();
        assert!(!state.relay_bank().sw1());
        assert_eq!(state.compressor_last_stop_s(), Some(now));
        waiting.exec(&mut state, now).unwrap();
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.status_line(), "! ! !");

        test_support::fill(&mut state, test_support::in_band());
        waiting.exec(&mut state, now + 3).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_ne!(state.app_mode(), AppMode::TempEnforcing);

        let nominal = NominalMode::init(&mut state, now + 3).unwrap();
        nominal.exec(&mut state, now + 3).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_eq!(state.status_line(), "");
        assert!(state.rest_active(now + 3));
    }

    #[test]
    fn failure_stop_has_no_outgoing_transition() {
        let mut state = test_support::fresh();
        let stopped = test_support::enter::<FailureStopMode>(&mut state, AppMode::FailureStop, 5);
        for sample in [
            test_support::hot_dry(),
            test_support::hot_humid(),
            test_support::low_vpd(),
            test_support::in_band(),
        ] {
            test_support::fill(&mut state, sample);
            stopped.exec(&mut state, 5_000).unwrap();
            assert_eq!(state.app_mode(), AppMode::FailureStop);
            assert!(!state.relay_bank().sw1());
            assert!(!state.relay_bank().sw2());
        }

        test_support::kill(&mut state);
        stopped.exec(&mut state, 9_000).unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_eq!(state.status_line(), "8995s");
        assert_ne!(state.app_mode(), AppMode::WaitProbe);
    }

    #[test]
    fn vpd_regulation_does_not_time_out() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::low_vpd());
        let drying =
            test_support::enter::<VpdRegulationMode>(&mut state, AppMode::VpdRegulation, 0);
        drying.exec(&mut state, 86_400).unwrap();
        assert_eq!(state.app_mode(), AppMode::VpdRegulation);
        assert!(state.relay_bank().sw2());
        assert!(!state.relay_bank().sw1());
        assert_eq!(state.status_line(), "86400s");
    }

    #[test]
    fn one_slot_that_misses_a_threshold_leaves_nominal_unchanged() {
        let mut state = test_support::fresh();
        let hot = test_support::hot_dry();
        let cool = ProbeMeasure {
            temp: 19.0,
            hygr: hot.hygr,
            vpd: hot.vpd,
            time: 0,
        };
        test_support::fill_with_one_difference(&mut state, hot, cool);
        assert!(!state.sample_rack().meets(|sample| sample.temp > 19.0));

        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);
        nominal.exec(&mut state, 0).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert!(!state.relay_bank().sw1());
        assert_eq!(state.status_line(), "");
    }
}
