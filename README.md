# FM-1 Rust

The [`fm1` crate](fm1/) is a bare-metal Rust board support package for the
M-VAVE FM-1 synthesizer. It runs on CPU0 of the JieLi AC791N, uses
`embedded-hal` 1.0 and needs no allocator.

The BSP builds on Julius Vitkauskas’s [`fm1-blink-rust` proof of concept][blink-origin].
His example demonstrated the timer-driven PLAY LED, the ST7789 display and a
return to USB download mode on a physical FM-1.

| Example | What to look or listen for |
| --- | --- |
| [`blink`](fm1/examples/blink.rs) | The PLAY LED turns on for half a second and off for half a second. |
| [`display`](fm1/examples/display.rs) | Three vertical bars fill the screen in red, green and blue. |
| [`tone`](fm1/examples/tone.rs) | A quiet, continuous square wave at about 441 Hz. |

All three behaviors have been observed on an FM-1 running M-Bootloader
`2.4-395E6CC`. Start with the [firmware build and flashing instructions](fm1/README.md#build-and-flash).

For development on a host, run `cargo test --workspace --all-features` with a
standard Rust toolchain. Building firmware uses the community pi32v2 toolchain.

[blink-origin]: https://github.com/jvitkauskas/pi32v2-isa/tree/701a76faf103d1889d9bdf67987b872899c27257/examples/fm1-blink-rust
