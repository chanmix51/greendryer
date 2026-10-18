#![no_std]
#![deny(clippy::large_stack_frames)]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

#[cfg(test)]
extern crate std;

mod display_actor;
mod game_state;

pub use display_actor::{DisplayActor, Frame};
pub use game_state::{CELLS, COLUMNS, Cell, Direction, EncoderEvent, GameState, ROWS};
