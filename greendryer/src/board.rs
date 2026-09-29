//! Pin, probe, and panel adapters for the cabinet that holds vegetal material.
//!
//! The binary owns the pins and the shared I2C master. These adapters are the
//! only place that talks to GPIO14, GPIO27, GPIO26, GPIO25, the SHT31, and
//! the SSD1306.

use core::future::Future;

use embassy_time::{Duration, Instant, Timer};
use embedded_hal_async::i2c::I2c as AsyncI2c;
use esp_hal::gpio::{Level, Output};
use ssd1306::{
    I2CDisplayInterface, Ssd1306Async, mode::BufferedGraphicsModeAsync, prelude::*,
    size::DisplaySize128x64,
};

use crate::io::draw_frame;
use crate::sht31::{self, ADDRESS, MEASURE_HIGH_NO_STRETCH, MEASURE_WAIT_MS};
use crate::{ClimateProbe, Panel, PanelView, ProbeError, ProbeMeasure, RelayOutput};

/// Active-low relay outputs.
///
/// SW1 is the fridge compressor on GPIO14. SW2 is the Peltier dehumidifier on
/// GPIO27. SW3 on GPIO26 and SW4 on GPIO25 stay off. Logical on drives the
/// pin low. The idle, safe level is high.
pub struct GpioRelays<'d> {
    sw1: Output<'d>,
    sw2: Output<'d>,
    sw3: Output<'d>,
    sw4: Output<'d>,
}

impl<'d> GpioRelays<'d> {
    /// Take the four outputs and drive every one high.
    ///
    /// The caller creates each pin at [`Level::High`] before this, so the
    /// first driven level is already the idle level. This applies it again
    /// before the first sample.
    pub fn new(sw1: Output<'d>, sw2: Output<'d>, sw3: Output<'d>, sw4: Output<'d>) -> Self {
        let mut relays = Self { sw1, sw2, sw3, sw4 };
        relays.apply(false, false);
        relays
    }
}

impl RelayOutput for GpioRelays<'_> {
    fn apply(&mut self, fridge: bool, dehumidifier: bool) {
        self.sw1.set_level(active_low(fridge));
        self.sw2.set_level(active_low(dehumidifier));
        self.sw3.set_level(Level::High);
        self.sw4.set_level(Level::High);
    }
}

fn active_low(on: bool) -> Level {
    if on { Level::Low } else { Level::High }
}

/// One SHT31 on the shared I2C bus.
///
/// `boot` stamps each sample with whole seconds since the cabinet started.
pub struct Sht31Probe<I> {
    bus: I,
    boot: Instant,
}

impl<I> Sht31Probe<I> {
    /// Read later samples from `bus`.
    pub const fn new(bus: I, boot: Instant) -> Self {
        Self { bus, boot }
    }
}

impl<I> Sht31Probe<I>
where
    I: AsyncI2c,
{
    async fn read_sample(&mut self) -> Result<ProbeMeasure, ProbeError> {
        if self
            .bus
            .write(ADDRESS, &MEASURE_HIGH_NO_STRETCH)
            .await
            .is_err()
        {
            log::error!("SHT31 write failed");
            return Err(ProbeError);
        }
        Timer::after(Duration::from_millis(MEASURE_WAIT_MS)).await;

        let mut buf = [0u8; 6];
        if self.bus.read(ADDRESS, &mut buf).await.is_err() {
            log::error!("SHT31 read failed");
            return Err(ProbeError);
        }
        let now_s = i32::try_from(self.boot.elapsed().as_secs()).unwrap_or(i32::MAX);
        match sht31::decode(&buf, now_s) {
            Ok(sample) => Ok(sample),
            Err(error) => {
                log::error!("SHT31 CRC failed");
                Err(error)
            }
        }
    }
}

impl<I> ClimateProbe for Sht31Probe<I>
where
    I: AsyncI2c,
{
    fn read(&mut self) -> impl Future<Output = Result<ProbeMeasure, ProbeError>> {
        self.read_sample()
    }
}

type Oled<I> =
    Ssd1306Async<I2CInterface<I>, DisplaySize128x64, BufferedGraphicsModeAsync<DisplaySize128x64>>;

/// SSD1306 128×64 panel at I2C address `0x3C`, rotation 0.
///
/// The 1024-byte framebuffer lives in this value. The display task stores it
/// inside [`crate::DisplayActor`].
pub struct Ssd1306Panel<I> {
    display: Oled<I>,
}

impl<I> Ssd1306Panel<I>
where
    I: AsyncI2c,
{
    /// Initialize the panel before the first frame.
    pub async fn init(i2c: I) -> Self {
        let interface = I2CDisplayInterface::new(i2c);
        let mut display = Ssd1306Async::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
            .into_buffered_graphics_mode();
        if display.init().await.is_err() {
            log::error!("OLED init failed");
            panic!("OLED init failed");
        }
        Self { display }
    }

    async fn paint(&mut self, view: &PanelView<'_>) {
        if draw_frame(&mut self.display, view).is_err() {
            log::error!("OLED frame rejected");
            return;
        }
        if self.display.flush().await.is_err() {
            log::error!("OLED flush failed");
        }
    }
}

impl<I> Panel for Ssd1306Panel<I>
where
    I: AsyncI2c,
{
    async fn render(&mut self, view: &PanelView<'_>) {
        self.paint(view).await;
    }
}
