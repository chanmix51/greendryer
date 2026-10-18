#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

mod display;
mod encoder;

use embassy_executor::Spawner;
use embassy_sync::mutex::Mutex;
use esp_backtrace as _;
use esp_hal::clock::CpuClock;
use esp_hal::rng::Rng;
use esp_hal::timer::timg::TimerGroup;
use rotary_spi::GameState;
use static_cell::StaticCell;

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

static PUBLISHED: StaticCell<display::Published> = StaticCell::new();

#[allow(
    clippy::large_stack_frames,
    reason = "it's not unusual to allocate larger buffers etc. in main"
)]
#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    esp_println::logger::init_logger_from_env();

    let config = esp_hal::Config::default().with_cpu_clock(CpuClock::max());
    let peripherals = esp_hal::init(config);

    // The following pins are used to bootstrap the chip. They are available
    // for use, but check the datasheet of the module for more information on them.
    // - GPIO0
    // - GPIO2
    // - GPIO5
    // - GPIO12
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

    encoder::install(
        peripherals.GPIO32,
        peripherals.GPIO33,
        peripherals.GPIO27,
        peripherals.IO_MUX,
    );

    let rng = Rng::new();
    let mut state = GameState::new(rng.random() as u16);
    let published = PUBLISHED.init(Mutex::new(state.clone()));

    match display::run(
        peripherals.SPI3,
        peripherals.GPIO23,
        peripherals.GPIO18,
        peripherals.GPIO5,
        peripherals.GPIO13,
        peripherals.GPIO17,
        published,
    ) {
        Ok(token) => spawner.spawn(token),
        Err(_err) => log::error!("failed to spawn display task"),
    }

    log::info!("rotary game started");
    loop {
        let event = encoder::EVENTS.receive().await;
        state.apply(event, rng.random() as u16);
        *published.lock().await = state.clone();
    }
}
