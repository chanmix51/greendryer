//! Published state of the cabinet controller.
//!
//! [`AppState`] owns the mode, the sample rack, compressor rest, the logical
//! relays, and the status line. The mode decision updates this value. The
//! probe, the relay pins, and the panel stay outside it.

use heapless::String;

use crate::{AppMode, RelayBank, SampleRack};

/// Whole seconds of compressor rest after each fridge on-to-off edge.
pub const COMPRESSOR_REST_S: u32 = 600;

/// Status text when compressor rest is clear.
///
/// The phrase is 16 characters and 17 bytes long.
pub const COMPRESSOR_READY: &str = "compresseur prêt";

/// The fridge was commanded on while compressor rest was still active.
///
/// The relay bank and the rest timestamp are left unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompressorRestError;

/// General state of the cabinet controller.
///
/// `N` is the sample-rack capacity. `L` is the status-line capacity in bytes.
/// `L` has to hold [`COMPRESSOR_READY`] and a glyph line of length `2N - 1`.
pub struct AppState<const N: usize, const L: usize> {
    app_mode: AppMode,
    /// Seconds of the last mode switch. Boot leaves this at 0.
    mode_since_s: u32,
    sample_rack: SampleRack<N>,
    /// Seconds of the last fridge on-to-off edge.
    ///
    /// Boot leaves this unset, so rest starts clear. Commanding the fridge off
    /// while it is already off does not replace the timestamp.
    compressor_last_stop_s: Option<u32>,
    relay_bank: RelayBank,
    status_line: String<L>,
}

impl<const N: usize, const L: usize> AppState<N, L> {
    /// Boot state: `WaitProbe`, every relay off, compressor rest clear, and an empty rack.
    ///
    /// The status line starts empty. The active mode fills it.
    pub fn new() -> Self {
        const {
            assert!(N > 0, "a sample rack needs at least one slot");
            assert!(
                L >= COMPRESSOR_READY.len(),
                "status line must hold 'compresseur prêt'"
            );
            assert!(
                L >= 2 * N - 1,
                "status line must hold a glyph line of length 2N - 1"
            );
        }
        Self {
            app_mode: AppMode::WaitProbe,
            mode_since_s: 0,
            sample_rack: SampleRack::new(),
            compressor_last_stop_s: None,
            relay_bank: RelayBank::all_off(),
            status_line: String::new(),
        }
    }

    /// Policy the controller is following.
    pub const fn app_mode(&self) -> AppMode {
        self.app_mode
    }

    /// Seconds of the last mode switch.
    pub const fn mode_since_s(&self) -> u32 {
        self.mode_since_s
    }

    /// Whole seconds since the last mode switch.
    pub const fn mode_elapsed_s(&self, now_s: u32) -> u32 {
        now_s.wrapping_sub(self.mode_since_s)
    }

    /// Record `mode` and `now_s` when the policy changes.
    ///
    /// Staying in the current mode leaves the switch timestamp where it is,
    /// so a deadline counted from entry keeps running.
    pub fn switch_mode(&mut self, mode: AppMode, now_s: u32) {
        if self.app_mode != mode {
            self.app_mode = mode;
            self.mode_since_s = now_s;
        }
    }

    /// Samples currently held for the cabinet.
    pub const fn sample_rack(&self) -> &SampleRack<N> {
        &self.sample_rack
    }

    /// Sample rack, for storing the next probe result.
    pub const fn sample_rack_mut(&mut self) -> &mut SampleRack<N> {
        &mut self.sample_rack
    }

    /// Timestamp of the last fridge on-to-off edge, if one has happened.
    pub const fn compressor_last_stop_s(&self) -> Option<u32> {
        self.compressor_last_stop_s
    }

    /// Whole seconds of compressor rest still remaining.
    ///
    /// Zero when rest is clear: at boot, or once [`COMPRESSOR_REST_S`] seconds
    /// have passed since the last fridge on-to-off edge.
    pub const fn rest_remaining_s(&self, now_s: u32) -> u32 {
        let Some(stopped_at) = self.compressor_last_stop_s else {
            return 0;
        };
        let elapsed = now_s.wrapping_sub(stopped_at);
        COMPRESSOR_REST_S.saturating_sub(elapsed)
    }

    /// Compressor rest is active for 600 s after a fridge on-to-off edge.
    pub const fn rest_active(&self, now_s: u32) -> bool {
        self.rest_remaining_s(now_s) > 0
    }

    /// Logical relay states. `false` is off.
    pub const fn relay_bank(&self) -> RelayBank {
        self.relay_bank
    }

    /// Store the fridge and dehumidifier commands.
    ///
    /// `true` is on. SW3 and SW4 stay off. `now_s` is stored only when the
    /// fridge goes from on to off. Commanding it off while it is already off
    /// leaves the edge timestamp unchanged.
    ///
    /// Returns [`CompressorRestError`] when `fridge_on` is set while compressor
    /// rest is still active. The relays and the rest timestamp stay as they were.
    pub const fn set_relays(
        &mut self,
        fridge_on: bool,
        dehumidifier_on: bool,
        now_s: u32,
    ) -> Result<(), CompressorRestError> {
        if fridge_on && self.rest_active(now_s) {
            return Err(CompressorRestError);
        }
        if self.relay_bank.sw1() && !fridge_on {
            self.compressor_last_stop_s = Some(now_s);
        }
        self.relay_bank = RelayBank::new(fridge_on, dehumidifier_on);
        Ok(())
    }

    /// Status line currently published for the panel.
    pub fn status_line(&self) -> &str {
        self.status_line.as_str()
    }

    /// Replace the status line.
    ///
    /// The previous line stays in place when `line` does not fit.
    pub fn set_status_line(&mut self, line: &str) -> Result<(), heapless::CapacityError> {
        let mut updated = String::new();
        updated.push_str(line)?;
        self.status_line = updated;
        Ok(())
    }
}

impl<const N: usize, const L: usize> Default for AppState<N, L> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use crate::{AppMode, ProbeError};

    use super::{AppState, COMPRESSOR_READY, COMPRESSOR_REST_S, CompressorRestError};

    const N: usize = 3;
    /// Byte capacity for `compresseur prêt`. The phrase is 16 characters; `ê` is two bytes.
    const L: usize = 17;

    #[test]
    fn boot_state_is_wait_probe_relays_off_rest_clear_and_an_empty_rack() {
        let state = AppState::<N, L>::new();

        assert_eq!(state.app_mode(), AppMode::WaitProbe);
        assert_eq!(state.mode_since_s(), 0);

        let relays = state.relay_bank();
        assert!(!relays.sw1());
        assert!(!relays.sw2());
        assert!(!relays.sw3());
        assert!(!relays.sw4());

        assert!(state.compressor_last_stop_s().is_none());
        assert!(!state.rest_active(0));
        assert!(!state.rest_active(COMPRESSOR_REST_S));
        assert_eq!(state.rest_remaining_s(0), 0);

        assert!(state.sample_rack().is_incomplete());
        assert!(!state.sample_rack().is_healthy());
        assert!(!state.sample_rack().is_sick());
        assert!(!state.sample_rack().is_dead());
        assert!(state.sample_rack().slots().iter().all(Option::is_none));

        assert_eq!(state.status_line(), "");
    }

    #[test]
    fn rest_becomes_active_only_after_a_fridge_on_to_off_edge_for_600_seconds() {
        let mut state = AppState::<N, L>::new();

        state.set_relays(true, false, 10).unwrap();
        assert!(state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert!(!state.relay_bank().sw3());
        assert!(!state.relay_bank().sw4());
        assert!(state.compressor_last_stop_s().is_none());
        assert!(!state.rest_active(10));

        state.set_relays(false, true, 1_000).unwrap();
        assert!(!state.relay_bank().sw1());
        assert!(state.relay_bank().sw2());
        assert_eq!(state.compressor_last_stop_s(), Some(1_000));
        assert!(state.rest_active(1_000));
        assert_eq!(state.rest_remaining_s(1_000), COMPRESSOR_REST_S);
        assert!(state.rest_active(1_000 + COMPRESSOR_REST_S - 1));
        assert_eq!(state.rest_remaining_s(1_000 + COMPRESSOR_REST_S - 1), 1);
        assert!(!state.rest_active(1_000 + COMPRESSOR_REST_S));
        assert_eq!(state.rest_remaining_s(1_000 + COMPRESSOR_REST_S), 0);

        state.set_relays(true, false, 2_000).unwrap();
        assert_eq!(state.compressor_last_stop_s(), Some(1_000));
        assert!(!state.rest_active(2_000));

        state.set_relays(false, false, 2_500).unwrap();
        assert_eq!(state.compressor_last_stop_s(), Some(2_500));
        assert!(state.rest_active(2_500));
        assert!(state.rest_active(2_500 + COMPRESSOR_REST_S - 1));
        assert!(!state.rest_active(2_500 + COMPRESSOR_REST_S));
    }

    #[test]
    fn fridge_off_while_already_off_does_not_move_the_edge_timestamp() {
        let mut state = AppState::<N, L>::new();

        state.set_relays(false, false, 50).unwrap();
        assert!(state.compressor_last_stop_s().is_none());
        assert!(!state.rest_active(50));

        state.set_relays(true, false, 80).unwrap();
        state.set_relays(false, false, 100).unwrap();
        assert_eq!(state.compressor_last_stop_s(), Some(100));

        state.set_relays(false, true, 400).unwrap();
        assert_eq!(state.compressor_last_stop_s(), Some(100));
        assert!(state.rest_active(100 + COMPRESSOR_REST_S - 1));
        assert!(!state.rest_active(100 + COMPRESSOR_REST_S));
        assert!(!state.rest_active(400 + COMPRESSOR_REST_S - 1));
    }

    #[test]
    fn fridge_start_during_rest_is_refused() {
        let mut state = AppState::<N, L>::new();
        state.set_relays(true, false, 10).unwrap();
        state.set_relays(false, true, 20).unwrap();

        let during_rest = 20 + COMPRESSOR_REST_S - 1;
        assert_eq!(
            state.set_relays(true, false, during_rest),
            Err(CompressorRestError)
        );
        assert!(!state.relay_bank().sw1());
        assert!(state.relay_bank().sw2());
        assert_eq!(state.compressor_last_stop_s(), Some(20));

        state.set_relays(false, false, during_rest).unwrap();
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert_eq!(state.compressor_last_stop_s(), Some(20));

        state
            .set_relays(true, false, 20 + COMPRESSOR_REST_S)
            .unwrap();
        assert!(state.relay_bank().sw1());
    }

    #[test]
    fn status_line_holds_compressor_ready_and_a_glyph_line() {
        let mut state = AppState::<N, L>::new();
        state.set_status_line(COMPRESSOR_READY).unwrap();
        assert_eq!(state.status_line(), "compresseur prêt");
        assert_eq!(state.status_line().chars().count(), 16);
        assert_eq!(COMPRESSOR_READY.len(), L);

        const WIDE: usize = 10;
        const WIDE_L: usize = 2 * WIDE - 1;
        let mut wide = AppState::<WIDE, WIDE_L>::new();
        wide.sample_rack_mut().push(Err(ProbeError));
        let glyphs = wide.sample_rack().glyph_line::<WIDE_L>();
        assert_eq!(glyphs.len(), 2 * WIDE - 1);
        wide.set_status_line(glyphs.as_str()).unwrap();
        assert_eq!(wide.status_line(), "! ? ? ? ? ? ? ? ? ?");
        assert_eq!(wide.status_line().len(), 2 * WIDE - 1);

        wide.set_status_line(COMPRESSOR_READY).unwrap();
        assert_eq!(wide.status_line(), "compresseur prêt");
    }

    #[test]
    fn mode_switch_records_the_timestamp_and_a_repeat_does_not() {
        let mut state = AppState::<N, L>::new();
        assert_eq!(state.mode_elapsed_s(12), 12);

        state.switch_mode(AppMode::Nominal, 40);
        assert_eq!(state.app_mode(), AppMode::Nominal);
        assert_eq!(state.mode_since_s(), 40);
        assert_eq!(state.mode_elapsed_s(55), 15);

        state.switch_mode(AppMode::Nominal, 90);
        assert_eq!(state.mode_since_s(), 40);
        assert_eq!(state.app_mode(), AppMode::Nominal);
    }
}
