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
mod app_state;
mod display_actor;
mod io;
mod main_actor;
pub mod modes;
mod sample_rack;

#[cfg(any(test, target_arch = "xtensa"))]
mod sht31;

#[cfg(target_arch = "xtensa")]
mod board;

pub use app_mode::AppMode;
pub use app_state::{
    AppState, COMPRESSOR_READY, COMPRESSOR_REST_S, CompressorRestError, SAMPLE_SLOTS, STATUS_BYTES,
};
pub use display_actor::DisplayActor;
pub use io::{MemoryPanel, Panel, PanelView, PinLevel, RecordingRelay, RelayBank, RelayOutput};
pub use main_actor::{ClimateProbe, MainActor};
pub use modes::{
    AppModError, ApplicationMode, FailureStopMode, HrEnforcingMode, NominalMode, TempEnforcingMode,
    VpdRegulationMode, WaitProbeMode,
};
pub use sample_rack::{ProbeError, ProbeMeasure, SampleRack, SampleSlot};

#[cfg(target_arch = "xtensa")]
pub use board::{GpioRelays, Sht31Probe, Ssd1306Panel};
