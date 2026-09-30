//! `COMPRESSOR_WAIT`: a hot, dry cabinet waits until the compressor may start.
//!
//! Both relays stay off. This policy has no deadline, so the wait does not
//! count toward the 15 minutes of `TEMP_ENFORCING`. Humidity at or above 70%
//! leaves first. The rack returns to `NOMINAL` when every slot is under
//! 19°C, or when every slot is under 0.8 kPa. One slot meeting only one
//! side does not decide. Rest clearing starts `TEMP_ENFORCING`.

use crate::{AppMode, AppState};

use super::{
    AppModError, ApplicationMode, HR_EMERGENCY_PERCENT, TEMP_START_C, VPD_HIGH_KPA,
    block_sample_rules, command_relays, publish_rest,
};

/// Both relays off while compressor rest blocks cooling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompressorWaitMode;

impl<const N: usize, const L: usize> ApplicationMode<N, L> for CompressorWaitMode {
    fn init(state: &mut AppState<N, L>, now_s: u32) -> Result<Self, AppModError> {
        // The fridge is already off, so this does not restart rest.
        command_relays(state, false, false, now_s)?;
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

        let cooled = state
            .sample_rack()
            .meets(|sample| sample.temp < TEMP_START_C);
        let less_dry = state
            .sample_rack()
            .meets(|sample| sample.vpd < VPD_HIGH_KPA);
        if cooled || less_dry {
            state.switch_mode(AppMode::Nominal, now_s);
            return Ok(());
        }

        if !state.rest_active(now_s) {
            state.switch_mode(AppMode::TempEnforcing, now_s);
            return Ok(());
        }

        command_relays(state, false, false, now_s)?;
        publish_rest(state, now_s)
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppMode, COMPRESSOR_REST_S, ProbeMeasure};

    use super::super::test_support;
    use super::super::{ApplicationMode, TEMP_START_C, VPD_HIGH_KPA};
    use super::CompressorWaitMode;

    fn waiting_at(now_s: u32) -> (crate::AppState<3, 17>, CompressorWaitMode) {
        let mut state = test_support::fresh();
        test_support::start_rest(&mut state, now_s);
        test_support::fill(&mut state, test_support::hot_dry());
        let mode =
            test_support::enter::<CompressorWaitMode>(&mut state, AppMode::CompressorWait, now_s);
        (state, mode)
    }

    #[test]
    fn title_is_att_compress() {
        assert_eq!(AppMode::CompressorWait.behavior_name(), "COMPRESSOR_WAIT");
        assert_eq!(AppMode::CompressorWait.title(), "ATT. COMPRESS.");
        assert_eq!(AppMode::CompressorWait.title().chars().count(), 14);
    }

    #[test]
    fn init_turns_both_relays_off_without_restarting_rest() {
        let mut state = test_support::fresh();
        test_support::start_rest(&mut state, 10);
        state.set_relays(false, true, 12).unwrap();
        assert_eq!(state.compressor_last_stop_s(), Some(10));

        CompressorWaitMode::init(&mut state, 12).unwrap();

        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        test_support::assert_spares_off(&state);
        assert_eq!(state.compressor_last_stop_s(), Some(10));
    }

    #[test]
    fn a_healthy_rack_shows_the_rest_countdown_and_does_not_time_out() {
        let (mut state, mode) = waiting_at(10);

        mode.exec(&mut state, 10).unwrap();
        assert_eq!(state.app_mode(), AppMode::CompressorWait);
        assert_eq!(state.status_line(), "reste 600s");
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_eq!(state.mode_since_s(), 10);

        mode.exec(&mut state, 10 + COMPRESSOR_REST_S - 1).unwrap();
        assert_eq!(state.app_mode(), AppMode::CompressorWait);
        assert_eq!(state.status_line(), "reste 1s");
        assert_ne!(state.app_mode(), AppMode::FailureStop);
        assert_eq!(state.compressor_last_stop_s(), Some(10));
    }

    #[test]
    fn humidity_at_or_above_70_percent_preempts_the_wait_when_rest_clears() {
        let (mut state, mode) = waiting_at(10);
        test_support::fill(&mut state, test_support::hot_humid());

        mode.exec(&mut state, 10 + COMPRESSOR_REST_S).unwrap();

        assert_eq!(state.app_mode(), AppMode::HrEnforcing);
        assert_ne!(state.app_mode(), AppMode::TempEnforcing);
        assert!(!state.relay_bank().sw1());
        assert_eq!(state.compressor_last_stop_s(), Some(10));
    }

    #[test]
    fn every_recovered_slot_returns_to_nominal_even_when_rest_clears() {
        let (mut state, mode) = waiting_at(10);
        test_support::fill(&mut state, test_support::cool_dry());

        mode.exec(&mut state, 10 + COMPRESSOR_REST_S).unwrap();

        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_ne!(state.app_mode(), AppMode::TempEnforcing);
    }

    #[test]
    fn a_mix_of_cool_slots_and_less_dry_slots_stays() {
        let (mut state, mode) = waiting_at(10);
        let cool_and_dry = test_support::cool_dry();
        let hot_and_less_dry = ProbeMeasure {
            temp: 22.0,
            hygr: 60,
            vpd: 0.7,
            time: 0,
        };
        assert!(cool_and_dry.temp < TEMP_START_C);
        assert!(cool_and_dry.vpd >= VPD_HIGH_KPA);
        assert!(hot_and_less_dry.temp >= TEMP_START_C);
        assert!(hot_and_less_dry.vpd < VPD_HIGH_KPA);
        test_support::fill_with_one_difference(&mut state, cool_and_dry, hot_and_less_dry);

        mode.exec(&mut state, 20).unwrap();

        assert_eq!(state.app_mode(), AppMode::CompressorWait);
        assert_eq!(state.status_line(), "reste 590s");
    }

    #[test]
    fn every_slot_under_0_8_kpa_returns_even_when_the_cabinet_is_still_hot() {
        let (mut state, mode) = waiting_at(10);
        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 22.0,
                hygr: 60,
                vpd: 0.7,
                time: 0,
            },
        );

        mode.exec(&mut state, 20).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
    }

    #[test]
    fn one_slot_still_above_both_thresholds_keeps_the_wait() {
        let (mut state, mode) = waiting_at(10);
        test_support::fill_with_one_difference(
            &mut state,
            test_support::cool_dry(),
            test_support::hot_dry(),
        );

        mode.exec(&mut state, 20).unwrap();

        assert_eq!(state.app_mode(), AppMode::CompressorWait);
        assert_eq!(state.status_line(), "reste 590s");
        assert!(!state.relay_bank().sw1());
    }

    #[test]
    fn exactly_19_degrees_or_exactly_0_8_kpa_does_not_return() {
        let (mut state, mode) = waiting_at(10);
        let hot = test_support::hot_dry();
        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: TEMP_START_C,
                ..hot
            },
        );
        mode.exec(&mut state, 10).unwrap();
        assert_eq!(state.app_mode(), AppMode::CompressorWait);

        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 21.0,
                hygr: 50,
                vpd: VPD_HIGH_KPA,
                time: 0,
            },
        );
        mode.exec(&mut state, 10).unwrap();
        assert_eq!(state.app_mode(), AppMode::CompressorWait);
    }

    #[test]
    fn eighteen_degrees_at_exactly_0_8_kpa_returns_to_nominal() {
        let (mut state, mode) = waiting_at(10);
        test_support::fill(
            &mut state,
            ProbeMeasure {
                temp: 18.0,
                hygr: 40,
                vpd: VPD_HIGH_KPA,
                time: 0,
            },
        );

        mode.exec(&mut state, 10).unwrap();
        assert_eq!(state.app_mode(), AppMode::Nominal);
    }

    #[test]
    fn rest_clear_starts_temp_enforcing_when_one_slot_is_still_hot_and_dry() {
        let (mut state, mode) = waiting_at(10);
        test_support::fill_with_one_difference(
            &mut state,
            test_support::cool_dry(),
            test_support::hot_dry(),
        );

        let cleared = 10 + COMPRESSOR_REST_S;
        mode.exec(&mut state, cleared).unwrap();

        assert_eq!(state.app_mode(), AppMode::TempEnforcing);
        assert_eq!(state.mode_since_s(), cleared);
        assert!(!state.rest_active(cleared));
    }

    #[test]
    fn a_sick_rack_shows_the_glyph_line_and_holds_the_mode() {
        let (mut state, mode) = waiting_at(10);
        state.sample_rack_mut().push(Err(crate::ProbeError));
        assert!(state.sample_rack().is_sick());

        mode.exec(&mut state, 30).unwrap();

        assert_eq!(state.app_mode(), AppMode::CompressorWait);
        assert_eq!(state.status_line(), "! * *");
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
    }

    #[test]
    fn a_dead_rack_does_not_apply_compressor_wait_rules() {
        let (mut state, mode) = waiting_at(10);
        test_support::kill(&mut state);
        state.set_status_line("keep").unwrap();

        mode.exec(&mut state, 40).unwrap();

        assert_eq!(state.app_mode(), AppMode::CompressorWait);
        assert_eq!(state.status_line(), "keep");
        assert!(!state.relay_bank().sw1());
    }
}
