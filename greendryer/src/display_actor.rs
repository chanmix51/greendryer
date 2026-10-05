//! Display worker for the cabinet that holds vegetal material.
//!
//! [`DisplayActor`] reads a published [`AppState`] and paints one frame when
//! that state changes, or when the fridge triangle for this second differs
//! from the triangle in the last frame. The 100 ms wake belongs to the
//! firmware task. This actor only decides whether the panel needs a new frame.

use crate::{AppState, FridgeTriangle, Panel, PanelView, ProbeMeasure, SampleRack};

/// Paints the cabinet panel from the published state.
///
/// `D` is the panel. The real SSD1306 framebuffer stays inside this actor
/// because the firmware task stores the panel here.
pub struct DisplayActor<D, const N: usize, const L: usize> {
    panel: D,
    shown: Option<AppState<N, L>>,
    /// Fridge triangle painted in the last frame.
    ///
    /// This is the blink the panel is showing. It is not stored on [`AppState`].
    fridge: Option<FridgeTriangle>,
    frames: u32,
}

impl<D, const N: usize, const L: usize> DisplayActor<D, N, L> {
    /// An actor that has not painted a frame yet.
    pub const fn new(panel: D) -> Self {
        Self {
            panel,
            shown: None,
            fridge: None,
            frames: 0,
        }
    }

    /// Frames painted since [`Self::new`].
    ///
    /// A refresh that sees the same published state and the same fridge
    /// triangle does not increment this.
    pub const fn frames(&self) -> u32 {
        self.frames
    }

    /// The panel this actor draws into.
    pub const fn panel(&self) -> &D {
        &self.panel
    }
}

impl<D, const N: usize, const L: usize> DisplayActor<D, N, L>
where
    D: Panel,
{
    /// Paint when `state` differs from the last frame, or when the fridge
    /// triangle for `now_s` differs from the triangle in that frame.
    ///
    /// `now_s` is the whole second the control loop already uses. It is not a
    /// field of [`AppState`]. Compressor rest blinks the downward triangle:
    /// an odd second draws its outline, and an even second draws nothing.
    /// Two wakes in one second with the same state and the same triangle
    /// paint once.
    pub async fn refresh(&mut self, state: &AppState<N, L>, now_s: u32) {
        let fridge = fridge_triangle(state, now_s);
        if self.shown.as_ref() == Some(state) && self.fridge == Some(fridge) {
            return;
        }
        let view = panel_view(state, fridge);
        self.panel.render(&view).await;
        self.shown = Some(state.clone());
        self.fridge = Some(fridge);
        self.frames = self.frames.saturating_add(1);
    }
}

/// Frame contents for one published state.
///
/// Temperature, humidity, and vapor-pressure deficit are drawn only from a
/// healthy rack. A failed, sick, incomplete, or empty rack leaves those
/// readings blank; the status line already carries the failure text.
/// `WAIT_PROBE` still draws the software version on the bottom line of the
/// blue area.
fn panel_view<const N: usize, const L: usize>(
    state: &AppState<N, L>,
    fridge: FridgeTriangle,
) -> PanelView<'_> {
    let relays = state.relay_bank();
    PanelView {
        title: state.app_mode().title(),
        fridge,
        dehumidifier_on: relays.sw2(),
        status_line: state.status_line(),
        reading: displayed_reading(state.sample_rack()),
    }
}

/// Downward triangle for this second.
///
/// Compressor rest hides it on even seconds and draws the outline on odd
/// seconds. The fridge relay is off for that whole rest. With rest clear,
/// the triangle is filled when the fridge is on and an outline when it is
/// off, on both even and odd seconds.
fn fridge_triangle<const N: usize, const L: usize>(
    state: &AppState<N, L>,
    now_s: u32,
) -> FridgeTriangle {
    if state.rest_active(now_s) {
        if now_s.is_multiple_of(2) {
            FridgeTriangle::Hidden
        } else {
            FridgeTriangle::Outline
        }
    } else if state.relay_bank().sw1() {
        FridgeTriangle::Filled
    } else {
        FridgeTriangle::Outline
    }
}

fn displayed_reading<const N: usize>(rack: &SampleRack<N>) -> Option<ProbeMeasure> {
    if !rack.is_healthy() {
        return None;
    }
    match rack.latest() {
        Some(Ok(sample)) => Some(sample),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use core::future::Future;
    use core::task::{Context, Poll, Waker};

    use embedded_graphics::{mock_display::MockDisplay, pixelcolor::BinaryColor, prelude::*};

    use super::*;
    use crate::{MemoryPanel, ProbeMeasure};

    const N: usize = 3;
    const L: usize = 17;
    const WINDOW: i32 = 64;

    fn ready<F: Future<Output = ()>>(future: F) {
        let mut future = core::pin::pin!(future);
        let waker = Waker::noop();
        let mut context = Context::from_waker(waker);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(()) => {}
            Poll::Pending => panic!("the memory panel should complete in one poll"),
        }
    }

    fn new_actor() -> DisplayActor<MemoryPanel<MockDisplay<BinaryColor>>, N, L> {
        let mut display = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);
        display.set_allow_overdraw(true);
        DisplayActor::new(MemoryPanel::new(display))
    }

    fn is_on(
        actor: &DisplayActor<MemoryPanel<MockDisplay<BinaryColor>>, N, L>,
        x: i32,
        y: i32,
    ) -> bool {
        actor.panel().target().get_pixel(Point::new(x, y)) == Some(BinaryColor::On)
    }

    fn any_on(
        actor: &DisplayActor<MemoryPanel<MockDisplay<BinaryColor>>, N, L>,
        y0: i32,
        y1: i32,
    ) -> bool {
        (y0..y1).any(|y| (0..WINDOW).any(|x| is_on(actor, x, y)))
    }

    #[test]
    fn redraws_only_when_the_published_state_changes() {
        let mut actor = new_actor();
        let mut state = AppState::<N, L>::new();

        assert_eq!(actor.frames(), 0);
        ready(actor.refresh(&state, 0));
        assert_eq!(actor.frames(), 1);
        assert!(
            any_on(&actor, 3, 13),
            "the first frame should paint the yellow-band title"
        );
        assert!(
            !any_on(&actor, 16, 54),
            "an empty rack must not invent a temperature"
        );
        assert!(
            any_on(&actor, 54, 64),
            "WAIT_PROBE centers the software version on the bottom line"
        );

        let published = state.clone();
        ready(actor.refresh(&published, 0));
        assert_eq!(actor.frames(), 1);

        state.set_status_line("1s").unwrap();
        ready(actor.refresh(&state, 0));
        assert_eq!(actor.frames(), 2);
        assert!(any_on(&actor, 3, 13));

        ready(actor.refresh(&state, 0));
        assert_eq!(actor.frames(), 2);
    }

    #[test]
    fn hides_the_reading_until_the_rack_is_healthy() {
        let mut actor = new_actor();
        let mut state = AppState::<N, L>::new();
        let sample = ProbeMeasure::new(1, 20.0, 50);

        state.sample_rack_mut().push(Ok(sample));
        state.sample_rack_mut().push(Ok(sample));
        ready(actor.refresh(&state, 0));
        assert!(
            !any_on(&actor, 16, 54),
            "an incomplete rack must not invent a temperature"
        );
        assert!(
            any_on(&actor, 54, 64),
            "WAIT_PROBE keeps the version line while the rack is incomplete"
        );

        state.sample_rack_mut().push(Ok(sample));
        ready(actor.refresh(&state, 0));
        assert_eq!(actor.frames(), 2);
        assert!(
            any_on(&actor, 16, 64),
            "a healthy rack should paint the latest sample"
        );

        state.sample_rack_mut().push(Err(crate::ProbeError));
        ready(actor.refresh(&state, 0));
        assert_eq!(actor.frames(), 3);
        assert!(
            !any_on(&actor, 16, 54),
            "a sick rack must not keep showing a temperature"
        );
        assert!(
            any_on(&actor, 54, 64),
            "WAIT_PROBE keeps the version line while the rack is sick"
        );
    }

    /// `MockDisplay` is 64 pixels wide, so one frame is split the way the panel
    /// tests split it: x = 0 holds the title, x = 64 holds the triangles.
    struct FrameTarget {
        left: MockDisplay<BinaryColor>,
        right: MockDisplay<BinaryColor>,
    }

    impl FrameTarget {
        fn new() -> Self {
            let mut left = MockDisplay::new();
            let mut right = MockDisplay::new();
            left.set_allow_out_of_bounds_drawing(true);
            left.set_allow_overdraw(true);
            right.set_allow_out_of_bounds_drawing(true);
            right.set_allow_overdraw(true);
            Self { left, right }
        }
    }

    impl OriginDimensions for FrameTarget {
        fn size(&self) -> Size {
            Size::new(128, 64)
        }
    }

    impl DrawTarget for FrameTarget {
        type Color = BinaryColor;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(point, color) in pixels {
                if point.x < WINDOW {
                    self.left.draw_iter(core::iter::once(Pixel(point, color)))?;
                } else {
                    let local = Point::new(point.x - WINDOW, point.y);
                    self.right
                        .draw_iter(core::iter::once(Pixel(local, color)))?;
                }
            }
            Ok(())
        }
    }

    type FrameActor = DisplayActor<MemoryPanel<FrameTarget>, N, L>;

    fn new_frame_actor() -> FrameActor {
        DisplayActor::new(MemoryPanel::new(FrameTarget::new()))
    }

    fn right_on(actor: &FrameActor, x: i32, y: i32) -> bool {
        actor
            .panel()
            .target()
            .right
            .get_pixel(Point::new(x - WINDOW, y))
            == Some(BinaryColor::On)
    }

    type LitPixels = std::vec::Vec<(i32, i32)>;

    fn lit_pixels(
        display: &MockDisplay<BinaryColor>,
        x0: i32,
        x1: i32,
        y0: i32,
        y1: i32,
    ) -> LitPixels {
        let mut lit = LitPixels::new();
        for y in y0..y1 {
            for x in x0..x1 {
                if display.get_pixel(Point::new(x, y)) == Some(BinaryColor::On) {
                    lit.push((x, y));
                }
            }
        }
        lit
    }

    /// Title, glyph line, and upward triangle: everything except the fridge triangle.
    fn steady_pixels(actor: &FrameActor) -> (LitPixels, LitPixels) {
        let target = actor.panel().target();
        let left = lit_pixels(&target.left, 0, WINDOW, 0, WINDOW);
        let mut right = lit_pixels(&target.right, 0, FRIDGE_LOCAL_X0, 0, WINDOW);
        right.extend(lit_pixels(
            &target.right,
            FRIDGE_LOCAL_X1,
            WINDOW,
            0,
            WINDOW,
        ));
        (left, right)
    }

    fn fridge_center_x() -> i32 {
        (128 - 4) - 10 / 2
    }

    fn dehumidifier_center_x() -> i32 {
        fridge_center_x() - 10 - 4
    }

    /// Local x of the downward triangle in the right-hand window.
    const FRIDGE_LOCAL_X0: i32 = (128 - 4) - 10 - 64;
    const FRIDGE_LOCAL_X1: i32 = (128 - 4) - 64 + 1;

    fn assert_downward_hidden(actor: &FrameActor) {
        let lit = lit_pixels(
            &actor.panel().target().right,
            FRIDGE_LOCAL_X0,
            FRIDGE_LOCAL_X1,
            0,
            16,
        );
        assert!(
            lit.is_empty(),
            "even second during rest should leave the downward triangle undrawn, lit {lit:?}"
        );
    }

    fn assert_downward_outline(actor: &FrameActor) {
        let center = fridge_center_x();
        assert!(
            right_on(actor, center, 13),
            "the downward triangle tip should be drawn"
        );
        assert!(
            !right_on(actor, center, 6),
            "the downward triangle should be a 1-pixel outline"
        );
    }

    fn assert_downward_filled(actor: &FrameActor) {
        let center = fridge_center_x();
        assert!(
            right_on(actor, center, 13),
            "the downward triangle tip should be drawn"
        );
        assert!(
            right_on(actor, center, 6),
            "the downward triangle should be filled"
        );
    }

    fn assert_upward_outline(actor: &FrameActor) {
        let center = dehumidifier_center_x();
        assert!(
            right_on(actor, center, 3),
            "the upward triangle tip should stay drawn"
        );
        assert!(
            !right_on(actor, center, 9),
            "the upward triangle should stay an outline"
        );
    }

    fn assert_upward_filled(actor: &FrameActor) {
        let center = dehumidifier_center_x();
        assert!(right_on(actor, center, 3));
        assert!(
            right_on(actor, center, 9),
            "the upward triangle should stay filled"
        );
    }

    fn assert_title_and_glyph(actor: &FrameActor) {
        let target = actor.panel().target();
        assert!(
            !lit_pixels(&target.left, 0, WINDOW, 3, 13).is_empty(),
            "the yellow-band title should stay"
        );
        let mut blue = lit_pixels(&target.left, 0, WINDOW, 16, WINDOW);
        blue.extend(lit_pixels(&target.right, 0, WINDOW, 16, WINDOW));
        assert!(
            !blue.is_empty(),
            "the glyph line should stay under the yellow band"
        );
    }

    /// Dead rack in `WAIT_PROBE` after a fridge on-to-off edge at `stopped_at`.
    fn dead_wait_probe(stopped_at: u32) -> AppState<N, L> {
        use crate::modes::test_support;
        use crate::modes::{ApplicationMode, WaitProbeMode};

        let mut state = test_support::fresh();
        state
            .set_relays(true, false, stopped_at.saturating_sub(1))
            .unwrap();
        test_support::kill(&mut state);
        state.switch_mode(crate::AppMode::WaitProbe, stopped_at);
        let waiting = WaitProbeMode::init(&mut state, stopped_at).unwrap();
        waiting.exec(&mut state, stopped_at).unwrap();

        assert_eq!(state.app_mode(), crate::AppMode::WaitProbe);
        assert_eq!(state.status_line(), "! ! !");
        assert!(!state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());
        assert!(state.rest_active(stopped_at));
        state
    }

    #[test]
    fn rest_blinks_the_fridge_outline_on_an_unchanged_dead_rack() {
        let stopped_at = 10;
        let state = dead_wait_probe(stopped_at);
        let mut again = state.clone();
        again.sample_rack_mut().push(Err(crate::ProbeError));
        assert!(
            again == state,
            "another probe error on a dead rack compares equal"
        );

        let mut actor = new_frame_actor();
        ready(actor.refresh(&state, stopped_at));
        assert_eq!(actor.frames(), 1);
        assert_downward_hidden(&actor);
        assert_upward_outline(&actor);
        assert_title_and_glyph(&actor);
        let stayed = steady_pixels(&actor);

        ready(actor.refresh(&again, stopped_at + 1));
        assert_eq!(actor.frames(), 2);
        assert_downward_outline(&actor);
        assert_upward_outline(&actor);
        assert_title_and_glyph(&actor);
        assert_eq!(steady_pixels(&actor), stayed);
        assert_eq!(state.status_line(), "! ! !");

        ready(actor.refresh(&state, stopped_at + 1));
        assert_eq!(
            actor.frames(),
            2,
            "two wakes in the same second with the same triangle paint once"
        );
    }

    #[test]
    fn the_same_dead_rack_draws_the_fridge_outline_once_rest_clears() {
        let stopped_at = 10;
        let state = dead_wait_probe(stopped_at);
        let clear_at = stopped_at + crate::COMPRESSOR_REST_S;
        assert!(!state.rest_active(clear_at));
        assert!(state.rest_active(clear_at - 1));

        let mut actor = new_frame_actor();
        ready(actor.refresh(&state, clear_at - 2));
        assert_eq!(actor.frames(), 1);
        assert_downward_hidden(&actor);
        let stayed = steady_pixels(&actor);

        ready(actor.refresh(&state, clear_at));
        assert_eq!(actor.frames(), 2);
        assert_downward_outline(&actor);
        assert_upward_outline(&actor);
        assert_title_and_glyph(&actor);
        assert_eq!(steady_pixels(&actor), stayed);
        assert_eq!(state.status_line(), "! ! !");

        ready(actor.refresh(&state, clear_at + 1));
        assert_eq!(
            actor.frames(),
            2,
            "even and odd seconds share the outline, so the second wake does not paint again"
        );
        assert_downward_outline(&actor);

        let mut odd = new_frame_actor();
        ready(odd.refresh(&state, clear_at + 1));
        assert_eq!(odd.frames(), 1);
        assert_downward_outline(&odd);

        let mut even = new_frame_actor();
        ready(even.refresh(&state, clear_at));
        assert_downward_outline(&even);
    }

    #[test]
    fn rest_clear_fills_the_fridge_triangle_on_even_and_odd_seconds() {
        let mut state = AppState::<N, L>::new();
        state.set_relays(true, false, 4).unwrap();
        assert!(!state.rest_active(4));
        assert!(state.relay_bank().sw1());
        assert!(!state.relay_bank().sw2());

        let mut even = new_frame_actor();
        ready(even.refresh(&state, 4));
        assert_eq!(even.frames(), 1);
        assert_downward_filled(&even);
        assert_upward_outline(&even);
        ready(even.refresh(&state, 4));
        assert_eq!(even.frames(), 1);

        let mut odd = new_frame_actor();
        ready(odd.refresh(&state, 5));
        assert_eq!(odd.frames(), 1);
        assert_downward_filled(&odd);
        assert_upward_outline(&odd);

        ready(even.refresh(&state, 5));
        assert_eq!(
            even.frames(),
            1,
            "odd and even seconds paint the same filled triangle"
        );
        assert_downward_filled(&even);
    }

    #[test]
    fn rest_clear_outlines_the_fridge_triangle_on_even_and_odd_seconds() {
        let state = AppState::<N, L>::new();
        assert!(!state.rest_active(8));
        assert!(!state.relay_bank().sw1());

        let mut even = new_frame_actor();
        ready(even.refresh(&state, 8));
        assert_eq!(even.frames(), 1);
        assert_downward_outline(&even);
        assert_upward_outline(&even);

        let mut odd = new_frame_actor();
        ready(odd.refresh(&state, 9));
        assert_downward_outline(&odd);
        assert_upward_outline(&odd);

        ready(even.refresh(&state, 9));
        assert_eq!(even.frames(), 1);
        assert_downward_outline(&even);
    }

    #[test]
    fn nominal_keeps_an_empty_status_and_wait_probe_keeps_the_glyph_line() {
        use crate::modes::test_support;
        use crate::modes::{ApplicationMode, NominalMode};

        let mut nominal = test_support::fresh();
        test_support::fill(&mut nominal, test_support::in_band());
        test_support::start_rest(&mut nominal, 100);
        let mode = test_support::enter::<NominalMode>(&mut nominal, crate::AppMode::Nominal, 100);
        mode.exec(&mut nominal, 100).unwrap();
        assert_eq!(nominal.app_mode(), crate::AppMode::Nominal);
        assert_eq!(nominal.status_line(), "");
        assert!(nominal.rest_active(100));
        assert!(!nominal.relay_bank().sw1());

        mode.exec(&mut nominal, 101).unwrap();
        assert_eq!(nominal.status_line(), "");
        assert_eq!(nominal.app_mode(), crate::AppMode::Nominal);

        let mut actor = new_frame_actor();
        ready(actor.refresh(&nominal, 100));
        assert_downward_hidden(&actor);
        assert_upward_outline(&actor);
        ready(actor.refresh(&nominal, 101));
        assert_eq!(actor.frames(), 2);
        assert_downward_outline(&actor);
        assert_eq!(nominal.status_line(), "");

        let waiting = dead_wait_probe(40);
        assert_eq!(waiting.status_line(), "! ! !");
        assert!(waiting.rest_active(40));
        assert!(!waiting.status_line().contains("reste"));
    }

    #[test]
    fn the_upward_triangle_stays_filled_while_the_fridge_blinks() {
        let mut state = AppState::<N, L>::new();
        state.set_relays(true, true, 1).unwrap();
        state.set_relays(false, true, 2).unwrap();
        assert!(state.rest_active(2));
        assert!(!state.relay_bank().sw1());
        assert!(state.relay_bank().sw2());

        let mut actor = new_frame_actor();
        ready(actor.refresh(&state, 2));
        assert_downward_hidden(&actor);
        assert_upward_filled(&actor);
        let stayed = steady_pixels(&actor);

        ready(actor.refresh(&state, 3));
        assert_eq!(actor.frames(), 2);
        assert_downward_outline(&actor);
        assert_upward_filled(&actor);
        assert_eq!(steady_pixels(&actor), stayed);
    }
}
