//! `NOMINAL`: both relays off while the cabinet is inside its climate band.
//!
//! Compressor rest may still be running. Rest blocks only the start of
//! `TEMP_ENFORCING`. It does not block `HR_ENFORCING` or `VPD_REGULATION`.

use crate::{AppMode, AppState};

use super::{
    AppModError, ApplicationMode, HR_EMERGENCY_PERCENT, TEMP_START_C, VPD_HIGH_KPA, VPD_LOW_KPA,
    block_sample_rules, settle_nominal,
};

/// Both relays off. Compressor rest may still be running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NominalMode;

impl<const N: usize, const L: usize> ApplicationMode<N, L> for NominalMode {
    fn init(_state: &mut AppState<N, L>, _now_s: u32) -> Result<Self, AppModError> {
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
            state.switch_mode(AppMode::HrEnforcing, now_s);
            return Ok(());
        }

        let hot_and_dry = state
            .sample_rack()
            .meets(|sample| sample.temp > TEMP_START_C && sample.vpd > VPD_HIGH_KPA);
        if hot_and_dry {
            if !state.rest_active(now_s) {
                state.switch_mode(AppMode::TempEnforcing, now_s);
                return Ok(());
            }
            // Rest blocks cooling. Do not fall through into `VPD_REGULATION`,
            // and do not start the cooling deadline.
            return settle_nominal(state, now_s);
        }

        if state.sample_rack().meets(|sample| sample.vpd < VPD_LOW_KPA) {
            state.switch_mode(AppMode::VpdRegulation, now_s);
            return Ok(());
        }

        settle_nominal(state, now_s)
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppMode, COMPRESSOR_REST_S, ProbeMeasure};

    use super::super::test_support;
    use super::super::{ApplicationMode, HR_EMERGENCY_PERCENT, VPD_HIGH_KPA, VPD_LOW_KPA};
    use super::NominalMode;

    #[test]
    fn init_leaves_the_relays_as_they_are() {
        let mut state = test_support::fresh();
        state.set_relays(false, true, 0).unwrap();
        let since = state.mode_since_s();

        NominalMode::init(&mut state, 30).unwrap();

        assert!(!state.relay_bank().sw1());
        assert!(state.relay_bank().sw2());
        assert_eq!(state.compressor_last_stop_s(), None);
        assert_eq!(state.mode_since_s(), since);
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
    }

    #[test]
    fn humidity_at_or_above_70_percent_switches_to_hr_enforcing() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::hot_humid());
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 10);

        nominal.exec(&mut state, 10).unwrap();

        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_eq!(state.mode_since_s(), 10);
    }

    #[test]
    fn humidity_of_exactly_70_percent_is_an_emergency() {
        let mut state = test_support::fresh();
        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 20.0,
                hygr: HR_EMERGENCY_PERCENT,
                vpd: 0.7,
                time: 0,
            },
        );
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);

        nominal.exec(&mut state, 0).unwrap();
        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
    }

    #[test]
    fn one_slot_under_70_percent_does_not_start_hr_enforcing() {
        let mut state = test_support::fresh();
        let emergency = test_support::hot_humid();
        let under = ProbeMeasure {
            temp: 19.0,
            hygr: 69,
            vpd: 0.7,
            time: 0,
        };
        test_support::fill_with_one_difference(&mut state, emergency, under);
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);

        nominal.exec(&mut state, 0).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_ne!(state.app_mode(), AppMode::HrEnforcing);
    }

    #[test]
    fn a_hot_dry_cabinet_with_rest_clear_switches_to_temp_enforcing() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::hot_dry());
        assert!(!state.rest_active(0));
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);

        nominal.exec(&mut state, 0).unwrap();
        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_eq!(state.mode_since_s(), 0);
    }

    #[test]
    fn temperature_of_exactly_19_degrees_does_not_start_cooling() {
        let mut state = test_support::fresh();
        let hot = test_support::hot_dry();
        test_support::fill(&mut state, ProbeMeasure { temp: 19.0, ..hot });
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);

        nominal.exec(&mut state, 0).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_eq!(state.status_line(), "compresseur prêt");
    }

    #[test]
    fn vpd_of_exactly_0_8_does_not_start_cooling() {
        let mut state = test_support::fresh();
        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 21.0,
                hygr: 50,
                vpd: VPD_HIGH_KPA,
                time: 0,
            },
        );
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);

        nominal.exec(&mut state, 0).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_ne!(state.app_mode(), AppMode::TempEnforcing);
        assert_ne!(state.app_mode(), AppMode::VpdRegulation);
    }

    #[test]
    fn rest_blocks_cooling_and_does_not_fall_through_into_vpd_regulation() {
        let mut state = test_support::fresh();
        test_support::start_rest(&mut state, 10);
        test_support::fill(&mut state, test_support::hot_dry());
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 10);
        let entered_at = state.mode_since_s();

        nominal.exec(&mut state, 10).unwrap();

        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_ne!(state.app_mode(), AppMode::TempEnforcing);
        assert_ne!(state.app_mode(), AppMode::VpdRegulation);
        assert_eq!(state.mode_since_s(), entered_at);
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_eq!(state.status_line(), "repos 600s");
        assert_eq!(state.compressor_last_stop_s(), Some(10));
    }

    #[test]
    fn rest_does_not_block_vpd_regulation_when_the_cabinet_is_not_waiting_to_cool() {
        let mut state = test_support::fresh();
        test_support::start_rest(&mut state, 10);
        test_support::fill(&mut state, test_support::low_vpd());
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 10);

        nominal.exec(&mut state, 10).unwrap();
        assert_eq!(state.app_mode(), AppMode::VpdRegulation);
    }

    #[test]
    fn vpd_under_0_6_with_humidity_under_70_switches_to_vpd_regulation() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::low_vpd());
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);

        nominal.exec(&mut state, 0).unwrap();
        assert_eq!(state.app_mode(), AppMode::VpdRegulation);
        assert_eq!(state.mode_since_s(), 0);
    }

    #[test]
    fn vpd_of_exactly_0_6_does_not_start_vpd_regulation() {
        let mut state = test_support::fresh();
        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 16.0,
                hygr: 60,
                vpd: VPD_LOW_KPA,
                time: 0,
            },
        );
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);

        nominal.exec(&mut state, 0).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.status_line(), "compresseur prêt");
    }

    #[test]
    fn one_slot_at_or_above_0_6_keeps_nominal() {
        let mut state = test_support::fresh();
        let low = test_support::low_vpd();
        let held = ProbeMeasure {
            vpd: VPD_LOW_KPA,
            ..low
        };
        test_support::fill_with_one_difference(&mut state, low, held);
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);

        nominal.exec(&mut state, 0).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
    }

    #[test]
    fn high_vpd_at_19_degrees_or_below_leaves_both_relays_off() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::cool_dry());
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);

        nominal.exec(&mut state, 0).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_eq!(state.status_line(), "compresseur prêt");
    }

    #[test]
    fn an_in_band_cabinet_shows_compresseur_pret_when_rest_is_clear() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::in_band());
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 4);

        nominal.exec(&mut state, 4).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.status_line(), "compresseur prêt");
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
    }

    #[test]
    fn rest_status_counts_down_and_a_later_off_command_does_not_extend_it() {
        let mut state = test_support::fresh();
        test_support::start_rest(&mut state, 100);
        test_support::fill(&mut state, test_support::in_band());
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 100);

        nominal.exec(&mut state, 100).unwrap();
        assert_eq!(state.status_line(), "repos 600s");
        assert_eq!(state.compressor_last_stop_s(), Some(100));

        nominal
            .exec(&mut state, 100 + COMPRESSOR_REST_S - 1)
            .unwrap();
        assert_eq!(state.status_line(), "repos 1s");
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.compressor_last_stop_s(), Some(100));

        nominal.exec(&mut state, 100 + COMPRESSOR_REST_S).unwrap();
        assert_eq!(state.status_line(), "compresseur prêt");
        assert!(!state.rest_active(100 + COMPRESSOR_REST_S));
        assert_eq!(state.compressor_last_stop_s(), Some(100));
        assert!(state.rest_active(100 + COMPRESSOR_REST_S - 1));
    }

    #[test]
    fn a_sick_rack_holds_the_mode_and_the_relays() {
        let mut state = test_support::fresh();
        state.set_relays(false, true, 0).unwrap();
        test_support::fill(&mut state, test_support::hot_humid());
        state.sample_rack_mut().push(Err(crate::ProbeError));
        assert!(state.sample_rack().is_sick());
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);

        nominal.exec(&mut state, 0).unwrap();

        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert!(!state.relay_bank().sw1());
        assert!(state.relay_bank().sw2());
        assert_eq!(state.status_line(), "! * *");
    }

    #[test]
    fn an_incomplete_rack_shows_the_glyph_line_and_does_not_switch() {
        let mut state = test_support::fresh();
        state.sample_rack_mut().push(Ok(test_support::hot_humid()));
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);

        nominal.exec(&mut state, 3).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.status_line(), "* ? ?");
        assert!(!state.relay_bank().sw1());
    }

    #[test]
    fn a_dead_rack_does_not_apply_nominal_rules() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::hot_dry());
        test_support::kill(&mut state);
        state.set_status_line("keep").unwrap();
        let nominal = test_support::enter::<NominalMode>(&mut state, AppMode::Nominal, 0);

        nominal.exec(&mut state, 15).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.status_line(), "keep");
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
    }
}
