//! Rotary-encoder game.
//!
//! [`GameState`] owns the PAC, the direction, the flower, and the score.
//! Pins, the panel, and the hardware RNG stay outside it. A flower placement
//! takes one `u16` candidate so a test can choose the cell.

/// Columns in the game area.
pub const COLUMNS: u8 = 24;

/// Rows in the game area. Screen row 0 is the score line, so the game uses
/// screen rows 1 through 15.
pub const ROWS: u8 = 15;

/// Cells in the game area.
pub const CELLS: u16 = 360;

const FLOWER_CELLS: u16 = CELLS - 1;

const _: () = assert!(CELLS == (COLUMNS as u16) * (ROWS as u16));
const _: () = assert!(FLOWER_CELLS == 359);

/// One game cell. `col` is in `0..24` and `row` is in `0..15`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    col: u8,
    row: u8,
}

impl Cell {
    /// A cell inside the game area.
    pub const fn new(col: u8, row: u8) -> Self {
        assert!(col < COLUMNS && row < ROWS);
        Self { col, row }
    }

    /// Column in `0..24`.
    pub const fn col(self) -> u8 {
        self.col
    }

    /// Game row in `0..15`. Screen row is this plus one.
    pub const fn row(self) -> u8 {
        self.row
    }

    /// Linear index, `row * 24 + col`.
    pub const fn index(self) -> u16 {
        self.row as u16 * COLUMNS as u16 + self.col as u16
    }

    const fn from_index(index: u16) -> Self {
        Self {
            col: (index % COLUMNS as u16) as u8,
            row: (index / COLUMNS as u16) as u8,
        }
    }
}

/// Orthogonal direction. Clockwise order is east, south, west, north.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    East,
    South,
    West,
    North,
}

impl Direction {
    const fn clockwise(self) -> Self {
        match self {
            Self::East => Self::South,
            Self::South => Self::West,
            Self::West => Self::North,
            Self::North => Self::East,
        }
    }

    const fn counter_clockwise(self) -> Self {
        match self {
            Self::East => Self::North,
            Self::North => Self::West,
            Self::West => Self::South,
            Self::South => Self::East,
        }
    }

    const fn delta(self) -> (i8, i8) {
        match self {
            Self::East => (1, 0),
            Self::South => (0, 1),
            Self::West => (-1, 0),
            Self::North => (0, -1),
        }
    }
}

/// One step from the KY-040.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncoderEvent {
    Clockwise,
    CounterClockwise,
    Click,
}

/// PAC, direction marker, flower, and score.
///
/// The PAC starts at `(11, 7)` facing east. Column 11 is the chosen center:
/// a 24-column row has no single middle column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameState {
    pac: Cell,
    direction: Direction,
    flower: Cell,
    score: u32,
}

impl GameState {
    /// Boot state for `candidate`.
    ///
    /// The PAC is `(11, 7)`, the direction is east, and the score is 0.
    /// The flower is [`place_flower`] over that PAC.
    pub const fn new(candidate: u16) -> Self {
        let pac = Cell::new(11, 7);
        Self {
            pac,
            direction: Direction::East,
            flower: place_flower(pac, candidate),
            score: 0,
        }
    }

    /// A state with an explicit layout.
    ///
    /// Boot placement goes through [`Self::new`]. This constructor stores
    /// `flower` as given so a test can put it on a chosen cell.
    pub const fn from_layout(pac: Cell, direction: Direction, flower: Cell, score: u32) -> Self {
        Self {
            pac,
            direction,
            flower,
            score,
        }
    }

    /// PAC cell.
    pub const fn pac(&self) -> Cell {
        self.pac
    }

    /// Direction the vector sits on.
    pub const fn direction(&self) -> Direction {
        self.direction
    }

    /// Flower cell. This is never the PAC cell after [`Self::new`] or a click.
    pub const fn flower(&self) -> Cell {
        self.flower
    }

    /// Flowers collected. Saturates at [`u32::MAX`].
    pub const fn score(&self) -> u32 {
        self.score
    }

    /// Cell one step from the PAC in the current direction, when that cell is
    /// inside the game area.
    pub const fn vector(&self) -> Option<Cell> {
        neighbor(self.pac, self.direction)
    }

    /// Apply one encoder event.
    ///
    /// `candidate` places the next flower when a click lands on the current
    /// one. Rotation and a click that does not score leave it unused.
    pub fn apply(&mut self, event: EncoderEvent, candidate: u16) {
        match event {
            EncoderEvent::Clockwise => {
                self.direction = turn(self.pac, self.direction, true);
            }
            EncoderEvent::CounterClockwise => {
                self.direction = turn(self.pac, self.direction, false);
            }
            EncoderEvent::Click => self.click(candidate),
        }
    }

    fn click(&mut self, candidate: u16) {
        let Some(next) = neighbor(self.pac, self.direction) else {
            return;
        };
        self.pac = next;
        if self.pac == self.flower {
            self.score = self.score.saturating_add(1);
            self.flower = place_flower(self.pac, candidate);
        }
    }
}

/// Uniform flower cell over the 359 cells that are not `pac`.
///
/// `candidate` is reduced modulo 359. An index at or past the PAC is shifted
/// up by one, so the PAC cell is skipped and every other cell remains reachable.
pub const fn place_flower(pac: Cell, candidate: u16) -> Cell {
    let pac_i = pac.index();
    let mut index = candidate % FLOWER_CELLS;
    if index >= pac_i {
        index += 1;
    }
    Cell::from_index(index)
}

/// Neighbor one orthogonal step away, or `None` when that step leaves the game area.
const fn neighbor(cell: Cell, direction: Direction) -> Option<Cell> {
    let (dx, dy) = direction.delta();
    let col = cell.col as i16 + dx as i16;
    let row = cell.row as i16 + dy as i16;
    if col >= 0 && col < COLUMNS as i16 && row >= 0 && row < ROWS as i16 {
        Some(Cell {
            col: col as u8,
            row: row as u8,
        })
    } else {
        None
    }
}

/// Advance one step. Outside directions are skipped until the neighbor is visible.
///
/// A corner has two outside directions. One event walks past both and stops
/// on the next direction whose cell is inside.
const fn turn(pac: Cell, mut direction: Direction, clockwise: bool) -> Direction {
    let mut step = 0;
    while step < 4 {
        direction = if clockwise {
            direction.clockwise()
        } else {
            direction.counter_clockwise()
        };
        if neighbor(pac, direction).is_some() {
            return direction;
        }
        step += 1;
    }
    direction
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boot_state_places_the_pac_and_a_flower_from_candidate_zero() {
        let state = GameState::new(0);

        assert_eq!(state.pac(), Cell::new(11, 7));
        assert_eq!(state.direction(), Direction::East);
        assert_eq!(state.score(), 0);
        assert_ne!(state.flower(), state.pac());
        assert_eq!(state.flower(), place_flower(state.pac(), 0));
        assert_eq!(state.vector(), Some(Cell::new(12, 7)));
    }

    #[test]
    fn clockwise_cycles_and_counter_clockwise_from_east_is_north() {
        let mut state = GameState::new(0);

        state.apply(EncoderEvent::Clockwise, 0);
        assert_eq!(state.direction(), Direction::South);
        state.apply(EncoderEvent::Clockwise, 0);
        assert_eq!(state.direction(), Direction::West);
        state.apply(EncoderEvent::Clockwise, 0);
        assert_eq!(state.direction(), Direction::North);
        state.apply(EncoderEvent::Clockwise, 0);
        assert_eq!(state.direction(), Direction::East);

        state.apply(EncoderEvent::CounterClockwise, 0);
        assert_eq!(state.direction(), Direction::North);
        assert_eq!(state.vector(), Some(Cell::new(11, 6)));
    }

    #[test]
    fn clockwise_at_the_north_east_corner_skips_north_and_east() {
        let mut state =
            GameState::from_layout(Cell::new(23, 0), Direction::West, Cell::new(0, 1), 0);

        state.apply(EncoderEvent::Clockwise, 0);

        assert_eq!(state.direction(), Direction::South);
        assert_eq!(state.vector(), Some(Cell::new(23, 1)));
    }

    #[test]
    fn click_steps_the_pac_and_keeps_the_direction() {
        let mut state = GameState::new(0);
        let flower = state.flower();

        state.apply(EncoderEvent::Click, 99);

        assert_eq!(state.pac(), Cell::new(12, 7));
        assert_eq!(state.direction(), Direction::East);
        assert_eq!(state.vector(), Some(Cell::new(13, 7)));
        assert_eq!(state.flower(), flower);
        assert_eq!(state.score(), 0);
    }

    #[test]
    fn click_facing_out_of_the_area_changes_nothing() {
        let flower = Cell::new(0, 1);
        let mut state = GameState::from_layout(Cell::new(23, 7), Direction::East, flower, 4);

        state.apply(EncoderEvent::Click, 99);

        assert_eq!(state.pac(), Cell::new(23, 7));
        assert_eq!(state.direction(), Direction::East);
        assert_eq!(state.flower(), flower);
        assert_eq!(state.score(), 4);
        assert_eq!(state.vector(), None);
    }

    #[test]
    fn click_onto_the_edge_hides_the_vector_until_the_next_rotation() {
        let mut state =
            GameState::from_layout(Cell::new(22, 7), Direction::East, Cell::new(0, 1), 0);

        state.apply(EncoderEvent::Click, 0);

        assert_eq!(state.pac(), Cell::new(23, 7));
        assert_eq!(state.direction(), Direction::East);
        assert_eq!(state.vector(), None);

        state.apply(EncoderEvent::Clockwise, 0);

        assert_eq!(state.direction(), Direction::South);
        assert_eq!(state.vector(), Some(Cell::new(23, 8)));
    }

    #[test]
    fn click_onto_the_flower_scores_and_places_the_next_flower() {
        let mut state =
            GameState::from_layout(Cell::new(11, 7), Direction::East, Cell::new(12, 7), 0);

        state.apply(EncoderEvent::Click, 0);

        assert_eq!(state.pac(), Cell::new(12, 7));
        assert_eq!(state.direction(), Direction::East);
        assert_eq!(state.score(), 1);
        assert_ne!(state.flower(), state.pac());
        assert_eq!(state.flower(), place_flower(Cell::new(12, 7), 0));
    }

    #[test]
    fn score_saturates_at_u32_max() {
        let mut state = GameState::from_layout(
            Cell::new(11, 7),
            Direction::East,
            Cell::new(12, 7),
            u32::MAX,
        );

        state.apply(EncoderEvent::Click, 1);

        assert_eq!(state.score(), u32::MAX);
        assert_eq!(state.pac(), Cell::new(12, 7));
        assert_ne!(state.flower(), state.pac());
    }

    #[test]
    fn candidates_zero_and_358_lie_on_either_side_of_the_pac() {
        let pac = Cell::new(11, 7);
        let low = place_flower(pac, 0);
        let high = place_flower(pac, 358);

        assert!(low.index() < pac.index());
        assert!(high.index() > pac.index());
        assert_ne!(low, pac);
        assert_ne!(high, pac);
        assert_eq!(low, Cell::new(0, 0));
        assert_eq!(high, Cell::new(23, 14));
        assert_eq!(GameState::new(0).flower(), low);
        assert_eq!(GameState::new(358).flower(), high);
    }

    #[test]
    fn every_reduced_candidate_skips_the_pac_cell() {
        let pac = Cell::new(11, 7);
        for candidate in 0..FLOWER_CELLS {
            assert_ne!(place_flower(pac, candidate), pac);
        }
        assert_eq!(place_flower(pac, pac.index() - 1).index(), pac.index() - 1);
        assert_eq!(place_flower(pac, pac.index()).index(), pac.index() + 1);
    }
}
