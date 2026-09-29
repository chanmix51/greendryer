#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

use embassy_embedded_hal::shared_bus::asynch::i2c::I2cDevice;
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::{Duration, Instant, Timer};
use esp_backtrace as _;
use esp_hal::Async;
use esp_hal::clock::CpuClock;
use esp_hal::gpio::{Level, Output, OutputConfig};
use esp_hal::i2c::master::{Config as I2cConfig, I2c};
use esp_hal::time::Rate;
use esp_hal::timer::timg::TimerGroup;
use greendryer::{AppState, GpioRelays, MainActor, SAMPLE_SLOTS, STATUS_BYTES, Sht31Probe};
use static_cell::StaticCell;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

type I2cBus = Mutex<CriticalSectionRawMutex, I2c<'static, Async>>;
type SharedI2c = I2cDevice<'static, CriticalSectionRawMutex, I2c<'static, Async>>;
type Published = Mutex<CriticalSectionRawMutex, AppState<SAMPLE_SLOTS, STATUS_BYTES>>;

static I2C_BUS: StaticCell<I2cBus> = StaticCell::new();
static PUBLISHED: StaticCell<Published> = StaticCell::new();

mod display_task {
    #![allow(
        clippy::large_stack_frames,
        reason = "the SSD1306 framebuffer is 1024 bytes, and this task awaits async I2C futures with large esp-hal error types"
    )]

    use super::{Duration, Published, SAMPLE_SLOTS, STATUS_BYTES, SharedI2c, Timer};
    use greendryer::{DisplayActor, Ssd1306Panel};

    /// Wake about every 100 ms and paint when the published state changes.
    #[embassy_executor::task]
    pub async fn run(i2c: SharedI2c, published: &'static Published) {
        let panel = Ssd1306Panel::init(i2c).await;
        let mut actor = DisplayActor::<_, SAMPLE_SLOTS, STATUS_BYTES>::new(panel);
        log::info!("display worker started");
        loop {
            let state = published.lock().await.clone();
            actor.refresh(&state).await;
            Timer::after(Duration::from_millis(100)).await;
        }
    }
}

#[allow(
    clippy::large_stack_frames,
    reason = "async I2C futures include large esp-hal error types"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt =
        esp_hal::interrupt::software::SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    // Active-low relays idle high. Drive GPIO14, GPIO27, GPIO26, and GPIO25
    // high before the display task starts and before the first sample.
    let output_config = OutputConfig::default();
    let sw1 = Output::new(peripherals.GPIO14, Level::High, output_config);
    let sw2 = Output::new(peripherals.GPIO27, Level::High, output_config);
    let sw3 = Output::new(peripherals.GPIO26, Level::High, output_config);
    let sw4 = Output::new(peripherals.GPIO25, Level::High, output_config);
    let relays = GpioRelays::new(sw1, sw2, sw3, sw4);

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
    let sht_i2c = I2cDevice::new(i2c_bus);
    let published = PUBLISHED.init(Mutex::new(AppState::new()));

    spawner.spawn(display_task::run(oled_i2c, published).expect("failed to spawn display worker"));

    let boot = Instant::now();
    let mut state = AppState::<SAMPLE_SLOTS, STATUS_BYTES>::new();
    let mut control = MainActor::new(Sht31Probe::new(sht_i2c, boot), relays);
    loop {
        let now_s = u32::try_from(boot.elapsed().as_secs()).unwrap_or(u32::MAX);
        if let Err(error) = control.step(&mut state, now_s).await {
            log::error!("cabinet step failed: {error:?}");
        }
        *published.lock().await = state.clone();
        Timer::after(Duration::from_secs(1)).await;
    }
}
