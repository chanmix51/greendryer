//! Display worker for the cabinet that holds vegetal material.
//!
//! [`DisplayActor`] reads a published [`AppState`] and paints one frame when
//! that state changes. The 100 ms wake belongs to the firmware task. This
//! actor only decides whether the panel needs a new frame.

use crate::{AppState, Panel, PanelView, ProbeMeasure, SampleRack};

/// Paints the cabinet panel from the published state.
///
/// `D` is the panel. The real SSD1306 framebuffer stays inside this actor
/// because the firmware task stores the panel here.
pub struct DisplayActor<D, const N: usize, const L: usize> {
    panel: D,
    shown: Option<AppState<N, L>>,
    frames: u32,
}

impl<D, const N: usize, const L: usize> DisplayActor<D, N, L> {
    /// An actor that has not painted a frame yet.
    pub const fn new(panel: D) -> Self {
        Self {
            panel,
            shown: None,
            frames: 0,
        }
    }

    /// Frames painted since [`Self::new`].
    ///
    /// A refresh that sees the same published state does not increment this.
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
    /// Paint `state` when it differs from the last frame.
    pub async fn refresh(&mut self, state: &AppState<N, L>) {
        if self.shown.as_ref() == Some(state) {
            return;
        }
        let view = panel_view(state);
        self.panel.render(&view).await;
        self.shown = Some(state.clone());
        self.frames = self.frames.saturating_add(1);
    }
}

/// Frame contents for one published state.
///
/// Temperature, humidity, and vapor-pressure deficit are drawn only from a
/// healthy rack. A failed, sick, incomplete, or empty rack leaves that area
/// blank; the status line already carries the failure text.
fn panel_view<const N: usize, const L: usize>(state: &AppState<N, L>) -> PanelView<'_> {
    let relays = state.relay_bank();
    PanelView {
        title: state.app_mode().title(),
        fridge_on: relays.sw1(),
        dehumidifier_on: relays.sw2(),
        status_line: state.status_line(),
        reading: displayed_reading(state.sample_rack()),
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
        ready(actor.refresh(&state));
        assert_eq!(actor.frames(), 1);
        assert!(
            any_on(&actor, 3, 13),
            "the first frame should paint the yellow-band title"
        );
        assert!(
            !any_on(&actor, 26, 46),
            "an empty rack must not invent a temperature"
        );

        let published = state.clone();
        ready(actor.refresh(&published));
        assert_eq!(actor.frames(), 1);

        state.set_status_line("1s").unwrap();
        ready(actor.refresh(&state));
        assert_eq!(actor.frames(), 2);
        assert!(any_on(&actor, 3, 13));

        ready(actor.refresh(&state));
        assert_eq!(actor.frames(), 2);
    }

    #[test]
    fn hides_the_reading_until_the_rack_is_healthy() {
        let mut actor = new_actor();
        let mut state = AppState::<N, L>::new();
        let sample = ProbeMeasure::new(1, 20.0, 50);

        state.sample_rack_mut().push(Ok(sample));
        state.sample_rack_mut().push(Ok(sample));
        ready(actor.refresh(&state));
        assert!(
            !any_on(&actor, 26, 46),
            "an incomplete rack must not invent a temperature"
        );

        state.sample_rack_mut().push(Ok(sample));
        ready(actor.refresh(&state));
        assert_eq!(actor.frames(), 2);
        assert!(
            any_on(&actor, 26, 46),
            "a healthy rack should paint the latest sample"
        );

        state.sample_rack_mut().push(Err(crate::ProbeError));
        ready(actor.refresh(&state));
        assert_eq!(actor.frames(), 3);
        assert!(
            !any_on(&actor, 26, 46),
            "a sick rack must not keep showing a temperature"
        );
    }
}
