//! The cabinet controller's sample loop.
//!
//! [`MainActor`] reads one probe result, pushes it into the rack, and runs
//! the active policy. A dead rack enters [`WaitProbeMode`] before that
//! policy's `exec`, except from [`AppMode::FailureStop`]. When `exec`
//! switches mode, the new policy's `init` and `exec` run in the same step.
//! The logical relay bank is then applied to the pins.

use core::fmt::Write;
use core::future::Future;

use crate::modes::{
    AppModError, ApplicationMode, CompressorWaitMode, FailureStopMode, HrEnforcingMode,
    NominalMode, TempEnforcingMode, VpdRegulationMode, WaitProbeMode,
};
use crate::{AppMode, AppState, ProbeError, ProbeMeasure, RelayBank, RelayOutput};

/// How many policies one sample may enter, including the policy already active.
///
/// The longest chains are three modes, such as `WAIT_PROBE` → `NOMINAL` →
/// `COMPRESSOR_WAIT` and `COMPRESSOR_WAIT` → `NOMINAL` → `VPD_REGULATION`.
const MAX_POLICY_PASSES: usize = 4;

/// One climate sample from the probe.
pub trait ClimateProbe {
    /// Read the probe once.
    ///
    /// A failed write, a failed read, and a bad CRC are all [`ProbeError`].
    fn read(&mut self) -> impl Future<Output = Result<ProbeMeasure, ProbeError>>;
}

/// Reads the probe, decides the policy, and applies the relays.
///
/// `P` is the probe and `R` is the pin adapter. The published [`AppState`]
/// stays with the caller. `now_s` is supplied on each step because the state
/// does not keep a clock.
pub struct MainActor<P, R> {
    probe: P,
    relays: R,
    /// Policy whose `init` has already run. Empty until the first step.
    active: Option<AppMode>,
}

impl<P, R> MainActor<P, R> {
    /// A new actor. No policy has been entered yet.
    pub const fn new(probe: P, relays: R) -> Self {
        Self {
            probe,
            relays,
            active: None,
        }
    }

    /// The probe this actor reads.
    pub const fn probe(&self) -> &P {
        &self.probe
    }

    /// The pin adapter this actor commands.
    pub const fn relays(&self) -> &R {
        &self.relays
    }
}

impl<P, R> MainActor<P, R>
where
    P: ClimateProbe,
    R: RelayOutput,
{
    /// Take one sample and publish the next relay commands.
    ///
    /// The probe result is stored even when it is an error. `FAILURE_STOP`
    /// ignores a dead rack. Every other policy is left for [`WaitProbeMode`]
    /// when every slot is [`ProbeError`], and that entry turns the fridge off
    /// when it was actually on.
    pub async fn step<const N: usize, const L: usize>(
        &mut self,
        state: &mut AppState<N, L>,
        now_s: u32,
    ) -> Result<(), AppModError> {
        let sample = self.probe.read().await;
        state.sample_rack_mut().push(sample);

        if state.app_mode() != AppMode::FailureStop && state.sample_rack().is_dead() {
            state.switch_mode(AppMode::WaitProbe, now_s);
        }

        for _ in 0..MAX_POLICY_PASSES {
            let mode = state.app_mode();
            if self.active != Some(mode) {
                init_mode(mode, state, now_s)?;
                self.active = Some(mode);
            }
            exec_mode(mode, state, now_s)?;
            if state.app_mode() == mode {
                let bank = state.relay_bank();
                bank.apply_to(&mut self.relays);
                log_sample(sample, state, bank, now_s);
                return Ok(());
            }
        }

        Err(AppModError::Cascade)
    }
}

fn init_mode<const N: usize, const L: usize>(
    mode: AppMode,
    state: &mut AppState<N, L>,
    now_s: u32,
) -> Result<(), AppModError> {
    match mode {
        AppMode::WaitProbe => {
            WaitProbeMode::init(state, now_s)?;
        }
        AppMode::Nominal => {
            NominalMode::init(state, now_s)?;
        }
        AppMode::CompressorWait => {
            CompressorWaitMode::init(state, now_s)?;
        }
        AppMode::VpdRegulation => {
            VpdRegulationMode::init(state, now_s)?;
        }
        AppMode::TempEnforcing => {
            TempEnforcingMode::init(state, now_s)?;
        }
        AppMode::HrEnforcing => {
            HrEnforcingMode::init(state, now_s)?;
        }
        AppMode::FailureStop => {
            FailureStopMode::init(state, now_s)?;
        }
    }
    Ok(())
}

fn exec_mode<const N: usize, const L: usize>(
    mode: AppMode,
    state: &mut AppState<N, L>,
    now_s: u32,
) -> Result<(), AppModError> {
    match mode {
        AppMode::WaitProbe => WaitProbeMode.exec(state, now_s),
        AppMode::Nominal => NominalMode.exec(state, now_s),
        AppMode::CompressorWait => CompressorWaitMode.exec(state, now_s),
        AppMode::VpdRegulation => VpdRegulationMode.exec(state, now_s),
        AppMode::TempEnforcing => TempEnforcingMode.exec(state, now_s),
        AppMode::HrEnforcing => HrEnforcingMode.exec(state, now_s),
        AppMode::FailureStop => FailureStopMode.exec(state, now_s),
    }
}

fn log_sample<const N: usize, const L: usize>(
    sample: Result<ProbeMeasure, ProbeError>,
    state: &AppState<N, L>,
    bank: RelayBank,
    now_s: u32,
) {
    let mode = state.app_mode().behavior_name();
    let rest = state.rest_active(now_s);
    match sample {
        Ok(reading) => {
            let temperature = fixed_point(reading.temp, 1);
            let deficit = fixed_point(reading.vpd, 3);
            log_reading(
                mode,
                temperature.as_str(),
                reading.hygr,
                deficit.as_str(),
                bank.sw1(),
                bank.sw2(),
                rest,
            );
        }
        Err(_) => log_failure(mode, bank.sw1(), bank.sw2(), rest),
    }
}

#[inline(never)]
fn log_reading(
    mode: &str,
    temperature: &str,
    hygr: u8,
    deficit: &str,
    fridge: bool,
    dehumidifier: bool,
    rest: bool,
) {
    log::info!(
        "T={temperature} HR={hygr}% VPD={deficit} mode={mode} fridge={fridge} dehumidifier={dehumidifier} rest={rest}"
    );
}

#[inline(never)]
fn log_failure(mode: &str, fridge: bool, dehumidifier: bool, rest: bool) {
    log::info!("probe error mode={mode} fridge={fridge} dehumidifier={dehumidifier} rest={rest}");
}

/// Format `value` with `decimals` digits after the point, without the float formatter.
///
/// The float formatter is larger than this crate's stack budget. Temperature is
/// logged with one decimal and vapor-pressure deficit with three.
#[inline(never)]
fn fixed_point(value: f32, decimals: u32) -> heapless::String<12> {
    let mut line = heapless::String::new();
    let scale = 10u32.pow(decimals);
    let scaled = libm::roundf(value * scale as f32) as i32;
    let negative = scaled < 0;
    let scaled = scaled.unsigned_abs();
    let whole = scaled / scale;
    let fraction = scaled % scale;
    if negative {
        let _ = line.push('-');
    }
    let _ = write!(
        line,
        "{whole}.{fraction:0width$}",
        width = decimals as usize
    );
    line
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::task::{Context, Poll, Waker};

    use crate::modes::test_support::{self, L, N};
    use crate::modes::{HR_ENFORCING_LIMIT_S, TEMP_ENFORCING_LIMIT_S};
    use crate::{
        AppMode, AppState, COMPRESSOR_REST_S, ClimateProbe, MainActor, PinLevel, ProbeError,
        ProbeMeasure, RecordingRelay,
    };

    type Cabinet = AppState<N, L>;

    struct FakeProbe {
        next: Option<Result<ProbeMeasure, ProbeError>>,
    }

    impl ClimateProbe for FakeProbe {
        async fn read(&mut self) -> Result<ProbeMeasure, ProbeError> {
            self.next.take().expect("a sample was loaded for this step")
        }
    }

    fn new_actor() -> MainActor<FakeProbe, RecordingRelay> {
        MainActor::new(FakeProbe { next: None }, RecordingRelay::new())
    }

    fn ready<F: Future>(future: F) -> F::Output {
        let mut future = core::pin::pin!(future);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("the fake probe should complete in one poll"),
        }
    }

    fn drive(
        actor: &mut MainActor<FakeProbe, RecordingRelay>,
        state: &mut Cabinet,
        sample: Result<ProbeMeasure, ProbeError>,
        now_s: u32,
    ) {
        actor.probe.next = Some(sample);
        ready(actor.step(state, now_s)).unwrap();
    }

    fn drive_ok(
        actor: &mut MainActor<FakeProbe, RecordingRelay>,
        state: &mut Cabinet,
        sample: ProbeMeasure,
        now_s: u32,
    ) {
        drive(actor, state, Ok(sample), now_s);
    }

    fn fill(
        actor: &mut MainActor<FakeProbe, RecordingRelay>,
        state: &mut Cabinet,
        sample: ProbeMeasure,
        now_s: u32,
    ) {
        for _ in 0..N {
            drive_ok(actor, state, sample, now_s);
        }
    }

    fn assert_outputs(
        actor: &MainActor<FakeProbe, RecordingRelay>,
        fridge: bool,
        dehumidifier: bool,
    ) {
        assert_eq!(actor.relays.fridge(), fridge);
        assert_eq!(actor.relays.dehumidifier(), dehumidifier);
        assert_eq!(actor.relays.fridge_pin(), PinLevel::from_active_low(fridge));
        assert_eq!(
            actor.relays.dehumidifier_pin(),
            PinLevel::from_active_low(dehumidifier)
        );
        assert_eq!(actor.relays.sw3_pin(), PinLevel::High);
        assert_eq!(actor.relays.sw4_pin(), PinLevel::High);
    }

    #[test]
    fn boot_stays_in_wait_probe_until_the_rack_is_full_then_a_hot_dry_cabinet_cools() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert!(!state.rest_active(0));

        let hot = test_support::hot_dry();
        drive_ok(&mut actor, &mut state, hot, 0);
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.status_line(), "* ? ?");
        assert_outputs(&actor, false, false);

        drive_ok(&mut actor, &mut state, hot, 1);
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.status_line(), "* * ?");
        assert!(!state.rest_active(1));

        drive_ok(&mut actor, &mut state, hot, 2);
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_eq!(state.mode_since_s(), 2);
        assert!(state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_outputs(&actor, true, false);
        assert!(!state.rest_active(2));
        assert_eq!(state.status_line(), "0s");
    }

    #[test]
    fn one_slot_that_misses_the_cooling_threshold_stays_nominal() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        let hot = test_support::hot_dry();
        let cool = ProbeMeasure { temp: 19.0, ..hot };

        drive_ok(&mut actor, &mut state, hot, 0);
        drive_ok(&mut actor, &mut state, hot, 0);
        assert_eq!(state.app_mode(), AppMode::WaitProbe);

        drive_ok(&mut actor, &mut state, cool, 0);
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_outputs(&actor, false, false);
        assert_eq!(state.status_line(), "");
    }

    #[test]
    fn humidity_outranks_a_hot_cabinet() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_humid(), 4);

        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_eq!(state.mode_since_s(), 4);
        assert_outputs(&actor, false, true);
        assert!(state.compressor_last_stop_s().is_none());
    }

    #[test]
    fn rest_blocks_cooling_and_does_not_fall_through_into_vpd_regulation() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);

        fill(&mut actor, &mut state, test_support::cooled(), 10);
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.compressor_last_stop_s(), Some(10));
        assert_outputs(&actor, false, false);

        fill(&mut actor, &mut state, test_support::hot_dry(), 20);
        assert_eq!(state.app_mode(), AppMode::CompressorWait);
        assert_ne!(state.app_mode(), AppMode::VpdRegulation);
        assert_ne!(state.app_mode(), AppMode::TempEnforcing);
        assert_eq!(state.mode_since_s(), 20);
        assert_outputs(&actor, false, false);
        assert_eq!(state.status_line(), "reste 590s");
        assert_eq!(state.compressor_last_stop_s(), Some(10));
    }

    #[test]
    fn compressor_wait_preempts_for_humidity_returns_when_recovered_and_cools_when_rest_clears() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        fill(&mut actor, &mut state, test_support::cooled(), 10);
        fill(&mut actor, &mut state, test_support::hot_dry(), 20);
        assert_eq!(state.app_mode(), AppMode::CompressorWait);

        let still_waiting = 10 + COMPRESSOR_REST_S - 1;
        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_dry(),
            still_waiting,
        );
        assert_eq!(state.app_mode(), AppMode::CompressorWait);
        assert_eq!(state.status_line(), "reste 1s");
        assert_outputs(&actor, false, false);

        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        fill(&mut actor, &mut state, test_support::cooled(), 10);
        fill(&mut actor, &mut state, test_support::hot_dry(), 20);
        let cleared = 10 + COMPRESSOR_REST_S;
        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_humid(),
            cleared - 2,
        );
        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_humid(),
            cleared - 1,
        );
        assert_eq!(state.app_mode(), AppMode::CompressorWait);
        drive_ok(&mut actor, &mut state, test_support::hot_humid(), cleared);
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_ne!(state.app_mode(), AppMode::TempEnforcing);
        assert_outputs(&actor, false, true);
        assert_eq!(state.compressor_last_stop_s(), Some(10));

        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        fill(&mut actor, &mut state, test_support::cooled(), 10);
        fill(&mut actor, &mut state, test_support::hot_dry(), 20);
        fill(&mut actor, &mut state, test_support::cool_dry(), 30);
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.status_line(), "");
        assert_outputs(&actor, false, false);

        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        fill(&mut actor, &mut state, test_support::cooled(), 10);
        fill(&mut actor, &mut state, test_support::hot_dry(), 20);
        fill(&mut actor, &mut state, test_support::low_vpd(), 40);
        assert_eq!(state.app_mode(), AppMode::VpdRegulation);
        assert_outputs(&actor, false, true);
        assert_eq!(state.compressor_last_stop_s(), Some(10));

        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        fill(&mut actor, &mut state, test_support::cooled(), 10);
        fill(&mut actor, &mut state, test_support::hot_dry(), 20);
        drive_ok(&mut actor, &mut state, test_support::cool_dry(), 25);
        drive_ok(&mut actor, &mut state, test_support::cool_dry(), 25);
        drive_ok(&mut actor, &mut state, test_support::hot_dry(), 25);
        assert_eq!(state.app_mode(), AppMode::CompressorWait);
        assert_eq!(state.status_line(), "reste 585s");

        let hot = test_support::hot_dry();
        fill(
            &mut actor,
            &mut state,
            ProbeMeasure { temp: 19.0, ..hot },
            26,
        );
        assert_eq!(state.app_mode(), AppMode::CompressorWait);
        fill(
            &mut actor,
            &mut state,
            ProbeMeasure {
                temp: 21.0,
                hygr: 50,
                vpd: 0.8,
                time: 0,
            },
            27,
        );
        assert_eq!(state.app_mode(), AppMode::CompressorWait);

        drive_ok(&mut actor, &mut state, test_support::hot_dry(), cleared);
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_eq!(state.mode_since_s(), cleared);
        assert_outputs(&actor, true, false);

        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_dry(),
            cleared + TEMP_ENFORCING_LIMIT_S - 1,
        );
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_dry(),
            cleared + TEMP_ENFORCING_LIMIT_S,
        );
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_eq!(state.mode_since_s(), cleared + TEMP_ENFORCING_LIMIT_S);
    }

    #[test]
    fn a_dead_compressor_wait_returns_through_nominal_and_does_not_resume_the_wait() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        fill(&mut actor, &mut state, test_support::cooled(), 10);
        fill(&mut actor, &mut state, test_support::hot_dry(), 20);
        assert_eq!(state.app_mode(), AppMode::CompressorWait);

        fill_err(&mut actor, &mut state, 30);
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.status_line(), "! ! !");
        assert_outputs(&actor, false, false);
        assert_eq!(state.compressor_last_stop_s(), Some(10));

        fill(&mut actor, &mut state, test_support::in_band(), 40);
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_ne!(state.app_mode(), AppMode::CompressorWait);
        assert_eq!(state.status_line(), "");
        assert!(state.rest_active(40));
    }

    #[test]
    fn rest_does_not_block_vpd_regulation() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        fill(&mut actor, &mut state, test_support::cooled(), 10);
        assert_eq!(state.compressor_last_stop_s(), Some(10));

        fill(&mut actor, &mut state, test_support::low_vpd(), 30);
        assert_eq!(state.app_mode(), AppMode::VpdRegulation);
        assert_outputs(&actor, false, true);
        assert_eq!(state.compressor_last_stop_s(), Some(10));
    }

    #[test]
    fn low_vpd_with_humidity_under_70_enters_vpd_regulation() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::low_vpd(), 0);

        assert_eq!(state.app_mode(), AppMode::VpdRegulation);
        assert_outputs(&actor, false, true);
        assert_eq!(state.status_line(), "0s");
    }

    #[test]
    fn high_vpd_at_or_below_19_degrees_leaves_both_relays_off() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::cool_dry(), 0);

        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_outputs(&actor, false, false);
        assert_eq!(state.status_line(), "");
    }

    #[test]
    fn temp_enforcing_latches_the_dehumidifier_and_keeps_it_on() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        assert_outputs(&actor, true, false);

        let latched = ProbeMeasure::new(0, 20.0, 65);
        assert!(latched.hygr >= 65 && latched.hygr < 70);
        assert!(latched.temp > 19.0 && latched.vpd > 0.8);
        fill(&mut actor, &mut state, latched, 5);
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_outputs(&actor, true, true);
        assert!(state.compressor_last_stop_s().is_none());

        fill(&mut actor, &mut state, test_support::hot_dry(), 8);
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_outputs(&actor, true, true);
    }

    #[test]
    fn one_slot_under_65_percent_does_not_latch_the_dehumidifier() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);

        let latched = ProbeMeasure::new(0, 20.0, 65);
        let under = ProbeMeasure {
            hygr: 64,
            ..latched
        };
        drive_ok(&mut actor, &mut state, latched, 2);
        drive_ok(&mut actor, &mut state, latched, 2);
        drive_ok(&mut actor, &mut state, under, 2);

        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_outputs(&actor, true, false);
    }

    #[test]
    fn cooling_returns_to_nominal_at_15_degrees_and_a_later_off_does_not_extend_rest() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        fill(&mut actor, &mut state, test_support::cooled(), 15);

        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_outputs(&actor, false, false);
        assert_eq!(state.compressor_last_stop_s(), Some(15));
        assert_eq!(state.status_line(), "");

        fill(&mut actor, &mut state, test_support::in_band(), 400);
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.status_line(), "");
        assert_eq!(state.compressor_last_stop_s(), Some(15));
        assert!(state.rest_active(15 + COMPRESSOR_REST_S - 1));
        assert!(!state.rest_active(15 + COMPRESSOR_REST_S));
        assert!(!state.rest_active(400 + COMPRESSOR_REST_S - 1));
    }

    #[test]
    fn seventy_percent_humidity_leaves_temp_enforcing_for_hr_and_starts_a_new_deadline() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        assert!(state.relay_bank().sw1());

        let handoff = 100;
        fill(&mut actor, &mut state, test_support::hot_humid(), handoff);
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_eq!(state.mode_since_s(), handoff);
        assert_outputs(&actor, false, true);
        assert_eq!(state.compressor_last_stop_s(), Some(handoff));

        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_humid(),
            handoff + HR_ENFORCING_LIMIT_S - 1,
        );
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_eq!(state.compressor_last_stop_s(), Some(handoff));

        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_humid(),
            handoff + HR_ENFORCING_LIMIT_S,
        );
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_eq!(state.mode_since_s(), handoff + HR_ENFORCING_LIMIT_S);
        assert_outputs(&actor, false, false);
        assert_eq!(state.compressor_last_stop_s(), Some(handoff));
    }

    #[test]
    fn hr_enforcing_returns_at_65_percent_and_stops_at_30_minutes() {
        let mut drying = new_actor();
        let mut state = test_support::fresh();
        fill(&mut drying, &mut state, test_support::hot_humid(), 0);
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);

        let clear = ProbeMeasure::new(0, 18.0, 65);
        drive_ok(&mut drying, &mut state, clear, HR_ENFORCING_LIMIT_S - 1);
        drive_ok(&mut drying, &mut state, clear, HR_ENFORCING_LIMIT_S - 1);
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);

        drive_ok(&mut drying, &mut state, clear, HR_ENFORCING_LIMIT_S);
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_outputs(&drying, false, false);

        let mut drying = new_actor();
        let mut state = test_support::fresh();
        fill(&mut drying, &mut state, test_support::hot_humid(), 0);
        drive_ok(
            &mut drying,
            &mut state,
            test_support::hot_humid(),
            HR_ENFORCING_LIMIT_S - 1,
        );
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        drive_ok(
            &mut drying,
            &mut state,
            test_support::hot_humid(),
            HR_ENFORCING_LIMIT_S,
        );
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_outputs(&drying, false, false);
        assert_eq!(state.status_line(), "0s");
    }

    #[test]
    fn temp_enforcing_stops_at_15_minutes() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);

        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_dry(),
            TEMP_ENFORCING_LIMIT_S - 1,
        );
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_outputs(&actor, true, false);

        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_dry(),
            TEMP_ENFORCING_LIMIT_S,
        );
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_outputs(&actor, false, false);
        assert_eq!(state.compressor_last_stop_s(), Some(TEMP_ENFORCING_LIMIT_S));
        assert_eq!(state.status_line(), "0s");
    }

    #[test]
    fn vpd_regulation_does_not_time_out_and_failure_stop_has_no_exit() {
        let mut regulating = new_actor();
        let mut state = test_support::fresh();
        fill(&mut regulating, &mut state, test_support::low_vpd(), 0);
        drive_ok(&mut regulating, &mut state, test_support::low_vpd(), 86_400);
        assert_eq!(state.app_mode(), AppMode::VpdRegulation);
        assert_outputs(&regulating, false, true);
        assert_eq!(state.status_line(), "86400s");

        let mut stopped = new_actor();
        let mut state = test_support::fresh();
        fill(&mut stopped, &mut state, test_support::hot_dry(), 0);
        drive_ok(
            &mut stopped,
            &mut state,
            test_support::hot_dry(),
            TEMP_ENFORCING_LIMIT_S,
        );
        assert_eq!(state.app_mode(), AppMode::FailureStop);

        for sample in [
            test_support::hot_humid(),
            test_support::low_vpd(),
            test_support::cooled(),
            test_support::in_band(),
        ] {
            fill(&mut stopped, &mut state, sample, 5_000);
            assert_eq!(state.app_mode(), AppMode::FailureStop);
            assert_outputs(&stopped, false, false);
        }
    }

    #[test]
    fn a_sick_rack_holds_the_mode_and_the_deadline_until_every_slot_is_valid() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        let latched = ProbeMeasure::new(0, 20.0, 65);
        fill(&mut actor, &mut state, latched, 3);
        assert_outputs(&actor, true, true);

        drive(
            &mut actor,
            &mut state,
            Err(ProbeError),
            TEMP_ENFORCING_LIMIT_S,
        );
        assert!(state.sample_rack().is_sick());
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_outputs(&actor, true, true);
        assert_eq!(state.status_line(), "! * *");
        assert!(state.compressor_last_stop_s().is_none());

        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_dry(),
            TEMP_ENFORCING_LIMIT_S,
        );
        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_dry(),
            TEMP_ENFORCING_LIMIT_S,
        );
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_outputs(&actor, true, true);

        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_dry(),
            TEMP_ENFORCING_LIMIT_S,
        );
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_outputs(&actor, false, false);
    }

    #[test]
    fn a_dead_rack_forces_the_relays_off_and_starts_rest_only_if_the_fridge_was_on() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        assert!(state.relay_bank().sw1());

        drive(&mut actor, &mut state, Err(ProbeError), 20);
        drive(&mut actor, &mut state, Err(ProbeError), 21);
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_outputs(&actor, true, false);

        drive(&mut actor, &mut state, Err(ProbeError), 30);
        assert!(state.sample_rack().is_dead());
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.mode_since_s(), 30);
        assert_outputs(&actor, false, false);
        assert_eq!(state.compressor_last_stop_s(), Some(30));
        assert_eq!(state.status_line(), "! ! !");

        drive(&mut actor, &mut state, Err(ProbeError), 42);
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.status_line(), "! ! !");
        assert!(state.rest_active(42));
        assert_eq!(state.compressor_last_stop_s(), Some(30));

        fill(&mut actor, &mut state, test_support::in_band(), 50);
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_outputs(&actor, false, false);
        assert_eq!(state.status_line(), "");
        assert_eq!(state.compressor_last_stop_s(), Some(30));

        let mut idle = new_actor();
        let mut state = test_support::fresh();
        fill(&mut idle, &mut state, test_support::in_band(), 0);
        assert_eq!(state.app_mode(), AppMode::Nominal);
        fill_err(&mut idle, &mut state, 80);
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_outputs(&idle, false, false);
        assert!(state.compressor_last_stop_s().is_none());
        assert!(!state.rest_active(80));
    }

    fn fill_err(actor: &mut MainActor<FakeProbe, RecordingRelay>, state: &mut Cabinet, now_s: u32) {
        for _ in 0..N {
            drive(actor, state, Err(ProbeError), now_s);
        }
    }

    #[test]
    fn failure_stop_does_not_enter_wait_probe_when_the_rack_dies() {
        let mut actor = new_actor();
        let mut state = test_support::fresh();
        fill(&mut actor, &mut state, test_support::hot_dry(), 0);
        drive_ok(
            &mut actor,
            &mut state,
            test_support::hot_dry(),
            TEMP_ENFORCING_LIMIT_S,
        );
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        let stopped_at = state.mode_since_s();

        fill_err(&mut actor, &mut state, 4_000);
        assert!(state.sample_rack().is_dead());
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_eq!(state.mode_since_s(), stopped_at);
        assert_outputs(&actor, false, false);
        assert_eq!(state.status_line(), "3100s");
        assert_ne!(state.status_line(), "! ! !");
    }
}
