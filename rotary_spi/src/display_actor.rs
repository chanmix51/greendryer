//! Panel frame for the rotary-encoder game.
//!
//! [`Frame`] is the pure description of one screen. [`DisplayActor`] draws
//! that frame into a [`DrawTarget`](embedded_graphics::draw_target::DrawTarget)
//! and skips the draw when the published [`GameState`] has not changed.
//! There is no framebuffer: glyphs are drawn straight into the target.

use embedded_graphics::mono_font::MonoTextStyle;
use embedded_graphics::mono_font::ascii::FONT_10X20;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::{PrimitiveStyle, Rectangle};
use embedded_graphics::text::{Baseline, Text};

use crate::GameState;

const PANEL_WIDTH: i32 = 240;
const GLYPH_WIDTH: i32 = FONT_10X20.character_size.width as i32;
const GLYPH_HEIGHT: i32 = FONT_10X20.character_size.height as i32;
const SCORE_DIGITS: usize = 10;

const _: () = assert!(FONT_10X20.character_size.width == 10);
const _: () = assert!(FONT_10X20.character_size.height == 20);

const PAC: char = '@';
const VECTOR: char = '-';
const FLOWER: char = '*';

/// One glyph in the game area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    symbol: char,
    col: u8,
    row: u8,
}

impl Glyph {
    /// Column in `0..24`.
    pub const fn col(self) -> u8 {
        self.col
    }

    /// Game row in `0..15`.
    pub const fn row(self) -> u8 {
        self.row
    }
}

/// Score line, game glyphs, and the border around the game area.
pub struct Frame {
    score: [u8; SCORE_DIGITS],
    score_len: usize,
    score_x: i32,
    glyphs: [Option<Glyph>; 3],
    border: Rectangle,
}

impl Frame {
    /// Frame for `state`.
    ///
    /// The score is the decimal text of the score, centered in screen row 0.
    /// A cell draws the flower, then the PAC, then the vector. The vector is
    /// omitted when it lies outside the game area or on the flower.
    pub fn from_state(state: &GameState) -> Self {
        let (score, score_len) = format_score(state.score());
        let mut glyphs = [None, None, None];
        let mut next = 0;

        let flower = state.flower();
        glyphs[next] = Some(Glyph {
            symbol: FLOWER,
            col: flower.col(),
            row: flower.row(),
        });
        next += 1;

        let pac = state.pac();
        if pac != flower {
            glyphs[next] = Some(Glyph {
                symbol: PAC,
                col: pac.col(),
                row: pac.row(),
            });
            next += 1;
        }

        if let Some(vector) = state.vector()
            && vector != flower
            && vector != pac
        {
            glyphs[next] = Some(Glyph {
                symbol: VECTOR,
                col: vector.col(),
                row: vector.row(),
            });
        }

        Self {
            score,
            score_len,
            score_x: score_origin_x(score_len),
            glyphs,
            border: game_border(),
        }
    }

    /// Decimal score, with no caption.
    pub fn score_text(&self) -> &str {
        core::str::from_utf8(&self.score[..self.score_len]).unwrap_or("")
    }

    /// Left edge of the score string. A score of 0 is `"0"` at x = 115.
    pub const fn score_x(&self) -> i32 {
        self.score_x
    }

    /// 1 px white stroke around the game area: `(0, 20)` size `240×300`.
    pub const fn border(&self) -> Rectangle {
        self.border
    }

    /// Whether this frame draws `symbol`.
    pub fn has_symbol(&self, symbol: char) -> bool {
        self.glyph(symbol).is_some()
    }

    /// First glyph with `symbol`.
    pub fn glyph(&self, symbol: char) -> Option<&Glyph> {
        self.glyphs
            .iter()
            .flatten()
            .find(|glyph| glyph.symbol == symbol)
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
        Text::with_baseline(
            self.score_text(),
            Point::new(self.score_x, 0),
            style,
            Baseline::Top,
        )
        .draw(target)?;

        for glyph in self.glyphs.iter().flatten() {
            let mut bytes = [0u8; 4];
            let text = glyph.symbol.encode_utf8(&mut bytes);
            let origin = Point::new(
                i32::from(glyph.col) * GLYPH_WIDTH,
                (i32::from(glyph.row) + 1) * GLYPH_HEIGHT,
            );
            Text::with_baseline(text, origin, style, Baseline::Top).draw(target)?;
        }

        // The 1 px stroke overlaps the outer pixels of the edge glyphs.
        // The panel clips it on x = 0, x = 239, and y = 319.
        self.border
            .into_styled(PrimitiveStyle::with_stroke(Rgb565::WHITE, 1))
            .draw(target)?;
        Ok(())
    }
}

/// Paints the game panel from the published state.
///
/// `D` is the panel. The ST7789 stays inside this actor because the firmware
/// task stores the panel here. `refresh` is synchronous: the panel SPI blocks.
pub struct DisplayActor<D> {
    target: D,
    shown: Option<GameState>,
    frames: u32,
}

impl<D> DisplayActor<D> {
    /// An actor that has not painted a frame yet.
    pub const fn new(target: D) -> Self {
        Self {
            target,
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

    #[cfg(test)]
    fn target(&self) -> &D {
        &self.target
    }
}

impl<D> DisplayActor<D>
where
    D: DrawTarget<Color = Rgb565>,
{
    /// Paint `state` when it differs from the last painted state.
    ///
    /// Two calls with equal states paint once. The panel is cleared to black
    /// before the glyphs, then the border is stroked over them.
    pub fn refresh(&mut self, state: &GameState) -> Result<(), D::Error> {
        if self.shown.as_ref() == Some(state) {
            return Ok(());
        }
        self.target.clear(Rgb565::BLACK)?;
        Frame::from_state(state).draw(&mut self.target)?;
        self.shown = Some(state.clone());
        self.frames = self.frames.saturating_add(1);
        Ok(())
    }
}

/// `(240 - 10 * digit_count) / 2`.
const fn score_origin_x(digits: usize) -> i32 {
    (PANEL_WIDTH - GLYPH_WIDTH * digits as i32) / 2
}

const fn game_border() -> Rectangle {
    Rectangle::new(Point::new(0, GLYPH_HEIGHT), Size::new(240, 300))
}

fn format_score(score: u32) -> ([u8; SCORE_DIGITS], usize) {
    let mut raw = [0u8; SCORE_DIGITS];
    let mut value = score;
    let mut end = SCORE_DIGITS;
    loop {
        end -= 1;
        raw[end] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    let len = SCORE_DIGITS - end;
    let mut score = [0u8; SCORE_DIGITS];
    let mut index = 0;
    while index < len {
        score[index] = raw[end + index];
        index += 1;
    }
    (score, len)
}

#[cfg(test)]
mod tests {
    use embedded_graphics::Pixel;
    use embedded_graphics::geometry::OriginDimensions;
    use embedded_graphics::pixelcolor::Rgb565;
    use embedded_graphics::prelude::*;
    use embedded_graphics::primitives::Rectangle;

    use super::*;
    use crate::{Cell, Direction, EncoderEvent, GameState};

    struct Recorder {
        clears: u32,
        pixels: u32,
    }

    impl Recorder {
        const fn new() -> Self {
            Self {
                clears: 0,
                pixels: 0,
            }
        }
    }

    impl OriginDimensions for Recorder {
        fn size(&self) -> Size {
            Size::new(240, 320)
        }
    }

    impl DrawTarget for Recorder {
        type Color = Rgb565;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            self.pixels = self
                .pixels
                .saturating_add(pixels.into_iter().count() as u32);
            Ok(())
        }

        fn clear(&mut self, _color: Self::Color) -> Result<(), Self::Error> {
            self.clears = self.clears.saturating_add(1);
            Ok(())
        }
    }

    #[test]
    fn score_zero_is_centered_and_the_border_frames_the_game_area() {
        let frame = Frame::from_state(&GameState::new(0));

        assert_eq!(frame.score_text(), "0");
        assert_eq!(frame.score_x(), 115);
        assert_eq!(
            frame.border(),
            Rectangle::new(Point::new(0, 20), Size::new(240, 300))
        );

        let ten = Frame::from_state(&GameState::from_layout(
            Cell::new(11, 7),
            Direction::East,
            Cell::new(0, 1),
            10,
        ));
        assert_eq!(ten.score_text(), "10");
        assert_eq!(ten.score_x(), 110);
    }

    #[test]
    fn frame_omits_the_dash_when_the_vector_sits_outside() {
        let mut state =
            GameState::from_layout(Cell::new(22, 7), Direction::East, Cell::new(0, 1), 0);
        state.apply(EncoderEvent::Click, 0);

        let frame = Frame::from_state(&state);
        assert!(!frame.has_symbol('-'));
        assert!(frame.has_symbol('@'));

        state.apply(EncoderEvent::Clockwise, 0);
        assert_eq!(state.direction(), Direction::South);
    }

    #[test]
    fn flower_on_the_vector_cell_draws_a_star_and_no_dash() {
        let state = GameState::from_layout(Cell::new(11, 7), Direction::East, Cell::new(12, 7), 0);
        let frame = Frame::from_state(&state);

        assert!(frame.has_symbol('*'));
        assert!(!frame.has_symbol('-'));
        let flower = frame.glyph('*').expect("flower glyph");
        assert_eq!(flower.col(), 12);
        assert_eq!(flower.row(), 7);
    }

    #[test]
    fn refresh_paints_the_first_state_and_skips_an_equal_one() {
        let mut actor = DisplayActor::new(Recorder::new());
        let state = GameState::new(0);

        assert_eq!(actor.frames(), 0);
        actor.refresh(&state).unwrap();
        assert_eq!(actor.frames(), 1);
        assert_eq!(actor.target().clears, 1);
        assert!(actor.target().pixels > 0);

        let published = state.clone();
        actor.refresh(&published).unwrap();
        assert_eq!(actor.frames(), 1);
        assert_eq!(actor.target().clears, 1);

        let changed = GameState::from_layout(state.pac(), state.direction(), state.flower(), 1);
        actor.refresh(&changed).unwrap();
        assert_eq!(actor.frames(), 2);
        assert_eq!(changed.score(), 1);
        assert_eq!(actor.target().clears, 2);
    }
}
