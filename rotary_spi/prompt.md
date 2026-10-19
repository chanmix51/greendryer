# Implementation prompt

Implement the rotary-encoder game in the `rotary_spi` crate. The board is an ESP32. A KY-040 moves a direction marker around an `@`, and a click steps that `@` toward a `*`. The panel is the same 2.0" ST7789V as `spi_display`.

Replace the placeholder loop in `rotary_spi/src/bin/main.rs`. Keep this crate `no_std` and `no_main`, on `esp-hal` 1.1 and `esp-rtos`, with the bootloader application descriptor and the existing Clippy denies. Host tests live in the library and run with `cargo test`. Under `cfg(test)` they may use `std`, as `greendryer` does. Do not modify `spi_display` or `greendryer`.

Copy the panel bring-up into this crate. `spi_display` is not a library. Add `embedded-graphics` 0.8, `mipidsi` 0.9, `embedded-hal-bus` 0.3, and `embassy-sync` 0.8. The display task owns the SPI bus for the life of the firmware. The value shared between tasks is the game state.

Follow the build order. For each step, write the host tests first, confirm they fail, then write the code that makes them pass.

## Build order

### 1. `GameState`

Write the tests, then implement `GameState` in the library. No GPIO, no SPI, and no hardware RNG in this step. Placing a flower takes one `u16` candidate so a test can choose the cell.

### 2. Frame and `DisplayActor`

Write the tests, then implement a pure frame description and a `DisplayActor` that draws it into `impl DrawTarget<Color = Rgb565>`. This step does not open the ST7789. A recording draw target is enough. Do not allocate a 240×320 framebuffer.

### 3. Firmware

Wire the KY-040 interrupt handler, the event channel, the published state, and the ST7789 task in the binary. The firmware build for this crate's ESP32 target is the check that the panel and the GPIO handler compile. `cargo test` does not need the chip.

## Board

The panel is 240×320. The font is `FONT_10X20`, so the character grid is 24 columns by 16 rows and each cell is 10×20 pixels. Cell (0, 0) is the top-left pixel of the panel. There is no text margin.

Screen row 0 is the score line. The game area is screen rows 1 through 15: 24 columns by 15 rows. Game coordinates are `col` in `0..24` and `row` in `0..15`. Screen row is `game row + 1`.

The score is the decimal value of a `u32`, with no caption, horizontally centered in row 0. Its glyph box stays inside that row. A score of `0` is the ten-pixel string `"0"` at x = 115. In general the x origin is `(240 - 10 * digit_count) / 2`.

The PAC starts at game cell `(11, 7)`. That is screen row 8. Column 11 is the chosen center: a 24-column row has no single middle column.

Glyphs and the rectangle are white (`Rgb565::WHITE`) on a black panel. Empty cells contribute no glyph.

Draw the game glyphs first, then a 1 px white stroke on `Rectangle::new(Point::new(0, 20), Size::new(240, 300))`. The stroke overlaps the outer pixels of the edge glyphs, and the panel clips the stroke on x = 0, x = 239, and y = 319. Keep that stroke width.

## Rules

Three glyphs:

- PAC is `@` (ASCII 64).
- VECTOR is `-` (ASCII 45). It occupies the cell one orthogonal step from the PAC. Diagonals are not used.
- FLOWER is `*` (ASCII 42). There is one flower.

Directions in clockwise order are east, south, west, north. Counter-clockwise is the reverse. The initial direction is east, so the vector cell starts at `(12, 7)`.

`GameState` stores the PAC cell, the direction, the flower cell, and the score. It starts at score 0.

Flower placement takes one integer and is uniform over the 359 game cells that are not the PAC. Let `CELLS = 360` and let `pac_i = row * 24 + col`. Reduce the candidate modulo 359, then if that index is greater than or equal to `pac_i`, add one. The resulting index `i` is cell `(i % 24, i / 24)`. Placement always finishes. The flower may land on the vector cell. It never lands on the PAC. Use this same function for the flower created at boot and for every flower created after a score.

One clockwise event advances one step in the clockwise order. If that neighbor would lie outside the game area, keep advancing clockwise until the neighbor is inside. One counter-clockwise event does the same in the other direction. One event therefore always leaves the vector on a visible cell, including at a corner, where two directions are outside. A corner step skips every outside direction and stops on the next inside one.

A click moves the PAC one cell in the current direction. The direction value stays the same, so the vector stays on the same side of the PAC. There is no animation and no repeat.

The vector cell may lie outside the game area after that move. Examples: the PAC is in column 22 facing east, and a click puts it in column 23; east of column 23 is outside. While the neighbor is outside, draw no `-`. A click in that state leaves the PAC, the direction, the flower, and the score unchanged. The following rotation event still uses the skip rule, so it moves from that outside direction to the next direction whose cell is inside.

When the PAC's new cell is the flower's cell, add one to the score with saturating arithmetic and place a new flower. The new flower is chosen after the PAC has moved.

Cell drawing, first match wins:

1. The flower cell draws `*`.
2. The PAC cell draws `@`.
3. The vector cell, when it is inside the game area, draws `-`.
4. Any other cell draws nothing.

A flower on the vector cell draws `*` only. The direction is unchanged, and a click still steps the PAC toward that cell.

## Encoder

The handler and the game task communicate through one channel:

`Channel<CriticalSectionRawMutex, EncoderEvent, 16>`

`EncoderEvent` is `Clockwise`, `CounterClockwise`, or `Click`.

From the handler, send with `try_send`. A full channel drops the new event. The handler does not take the published-state mutex and does not touch SPI. `send().await` is not available in the handler.

Pins, with internal pull-ups:

- GPIO32 is CLK. Interrupt on the falling edge only. Sample GPIO33 (DT) in that handler.
- GPIO27 is SW. Interrupt on the falling edge. The switch is active-low. One falling edge is one `Click`. Holding the switch produces no further clicks.

`DT_HIGH_IS_CLOCKWISE` is a named `bool` constant, default `true`. On an accepted CLK falling edge, DT high enqueues `Clockwise` and DT low enqueues `CounterClockwise`.

Each pin has its own last-accepted timestamp. An edge arriving less than 15 ms after that pin's last accepted edge is ignored. The handler reads the clock and returns. It does not delay inside the interrupt.

## Tasks

The game task owns `GameState`. It waits on the channel, applies one event, and publishes a clone. It is the only writer of the game. The flower candidate comes from the ESP32 hardware RNG, reduced as specified above. Publish the initial state, including the first flower, before the display task paints.

The published value is `Mutex<CriticalSectionRawMutex, GameState>`, the same pattern as `greendryer`'s published `AppState`.

The display task owns SPI. It wakes every 100 ms, clones the mutex, and calls `DisplayActor::refresh`. `refresh` paints only when the cloned state differs from the last painted state. `GameState` implements `PartialEq`. Two wakes with the same state paint once. `refresh` is synchronous: the panel SPI is blocking, as in `spi_display`.

Panel bring-up matches `spi_display`:

- SPI3, MOSI GPIO23, SCK GPIO18, CS GPIO5, DC GPIO17, backlight GPIO13.
- CS is a strapping pin and idles high. Backlight is driven high.
- Reset is tied to EN. Wait 120 ms before the first command. This task does not drive a reset GPIO.
- Mode 3, 20 MHz, MSB first, color order RGB, color inversion inverted, rotation 0, display size 240×320, offset (0, 0).
- A 512-byte scratch buffer for `SpiInterface`.
- `ExclusiveDevice` around the blocking SPI master. Draw directly into the mipidsi target.

## Done when

Host tests cover all of the following:

- Boot state is PAC `(11, 7)`, direction east, score 0, and a flower produced by candidate `0` that is not the PAC cell.
- Clockwise from east yields south, then west, then north, then east. Counter-clockwise from east yields north.
- With the PAC at `(23, 0)`, clockwise from west yields south. East and north are outside, and one event skips both.
- A click from `(11, 7)` facing east moves the PAC to `(12, 7)` and leaves the direction east. The vector cell is then `(13, 7)`.
- A click from `(23, 7)` facing east leaves the PAC at `(23, 7)` and leaves the direction east.
- After a click moves the PAC from `(22, 7)` to `(23, 7)` facing east, the frame has no `-`. The next clockwise event sets the direction to south.
- A flower on the vector cell appears as `*` in the frame, and the frame has no `-`.
- A click onto the flower adds one to the score and places the next flower from the supplied candidate. That flower is not the PAC's new cell.
- Candidate indexes `0` and `358` map to the two cells on either side of the PAC in linear order, never to the PAC itself.
- The frame's score string for score `0` is `"0"` at x = 115. The rectangle is `(0, 20)` size `240×300`.
- `DisplayActor::refresh` paints the first state and leaves the frame count unchanged on a second call with an equal state. A changed score paints again.

The binary publishes state, spawns the display task, and applies channel events from the two falling-edge handlers.
