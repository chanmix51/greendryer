//! Relay commands and the panel frame.
//!
//! [`RelayBank`] stores the logical state of the four relays. [`RelayOutput`]
//! applies the fridge and the dehumidifier to pins. [`RecordingRelay`] keeps
//! those commands, and the active-low pin levels they drive, in memory.
//!
//! [`Panel`] paints one frame into a [`DrawTarget`](embedded_graphics::draw_target::DrawTarget).
//! [`MemoryPanel`] is the in-memory implementation.

use core::fmt::Write;

use embedded_graphics::{
    mono_font::{
        MonoFont, MonoTextStyle,
        iso_8859_1::{FONT_6X10, FONT_10X20},
    },
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle, Triangle},
    text::{Baseline, Text},
};
use embedded_layout::align::{Align, horizontal, vertical};

use crate::ProbeMeasure;

const PANEL_WIDTH: i32 = 128;
const PANEL_HEIGHT: i32 = 64;
const YELLOW_BAND_HEIGHT: i32 = 16;

const GLYPH_6_WIDTH: i32 = 6;
const GLYPH_6_HEIGHT: i32 = 10;
const GLYPH_10_HEIGHT: i32 = 20;

const TEXT_X: i32 = 4;
const TITLE_Y: i32 = (YELLOW_BAND_HEIGHT - GLYPH_6_HEIGHT) / 2;
const BLUE_TOP: i32 = YELLOW_BAND_HEIGHT;
const BLUE_HEIGHT: i32 = PANEL_HEIGHT - YELLOW_BAND_HEIGHT;
/// Status, temperature, and vapor-pressure deficit stacked with no gap.
const BLUE_BLOCK_HEIGHT: i32 = GLYPH_6_HEIGHT + GLYPH_10_HEIGHT + GLYPH_6_HEIGHT;

const TRI_W: i32 = 10;
const TRI_H: i32 = 10;
const TRI_GAP: i32 = 4;
const TRI_RIGHT_PAD: i32 = 4;
const TRI_TOP: i32 = (YELLOW_BAND_HEIGHT - TRI_H) / 2;

const FRIDGE_CX: i32 = (PANEL_WIDTH - TRI_RIGHT_PAD) - TRI_W / 2;
const DEHUM_CX: i32 = FRIDGE_CX - TRI_W - TRI_GAP;
const DEHUM_LEFT: i32 = DEHUM_CX - TRI_W / 2;

const _: () = assert!(TEXT_X + 16 * GLYPH_6_WIDTH == DEHUM_LEFT);
const _: () = assert!(BLUE_BLOCK_HEIGHT <= BLUE_HEIGHT);
const _: () = assert!(TITLE_Y == TRI_TOP);

/// The blue area under the yellow band.
const BLUE_AREA: Rectangle = Rectangle::new(
    Point::new(0, BLUE_TOP),
    Size::new(PANEL_WIDTH as u32, BLUE_HEIGHT as u32),
);

/// Pin level driven by an active-low relay.
///
/// Logical on drives the pin low. Logical off drives the pin high, which is
/// the idle, safe level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinLevel {
    Low,
    High,
}

impl PinLevel {
    /// Map a logical relay command onto an active-low pin.
    pub const fn from_active_low(on: bool) -> Self {
        if on { Self::Low } else { Self::High }
    }
}

/// Logical state of the four relays.
///
/// `false` is off and `true` is on. SW1 is the fridge compressor. SW2 is the
/// Peltier dehumidifier. SW3 and SW4 stay off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RelayBank {
    sw1: bool,
    sw2: bool,
    sw3: bool,
    sw4: bool,
}

impl RelayBank {
    /// Fridge and dehumidifier commands. SW3 and SW4 are off.
    pub const fn new(fridge_on: bool, dehumidifier_on: bool) -> Self {
        Self {
            sw1: fridge_on,
            sw2: dehumidifier_on,
            sw3: false,
            sw4: false,
        }
    }

    /// Every relay off.
    pub const fn all_off() -> Self {
        Self::new(false, false)
    }

    /// SW1, the fridge compressor.
    pub const fn sw1(self) -> bool {
        self.sw1
    }

    /// SW2, the Peltier dehumidifier.
    pub const fn sw2(self) -> bool {
        self.sw2
    }

    /// SW3. Always off.
    pub const fn sw3(self) -> bool {
        self.sw3
    }

    /// SW4. Always off.
    pub const fn sw4(self) -> bool {
        self.sw4
    }

    /// Apply SW1 and SW2. SW3 and SW4 stay off, so they are not commanded on.
    pub fn apply_to(self, output: &mut impl RelayOutput) {
        output.apply(self.sw1, self.sw2);
    }
}

impl Default for RelayBank {
    fn default() -> Self {
        Self::all_off()
    }
}

/// Applies logical fridge and dehumidifier commands to pins.
pub trait RelayOutput {
    /// `fridge` and `dehumidifier` are logical states: `true` is on.
    fn apply(&mut self, fridge: bool, dehumidifier: bool);
}

/// In-memory [`RelayOutput`].
///
/// Records the last fridge and dehumidifier commands and the active-low pin
/// levels those commands drive. SW3 and SW4 are recorded at the idle level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordingRelay {
    fridge: bool,
    dehumidifier: bool,
    fridge_pin: PinLevel,
    dehumidifier_pin: PinLevel,
    sw3_pin: PinLevel,
    sw4_pin: PinLevel,
}

impl RecordingRelay {
    /// Both actuators off, and every pin at the idle level.
    pub const fn new() -> Self {
        Self {
            fridge: false,
            dehumidifier: false,
            fridge_pin: PinLevel::High,
            dehumidifier_pin: PinLevel::High,
            sw3_pin: PinLevel::High,
            sw4_pin: PinLevel::High,
        }
    }

    /// Last logical fridge command.
    pub const fn fridge(self) -> bool {
        self.fridge
    }

    /// Last logical dehumidifier command.
    pub const fn dehumidifier(self) -> bool {
        self.dehumidifier
    }

    /// Pin level driven for the fridge.
    pub const fn fridge_pin(self) -> PinLevel {
        self.fridge_pin
    }

    /// Pin level driven for the dehumidifier.
    pub const fn dehumidifier_pin(self) -> PinLevel {
        self.dehumidifier_pin
    }

    /// Pin level driven for SW3. Idle, because SW3 stays off.
    pub const fn sw3_pin(self) -> PinLevel {
        self.sw3_pin
    }

    /// Pin level driven for SW4. Idle, because SW4 stays off.
    pub const fn sw4_pin(self) -> PinLevel {
        self.sw4_pin
    }
}

impl Default for RecordingRelay {
    fn default() -> Self {
        Self::new()
    }
}

impl RelayOutput for RecordingRelay {
    fn apply(&mut self, fridge: bool, dehumidifier: bool) {
        self.fridge = fridge;
        self.dehumidifier = dehumidifier;
        self.fridge_pin = PinLevel::from_active_low(fridge);
        self.dehumidifier_pin = PinLevel::from_active_low(dehumidifier);
        self.sw3_pin = PinLevel::High;
        self.sw4_pin = PinLevel::High;
    }
}

/// What one panel frame shows.
///
/// `reading` is the climate sample to paint. It is `None` when the probe has
/// no valid sample, and the frame then leaves temperature, humidity, and
/// vapor-pressure deficit blank.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelView<'a> {
    /// Yellow-band title.
    pub title: &'a str,
    /// Logical fridge command. `true` fills the downward triangle.
    pub fridge_on: bool,
    /// Logical dehumidifier command. `true` fills the upward triangle.
    pub dehumidifier_on: bool,
    /// Status line drawn directly under the yellow band.
    pub status_line: &'a str,
    /// Climate sample. Absent when the probe has failed.
    pub reading: Option<ProbeMeasure>,
}

/// Draws one frame into a target.
///
/// Callers monomorphize this trait. The future is not used as a trait object.
#[allow(
    async_fn_in_trait,
    reason = "the panel is monomorphized at the call site, as the actor uses static dispatch"
)]
pub trait Panel {
    /// Paint `view`, replacing the previous frame.
    async fn render(&mut self, view: &PanelView<'_>);
}

/// [`Panel`] that draws into any embedded-graphics target.
pub struct MemoryPanel<D> {
    target: D,
}

impl<D> MemoryPanel<D> {
    /// Draw later frames into `target`.
    pub const fn new(target: D) -> Self {
        Self { target }
    }

    /// The target later frames are drawn into.
    pub const fn target(&self) -> &D {
        &self.target
    }
}

impl<D> Panel for MemoryPanel<D>
where
    D: DrawTarget<Color = BinaryColor, Error = core::convert::Infallible>,
{
    async fn render(&mut self, view: &PanelView<'_>) {
        draw_frame(&mut self.target, view).unwrap();
    }
}

/// Paint one 128×64 frame.
///
/// The top 16 rows are the yellow band: the mode title on the left, and the
/// dehumidifier and fridge triangles on the right. The status line, and a
/// valid reading's temperature, humidity, and vapor-pressure deficit, are
/// centered together in the blue area below that band.
pub(crate) fn draw_frame<D>(target: &mut D, view: &PanelView<'_>) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    target.clear(BinaryColor::Off)?;
    draw_text(target, view.title, Point::new(TEXT_X, TITLE_Y), &FONT_6X10)?;
    draw_triangle(target, DEHUM_CX, true, view.dehumidifier_on)?;
    draw_triangle(target, FRIDGE_CX, false, view.fridge_on)?;
    draw_blue_area(target, view.status_line, view.reading)?;
    Ok(())
}

/// Separate stack frame for glyph rasterization.
#[inline(never)]
fn draw_text<D>(target: &mut D, text: &str, at: Point, font: &MonoFont) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let style = MonoTextStyle::new(font, BinaryColor::On);
    Text::with_baseline(text, at, style, Baseline::Top).draw(target)?;
    Ok(())
}

/// Center the blue-area lines as one block.
///
/// The status line is included when it is not empty. Temperature, humidity,
/// and vapor-pressure deficit are included when `reading` is present. Each
/// line is centered horizontally, and the block is centered vertically in
/// the blue area.
#[inline(never)]
fn draw_blue_area<D>(
    target: &mut D,
    status_line: &str,
    reading: Option<ProbeMeasure>,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let mut temperature: heapless::String<32> = heapless::String::new();
    let mut deficit: heapless::String<16> = heapless::String::new();
    if let Some(reading) = reading {
        format_temperature(reading, &mut temperature);
        format_deficit(reading, &mut deficit);
    }

    if reading.is_some() && !status_line.is_empty() {
        draw_status_and_reading(target, status_line, &temperature, &deficit)
    } else if reading.is_some() {
        draw_reading_only(target, &temperature, &deficit)
    } else if !status_line.is_empty() {
        draw_status_only(target, status_line)
    } else {
        Ok(())
    }
}

/// Top of a block of `block_height` centered in the blue area.
const fn blue_block_top(block_height: i32) -> i32 {
    BLUE_TOP + (BLUE_HEIGHT - block_height) / 2
}

/// Separate stack frame for the temperature format.
#[inline(never)]
fn format_temperature(reading: ProbeMeasure, out: &mut heapless::String<32>) {
    write!(out, "{:.1}°C {}%", reading.temp, reading.hygr).expect("temperature line fits");
}

/// Separate stack frame for the vapor-pressure deficit format.
#[inline(never)]
fn format_deficit(reading: ProbeMeasure, out: &mut heapless::String<16>) {
    write!(out, "{:.3} kPa", reading.vpd).expect("vapor-pressure deficit line fits");
}

#[inline(never)]
fn draw_status_and_reading<D>(
    target: &mut D,
    status_line: &str,
    temperature: &str,
    deficit: &str,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let top = blue_block_top(BLUE_BLOCK_HEIGHT);
    draw_centered_line(target, status_line, &FONT_6X10, top)?;
    draw_centered_line(target, temperature, &FONT_10X20, top + GLYPH_6_HEIGHT)?;
    draw_centered_line(
        target,
        deficit,
        &FONT_6X10,
        top + GLYPH_6_HEIGHT + GLYPH_10_HEIGHT,
    )
}

#[inline(never)]
fn draw_reading_only<D>(target: &mut D, temperature: &str, deficit: &str) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let top = blue_block_top(GLYPH_10_HEIGHT + GLYPH_6_HEIGHT);
    draw_centered_line(target, temperature, &FONT_10X20, top)?;
    draw_centered_line(target, deficit, &FONT_6X10, top + GLYPH_10_HEIGHT)
}

#[inline(never)]
fn draw_status_only<D>(target: &mut D, status_line: &str) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    draw_centered_line(
        target,
        status_line,
        &FONT_6X10,
        blue_block_top(GLYPH_6_HEIGHT),
    )
}

/// Center one line horizontally in the blue area, with its top at `top_y`.
#[inline(never)]
fn draw_centered_line<D>(
    target: &mut D,
    text: &str,
    font: &MonoFont,
    top_y: i32,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let style = MonoTextStyle::new(font, BinaryColor::On);
    let text = Text::with_baseline(text, Point::zero(), style, Baseline::Top).align_to(
        &BLUE_AREA,
        horizontal::Center,
        vertical::Top,
    );
    text.translate(Point::new(0, top_y - BLUE_TOP))
        .draw(target)
        .map(|_| ())
}

/// Separate stack frame for one relay triangle.
#[inline(never)]
fn draw_triangle<D>(
    target: &mut D,
    center_x: i32,
    points_up: bool,
    on: bool,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = BinaryColor>,
{
    let bottom = TRI_TOP + TRI_H;
    let half = TRI_W / 2;
    let (apex_y, base_y) = if points_up {
        (TRI_TOP, bottom)
    } else {
        (bottom, TRI_TOP)
    };
    let style = if on {
        PrimitiveStyle::with_fill(BinaryColor::On)
    } else {
        PrimitiveStyle::with_stroke(BinaryColor::On, 1)
    };
    Triangle::new(
        Point::new(center_x, apex_y),
        Point::new(center_x - half, base_y),
        Point::new(center_x + half, base_y),
    )
    .into_styled(style)
    .draw(target)
}

#[cfg(test)]
#[allow(
    clippy::large_stack_frames,
    reason = "MockDisplay stores a 64 by 64 framebuffer"
)]
mod tests {
    use core::fmt::Write;
    use core::future::Future;
    use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    use embedded_graphics::{
        draw_target::DrawTargetExt,
        mock_display::MockDisplay,
        mono_font::{
            MonoFont, MonoTextStyle,
            iso_8859_1::{FONT_6X10, FONT_10X20},
        },
        pixelcolor::BinaryColor,
        prelude::*,
        text::{Baseline, Text},
    };

    use super::{MemoryPanel, Panel, PanelView, PinLevel, RecordingRelay, RelayBank, RelayOutput};
    use crate::{AppMode, ProbeMeasure};

    const PANEL_WIDTH: i32 = 128;
    const PANEL_HEIGHT: i32 = 64;
    const WINDOW: i32 = 64;
    const TITLE_X: i32 = 4;
    const TITLE_Y: i32 = 3;
    const BLUE_TOP: i32 = 16;
    const BLUE_HEIGHT: i32 = PANEL_HEIGHT - BLUE_TOP;

    fn block_on<F: Future>(future: F) -> F::Output {
        fn raw_waker() -> RawWaker {
            fn clone(data: *const ()) -> RawWaker {
                RawWaker::new(data, vtable())
            }
            fn noop(_: *const ()) {}
            fn vtable() -> &'static RawWakerVTable {
                &RawWakerVTable::new(clone, noop, noop, noop)
            }
            RawWaker::new(core::ptr::null(), vtable())
        }

        // SAFETY: the vtable functions ignore their data pointer, so a null
        // pointer is a valid no-op waker for one poll.
        let waker = unsafe { Waker::from_raw(raw_waker()) };
        let mut context = Context::from_waker(&waker);
        let mut future = core::pin::pin!(future);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(output) => output,
            Poll::Pending => panic!("panel render waited"),
        }
    }

    fn new_mock() -> MockDisplay<BinaryColor> {
        let mut display = MockDisplay::new();
        display.set_allow_out_of_bounds_drawing(true);
        display.set_allow_overdraw(true);
        display
    }

    /// Draw the 128×64 frame into one 64-pixel-wide window.
    ///
    /// `MockDisplay` is capped at 64×64. The window at x = 0 holds the titles
    /// and readings; the window at x = 64 holds the relay triangles.
    fn render_window(view: &PanelView<'_>, origin_x: i32) -> MockDisplay<BinaryColor> {
        let mut display = new_mock();
        {
            let mut panel = MemoryPanel::new(display.translated(Point::new(-origin_x, 0)));
            block_on(panel.render(view));
        }
        display
    }

    fn render_window_twice(
        first: &PanelView<'_>,
        second: &PanelView<'_>,
        origin_x: i32,
    ) -> MockDisplay<BinaryColor> {
        let mut display = new_mock();
        {
            let mut panel = MemoryPanel::new(display.translated(Point::new(-origin_x, 0)));
            block_on(panel.render(first));
            block_on(panel.render(second));
        }
        display
    }

    struct FrameWindows {
        left: MockDisplay<BinaryColor>,
        right: MockDisplay<BinaryColor>,
    }

    fn render(view: &PanelView<'_>) -> FrameWindows {
        FrameWindows {
            left: render_window(view, 0),
            right: render_window(view, WINDOW),
        }
    }

    fn is_on(display: &MockDisplay<BinaryColor>, x: i32, y: i32) -> bool {
        display.get_pixel(Point::new(x, y)) == Some(BinaryColor::On)
    }

    fn on_at(display: &MockDisplay<BinaryColor>, origin_x: i32, x: i32, y: i32) -> bool {
        let local_x = x - origin_x;
        assert!(
            (0..WINDOW).contains(&local_x) && (0..WINDOW).contains(&y),
            "logical ({x}, {y}) is outside the window at x = {origin_x}"
        );
        is_on(display, local_x, y)
    }

    fn any_on(display: &MockDisplay<BinaryColor>, y0: i32, y1: i32) -> bool {
        (y0..y1).any(|y| (0..WINDOW).any(|x| is_on(display, x, y)))
    }

    fn assert_blank(display: &MockDisplay<BinaryColor>, y0: i32, y1: i32) {
        for y in y0..y1 {
            for x in 0..WINDOW {
                assert!(
                    !is_on(display, x, y),
                    "invented a measurement pixel at local ({x}, {y})"
                );
            }
        }
    }

    fn draw_reference(
        text: &str,
        at: Point,
        font: &MonoFont,
        origin_x: i32,
    ) -> MockDisplay<BinaryColor> {
        let mut display = new_mock();
        let style = MonoTextStyle::new(font, BinaryColor::On);
        Text::with_baseline(text, at, style, Baseline::Top)
            .draw(&mut display.translated(Point::new(-origin_x, 0)))
            .unwrap();
        display
    }

    fn assert_text(frame: &FrameWindows, text: &str, at: Point, font: &MonoFont) {
        let cell = font.character_size;
        let width = text.chars().count() as i32 * cell.width as i32;
        let height = cell.height as i32;
        let expected_left = draw_reference(text, at, font, 0);
        let expected_right = draw_reference(text, at, font, WINDOW);
        for y in at.y..at.y + height {
            for x in at.x..at.x + width {
                let (actual, expected, local_x) = if x < WINDOW {
                    (&frame.left, &expected_left, x)
                } else {
                    (&frame.right, &expected_right, x - WINDOW)
                };
                assert_eq!(
                    is_on(actual, local_x, y),
                    is_on(expected, local_x, y),
                    "{text:?} pixel at logical ({x}, {y})"
                );
            }
        }
    }

    fn reading_lines(reading: ProbeMeasure) -> (heapless::String<32>, heapless::String<16>) {
        let mut temperature = heapless::String::new();
        write!(temperature, "{:.1}°C {}%", reading.temp, reading.hygr).unwrap();
        let mut vpd = heapless::String::new();
        write!(vpd, "{:.3} kPa", reading.vpd).unwrap();
        (temperature, vpd)
    }

    /// Horizontal origin that centers `text` in the 128-pixel panel.
    fn centered_x(text: &str, cell_width: i32) -> i32 {
        let width = text.chars().count() as i32 * cell_width;
        (PANEL_WIDTH - width) / 2
    }

    /// Top of a block of `block_height` centered in the blue area.
    fn centered_top(block_height: i32) -> i32 {
        BLUE_TOP + (BLUE_HEIGHT - block_height) / 2
    }

    fn fridge_center_x() -> i32 {
        (PANEL_WIDTH - 4) - 10 / 2
    }

    fn dehumidifier_center_x() -> i32 {
        fridge_center_x() - 10 - 4
    }

    fn assert_triangle(frame: &FrameWindows, center_x: i32, points_up: bool, on: bool) {
        let top = 3;
        let bottom = top + 10;
        let tip_y = if points_up { top } else { bottom };
        let interior_y = if points_up { 9 } else { 6 };
        assert!(
            on_at(&frame.right, WINDOW, center_x, tip_y),
            "triangle tip at ({center_x}, {tip_y}) should be drawn"
        );
        assert_eq!(
            on_at(&frame.right, WINDOW, center_x, interior_y),
            on,
            "triangle interior at ({center_x}, {interior_y}), filled = {on}"
        );
        assert!(
            !on_at(&frame.right, WINDOW, 112, 8),
            "gap between the triangles should stay clear"
        );
    }

    #[test]
    fn relay_bank_uses_false_for_off_and_keeps_unused_relays_off() {
        let off = RelayBank::all_off();
        assert!(!off.sw1());
        assert!(!off.sw2());
        assert!(!off.sw3());
        assert!(!off.sw4());
        assert_eq!(off, RelayBank::default());

        let cooling = RelayBank::new(true, false);
        assert!(cooling.sw1());
        assert!(!cooling.sw2());
        assert!(!cooling.sw3());
        assert!(!cooling.sw4());

        let drying = RelayBank::new(false, true);
        assert!(!drying.sw1());
        assert!(drying.sw2());

        let both = RelayBank::new(true, true);
        assert!(both.sw1() && both.sw2());
        assert!(!both.sw3() && !both.sw4());

        let mut relays = RecordingRelay::new();
        both.apply_to(&mut relays);
        assert!(relays.fridge());
        assert!(relays.dehumidifier());
        assert_eq!(relays.sw3_pin(), PinLevel::High);
        assert_eq!(relays.sw4_pin(), PinLevel::High);
    }

    #[test]
    fn recording_relay_maps_logical_on_to_pin_low() {
        let mut relays = RecordingRelay::new();
        assert!(!relays.fridge());
        assert!(!relays.dehumidifier());
        assert_eq!(relays.fridge_pin(), PinLevel::High);
        assert_eq!(relays.dehumidifier_pin(), PinLevel::High);
        assert_eq!(relays.sw3_pin(), PinLevel::High);
        assert_eq!(relays.sw4_pin(), PinLevel::High);

        relays.apply(true, false);
        assert!(relays.fridge());
        assert!(!relays.dehumidifier());
        assert_eq!(relays.fridge_pin(), PinLevel::Low);
        assert_eq!(relays.dehumidifier_pin(), PinLevel::High);
        assert_eq!(relays.sw3_pin(), PinLevel::High);
        assert_eq!(relays.sw4_pin(), PinLevel::High);

        relays.apply(false, true);
        assert!(!relays.fridge());
        assert!(relays.dehumidifier());
        assert_eq!(relays.fridge_pin(), PinLevel::High);
        assert_eq!(relays.dehumidifier_pin(), PinLevel::Low);

        relays.apply(false, false);
        assert!(!relays.fridge());
        assert!(!relays.dehumidifier());
        assert_eq!(relays.fridge_pin(), PinLevel::High);
        assert_eq!(relays.dehumidifier_pin(), PinLevel::High);
    }

    #[test]
    fn frame_draws_title_status_filled_and_outline_triangles() {
        assert_eq!(FONT_6X10.character_size, Size::new(6, 10));
        assert_eq!(FONT_10X20.character_size, Size::new(10, 20));

        let reading = ProbeMeasure::new(1, 18.5, 60);
        let (temperature, vpd) = reading_lines(reading);
        let view = PanelView {
            title: AppMode::Nominal.title(),
            fridge_on: true,
            dehumidifier_on: false,
            status_line: "compresseur prêt",
            reading: Some(reading),
        };
        let frame = render(&view);

        let status_y = centered_top(10 + 20 + 10);
        let reading_y = status_y + 10;
        let vpd_y = reading_y + 20;
        let vpd_end = vpd_y + 10;
        assert_eq!(status_y - BLUE_TOP, PANEL_HEIGHT - vpd_end);

        assert_text(&frame, view.title, Point::new(TITLE_X, TITLE_Y), &FONT_6X10);
        assert_text(
            &frame,
            view.status_line,
            Point::new(centered_x(view.status_line, 6), status_y),
            &FONT_6X10,
        );
        assert_text(
            &frame,
            &temperature,
            Point::new(centered_x(&temperature, 10), reading_y),
            &FONT_10X20,
        );
        assert_text(
            &frame,
            &vpd,
            Point::new(centered_x(&vpd, 6), vpd_y),
            &FONT_6X10,
        );
        assert_blank(&frame.left, BLUE_TOP, status_y);
        assert_blank(&frame.right, BLUE_TOP, status_y);
        assert_blank(&frame.left, vpd_end, WINDOW);
        assert_blank(&frame.right, vpd_end, WINDOW);

        assert_triangle(&frame, fridge_center_x(), false, true);
        assert_triangle(&frame, dehumidifier_center_x(), true, false);
    }

    #[test]
    fn filled_triangle_follows_the_relay_that_is_on() {
        let view = PanelView {
            title: AppMode::VpdRegulation.title(),
            fridge_on: false,
            dehumidifier_on: true,
            status_line: "12s",
            reading: Some(ProbeMeasure::new(2, 19.0, 64)),
        };
        assert_eq!(view.title.chars().count(), 16);

        let frame = render(&view);
        let status_y = centered_top(10 + 20 + 10);
        assert_text(&frame, view.title, Point::new(TITLE_X, TITLE_Y), &FONT_6X10);
        assert_text(
            &frame,
            view.status_line,
            Point::new(centered_x(view.status_line, 6), status_y),
            &FONT_6X10,
        );
        assert_triangle(&frame, fridge_center_x(), false, false);
        assert_triangle(&frame, dehumidifier_center_x(), true, true);
    }

    #[test]
    fn failed_probe_does_not_invent_a_temperature() {
        let reading = ProbeMeasure::new(3, 21.0, 55);
        let good = PanelView {
            title: AppMode::Nominal.title(),
            fridge_on: true,
            dehumidifier_on: true,
            status_line: "compresseur prêt",
            reading: Some(reading),
        };
        let failed = PanelView {
            title: AppMode::WaitProbe.title(),
            fridge_on: false,
            dehumidifier_on: false,
            status_line: "! ! !",
            reading: None,
        };

        let fresh = render(&failed);
        let status_y = centered_top(10);
        let status_end = status_y + 10;
        assert_eq!(status_y - BLUE_TOP, PANEL_HEIGHT - status_end);
        assert_text(
            &fresh,
            failed.title,
            Point::new(TITLE_X, TITLE_Y),
            &FONT_6X10,
        );
        assert_text(
            &fresh,
            failed.status_line,
            Point::new(centered_x(failed.status_line, 6), status_y),
            &FONT_6X10,
        );
        assert_blank(&fresh.left, BLUE_TOP, status_y);
        assert_blank(&fresh.right, BLUE_TOP, status_y);
        assert_blank(&fresh.left, status_end, WINDOW);
        assert_blank(&fresh.right, status_end, WINDOW);
        assert_triangle(&fresh, fridge_center_x(), false, false);
        assert_triangle(&fresh, dehumidifier_center_x(), true, false);

        let previous = render(&good);
        let reading_y = centered_top(10 + 20 + 10) + 10;
        assert!(any_on(&previous.left, reading_y, reading_y + 20));

        let replaced_left = render_window_twice(&good, &failed, 0);
        let replaced_right = render_window_twice(&good, &failed, WINDOW);
        assert_blank(&replaced_left, BLUE_TOP, status_y);
        assert_blank(&replaced_right, BLUE_TOP, status_y);
        assert_blank(&replaced_left, status_end, WINDOW);
        assert_blank(&replaced_right, status_end, WINDOW);
        assert_text(
            &FrameWindows {
                left: replaced_left,
                right: replaced_right,
            },
            failed.status_line,
            Point::new(centered_x(failed.status_line, 6), status_y),
            &FONT_6X10,
        );
    }
}
