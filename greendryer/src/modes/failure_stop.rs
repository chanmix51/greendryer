//! `FAILURE_STOP`: `TEMP_ENFORCING` or `HR_ENFORCING` missed its deadline.
//!
//! Both relays stay off. The status line is the whole seconds since this
//! entry. The sample rack is not consulted. Only a reboot leaves this policy,
//! and that reboot starts in `WAIT_PROBE` with compressor rest clear.

use crate::AppState;

use super::{AppModError, ApplicationMode, command_relays, publish_elapsed};

/// Both relays off until the cabinet controller is rebooted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FailureStopMode;

impl<const N: usize, const L: usize> ApplicationMode<N, L> for FailureStopMode {
    fn init(state: &mut AppState<N, L>, now_s: u32) -> Result<Self, AppModError> {
        command_relays(state, false, false, now_s)?;
        Ok(Self)
    }

    fn exec(&self, state: &mut AppState<N, L>, now_s: u32) -> Result<(), AppModError> {
        publish_elapsed(state, now_s)
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppMode, COMPRESSOR_REST_S, ProbeError};

    use super::super::ApplicationMode;
    use super::super::test_support;
    use super::FailureStopMode;

    #[test]
    fn init_turns_every_relay_off_and_starts_rest_if_the_fridge_was_on() {
        let mut state = test_support::fresh();
        state.set_relays(true, true, 0).unwrap();

        FailureStopMode::init(&mut state, 30).unwrap();

        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        test_support::assert_spares_off(&state);
        assert_eq!(state.compressor_last_stop_s(), Some(30));
        assert!(state.rest_active(30));
        assert!(!state.rest_active(30 + COMPRESSOR_REST_S));
    }

    #[test]
    fn init_does_not_start_rest_when_the_fridge_is_already_off() {
        let mut state = test_support::fresh();
        state.set_relays(false, true, 0).unwrap();

        FailureStopMode::init(&mut state, 30).unwrap();
        assert!(state.compressor_last_stop_s().is_none());
        assert!(!state.relay_bank().sw2());

        FailureStopMode::init(&mut state, 90).unwrap();
        assert!(state.compressor_last_stop_s().is_none());
    }

    #[test]
    fn exec_shows_the_seconds_since_this_entry() {
        let mut state = test_support::fresh();
        let mode = test_support::enter::<FailureStopMode>(&mut state, AppMode::FailureStop, 11);

        mode.exec(&mut state, 11).unwrap();
        assert_eq!(state.status_line(), "0s");
        assert_eq!(state.app_mode(), AppMode::FailureStop);

        mode.exec(&mut state, 28).unwrap();
        assert_eq!(state.status_line(), "17s");
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
    }

    #[test]
    fn a_healthy_rack_does_not_leave() {
        let mut state = test_support::fresh();
        let mode = test_support::enter::<FailureStopMode>(&mut state, AppMode::FailureStop, 0);

        for sample in [
            test_support::hot_dry(),
            test_support::hot_humid(),
            test_support::low_vpd(),
            test_support::cooled(),
            test_support::in_band(),
        ] {
            test_support::fill(&mut state, sample);
            mode.exec(&mut state, 60).unwrap();
            assert_eq!(state.app_mode(), AppMode::FailureStop);
            assert_eq!(state.status_line(), "60s");
            assert!(!state.relay_bank().sw1());
            assert!(!state.relay_bank().sw2());
        }
    }

    #[test]
    fn a_dead_rack_does_not_enter_wait_probe() {
        let mut state = test_support::fresh();
        let mode = test_support::enter::<FailureStopMode>(&mut state, AppMode::FailureStop, 4);
        test_support::kill(&mut state);

        mode.exec(&mut state, 19).unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_ne!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.status_line(), "15s");
    }

    #[test]
    fn a_sick_or_incomplete_rack_still_shows_seconds() {
        let mut state = test_support::fresh();
        let mode = test_support::enter::<FailureStopMode>(&mut state, AppMode::FailureStop, 0);

        mode.exec(&mut state, 2).unwrap();
        assert_eq!(state.status_line(), "2s");
        assert_ne!(state.status_line(), "? ? ?");

        state.sample_rack_mut().push(Ok(test_support::hot_dry()));
        state.sample_rack_mut().push(Err(ProbeError));
        mode.exec(&mut state, 6).unwrap();
        assert_eq!(state.app_mode(), AppMode::FailureStop);
        assert_eq!(state.status_line(), "6s");
        assert_ne!(state.status_line(), "* ! ?");
    }
}
