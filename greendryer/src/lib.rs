#![no_std]
#![deny(clippy::large_stack_frames)]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]

#[cfg(test)]
extern crate std;

mod app_mode;
mod io;
mod sample_rack;

pub use app_mode::AppMode;
pub use io::{MemoryPanel, Panel, PanelView, PinLevel, RecordingRelay, RelayBank, RelayOutput};
pub use sample_rack::{ProbeError, ProbeMeasure, SampleRack, SampleSlot};
