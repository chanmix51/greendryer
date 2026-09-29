//! `VPD_REGULATION`: raise a vapor-pressure deficit that is under 0.6 kPa.
//!
//! The dehumidifier stays on and the fridge stays off. This policy has no
//! deadline. It leaves for `NOMINAL` when every sample is at or above 0.8 kPa.

use crate::{AppMode, AppState};

use super::{
    AppModError, ApplicationMode, VPD_HIGH_KPA, block_sample_rules, command_relays, publish_elapsed,
};

/// Dehumidifier on, fridge off, until vapor-pressure deficit recovers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VpdRegulationMode;

impl<const N: usize, const L: usize> ApplicationMode<N, L> for VpdRegulationMode {
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
            .meets(|sample| sample.vpd >= VPD_HIGH_KPA)
        {
            state.switch_mode(AppMode::Nominal, now_s);
            return Ok(());
        }
        publish_elapsed(state, now_s)
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppMode, ProbeMeasure};

    use super::super::test_support;
    use super::super::{ApplicationMode, VPD_HIGH_KPA};
    use super::VpdRegulationMode;

    #[test]
    fn init_turns_the_dehumidifier_on_and_the_fridge_off() {
        let mut state = test_support::fresh();
        state.set_relays(true, false, 0).unwrap();

        VpdRegulationMode::init(&mut state, 12).unwrap();

        assert!(!state.relay_bank().sw1());
        assert!(state.relay_bank().sw2());
        test_support::assert_spares_off(&state);
        assert_eq!(state.compressor_last_stop_s(), Some(12));
    }

    #[test]
    fn init_does_not_move_rest_when_the_fridge_is_already_off() {
        let mut state = test_support::fresh();
        test_support::start_rest(&mut state, 4);

        VpdRegulationMode::init(&mut state, 50).unwrap();
        assert_eq!(state.compressor_last_stop_s(), Some(4));
        assert!(state.relay_bank().sw2());
        assert!(!state.relay_bank().sw1());
    }

    #[test]
    fn vpd_at_or_above_0_8_returns_to_nominal() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::hot_dry());
        let mode = test_support::enter::<VpdRegulationMode>(&mut state, AppMode::VpdRegulation, 6);

        mode.exec(&mut state, 9).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.mode_since_s(), 9);
        assert!(state.relay_bank().sw2());
        assert!(!state.relay_bank().sw1());
    }

    #[test]
    fn vpd_of_exactly_0_8_returns_to_nominal() {
        let mut state = test_support::fresh();
        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 18.0,
                hygr: 40,
                vpd: VPD_HIGH_KPA,
                time: 0,
            },
        );
        let mode = test_support::enter::<VpdRegulationMode>(&mut state, AppMode::VpdRegulation, 0);

        mode.exec(&mut state, 1).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
    }

    #[test]
    fn one_slot_under_0_8_stays() {
        let mut state = test_support::fresh();
        let recovered = test_support::hot_dry();
        test_support::fill_with_one_difference(
            &mut state,
            recovered,
            ProbeMeasure {
                vpd: VPD_HIGH_KPA - 0.01,
                ..recovered
            },
        );
        let mode = test_support::enter::<VpdRegulationMode>(&mut state, AppMode::VpdRegulation, 0);

        mode.exec(&mut state, 4).unwrap();
        assert_eq!(state.app_mode(), AppMode::VpdRegulation);
        assert_eq!(state.status_line(), "4s");
        assert!(state.relay_bank().sw2());
    }

    #[test]
    fn low_vpd_stays_and_a_long_run_does_not_stop_the_mode() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::low_vpd());
        let mode = test_support::enter::<VpdRegulationMode>(&mut state, AppMode::VpdRegulation, 2);

        mode.exec(&mut state, 2).unwrap();
        assert_eq!(state.status_line(), "0s");
        assert_eq!(state.app_mode(), AppMode::VpdRegulation);

        mode.exec(&mut state, 2 + 24 * 60 * 60).unwrap();
        assert_eq!(state.app_mode(), AppMode::VpdRegulation);
        assert!(state.relay_bank().sw2());
        assert!(!state.relay_bank().sw1());
        assert_ne!(state.app_mode(), AppMode::FailureStop);
    }

    #[test]
    fn a_sick_rack_holds_the_mode_and_the_dehumidifier() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::hot_dry());
        state.sample_rack_mut().push(Err(crate::ProbeError));
        let mode = test_support::enter::<VpdRegulationMode>(&mut state, AppMode::VpdRegulation, 0);

        mode.exec(&mut state, 1_000).unwrap();
        assert_eq!(state.app_mode(), AppMode::VpdRegulation);
        assert!(state.relay_bank().sw2());
        assert!(!state.relay_bank().sw1());
        assert_eq!(state.status_line(), "! * *");
    }

    #[test]
    fn an_incomplete_rack_shows_the_glyph_line() {
        let mut state = test_support::fresh();
        state.sample_rack_mut().push(Ok(test_support::hot_dry()));
        let mode = test_support::enter::<VpdRegulationMode>(&mut state, AppMode::VpdRegulation, 0);

        mode.exec(&mut state, 3).unwrap();
        assert_eq!(state.app_mode(), AppMode::VpdRegulation);
        assert_eq!(state.status_line(), "* ? ?");
        assert!(state.relay_bank().sw2());
    }
}
