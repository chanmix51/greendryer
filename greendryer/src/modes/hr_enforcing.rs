//! `HR_ENFORCING`: humidity at or above 70% is an emergency.
//!
//! The fridge stays off and the dehumidifier stays on. The policy returns to
//! `NOMINAL` when every sample is at or below 65% humidity, without a warning.
//! Thirty minutes from this entry, including an entry that came from
//! `TEMP_ENFORCING` or `COMPRESSOR_WAIT`, it returns to `NOMINAL` and logs a
//! warning when every sample is under 70% humidity and above 0.6 kPa.
//! Otherwise that deadline is `FAILURE_STOP`. The fridge is already off, so
//! the recovery does not start compressor rest.

use crate::{AppMode, AppState};

use super::{
    AppModError, ApplicationMode, HR_CLEAR_PERCENT, HR_EMERGENCY_PERCENT, HR_ENFORCING_LIMIT_S,
    VPD_LOW_KPA, block_sample_rules, command_relays, publish_elapsed,
};

/// Fridge off, dehumidifier on, until humidity falls or the deadline expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HrEnforcingMode;

impl<const N: usize, const L: usize> ApplicationMode<N, L> for HrEnforcingMode {
    fn init(state: &mut AppState<N, L>, now_s: u32) -> Result<Self, AppModError> {
        command_relays(state, false, true, now_s)?;
        Ok(Self)
    }

    fn exec(&self, state: &mut AppState<N, L>, now_s: u32) -> Result<(), AppModError> {
        if block_sample_rules(state)? {
            return Ok(());
        }
        if state
            .sample_rack()
            .meets(|sample| sample.hygr <= HR_CLEAR_PERCENT)
        {
            state.switch_mode(AppMode::Nominal, now_s);
            return Ok(());
        }
        if state.mode_elapsed_s(now_s) >= HR_ENFORCING_LIMIT_S {
            let recovered = state
                .sample_rack()
                .meets(|sample| sample.hygr < HR_EMERGENCY_PERCENT && sample.vpd > VPD_LOW_KPA);
            if recovered {
                log::warn!("HR_ENFORCING deadline recovery");
                state.switch_mode(AppMode::Nominal, now_s);
            } else {
                state.switch_mode(AppMode::FailureStop, now_s);
            }
            return Ok(());
        }
        publish_elapsed(state, now_s)
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppMode, ProbeMeasure};

    use super::super::test_support;
    use super::super::{
        ApplicationMode, CompressorWaitMode, FailureStopMode, HR_CLEAR_PERCENT,
        HR_EMERGENCY_PERCENT, HR_ENFORCING_LIMIT_S, NominalMode, TempEnforcingMode, VPD_LOW_KPA,
    };
    use super::HrEnforcingMode;

    fn drying_at(now_s: u32) -> (crate::AppState<3, 17>, HrEnforcingMode) {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::hot_humid());
        let mode = test_support::enter::<HrEnforcingMode>(&mut state, AppMode::HrEnforcing, now_s);
        (state, mode)
    }

    #[test]
    fn init_turns_the_dehumidifier_on_and_the_fridge_off() {
        let mut state = test_support::fresh();
        state.set_relays(true, false, 0).unwrap();

        HrEnforcingMode::init(&mut state, 15).unwrap();

        assert!(!state.relay_bank().sw1());
        assert!(state.relay_bank().sw2());
        test_support::assert_spares_off(&state);
        assert_eq!(state.compressor_last_stop_s(), Some(15));
    }

    #[test]
    fn a_second_off_command_does_not_extend_rest() {
        let mut state = test_support::fresh();
        state.set_relays(true, false, 0).unwrap();
        HrEnforcingMode::init(&mut state, 15).unwrap();

        HrEnforcingMode::init(&mut state, 90).unwrap();
        assert_eq!(state.compressor_last_stop_s(), Some(15));
        assert!(state.rest_active(15 + crate::COMPRESSOR_REST_S - 1));
        assert!(!state.rest_active(90 + crate::COMPRESSOR_REST_S - 1));
    }

    #[test]
    fn humidity_at_or_below_65_percent_returns_to_nominal() {
        let (mut state, mode) = drying_at(0);
        test_support::fill(&mut state, ProbeMeasure::new(0, 18.0, HR_CLEAR_PERCENT));

        mode.exec(&mut state, 20).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.mode_since_s(), 20);
    }

    #[test]
    fn one_slot_above_65_percent_stays() {
        let (mut state, mode) = drying_at(0);
        let clear = ProbeMeasure {
            temp: 18.0,
            hygr: HR_CLEAR_PERCENT,
            vpd: 0.7,
            time: 0,
        };
        test_support::fill_with_one_difference(
            &mut state,
            clear,
            ProbeMeasure {
                hygr: HR_CLEAR_PERCENT + 1,
                ..clear
            },
        );

        mode.exec(&mut state, 5).unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_eq!(state.status_line(), "5s");
        assert!(state.relay_bank().sw2());
        assert!(!state.relay_bank().sw1());
    }

    #[test]
    fn the_30_minute_deadline_goes_to_failure_stop_when_humidity_is_still_at_70() {
        let (mut state, mode) = drying_at(50);
        mode.exec(&mut state, 50 + HR_ENFORCING_LIMIT_S - 1)
            .unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_eq!(state.status_line(), "1799s");
        assert!(state.relay_bank().sw2());

        test_support::capture_warnings();
        mode.exec(&mut state, 50 + HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_eq!(state.mode_since_s(), 50 + HR_ENFORCING_LIMIT_S);
        test_support::assert_warnings(&[]);

        FailureStopMode::init(&mut state, 50 + HR_ENFORCING_LIMIT_S).unwrap();
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert!(state.compressor_last_stop_s().is_none());
    }

    #[test]
    fn humidity_clearing_beats_the_deadline_and_does_not_log_a_warning() {
        let (mut state, mode) = drying_at(0);
        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 18.0,
                hygr: 65,
                vpd: 0.72,
                time: 0,
            },
        );

        test_support::capture_warnings();
        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_ne!(state.app_mode(), AppMode::FailureStop);
        test_support::assert_warnings(&[]);
    }

    #[test]
    fn at_30_minutes_every_slot_under_70_and_above_0_6_returns_to_nominal_and_logs_a_warning() {
        let (mut state, mode) = drying_at(40);
        let recovered = test_support::deadline_recovered();
        test_support::fill(&mut state, recovered);

        let now = 40 + HR_ENFORCING_LIMIT_S;
        mode.exec(&mut state, now - 1).unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);

        test_support::capture_warnings();
        mode.exec(&mut state, now).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.mode_since_s(), now);
        assert!(state.compressor_last_stop_s().is_none());
        assert!(state.relay_bank().sw2());
        test_support::assert_warnings(&["HR_ENFORCING deadline recovery"]);

        let nominal = NominalMode::init(&mut state, now).unwrap();
        nominal.exec(&mut state, now).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_eq!(state.status_line(), "");
        assert!(state.compressor_last_stop_s().is_none());
        test_support::assert_warnings(&["HR_ENFORCING deadline recovery"]);
    }

    #[test]
    fn one_slot_at_or_above_70_percent_rejects_the_30_minute_recovery() {
        let (mut state, mode) = drying_at(0);
        let recovered = test_support::deadline_recovered();
        test_support::fill_with_one_difference(
            &mut state,
            recovered,
            ProbeMeasure {
                hygr: HR_EMERGENCY_PERCENT,
                ..recovered
            },
        );

        test_support::capture_warnings();
        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        test_support::assert_warnings(&[]);
    }

    #[test]
    fn one_slot_at_or_below_0_6_kpa_rejects_the_30_minute_recovery() {
        let (mut state, mode) = drying_at(0);
        let recovered = test_support::deadline_recovered();
        let on_the_bound = ProbeMeasure {
            vpd: VPD_LOW_KPA,
            ..recovered
        };
        assert!(on_the_bound.hygr < HR_EMERGENCY_PERCENT);
        assert_eq!(on_the_bound.vpd, VPD_LOW_KPA);
        test_support::fill_with_one_difference(&mut state, recovered, on_the_bound);

        test_support::capture_warnings();
        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        test_support::assert_warnings(&[]);
    }

    #[test]
    fn exactly_70_percent_is_not_under_70_at_the_deadline() {
        let (mut state, mode) = drying_at(0);
        let sample = ProbeMeasure::new(0, 18.0, HR_EMERGENCY_PERCENT);
        assert_eq!(sample.hygr, HR_EMERGENCY_PERCENT);
        assert!(sample.vpd > VPD_LOW_KPA);
        assert!(sample.hygr > HR_CLEAR_PERCENT);
        test_support::fill(&mut state, sample);

        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
    }

    #[test]
    fn exactly_0_6_kpa_is_not_above_0_6_at_the_deadline() {
        let (mut state, mode) = drying_at(0);
        let recovered = test_support::deadline_recovered();
        let sample = ProbeMeasure {
            vpd: VPD_LOW_KPA,
            ..recovered
        };
        assert!(sample.hygr < HR_EMERGENCY_PERCENT);
        assert!(sample.hygr > HR_CLEAR_PERCENT);
        test_support::fill(&mut state, sample);

        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
    }

    #[test]
    fn a_30_minute_recovery_on_a_hot_dry_rack_continues_into_temp_enforcing() {
        let (mut state, mode) = drying_at(0);
        test_support::fill(&mut state, test_support::recovered_hot_dry());
        let now = HR_ENFORCING_LIMIT_S;

        test_support::capture_warnings();
        mode.exec(&mut state, now).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert!(state.compressor_last_stop_s().is_none());
        test_support::assert_warnings(&["HR_ENFORCING deadline recovery"]);

        let nominal = NominalMode::init(&mut state, now).unwrap();
        nominal.exec(&mut state, now).unwrap();
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);

        let cooling = TempEnforcingMode::init(&mut state, now).unwrap();
        assert!(state.relay_bank().sw1());
        assert!(state.compressor_last_stop_s().is_none());
        cooling.exec(&mut state, now).unwrap();
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_eq!(state.mode_since_s(), now);
    }

    #[test]
    fn a_30_minute_recovery_with_rest_set_continues_into_compressor_wait() {
        let (mut state, mode) = drying_at(0);
        test_support::fill(&mut state, test_support::recovered_hot_dry());
        let now = HR_ENFORCING_LIMIT_S;
        let rested_at = now - 100;
        test_support::start_rest(&mut state, rested_at);

        test_support::capture_warnings();
        mode.exec(&mut state, now).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.compressor_last_stop_s(), Some(rested_at));
        test_support::assert_warnings(&["HR_ENFORCING deadline recovery"]);

        let nominal = NominalMode::init(&mut state, now).unwrap();
        nominal.exec(&mut state, now).unwrap();
        assert_eq!(state.app_mode(), AppMode::CompressorWait);

        let waiting = CompressorWaitMode::init(&mut state, now).unwrap();
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_eq!(state.compressor_last_stop_s(), Some(rested_at));
        waiting.exec(&mut state, now).unwrap();
        assert_eq!(state.app_mode(), AppMode::CompressorWait);
        assert_eq!(state.status_line(), "reste 500s");
    }

    #[test]
    fn a_sick_rack_holds_the_mode_and_the_deadline_does_not_fire() {
        let (mut state, mode) = drying_at(0);
        state.sample_rack_mut().push(Err(crate::ProbeError));
        assert!(state.sample_rack().is_sick());

        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert!(state.relay_bank().sw2());
        assert!(!state.relay_bank().sw1());
        assert_eq!(state.status_line(), "! * *");
    }

    #[test]
    fn the_deadline_fires_once_a_sick_rack_is_healthy_again() {
        let (mut state, mode) = drying_at(0);
        state.sample_rack_mut().push(Err(crate::ProbeError));
        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);

        test_support::fill(&mut state, test_support::hot_humid());
        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
    }

    #[test]
    fn a_recovered_rack_returns_to_nominal_once_a_sick_rack_is_healthy_again() {
        let (mut state, mode) = drying_at(0);
        state.sample_rack_mut().push(Err(crate::ProbeError));
        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);

        test_support::fill(&mut state, test_support::deadline_recovered());
        test_support::capture_warnings();
        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        test_support::assert_warnings(&["HR_ENFORCING deadline recovery"]);
        assert!(state.compressor_last_stop_s().is_none());
    }

    #[test]
    fn an_incomplete_rack_shows_the_glyph_line_and_the_deadline_does_not_fire() {
        let mut state = test_support::fresh();
        let mode = test_support::enter::<HrEnforcingMode>(&mut state, AppMode::HrEnforcing, 0);

        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_eq!(state.status_line(), "? ? ?");
        assert!(state.relay_bank().sw2());
    }
}
