//! `HR_ENFORCING`: humidity at or above 70% is an emergency.
//!
//! The fridge stays off and the dehumidifier stays on. The policy returns to
//! `NOMINAL` when every sample is at or below 65% humidity. Thirty minutes
//! from this entry, including an entry that came from `TEMP_ENFORCING`, go to
//! `FAILURE_STOP`.

use crate::{AppMode, AppState};

use super::{
    AppModError, ApplicationMode, HR_CLEAR_PERCENT, HR_ENFORCING_LIMIT_S, block_sample_rules,
    command_relays, publish_elapsed,
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
            state.switch_mode(AppMode::FailureStop, now_s);
            return Ok(());
        }
        publish_elapsed(state, now_s)
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppMode, ProbeMeasure};

    use super::super::test_support;
    use super::super::{ApplicationMode, FailureStopMode, HR_CLEAR_PERCENT, HR_ENFORCING_LIMIT_S};
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
    fn the_30_minute_deadline_goes_to_failure_stop() {
        let (mut state, mode) = drying_at(50);
        mode.exec(&mut state, 50 + HR_ENFORCING_LIMIT_S - 1)
            .unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_eq!(state.status_line(), "1799s");
        assert!(state.relay_bank().sw2());

        mode.exec(&mut state, 50 + HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_eq!(state.mode_since_s(), 50 + HR_ENFORCING_LIMIT_S);

        FailureStopMode::init(&mut state, 50 + HR_ENFORCING_LIMIT_S).unwrap();
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert!(state.compressor_last_stop_s().is_none());
    }

    #[test]
    fn humidity_clearing_beats_the_deadline() {
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

        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_ne!(state.app_mode(), AppMode::FailureStop);
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
    fn an_incomplete_rack_shows_the_glyph_line_and_the_deadline_does_not_fire() {
        let mut state = test_support::fresh();
        let mode = test_support::enter::<HrEnforcingMode>(&mut state, AppMode::HrEnforcing, 0);

        mode.exec(&mut state, HR_ENFORCING_LIMIT_S).unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_eq!(state.status_line(), "? ? ?");
        assert!(state.relay_bank().sw2());
    }
}
