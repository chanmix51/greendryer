#![no_std]
#![no_main]
#![deny(
    clippy::mem_forget,
    reason = "mem::forget is generally not safe to do with esp_hal types, especially those \
    holding buffers for the duration of a data transfer."
)]
#![deny(clippy::large_stack_frames)]

// Embassy
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
    rng::Rng,
    time::Rate,
    timer::timg::TimerGroup,
};

// OLED
use embedded_graphics::{
    mono_font::{MonoTextStyleBuilder, ascii::FONT_6X10},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{Circle, PrimitiveStyle},
    text::{Baseline, Text},
};
use ssd1306::{I2CDisplayInterface, Ssd1306Async, mode::BufferedGraphicsModeAsync, prelude::*};

// General
use log::{error, info};

// This creates a default app-descriptor required by the esp-idf bootloader.
// For more information see: <https://docs.espressif.com/projects/esp-idf/en/stable/esp32/api-reference/system/app_image_format.html#application-description>
esp_bootloader_esp_idf::esp_app_desc!();

const OLED_WIDTH: i32 = 128;
const TITLE: &str = "SWITCH STATE";
const TITLE_Y: i32 = 3;
const BLUE_TOP: i32 = 16;
const ROW_HEIGHT: i32 = 12;
const LABEL_X: i32 = 4;
const LED_DIAMETER: u32 = 8;
const LED_RIGHT_PAD: i32 = 4;

type OledI2c = I2c<'static, Async>;
type OledDisplay = Ssd1306Async<
    I2CInterface<OledI2c>,
    DisplaySize128x64,
    BufferedGraphicsModeAsync<DisplaySize128x64>,
>;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct AppState {
    switch_1: bool,
    switch_2: bool,
    switch_3: bool,
    switch_4: bool,
}

impl AppState {
    fn switches(self) -> [(&'static str, bool); 4] {
        [
            ("SW1", self.switch_1),
            ("SW2", self.switch_2),
            ("SW3", self.switch_3),
            ("SW4", self.switch_4),
        ]
    }
}

static STATE: Mutex<CriticalSectionRawMutex, AppState> = Mutex::new(AppState {
    switch_1: false,
    switch_2: false,
    switch_3: false,
    switch_4: false,
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

        let text_style = MonoTextStyleBuilder::new()
            .font(&FONT_6X10)
            .text_color(BinaryColor::On)
            .build();

        let title_width = TITLE.len() as i32 * FONT_6X10.character_size.width as i32;
        let title_x = (OLED_WIDTH - title_width) / 2;
        Text::with_baseline(
            TITLE,
            Point::new(title_x, TITLE_Y),
            text_style,
            Baseline::Top,
        )
        .draw(&mut self.display)
        .unwrap();

        let led_x = OLED_WIDTH - LED_RIGHT_PAD - LED_DIAMETER as i32;
        for (index, (label, on)) in state.switches().iter().copied().enumerate() {
            let row_top = BLUE_TOP + index as i32 * ROW_HEIGHT;
            Text::with_baseline(
                label,
                Point::new(LABEL_X, row_top + 1),
                text_style,
                Baseline::Top,
            )
            .draw(&mut self.display)
            .unwrap();

            let led_y = row_top + (ROW_HEIGHT - LED_DIAMETER as i32) / 2;
            let style = if on {
                PrimitiveStyle::with_fill(BinaryColor::On)
            } else {
                PrimitiveStyle::with_stroke(BinaryColor::On, 1)
            };
            Circle::new(Point::new(led_x, led_y), LED_DIAMETER)
                .into_styled(style)
                .draw(&mut self.display)
                .unwrap();
        }

        if let Err(e) = self.display.flush().await {
            error!("OLED flush failed: {e:?}");
            panic!("OLED flush failed: {e:?}");
        }
    }
}

fn level_from_switch(on: bool) -> Level {
    if on { Level::High } else { Level::Low }
}

fn random_bit(rng: &Rng) -> bool {
    rng.random() & 1 != 0
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
    // generator parameters: --chip esp32 -o esp32-wroom-32e -o unstable-hal -o embassy -o log -o esp-backtrace -o neovim

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

    let output_config = OutputConfig::default();
    let mut sw1 = Output::new(peripherals.GPIO14, Level::Low, output_config);
    let mut sw2 = Output::new(peripherals.GPIO27, Level::Low, output_config);
    let mut sw3 = Output::new(peripherals.GPIO26, Level::Low, output_config);
    let mut sw4 = Output::new(peripherals.GPIO25, Level::Low, output_config);

    spawner.spawn(display_worker::run(i2c).expect("failed to spawn display worker"));

    let rng = Rng::new();
    loop {
        let new_state = AppState {
            switch_1: random_bit(&rng),
            switch_2: random_bit(&rng),
            switch_3: random_bit(&rng),
            switch_4: random_bit(&rng),
        };

        sw1.set_level(level_from_switch(new_state.switch_1));
        sw2.set_level(level_from_switch(new_state.switch_2));
        sw3.set_level(level_from_switch(new_state.switch_3));
        sw4.set_level(level_from_switch(new_state.switch_4));

        *STATE.lock().await = new_state;

        info!(
            "SW1={} SW2={} SW3={} SW4={}",
            new_state.switch_1, new_state.switch_2, new_state.switch_3, new_state.switch_4
        );

        Timer::after(Duration::from_secs(2)).await;
    }

    // for inspiration have a look at the examples at https://github.com/esp-rs/esp-hal/tree/esp-hal-v1.1.0/examples
}
