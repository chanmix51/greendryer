//! ST7789V panel worker.
//!
//! The task owns the bus and the panel for the life of the firmware. Reset is
//! tied to the ESP32 EN pin, so this task never drives a reset GPIO.

use core::convert::Infallible;

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Delay, Duration, Timer};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_hal_bus::spi::ExclusiveDevice;
use esp_hal::gpio::{DriveMode, Level, Output, OutputConfig};
use esp_hal::peripherals::{GPIO5, GPIO13, GPIO17, GPIO18, GPIO23, SPI3};
use esp_hal::spi::master::{Config, Spi};
use esp_hal::spi::{BitOrder, Mode};
use esp_hal::time::Rate;
use mipidsi::Builder;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, ColorOrder, Orientation, Rotation};
use rotary_spi::{DisplayActor, GameState};
use static_cell::ConstStaticCell;

/// These 2.0" panels are usually inverted. Flip this on hardware if the image is a negative.
const COLOR_INVERSION: ColorInversion = ColorInversion::Inverted;

/// ST7789 model default. Flip to [`ColorOrder::Bgr`] on hardware if red and blue are swapped.
const COLOR_ORDER: ColorOrder = ColorOrder::Rgb;

const PANEL_WIDTH: u16 = 240;
const PANEL_HEIGHT: u16 = 320;

pub type Published = Mutex<CriticalSectionRawMutex, GameState>;

static SPI_BUFFER: ConstStaticCell<[u8; 512]> = ConstStaticCell::new([0; 512]);

#[embassy_executor::task]
pub async fn run(
    spi: SPI3<'static>,
    mosi: GPIO23<'static>,
    sck: GPIO18<'static>,
    cs: GPIO5<'static>,
    backlight: GPIO13<'static>,
    dc: GPIO17<'static>,
    published: &'static Published,
) {
    let pin_config = OutputConfig::default().with_drive_mode(DriveMode::PushPull);
    let _backlight = Output::new(backlight, Level::High, pin_config);
    // GPIO5 is an ESP32 strapping pin. It is the panel chip-select and idles high.
    let cs = Output::new(cs, Level::High, pin_config);
    let dc = Output::new(dc, Level::Low, pin_config);

    // EN already reset the panel. Wait out the post-reset blanking before the first command.
    Timer::after(Duration::from_millis(120)).await;

    let spi = match open_spi(spi, sck, mosi) {
        Ok(spi) => spi,
        Err((err, sck, mosi)) => {
            log::error!("ST7789 SPI config failed: {err}");
            let _hold = (sck, mosi, cs, dc);
            park().await;
        }
    };

    let Some(panel) = open_panel(spi, cs, dc) else {
        park().await;
    };
    let mut actor = DisplayActor::new(panel);
    log::info!("display task started");
    loop {
        let state = published.lock().await.clone();
        if actor.refresh(&state).is_err() {
            log::error!("ST7789 draw failed");
        }
        Timer::after(Duration::from_millis(100)).await;
    }
}

#[inline(never)]
fn open_spi(
    spi: SPI3<'static>,
    sck: GPIO18<'static>,
    mosi: GPIO23<'static>,
) -> Result<
    Spi<'static, esp_hal::Blocking>,
    (
        esp_hal::spi::master::ConfigError,
        GPIO18<'static>,
        GPIO23<'static>,
    ),
> {
    let spi_config = Config::default()
        .with_frequency(Rate::from_mhz(20))
        .with_mode(Mode::_3)
        .with_read_bit_order(BitOrder::MsbFirst)
        .with_write_bit_order(BitOrder::MsbFirst);

    match Spi::new(spi, spi_config) {
        Ok(spi) => Ok(spi.with_sck(sck).with_mosi(mosi)),
        Err(err) => Err((err, sck, mosi)),
    }
}

#[inline(never)]
fn open_panel(
    spi: Spi<'static, esp_hal::Blocking>,
    cs: Output<'static>,
    dc: Output<'static>,
) -> Option<impl DrawTarget<Color = Rgb565>> {
    let spi_device = match ExclusiveDevice::new(spi, cs, Delay) {
        Ok(device) => device,
        Err(err) => hold_infallible(err),
    };
    let interface = SpiInterface::new(spi_device, dc, SPI_BUFFER.take());
    let mut delay = Delay;
    match Builder::new(ST7789, interface)
        .display_size(PANEL_WIDTH, PANEL_HEIGHT)
        .display_offset(0, 0)
        .orientation(Orientation::new().rotate(Rotation::Deg0))
        .color_order(COLOR_ORDER)
        .invert_colors(COLOR_INVERSION)
        .init(&mut delay)
    {
        Ok(display) => {
            log::info!("ST7789 initialized");
            Some(display)
        }
        Err(err) => {
            log::error!("ST7789 init failed: {err:?}");
            None
        }
    }
}

async fn park() -> ! {
    core::future::pending::<()>().await;
    unreachable!()
}

fn hold_infallible<T>(err: Infallible) -> T {
    match err {}
}
