//! `TEMP_ENFORCING`: the fridge cools the cabinet.
//!
//! The dehumidifier stays off until every sample is at or above 65% humidity,
//! then stays on until this policy ends. Humidity is tested before the
//! temperature exit and before the 15-minute deadline.

use crate::{AppMode, AppState};

use super::{
    AppModError, ApplicationMode, HR_CLEAR_PERCENT, HR_EMERGENCY_PERCENT, TEMP_DONE_C,
    TEMP_ENFORCING_LIMIT_S, block_sample_rules, command_relays, publish_elapsed, stop_fridge,
};

/// Fridge on. The dehumidifier latches on once humidity reaches 65%.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TempEnforcingMode;

impl<const N: usize, const L: usize> ApplicationMode<N, L> for TempEnforcingMode {
    fn init(state: &mut AppState<N, L>, now_s: u32) -> Result<Self, AppModError> {
        let dehumidifier_on = state.relay_bank().sw2();
        command_relays(state, true, dehumidifier_on, now_s)?;
        Ok(Self)
    }

    fn exec(&self, state: &mut AppState<N, L>, now_s: u32) -> Result<(), AppModError> {
        if block_sample_rules(state)? {
            return Ok(());
        }

        if state
            .sample_rack()
            .meets(|sample| sample.hygr >= HR_EMERGENCY_PERCENT)
        {
            stop_fridge(state, now_s)?;
            state.switch_mode(AppMode::HrEnforcing, now_s);
            return Ok(());
        }

        if state
            .sample_rack()
            .meets(|sample| sample.temp <= TEMP_DONE_C)
        {
            stop_fridge(state, now_s)?;
            state.switch_mode(AppMode::Nominal, now_s);
            return Ok(());
        }

        if state.mode_elapsed_s(now_s) >= TEMP_ENFORCING_LIMIT_S {
            stop_fridge(state, now_s)?;
            state.switch_mode(AppMode::FailureStop, now_s);
            return Ok(());
        }

        if !state.relay_bank().sw2()
            && state
                .sample_rack()
                .meets(|sample| sample.hygr >= HR_CLEAR_PERCENT)
        {
            command_relays(state, true, true, now_s)?;
        }
        publish_elapsed(state, now_s)
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppMode, COMPRESSOR_REST_S, ProbeMeasure};

    use super::super::test_support;
    use super::super::{
        AppModError, ApplicationMode, FailureStopMode, HrEnforcingMode, NominalMode,
        TEMP_ENFORCING_LIMIT_S,
    };
    use super::TempEnforcingMode;

    fn cooling_at(now_s: u32) -> (crate::AppState<3, 17>, TempEnforcingMode) {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::hot_dry());
        let mode =
            test_support::enter::<TempEnforcingMode>(&mut state, AppMode::TempEnforcing, now_s);
        (state, mode)
    }

    #[test]
    fn init_turns_the_fridge_on_and_leaves_the_dehumidifier() {
        let mut off = test_support::fresh();
        TempEnforcingMode::init(&mut off, 0).unwrap();
        assert!(off.relay_bank().sw1());
        assert!(!off.relay_bank().sw2());
        test_support::assert_spares_off(&off);
        assert!(off.compressor_last_stop_s().is_none());

        let mut dehumidifying = test_support::fresh();
        dehumidifying.set_relays(false, true, 0).unwrap();
        TempEnforcingMode::init(&mut dehumidifying, 5).unwrap();
        assert!(dehumidifying.relay_bank().sw1());
        assert!(dehumidifying.relay_bank().sw2());
    }

    #[test]
    fn init_during_rest_is_refused_and_leaves_the_relays() {
        let mut state = test_support::fresh();
        test_support::start_rest(&mut state, 10);
        state.switch_mode(AppMode::TempEnforcing, 10);

        let error = TempEnforcingMode::init(&mut state, 10);
        assert_eq!(error, Err(AppModError::CompressorRest));
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_eq!(state.compressor_last_stop_s(), Some(10));
    }

    #[test]
    fn humidity_at_or_above_70_goes_to_hr_enforcing_and_starts_rest() {
        let (mut state, mode) = cooling_at(0);
        test_support::fill(&mut state, test_support::hot_humid());

        let handoff = 1_000;
        mode.exec(&mut state, handoff).unwrap();

        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_ne!(state.app_mode(), AppMode::FailureStop);
        assert_eq!(state.mode_since_s(), handoff);
        assert!(!state.relay_bank().sw1());
        assert_eq!(state.compressor_last_stop_s(), Some(handoff));
        assert!(state.rest_active(handoff));

        HrEnforcingMode::init(&mut state, handoff).unwrap();
        assert!(state.relay_bank().sw2());
        assert!(!state.relay_bank().sw1());
        assert_eq!(state.compressor_last_stop_s(), Some(handoff));
    }

    #[test]
    fn humidity_wins_over_a_finished_cool_down_and_over_the_deadline() {
        let (mut state, mode) = cooling_at(0);
        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 15.0,
                hygr: 70,
                vpd: 0.4,
                time: 0,
            },
        );

        mode.exec(&mut state, TEMP_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_eq!(state.mode_since_s(), TEMP_ENFORCING_LIMIT_S);
        assert!(!state.relay_bank().sw1());
    }

    #[test]
    fn one_slot_under_70_percent_does_not_hand_off() {
        let (mut state, mode) = cooling_at(0);
        let emergency = test_support::hot_humid();
        test_support::fill_with_one_difference(
            &mut state,
            emergency,
            ProbeMeasure {
                hygr: 69,
                ..emergency
            },
        );

        mode.exec(&mut state, 10).unwrap();
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert!(state.relay_bank().sw1());
        assert!(state.compressor_last_stop_s().is_none());
    }

    #[test]
    fn temperature_at_or_below_15_returns_to_nominal_and_turns_the_fridge_off() {
        let (mut state, mode) = cooling_at(30);
        test_support::fill(&mut state, test_support::cooled());

        mode.exec(&mut state, 45).unwrap();

        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.mode_since_s(), 45);
        assert!(!state.relay_bank().sw1());
        assert_eq!(state.compressor_last_stop_s(), Some(45));

        let nominal = NominalMode::init(&mut state, 45).unwrap();
        nominal.exec(&mut state, 45).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_eq!(state.compressor_last_stop_s(), Some(45));
        assert_eq!(state.status_line(), "");
    }

    #[test]
    fn temperature_above_15_stays_and_one_warm_slot_blocks_the_exit() {
        let (mut state, mode) = cooling_at(0);
        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 15.1,
                hygr: 50,
                vpd: 0.9,
                time: 0,
            },
        );
        mode.exec(&mut state, 12).unwrap();
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_eq!(state.status_line(), "12s");
        assert!(state.relay_bank().sw1());

        let cooled = test_support::cooled();
        test_support::fill_with_one_difference(
            &mut state,
            cooled,
            ProbeMeasure {
                temp: 15.1,
                ..cooled
            },
        );
        mode.exec(&mut state, 20).unwrap();
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert!(state.relay_bank().sw1());
    }

    #[test]
    fn the_15_minute_deadline_goes_to_failure_stop_and_turns_the_fridge_off() {
        let (mut state, mode) = cooling_at(100);
        mode.exec(&mut state, 100 + TEMP_ENFORCING_LIMIT_S - 1)
            .unwrap();
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert!(state.relay_bank().sw1());
        assert_eq!(state.status_line(), "899s");

        mode.exec(&mut state, 100 + TEMP_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_eq!(state.mode_since_s(), 100 + TEMP_ENFORCING_LIMIT_S);
        assert!(!state.relay_bank().sw1());
        assert_eq!(
            state.compressor_last_stop_s(),
            Some(100 + TEMP_ENFORCING_LIMIT_S)
        );

        FailureStopMode::init(&mut state, 100 + TEMP_ENFORCING_LIMIT_S).unwrap();
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_eq!(
            state.compressor_last_stop_s(),
            Some(100 + TEMP_ENFORCING_LIMIT_S)
        );
    }

    #[test]
    fn a_finished_cool_down_beats_the_deadline() {
        let (mut state, mode) = cooling_at(0);
        test_support::fill(&mut state, test_support::cooled());

        mode.exec(&mut state, TEMP_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_ne!(state.app_mode(), AppMode::FailureStop);
    }

    #[test]
    fn the_dehumidifier_latches_at_65_percent_and_stays_on() {
        let (mut state, mode) = cooling_at(0);
        assert!(!state.relay_bank().sw2());

        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 20.0,
                hygr: 64,
                vpd: 0.85,
                time: 0,
            },
        );
        mode.exec(&mut state, 1).unwrap();
        assert!(!state.relay_bank().sw2());
        assert!(state.relay_bank().sw1());

        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 20.0,
                hygr: 65,
                vpd: 0.82,
                time: 0,
            },
        );
        mode.exec(&mut state, 2).unwrap();
        assert!(state.relay_bank().sw2());
        assert!(state.relay_bank().sw1());
        assert!(state.compressor_last_stop_s().is_none());
        assert_eq!(state.status_line(), "2s");

        test_support::fill(&mut state, test_support::hot_dry());
        mode.exec(&mut state, 3).unwrap();
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert!(state.relay_bank().sw2());
        assert!(state.relay_bank().sw1());
    }

    #[test]
    fn one_slot_under_65_percent_does_not_latch_the_dehumidifier() {
        let (mut state, mode) = cooling_at(0);
        let latched = ProbeMeasure {
            temp: 20.0,
            hygr: 65,
            vpd: 0.82,
            time: 0,
        };
        test_support::fill_with_one_difference(
            &mut state,
            latched,
            ProbeMeasure {
                hygr: 64,
                ..latched
            },
        );

        mode.exec(&mut state, 1).unwrap();
        assert!(!state.relay_bank().sw2());
        assert!(state.relay_bank().sw1());
    }

    #[test]
    fn a_sick_rack_holds_the_mode_and_the_deadline_does_not_fire() {
        let (mut state, mode) = cooling_at(0);
        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 20.0,
                hygr: 65,
                vpd: 0.82,
                time: 0,
            },
        );
        mode.exec(&mut state, 1).unwrap();
        assert!(state.relay_bank().sw2());

        state.sample_rack_mut().push(Err(crate::ProbeError));
        assert!(state.sample_rack().is_sick());
        mode.exec(&mut state, TEMP_ENFORCING_LIMIT_S).unwrap();

        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert!(state.relay_bank().sw1());
        assert!(state.relay_bank().sw2());
        assert_eq!(state.status_line(), "! * *");
        assert!(state.compressor_last_stop_s().is_none());
    }

    #[test]
    fn the_deadline_fires_once_a_sick_rack_is_healthy_again() {
        let (mut state, mode) = cooling_at(0);
        state.sample_rack_mut().push(Err(crate::ProbeError));
        mode.exec(&mut state, TEMP_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);

        test_support::fill(&mut state, test_support::hot_dry());
        mode.exec(&mut state, TEMP_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert!(!state.relay_bank().sw1());
    }

    #[test]
    fn recovery_into_a_finished_cool_down_beats_a_deadline_that_elapsed_while_sick() {
        let (mut state, mode) = cooling_at(0);
        state.sample_rack_mut().push(Err(crate::ProbeError));
        mode.exec(&mut state, TEMP_ENFORCING_LIMIT_S).unwrap();

        test_support::fill(&mut state, test_support::cooled());
        mode.exec(&mut state, TEMP_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
    }

    #[test]
    fn an_incomplete_rack_shows_the_glyph_line_and_the_deadline_does_not_fire() {
        let mut state = test_support::fresh();
        let mode = test_support::enter::<TempEnforcingMode>(&mut state, AppMode::TempEnforcing, 0);
        assert!(state.relay_bank().sw1());

        mode.exec(&mut state, TEMP_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_eq!(state.status_line(), "? ? ?");
        assert!(state.relay_bank().sw1());
        assert!(state.compressor_last_stop_s().is_none());
    }

    #[test]
    fn returning_to_nominal_does_not_extend_rest_when_nominal_commands_the_fridge_off() {
        let (mut state, mode) = cooling_at(0);
        test_support::fill(&mut state, test_support::cooled());
        mode.exec(&mut state, 8).unwrap();
        assert_eq!(state.compressor_last_stop_s(), Some(8));

        let nominal = NominalMode::init(&mut state, 8).unwrap();
        nominal.exec(&mut state, 8 + COMPRESSOR_REST_S - 1).unwrap();
        assert_eq!(state.compressor_last_stop_s(), Some(8));
        assert!(state.rest_active(8 + COMPRESSOR_REST_S - 1));
        assert!(!state.rest_active(8 + COMPRESSOR_REST_S));
    }
}
