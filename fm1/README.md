# `fm1`: Board Support Package for M-VAVE FM-1

[![crates.io][crate-badge]][crate]
[![docs.rs][docs-badge]][docs]
[![Build status][ci-badge]][ci]

A `no_std`, allocation-free board support package for the M-VAVE FM-1.
Pass a function of type `fn(fm1::Board) -> !` to `fm1::entry!`. Startup sets
up the CPU0 stacks, initializes RAM and hands the application exclusive
ownership of the board peripherals. The application supplies its panic handler.

```rust,no_run
use fm1::{Board, embedded_hal::{delay::DelayNs, digital::StatefulOutputPin}};

fm1::entry!(app);

fn app(mut board: Board) -> ! {
    loop {
        let Ok(()) = board.panel.led().toggle();
        board.timers.delay_ms(500);
    }
}
```

With an external runtime, acquire the peripherals once with `Board::take()`
in its CPU0 initializer. This starts the timers and leaves interrupt setup to
the caller. Bind TIMER5 and call `board.timers.on_interrupt()` in its handler
to maintain the tick count. [The FM-1 RTICX crate](../fm1-rticx/) uses this
entry path.

## Board peripherals

| Handle | Interface |
| --- | --- |
| `board.panel` | Borrow the PLAY/STOP indicator as an `OutputPin` or `StatefulOutputPin`. Its `set_color` method selects amber (default) or green. Sampling the active-low button briefly blanks the indicator and restores its color and on/off state. |
| `board.panel.into_controls()` | Scan 14 panel buttons, 27 piano keys and their indicators, seven encoders, both PLAY/STOP LED channels, and the 10-bit MASTER potentiometer. |
| `board.panel.into_touch_controls()` | Scan the same panel with per-key strike velocity and polyphonic aftertouch. |
| `board.panel.into_matrix()` | Select one of 11 columns, sample six switch rows or drive the four LED rows. The application controls scan timing and debounce. |
| `board.timers` | TIMER4 provides a 24 MHz cycle counter and `DelayNs`. TIMER5 counts 10 ms ticks. `take_alarm()` hands out the independent TIMER2 countdown once. |
| `board.display.init()` | Initialize the 240 × 240 ST7789 LCD. `write_rect` sends big-endian RGB565 pixels by DMA. `write_rect_buffered` lets drawing overlap the last transfer, `flush` waits for it, and `fill_rect` repeats a solid color from SRAM. |
| `board.headphones` | Read the raw PB5 jack switch with `is_inserted()` or `InputPin`. |
| `board.take_usb()` | Call `into_midi(identity)` on the returned port to enumerate as a full-speed USB-MIDI 1.0 device with one input and one output. Poll the owned handle, receive up to 16 event packets at once, and retry the unsent suffix when the host is busy. |
| `board.audio.start()` | With the `audio` feature, stream signed Q23 stereo samples to the DAC through ALNK0. Use `into_stream()` to move ownership into an interrupt task. Dropping the stream stops DMA. |

By default audio consumes 256 stereo frames about every 5.8 ms at the stock clock rate
of 24 MHz / 544. `start_with_frames` and `into_stream_with_frames` select a
smaller DMA half at startup. Submit a prepared block with `try_write`, or use
`try_write_with` to render when the next half becomes available. A long blocking
LCD transfer can miss the refill deadline and cause old samples to repeat.
`Refill::Late` reports a missed deadline during preparation or copying.

When initializing USB-MIDI, supply your application's USB vendor ID, product
ID and descriptor strings to `into_midi`.

## Drawing with embedded-graphics

Enable the optional `embedded-graphics` feature on `fm1` or `fm1-rticx`.
`fm1::display::graphics` re-exports `embedded_graphics` and `mipidsi`.
Both drawing paths use the BSP's SPI1 DMA strip and work without a heap.

Use `init_mipidsi` to draw directly to the 240 × 240 panel. It returns a
`mipidsi::Display` using the upstream `mipidsi::models::ST7789`. Each draw
finishes its transfer before returning.

```rust,no_run
# #[cfg(feature = "embedded-graphics")]
# fn draw(mut board: fm1::Board) {
use fm1::display::SpiClock;
use fm1::display::graphics::embedded_graphics::{pixelcolor::Rgb565, prelude::*};

let mut lcd = board.display.init_mipidsi(SpiClock::Div1, &mut board.timers).unwrap();
lcd.clear(Rgb565::BLACK).unwrap();
Pixel(Point::new(24, 5), Rgb565::GREEN).draw(&mut lcd).unwrap();
# }
```

To overlap drawing and transfer, use the native `Display` with a caller-sized
`Framebuffer<W, H>`. It stores two bytes per pixel and tracks the first and last
dirty rows. A full-screen canvas uses 115,200 bytes of pixel storage and belongs
in static RAM. The example sends one 96-pixel row at panel y = 37. A second write
without further drawing sends nothing. Call `mark_all_dirty()` for an initial
full repaint or when moving the canvas.

```rust,no_run
# #[cfg(feature = "embedded-graphics")]
# fn draw(lcd: &mut fm1::display::Display) -> Result<(), fm1::display::Error> {
use fm1::display::graphics::{
    Framebuffer,
    embedded_graphics::{pixelcolor::Rgb565, prelude::*},
};

let mut meter = Framebuffer::<96, 12>::new();
Pixel(Point::new(24, 5), Rgb565::GREEN).draw(&mut meter).unwrap();
lcd.write_framebuffer_buffered(8, 32, &mut meter)?;
// The meter can be redrawn while SPI1 sends the copied pixels.
lcd.flush()
# }
```

## Panel controls

Pass a desired `Leds` value to `Controls::scan` every 100 µs. Each completed
11-column sweep yields an owned `Scan`. Use `is_down` for held switches,
`pressed` and `released` for edges, or iterate over `events()` to dispatch
changes. Process every completed scan when counting presses or encoder motion.

```rust
use fm1::panel::{Button, Controls, Encoder, Leds, PlayLedState};

fn scan_column(controls: &mut Controls, leds: &mut Leds, cutoff: &mut i32) {
    let Some(scan) = controls.scan(leds) else { return };
    *cutoff = cutoff.saturating_add(i32::from(scan.encoder_delta(Encoder::Knob1)));
    if scan.pressed(Button::Rec) {
        leds.toggle(Button::Rec);
    }
    if scan.pressed(Button::Play) {
        leds.set_play(PlayLedState::Both);
    }
}
```

`Scan::master()` returns an optional absolute ADC reading. `level.raw()` spans
0–1023. Encoder deltas count complete detents, positive clockwise. For a digital piano
key, pass an index in `0..=26` to `PianoKey::new`, counting from the left. A button or key and
its LED are independent, so a lamp can stay lit after release. The transport LED
has four states (`Off`, `Green`, `Red`, `Both`). `Red` addresses the same warm
STOP channel as `PlayColor::Amber` on the fixed PLAY handle.

With RTICX, start the clock with `Clock::start(board.timers.with_panel_scan_rate())`.
Call the returned timer's `on_interrupt()` and the panel's `scan(&leds)` on every
TIMER5 interrupt. Async deadlines retain their 10 ms unit.

### Piano velocity and aftertouch

Use `board.panel.into_touch_controls()` and call `scan(&leds)` at the same
10 kHz column rate. Its two snapshots describe the same sweep. Each press
has a velocity in 1–127, held constant until release. The lower 20 keys
(F3–C5) also measure pressure in 0–127. The upper seven (C♯5–G5) estimate
velocity through the shared LED line. Their `pressure()` returns `None`.
An upper key already held at startup uses velocity 64 until its first release
lets the scanner measure the open contact.

```rust
use fm1::panel::{Event, Leds, Switch, TouchControls};

fn scan_keyboard(
    controls: &mut TouchControls,
    leds: &Leds,
    mut note_on: impl FnMut(u8, u8),
    mut note_off: impl FnMut(u8),
    mut aftertouch: impl FnMut(u8, u8),
) {
    let Some((scan, touch)) = controls.scan(leds) else { return };
    for event in scan.events() {
        match event {
            Event::Pressed(Switch::Piano(key)) => {
                if let Some(velocity) = touch.velocity(key) {
                    note_on(key.index() + 53, velocity);
                }
            }
            Event::Released(Switch::Piano(key)) => note_off(key.index() + 53),
            _ => {}
        }
    }
    for (key, pressure) in touch.pressure_changes() {
        aftertouch(key.index() + 53, pressure);
    }
}
```

The scanner shares the ADC with MASTER. Each key has a measurement slot every
4.4 ms. Sampling can take several slots to distinguish the landing from contact
bounce. Measuring a held analog key briefly blanks its column. One sweep in four is
reserved for lighting, so indicators can look dimmer in this mode.

## Build and flash

The [`fm1` workflow][ci] runs host tests and builds every firmware example
when the crate or its build inputs change. Download the `fm1-firmware` artifact
from a successful run for the flashable `.fwsc` packages. CI builds the
patched compiler on a cache miss and uses the upstream pi32v2 LLVM release
for the assembler, linker and runtime.

Firmware builds have been tested on x86-64 Linux with Cargo 1.100.0, Python
3.11+ and the following toolchain. Host tests use stable Rust.

| Tool | Tested revision |
| --- | --- |
| [Patched Rust compiler][rust-fork] with Rust library sources | Rust 1.100.0, `3ee95ae` |
| [pi32v2 LLVM tools][llvm-tools], including compiler-rt | `pi32v2-23.1.5-5` |
| [pi32v2-isa][isa], which supplies the FM-1 packager | `701a76f` |

Follow [the upstream compiler build instructions][compiler-build] to build
Rust against LLVM with the host and PI32V2 backends. Set these paths for your
installation.

```sh
export RUSTC=/path/to/pi32v2-rust/build/host/stage1/bin/rustc
export PI32V2_BIN=/path/to/pi32v2-llvm/bin
export PI32V2_ISA=/path/to/pi32v2-isa
```

From the `fm1-rust` workspace root, `./fm1/scripts/build-example.sh blink`
writes `target/firmware/blink.fwsc` alongside its ELF and raw application
binary. Use `display` for the native RGB test card, `graphics` for the
embedded-graphics card or `tone` for the audio test. The script enables
each example's required features.

1. On an FM-1 with M-Bootloader installed, hold **REC** while powering on to
   enter USB download mode. Connect the device with a USB data cable.
2. In [M-Flasher][flasher], back up the unit. Choose **Firmware**, select
   `target/firmware/blink.fwsc` and restart the unit after flashing. The
   package updates the application area and preserves the bootloader.
3. The PLAY LED should blink once per second. Hold **PLAY** for two seconds
   to return to download mode. In the display, graphics and tone examples,
   press **PLAY** to return. The graphics card normally reads **ST7789**.
   Hold **PLAY** at startup to select **FRAMEBUFFER**, then release it.

## Use from another crate

Add `fm1 = "0.1.1"` under `[dependencies]` in your application's
`Cargo.toml`, or create a new binary crate with the [published BSP][crate].

```sh
cargo new --bin my-app
cd my-app
cargo add fm1@0.1.1
```

Keep the target specification in the application root so Cargo can load it
before building dependencies. The [blink example][blink-example] shows how
to use `fm1::entry!` and supply a panic handler in `src/main.rs`.

```sh
release=https://raw.githubusercontent.com/fm1-rust/fm1-rust/refs/tags/fm1/v0.1.1/fm1
curl -fsSLo pi32v2-none.json "$release/targets/pi32v2-none.json"
# For a new project, replace Cargo's generated main.rs with the blink example.
curl -fsSLo src/main.rs "$release/examples/blink.rs"
```

You can type-check the example on the host with `cargo check`. To build
firmware, set `RUSTC`, `PI32V2_BIN` and `PI32V2_ISA` as shown in
[Build and flash](#build-and-flash). The `fm1` build script supplies the
startup code and linker script.

```sh
export RUSTC_BOOTSTRAP=1
export CARGO_TARGET_PI32V2_NONE_LINKER="$PI32V2_BIN/ld.lld"
export RUSTFLAGS="-C link-arg=-Tfm1.ld"
cargo build --release --target ./pi32v2-none.json \
  -Zjson-target-spec -Zbuild-std=core,compiler_builtins \
  -Zbuild-std-features=compiler-builtins-mem
python3 "$PI32V2_ISA/tools/hwtest/fm1app.py" my-app.fwsc \
  --from-elf target/pi32v2-none/release/my-app
```

For a different binary name, replace `my-app` in the packaging command.
Select the resulting `.fwsc` file in [M-Flasher][flasher].

## License

Original BSP code is licensed under the [MIT License][mit].

Julius Vitkauskas’s [Rust blink example][blink-origin] and its [C predecessor][c-blink]
supply the startup assembly, linker layout, pi32v2 target, timer interrupt,
PLAY wiring, return-to-bootloader sequence and ST7789 bring-up. His [MIT-0
license][mit-0] permits us to distribute these adaptations under MIT.

`fm1` incorporates Rust code adapted from Felucca’s C headers under the
[Mozilla Public License 2.0][mpl]. The default build uses the [panel input
scanner][panel-driver], [ADC setup][adc-driver] and [USB controller][usb-controller],
along with Felucca’s [USB endpoint driver][usb-driver]. The `audio` feature
enables the port of the [ALNK0 audio driver][audio-driver] by Leo Kuroshita
(Hügelton Instruments) and Kerem Kilic (Ellic Studio).

The panel matrix and headphone switch wiring follow the [Felucca board map][felucca].
The physical control names follow [Charles Vestal’s panel map][panel-labels].
The green PLAY indicator uses the position measured by [Araminta][play-green].
Piano expression uses Felucca’s row-drive and probe wiring and the strike curves
from Charles Vestal’s [MPL-2.0 `fm1-base` touch library][touch-driver].

[crate-badge]: https://img.shields.io/crates/v/fm1?logo=rust
[crate]: https://crates.io/crates/fm1
[docs-badge]: https://docs.rs/fm1/badge.svg
[docs]: https://docs.rs/fm1/
[ci-badge]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1.yml/badge.svg?branch=main&event=push
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1.yml
[mit]: https://github.com/fm1-rust/fm1-rust/blob/main/fm1/LICENSE
[mit-0]: https://spdx.org/licenses/MIT-0.html
[mpl]: https://www.mozilla.org/MPL/2.0/
[blink-origin]: https://github.com/jvitkauskas/pi32v2-isa/tree/701a76faf103d1889d9bdf67987b872899c27257/examples/fm1-blink-rust
[c-blink]: https://github.com/jvitkauskas/pi32v2-isa/tree/701a76faf103d1889d9bdf67987b872899c27257/examples/fm1-blink
[blink-example]: https://github.com/fm1-rust/fm1-rust/blob/fm1/v0.1.1/fm1/examples/blink.rs
[compiler-build]: https://github.com/jvitkauskas/pi32v2-isa/blob/701a76faf103d1889d9bdf67987b872899c27257/examples/fm1-blink-rust/README.md#the-patched-rustc
[rust-fork]: https://github.com/jvitkauskas/pi32v2-rust/tree/3ee95ae1dfc454a3eca42af41db34e356d8ff658
[llvm-tools]: https://github.com/jvitkauskas/pi32v2-llvm/releases/tag/pi32v2-23.1.5-5
[isa]: https://github.com/jvitkauskas/pi32v2-isa/tree/701a76faf103d1889d9bdf67987b872899c27257
[flasher]: https://github.com/jvitkauskas/m-flasher/blob/main/docs/usage.md
[felucca]: https://github.com/hugelton/felucca-bsp/tree/2a5af3dfc8bccb92e37d48a3e7e0bf9a5827e51f/boards/fm1
[play-green]: https://github.com/Wulgaren/Araminta/blob/62911ec19b1010b16d2ab508e91eed08775386e6/firmware/src/ui_input.c#L29-L30
[adc-driver]: https://github.com/hugelton/felucca-bsp/blob/f869dfa8dc3c081f040407e370d303349e4444d5/boards/fm1/fm1_adc.h
[audio-driver]: https://github.com/hugelton/felucca-bsp/blob/f869dfa8dc3c081f040407e370d303349e4444d5/boards/fm1/fm1_audio.h
[usb-controller]: https://github.com/hugelton/felucca-bsp/blob/f869dfa8dc3c081f040407e370d303349e4444d5/chip/fm1_usb.h
[usb-driver]: https://github.com/hugelton/felucca-bsp/blob/f869dfa8dc3c081f040407e370d303349e4444d5/chip/usb.c
[panel-driver]: https://github.com/hugelton/felucca-bsp/blob/f869dfa8dc3c081f040407e370d303349e4444d5/boards/fm1/fm1_input.h
[panel-labels]: https://github.com/charlesvestal/fm1-base/blob/7932dff6f1474cc8c3420e0df75eb75879e47319/device/panel.c
[touch-driver]: https://github.com/charlesvestal/fm1-base/blob/7932dff6f1474cc8c3420e0df75eb75879e47319/lib/touch.c
