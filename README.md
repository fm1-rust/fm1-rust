# FM-1 Rust

The [`fm1` crate](fm1/) is a bare-metal Rust board support package for the
M-VAVE FM-1 synthesizer. It runs on CPU0 of the JieLi AC791N, uses
`embedded-hal` 1.0 and needs no allocator.

## Crates

| Crate | Description | crates.io | Documentation | Build status |
| --- | --- | --- | --- | --- |
| [`fm1`](fm1/) | Board support and peripheral drivers | [![crates.io][fm1-version]][fm1-crate] | [![docs.rs][fm1-docs-badge]][fm1-docs] | [![Build status][fm1-build]][fm1-ci] |
| [`fm1-graphics`](fm1-graphics/) | Text grids and waveform plots over the BSP display | Unpublished | [Drawing guide](fm1-graphics/) | [![Build status][fm1-graphics-build]][fm1-graphics-ci] |
| [`fm1-rticx`](fm1-rticx/) | RTICX tasks and clocks for the FM-1 | Unpublished | [Application guide](fm1-rticx/README.md#application-setup) | [![Build status][fm1-rticx-build]][fm1-rticx-ci] |
| [`rticx-pi32v2`](rticx-pi32v2/) | RTICX runtime for AC791N CPU0 | [![crates.io][pi32v2-version]][pi32v2-crate] | [![docs.rs][pi32v2-docs-badge]][pi32v2-docs] | [![Build status][pi32v2-build]][pi32v2-ci] |
| [`rticx-pi32v2-macro`](rticx-pi32v2-macro/) | Application macros for the pi32v2 runtime and FM-1 board crate | [![crates.io][macro-version]][macro-crate] | [![docs.rs][macro-docs-badge]][macro-docs] | [![Build status][pi32v2-build]][pi32v2-ci] |

## Examples

The BSP builds on Julius Vitkauskas’s [`fm1-blink-rust` proof of concept][blink-origin].
His example demonstrated the timer-driven PLAY LED, the ST7789 display and a
return to USB download mode on a physical FM-1.

| Example | What to look or listen for |
| --- | --- |
| [`blink`](fm1/examples/blink.rs) | The PLAY LED turns on for half a second and off for half a second. |
| [`display`](fm1/examples/display.rs) | Three vertical bars fill the screen in red, green and blue. |
| [`graphics`](fm1/examples/graphics.rs) | RGB swatches with the label ST7789. Hold PLAY at startup for the framebuffer version of the card. |
| [`tone`](fm1/examples/tone.rs) | A quiet, continuous square wave at about 441 Hz. |

The blink, display and tone examples have been observed on an FM-1 running M-Bootloader
`2.4-395E6CC`. Start with the [firmware build and flashing instructions](fm1/README.md#build-and-flash).

For interrupt-driven applications, [`fm1-rticx`](fm1-rticx/) adds priority-based
hardware and async tasks, shared resources and a 100 Hz clock. Its examples
schedule the PLAY LED and button independently, and refill audio DMA while
repainting the LCD.

For development on a host, run `cargo test --workspace --all-features` with a
standard Rust toolchain. Building firmware uses the community pi32v2 toolchain.

## License

Original code in the workspace crates is licensed under the [MIT License](fm1/LICENSE).
The `fm1` BSP incorporates MPL-2.0 code from Felucca’s C headers and USB driver
and from the `fm1-base` piano touch library. See the [BSP license section](fm1/README.md#license)
for the upstream sources and credits.

[fm1-version]: https://img.shields.io/crates/v/fm1?logo=rust
[fm1-crate]: https://crates.io/crates/fm1
[fm1-docs-badge]: https://docs.rs/fm1/badge.svg
[fm1-docs]: https://docs.rs/fm1/
[fm1-build]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1.yml/badge.svg?branch=main&event=push
[fm1-ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1.yml
[fm1-graphics-build]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-graphics.yml/badge.svg?branch=main&event=push
[fm1-graphics-ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-graphics.yml
[fm1-rticx-build]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-rticx.yml/badge.svg?branch=main&event=push
[fm1-rticx-ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-rticx.yml
[pi32v2-version]: https://img.shields.io/crates/v/rticx-pi32v2?logo=rust
[pi32v2-crate]: https://crates.io/crates/rticx-pi32v2
[pi32v2-docs-badge]: https://docs.rs/rticx-pi32v2/badge.svg
[pi32v2-docs]: https://docs.rs/rticx-pi32v2/
[pi32v2-build]: https://github.com/fm1-rust/fm1-rust/actions/workflows/rticx-pi32v2.yml/badge.svg?branch=main&event=push
[pi32v2-ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/rticx-pi32v2.yml
[macro-version]: https://img.shields.io/crates/v/rticx-pi32v2-macro?logo=rust
[macro-crate]: https://crates.io/crates/rticx-pi32v2-macro
[macro-docs-badge]: https://docs.rs/rticx-pi32v2-macro/badge.svg
[macro-docs]: https://docs.rs/rticx-pi32v2-macro/
[blink-origin]: https://github.com/jvitkauskas/pi32v2-isa/tree/701a76faf103d1889d9bdf67987b872899c27257/examples/fm1-blink-rust
