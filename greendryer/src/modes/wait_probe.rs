//! `WAIT_PROBE`: the sample rack is incomplete or dead.
//!
//! Both relays stay off. Every slot must hold a valid sample before the
//! cabinet leaves for `NOMINAL`. The mode that was interrupted is not resumed.

use crate::{AppMode, AppState};

use super::{AppModError, ApplicationMode, publish_glyphs};

/// Initial policy, and the policy entered when the sample rack is dead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaitProbeMode;

impl<const N: usize, const L: usize> ApplicationMode<N, L> for WaitProbeMode {
    fn init(state: &mut AppState<N, L>, now_s: u32) -> Result<Self, AppModError> {
        super::command_relays(state, false, false, now_s)?;
        publish_glyphs(state)?;
        Ok(Self)
    }

    fn exec(&self, state: &mut AppState<N, L>, now_s: u32) -> Result<(), AppModError> {
        if !state.sample_rack().is_healthy() {
            return publish_glyphs(state);
        }
        state.switch_mode(AppMode::Nominal, now_s);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppMode, AppState, COMPRESSOR_REST_S, ProbeError};

    use super::super::ApplicationMode;
    use super::super::test_support::{self, L, N};
    use super::WaitProbeMode;

    #[test]
    fn init_turns_every_relay_off() {
        let mut state = test_support::fresh();
        state.set_relays(true, true, 0).unwrap();

        let mode = WaitProbeMode::init(&mut state, 25).unwrap();
        let _ = mode;

        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        test_support::assert_spares_off(&state);
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.status_line(), "? ? ?");
        assert_eq!(AppMode::WaitProbe.title(), "ATT. SONDE");
    }

    #[test]
    fn init_starts_rest_only_when_the_fridge_was_on() {
        let mut state = test_support::fresh();
        state.set_relays(true, false, 0).unwrap();
        WaitProbeMode::init(&mut state, 25).unwrap();
        assert_eq!(state.compressor_last_stop_s(), Some(25));
        assert!(state.rest_active(25));
        assert!(!state.rest_active(25 + COMPRESSOR_REST_S));
        assert_eq!(state.status_line(), "? ? ?");

        let mut already_off = test_support::fresh();
        WaitProbeMode::init(&mut already_off, 25).unwrap();
        assert!(already_off.compressor_last_stop_s().is_none());

        WaitProbeMode::init(&mut already_off, 80).unwrap();
        assert!(already_off.compressor_last_stop_s().is_none());
        assert!(!already_off.rest_active(80));
    }

    #[test]
    fn an_incomplete_rack_shows_the_glyph_line_and_stays() {
        let mut state = test_support::fresh();
        let mode = WaitProbeMode::init(&mut state, 0).unwrap();

        mode.exec(&mut state, 12).unwrap();
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.status_line(), "? ? ?");
        assert_eq!(state.status_line().len(), 2 * N - 1);
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());

        state.sample_rack_mut().push(Ok(test_support::hot_dry()));
        mode.exec(&mut state, 40).unwrap();
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.status_line(), "* ? ?");
    }

    #[test]
    fn a_sick_rack_shows_the_glyph_line_and_holds_the_relays() {
        let mut state = test_support::fresh();
        state.sample_rack_mut().push(Ok(test_support::hot_dry()));
        state.sample_rack_mut().push(Err(ProbeError));
        state.sample_rack_mut().push(Ok(test_support::hot_dry()));
        assert!(state.sample_rack().is_sick());

        let mode = WaitProbeMode::init(&mut state, 0).unwrap();
        mode.exec(&mut state, 4).unwrap();

        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.status_line(), "* ! *");
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_ne!(state.app_mode(), AppMode::Nominal);
    }

    #[test]
    fn a_dead_rack_shows_the_glyph_line_and_not_compressor_rest() {
        let mut state = test_support::fresh();
        state.set_relays(true, false, 0).unwrap();
        test_support::kill(&mut state);
        state.switch_mode(AppMode::Nominal, 0);
        state.switch_mode(AppMode::WaitProbe, 40);

        let mode = WaitProbeMode::init(&mut state, 40).unwrap();
        assert_eq!(state.compressor_last_stop_s(), Some(40));
        mode.exec(&mut state, 40).unwrap();
        assert_eq!(state.status_line(), "! ! !");
        assert_eq!(state.app_mode(), AppMode::WaitProbe);

        mode.exec(&mut state, 52).unwrap();
        assert_eq!(state.status_line(), "! ! !");
        assert!(state.rest_active(52));
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
    }

    #[test]
    fn a_single_error_slot_is_dead_and_shows_the_glyph_line() {
        let mut state = AppState::<1, L>::new();
        state.sample_rack_mut().push(Err(ProbeError));
        assert!(state.sample_rack().is_dead());

        let mode = WaitProbeMode::init(&mut state, 0).unwrap();
        mode.exec(&mut state, 8).unwrap();
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.status_line(), "!");
    }

    #[test]
    fn a_healthy_rack_switches_to_nominal_and_leaves_the_relays_off() {
        let mut state = test_support::fresh();
        test_support::fill(&mut state, test_support::hot_dry());

        let mode = WaitProbeMode::init(&mut state, 7).unwrap();
        mode.exec(&mut state, 7).unwrap();

        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.mode_since_s(), 7);
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        test_support::assert_spares_off(&state);
    }

    #[test]
    fn one_missing_slot_stays_in_wait_probe() {
        let mut state = test_support::fresh();
        state.sample_rack_mut().push(Ok(test_support::hot_dry()));
        state.sample_rack_mut().push(Ok(test_support::hot_dry()));
        assert!(state.sample_rack().is_incomplete());

        let mode = WaitProbeMode::init(&mut state, 0).unwrap();
        mode.exec(&mut state, 1).unwrap();
        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.status_line(), "* * ?");
    }
}
