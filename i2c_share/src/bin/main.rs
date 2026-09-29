#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use core::fmt::Write;

// Embassy
use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Timer};

// ESP
use esp_backtrace as _;
use esp_hal::{
    Async, Config as EspConfig,
    clock::CpuClock,
    gpio::{Level, Output, OutputConfig},
    i2c::master::{Config as I2cConfig, I2c},
    time::Rate,
    timer::timg::TimerGroup,
};

// OLED
use embedded_graphics::{
    mono_font::{
        MonoTextStyleBuilder,
        iso_8859_1::{FONT_6X10, FONT_10X20},
    },
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Triangle},
    text::{Baseline, Text},
};
use ssd1306::{I2CDisplayInterface, Ssd1306Async, mode::BufferedGraphicsModeAsync, prelude::*};

// I2C / probe
use embedded_hal_async::i2c::I2c as _;
use static_cell::StaticCell;

// General
use log::{error, info};

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

const OLED_WIDTH: i32 = 128;
const YELLOW_H: i32 = 16;
const BLUE_TOP: i32 = 16;
const MODE_LABEL: &str = "ÉCHEC / ARRÊT";
const MODE_X: i32 = 4;
const MODE_Y: i32 = 3;
const CONTENT_X: i32 = 12;
const TEMP_RH_Y: i32 = BLUE_TOP + 12;
const VPD_Y: i32 = BLUE_TOP + TEMP_RH_Y + 12;

const TRI_W: i32 = 10;
const TRI_H: i32 = 10;
const TRI_GAP: i32 = 4;
const TRI_RIGHT_PAD: i32 = 4;

const VPD_HIGH_KPA: f32 = 0.750;
const VPD_LOW_KPA: f32 = 0.650;

const SHT31_ADDR: u8 = 0x44;
const SHT31_MEASURE_HIGH_NO_STRETCH: [u8; 2] = [0x24, 0x00];
const SHT31_MEASURE_MS: u64 = 16;

type I2cBus = Mutex<CriticalSectionRawMutex, I2c<'static, Async>>;
type OledI2c = I2cDevice<'static, CriticalSectionRawMutex, I2c<'static, Async>>;
type OledDisplay = Ssd1306Async<
    I2CInterface<OledI2c>,
    DisplaySize128x64,
    BufferedGraphicsModeAsync<DisplaySize128x64>,
>;

static I2C_BUS: StaticCell<I2cBus> = StaticCell::new();

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct AppState {
    switch_1: bool,
    switch_2: bool,
    switch_3: bool,
    switch_4: bool,
    temperature_c: Option<f32>,
    humidity_rh: Option<f32>,
    vpd_kpa: Option<f32>,
    probe_error: bool,
}

static STATE: Mutex<CriticalSectionRawMutex, AppState> = Mutex::new(AppState {
    switch_1: false,
    switch_2: false,
    switch_3: false,
    switch_4: false,
    temperature_c: None,
    humidity_rh: None,
    vpd_kpa: None,
    probe_error: false,
});

struct Displayer {
    display: OledDisplay,
}

#[allow(
    clippy::large_stack_frames,
    reason = "the SSD1306 framebuffer is 1024 bytes"
)]
impl Displayer {
    async fn init(i2c: OledI2c) -> Self {
        let interface = I2CDisplayInterface::new(i2c);
        let mut display = Ssd1306Async::new(interface, DisplaySize128x64, DisplayRotation::Rotate0)
            .into_buffered_graphics_mode();

        if let Err(e) = display.init().await {
            error!("OLED init failed: {e:?}");
            panic!("OLED init failed: {e:?}");
        }

        Self { display }
    }

    async fn render(&mut self, state: &AppState) {
        self.display.clear_buffer();

        let small = MonoTextStyleBuilder::new()
            .font(&FONT_6X10)
            .text_color(BinaryColor::On)
            .build();
        let large = MonoTextStyleBuilder::new()
            .font(&FONT_10X20)
            .text_color(BinaryColor::On)
            .build();

        Text::with_baseline(MODE_LABEL, Point::new(MODE_X, MODE_Y), small, Baseline::Top)
            .draw(&mut self.display)
            .unwrap();

        let down_right = OLED_WIDTH - TRI_RIGHT_PAD;
        let down_cx = down_right - TRI_W / 2;
        let up_cx = down_cx - TRI_W - TRI_GAP;
        draw_up_triangle(&mut self.display, up_cx, !state.switch_1);
        draw_down_triangle(&mut self.display, down_cx, !state.switch_2);

        if state.probe_error {
            Text::with_baseline(
                "probe read error",
                Point::new(CONTENT_X, TEMP_RH_Y),
                small,
                Baseline::Top,
            )
            .draw(&mut self.display)
            .unwrap();
        } else if let (Some(temp_c), Some(humidity_rh)) = (state.temperature_c, state.humidity_rh) {
            let mut line: heapless::String<16> = heapless::String::new();
            write!(&mut line, "{temp_c:04.1}°C {humidity_rh:02.0}%").unwrap();
            Text::with_baseline(
                &line,
                Point::new(CONTENT_X, TEMP_RH_Y),
                large,
                Baseline::Top,
            )
            .draw(&mut self.display)
            .unwrap();
        }

        if let Some(vpd_kpa) = state.vpd_kpa {
            let mut line: heapless::String<16> = heapless::String::new();
            write!(&mut line, "{vpd_kpa:.3} kPa").unwrap();
            Text::with_baseline(&line, Point::new(CONTENT_X + 26, VPD_Y), small, Baseline::Top)
                .draw(&mut self.display)
                .unwrap();
        }

        if let Err(e) = self.display.flush().await {
            error!("OLED flush failed: {e:?}");
            panic!("OLED flush failed: {e:?}");
        }
    }
}

fn triangle_y_range() -> (i32, i32) {
    let top = (YELLOW_H - TRI_H) / 2;
    (top, top + TRI_H)
}

fn draw_up_triangle(display: &mut OledDisplay, cx: i32, filled: bool) {
    let (top, bottom) = triangle_y_range();
    let half = TRI_W / 2;
    Triangle::new(
        Point::new(cx, top),
        Point::new(cx - half, bottom),
        Point::new(cx + half, bottom),
    )
    .into_styled(if filled { PrimitiveStyle::with_fill( BinaryColor::On ) } else { PrimitiveStyle::with_stroke( BinaryColor::On, 1 ) })
    .draw(display)
    .unwrap();
}

fn draw_down_triangle(display: &mut OledDisplay, cx: i32, filled: bool) {
    let (top, bottom) = triangle_y_range();
    let half = TRI_W / 2;
    Triangle::new(
        Point::new(cx, bottom),
        Point::new(cx - half, top),
        Point::new(cx + half, top),
    )
    .into_styled(if filled { PrimitiveStyle::with_fill( BinaryColor::On ) } else { PrimitiveStyle::with_stroke( BinaryColor::On, 1 ) })
    .draw(display)
    .unwrap();
}

fn level_from_switch(on: bool) -> Level {
    if on { Level::Low } else { Level::High }
}

fn saturation_vapor_pressure_kpa(temp_c: f32) -> f32 {
    0.6108 * libm::expf(17.27 * temp_c / (temp_c + 237.3))
}

fn vpd_kpa(temp_c: f32, humidity_rh: f32) -> f32 {
    saturation_vapor_pressure_kpa(temp_c) * (1.0 - humidity_rh / 100.0)
}

fn switches_from_vpd(vpd: f32) -> (bool, bool) {
    if vpd > VPD_HIGH_KPA {
        (true, false)
    } else if vpd < VPD_LOW_KPA {
        (false, true)
    } else {
        (false, false)
    }
}

#[derive(Debug)]
enum ProbeError {
    I2c,
    Crc,
}

fn sht3x_crc(data: &[u8; 2]) -> u8 {
    let mut crc: u8 = 0xFF;
    for &byte in data {
        crc ^= byte;
        for _ in 0..8 {
            if crc & 0x80 != 0 {
                crc = crc.wrapping_shl(1) ^ 0x31;
            } else {
                crc = crc.wrapping_shl(1);
            }
        }
    }
    crc
}

fn check_crc(data: &[u8], expected: u8) -> Result<(), ()> {
    let word: [u8; 2] = data.try_into().map_err(|_| ())?;
    if sht3x_crc(&word) == expected {
        Ok(())
    } else {
        Err(())
    }
}

/// Single-shot high-repeatability read without clock stretching.
/// ESP32 I2C master can hang on the 0x2Cxx stretch commands used by typical SHT3x drivers.
#[allow(
    clippy::large_stack_frames,
    reason = "async I2C futures include large esp-hal error types"
)]
async fn read_sht31(i2c: &mut OledI2c) -> Result<(f32, f32), ProbeError> {
    if let Err(e) = i2c.write(SHT31_ADDR, &SHT31_MEASURE_HIGH_NO_STRETCH).await {
        error!("SHT31 write failed: {e:?}");
        return Err(ProbeError::I2c);
    }
    Timer::after(Duration::from_millis(SHT31_MEASURE_MS)).await;

    let mut buf = [0u8; 6];
    if let Err(e) = i2c.read(SHT31_ADDR, &mut buf).await {
        error!("SHT31 read failed: {e:?}");
        return Err(ProbeError::I2c);
    }

    check_crc(&buf[0..2], buf[2]).map_err(|()| ProbeError::Crc)?;
    check_crc(&buf[3..5], buf[5]).map_err(|()| ProbeError::Crc)?;

    let st = u16::from_be_bytes([buf[0], buf[1]]);
    let srh = u16::from_be_bytes([buf[3], buf[4]]);
    let temperature_c = (f32::from(st) * 175.0) / 65535.0 - 45.0;
    let humidity_rh = (f32::from(srh) * 100.0) / 65535.0;
    Ok((temperature_c, humidity_rh))
}

mod display_worker {
    #![allow(
        clippy::large_stack_frames,
        reason = "the SSD1306 framebuffer is 1024 bytes"
    )]

    use super::*;

    #[embassy_executor::task]
    pub async fn run(i2c: OledI2c) {
        let mut displayer = Displayer::init(i2c).await;
        info!("Display worker started");

        let mut last_state: Option<AppState> = None;
        loop {
            let state = *STATE.lock().await;
            if last_state != Some(state) {
                displayer.render(&state).await;
                last_state = Some(state);
            }
            Timer::after(Duration::from_millis(100)).await;
        }
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    // generator version: 1.3.0
    // generator parameters: --chip esp32 -o esp32-wroom-32e -o unstable-hal -o embassy -o log -o esp-backtrace -o vscode -o neovim

    esp_println::logger::init_logger_from_env();

    let config = EspConfig::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // The following pins are used to bootstrap the chip. They are available
    // for use, but check the datasheet of the module for more information on them.
    // - GPIO0
    // - GPIO2
    // - GPIO5
    // - GPIO12 (SW2 relay; high-Z at boot, driven push-pull after init)
    // - GPIO15
    // These GPIO pins are in use by some feature of the module and should not be used.
    let _ = peripherals.GPIO6;
    let _ = peripherals.GPIO7;
    let _ = peripherals.GPIO8;
    let _ = peripherals.GPIO9;
    let _ = peripherals.GPIO10;
    let _ = peripherals.GPIO11;
    let _ = peripherals.GPIO16;
    let _ = peripherals.GPIO20;

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    info!("Embassy initialized!");

    let i2c = I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(400)),
    )
    .expect("I2C configuration is invalid")
    .with_sda(peripherals.GPIO21)
    .with_scl(peripherals.GPIO22)
    .into_async();

    let i2c_bus = I2C_BUS.init(Mutex::new(i2c));
    let oled_i2c = I2cDevice::new(i2c_bus);
    let mut sht_i2c = I2cDevice::new(i2c_bus);

    let output_config = OutputConfig::default();
    let mut sw1 = Output::new(peripherals.GPIO14, Level::High, output_config);
    let mut sw2 = Output::new(peripherals.GPIO27, Level::High, output_config);
    let mut sw3 = Output::new(peripherals.GPIO26, Level::High, output_config);
    let mut sw4 = Output::new(peripherals.GPIO25, Level::High, output_config);

    spawner.spawn(display_worker::run(oled_i2c).expect("failed to spawn display worker"));

    let mut state = AppState::default();
    loop {
        match read_sht31(&mut sht_i2c).await {
            Ok((temperature_c, humidity_rh)) => {
                let vpd = vpd_kpa(temperature_c, humidity_rh);
                let (switch_1, switch_2) = switches_from_vpd(vpd);
                state.switch_1 = switch_1;
                state.switch_2 = switch_2;
                state.switch_3 = false;
                state.switch_4 = false;
                state.temperature_c = Some(temperature_c);
                state.humidity_rh = Some(humidity_rh);
                state.vpd_kpa = Some(vpd);
                state.probe_error = false;

                sw1.set_level(level_from_switch(state.switch_1));
                sw2.set_level(level_from_switch(state.switch_2));
                sw3.set_level(level_from_switch(state.switch_3));
                sw4.set_level(level_from_switch(state.switch_4));

                *STATE.lock().await = state;
                info!(
                    "T={temperature_c:.2}C RH={humidity_rh:.2}% VPD={vpd:.3}kPa SW1={switch_1} SW2={switch_2}"
                );
            }
            Err(e) => {
                error!("probe read error: {e:?}");
                state.probe_error = true;
                *STATE.lock().await = state;
            }
        }

        Timer::after(Duration::from_secs(1)).await;
    }
}
