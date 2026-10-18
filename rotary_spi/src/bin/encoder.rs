//! KY-040 handler.
//!
//! GPIO32 (CLK) and GPIO27 (SW) each take a falling edge. The handler samples
//! GPIO33 (DT) on a CLK edge, debounces each pin, and `try_send`s one event.
//! A full channel drops the new event. The handler does not take the published
//! state mutex and does not touch SPI.

use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant};
use esp_hal::gpio::{Event, Input, InputConfig, Pull};
use esp_hal::peripherals::{GPIO27, GPIO32, GPIO33, IO_MUX};
use rotary_spi::EncoderEvent;

/// DT high on a CLK falling edge is clockwise. Flip this if the module is mirrored.
const DT_HIGH_IS_CLOCKWISE: bool = true;

const DEBOUNCE: Duration = Duration::from_millis(15);

pub static EVENTS: Channel<CriticalSectionRawMutex, EncoderEvent, 16> = Channel::new();

struct Encoder {
    clk: Input<'static>,
    dt: Input<'static>,
    sw: Input<'static>,
    clk_at: Option<Instant>,
    sw_at: Option<Instant>,
}

static ENCODER: critical_section::Mutex<RefCell<Option<Encoder>>> =
    critical_section::Mutex::new(RefCell::new(None));

pub fn install(
    clk: GPIO32<'static>,
    dt: GPIO33<'static>,
    sw: GPIO27<'static>,
    io_mux: IO_MUX<'static>,
) {
    let config = InputConfig::default().with_pull(Pull::Up);
    let clk = Input::new(clk, config);
    let dt = Input::new(dt, config);
    let sw = Input::new(sw, config);

    let mut io = esp_hal::gpio::Io::new(io_mux);
    io.set_interrupt_handler(on_encoder);

    critical_section::with(|cs| {
        let mut slot = ENCODER.borrow_ref_mut(cs);
        let encoder = slot.insert(Encoder {
            clk,
            dt,
            sw,
            clk_at: None,
            sw_at: None,
        });
        encoder.clk.listen(Event::FallingEdge);
        encoder.sw.listen(Event::FallingEdge);
    });
}

#[esp_hal::handler]
fn on_encoder() {
    let now = Instant::now();
    let mut pending = [None, None];
    critical_section::with(|cs| {
        let mut slot = ENCODER.borrow_ref_mut(cs);
        let Some(encoder) = slot.as_mut() else {
            return;
        };
        if encoder.clk.is_interrupt_set() {
            encoder.clk.clear_interrupt();
            if edge_accepted(encoder.clk_at, now) {
                encoder.clk_at = Some(now);
                pending[0] = Some(if encoder.dt.is_high() == DT_HIGH_IS_CLOCKWISE {
                    EncoderEvent::Clockwise
                } else {
                    EncoderEvent::CounterClockwise
                });
            }
        }
        if encoder.sw.is_interrupt_set() {
            encoder.sw.clear_interrupt();
            if edge_accepted(encoder.sw_at, now) {
                encoder.sw_at = Some(now);
                pending[1] = Some(EncoderEvent::Click);
            }
        }
    });
    for event in pending.into_iter().flatten() {
        let _ = EVENTS.try_send(event);
    }
}

/// An edge less than 15 ms after this pin's last accepted edge is ignored.
fn edge_accepted(last: Option<Instant>, now: Instant) -> bool {
    match last {
        Some(earlier) => now.saturating_duration_since(earlier) >= DEBOUNCE,
        None => true,
    }
}
