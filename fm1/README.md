# `fm1`: Board Support Package for M-VAVE FM-1

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

## Board peripherals

| Handle | Interface |
| --- | --- |
| `board.panel` | Borrow the PLAY LED as an `OutputPin` or `StatefulOutputPin`, or the active-low PLAY button as an `InputPin`. Sampling the button briefly blanks the LED and restores its state. |
| `board.panel.into_matrix()` | Select one of 11 columns, sample six switch rows or drive the four LED rows. The application controls scan timing and debounce. |
| `board.timers` | TIMER4 provides a 24 MHz cycle counter and `DelayNs`. TIMER5 counts 10 ms ticks. Use `wrapping_sub` for elapsed time. |
| `board.display.init()` | Initialize the 240 × 240 ST7789 LCD. `write_rect` sends big-endian RGB565 pixels from RAM or flash using SPI1 DMA. |
| `board.headphones` | Read the raw PB5 jack switch with `is_inserted()` or `InputPin`. |
| `board.audio.start()` | With the `audio` feature, stream signed Q23 stereo samples to the DAC through ALNK0. Dropping the stream stops DMA. |

Audio consumes 256 stereo frames about every 5.8 ms at the stock clock rate
of 24 MHz / 544. Submit a prepared block with `try_write`. A long blocking
LCD transfer can miss the refill deadline and cause old samples to repeat.
`Refill::Late` reports a boundary crossed during the copy.

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
binary. Use `display` to build the RGB test card or `tone` for the audio test.
The script enables the `audio` feature for `tone`.

1. On an FM-1 with M-Bootloader installed, hold **REC** while powering on to
   enter USB download mode. Connect the device with a USB data cable.
2. In [M-Flasher][flasher], back up the unit. Choose **Firmware**, select
   `target/firmware/blink.fwsc` and restart the unit after flashing. The
   package updates the application area and preserves the bootloader.
3. The PLAY LED should blink once per second. Hold **PLAY** for two seconds
   to return to download mode. In the display and tone examples, press **PLAY**
   to return.

## Use from another crate

Add `fm1 = "0.1.0"` under `[dependencies]` in your application's
`Cargo.toml`, or create a new binary crate with the [published BSP][crate].

```sh
cargo new --bin my-app
cd my-app
cargo add fm1@0.1.0
```

Keep the target specification in the application root so Cargo can load it
before building dependencies. The [blink example][blink-example] shows how
to use `fm1::entry!` and supply a panic handler in `src/main.rs`.

```sh
release=https://raw.githubusercontent.com/fm1-rust/fm1-rust/refs/tags/fm1/v0.1.0/fm1
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

Licensed under the * [MIT license](http://opensource.org/licenses/MIT).

Julius Vitkauskas wrote the original [`fm1-blink-rust` example][blink-origin],
a Rust port of his [C blink example][c-blink]. The BSP adapts his PLAY wiring,
timer interrupt, return-to-bootloader sequence and ST7789 bring-up. The
startup assembly, linker layout and pi32v2 target also come from that work,
licensed under MIT-0.

The panel matrix and headphone switch wiring follow [felucca-bsp][felucca].
The optional audio module ports its MPL-2.0 driver by Leo Kuroshita
(Hügelton Instruments) and Kerem Kilic (Ellic Studio). Repository integration
is MIT-licensed.

[blink-origin]: https://github.com/jvitkauskas/pi32v2-isa/tree/701a76faf103d1889d9bdf67987b872899c27257/examples/fm1-blink-rust
[c-blink]: https://github.com/jvitkauskas/pi32v2-isa/tree/701a76faf103d1889d9bdf67987b872899c27257/examples/fm1-blink
[blink-example]: https://github.com/fm1-rust/fm1-rust/blob/fm1/v0.1.0/fm1/examples/blink.rs
[crate]: https://crates.io/crates/fm1/0.1.0
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1.yml
[compiler-build]: https://github.com/jvitkauskas/pi32v2-isa/blob/701a76faf103d1889d9bdf67987b872899c27257/examples/fm1-blink-rust/README.md#the-patched-rustc
[rust-fork]: https://github.com/jvitkauskas/pi32v2-rust/tree/3ee95ae1dfc454a3eca42af41db34e356d8ff658
[llvm-tools]: https://github.com/jvitkauskas/pi32v2-llvm/releases/tag/pi32v2-23.1.5-5
[isa]: https://github.com/jvitkauskas/pi32v2-isa/tree/701a76faf103d1889d9bdf67987b872899c27257
[flasher]: https://github.com/jvitkauskas/m-flasher/blob/main/docs/usage.md
[felucca]: https://github.com/hugelton/felucca-bsp/tree/2a5af3dfc8bccb92e37d48a3e7e0bf9a5827e51f/boards/fm1
