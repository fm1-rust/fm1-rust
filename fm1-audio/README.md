# FM-1 Audio

[![crates.io][crate-badge]][crate]
[![docs.rs][docs-badge]][docs]
[![Build status][ci-badge]][ci]

Extended audio functionality for the M-VAVE FM-1.

## About

`fm1-audio` re-exports the BSP's `fm1::audio` API and adds a 128-frame stereo
output with a software mixer for the M-VAVE FM-1. It runs without an allocator.
`fm1_audio::Stream` and `fm1::audio::Stream` are the same type. The BSP's
`AudioPort::start` and `into_stream` use 256 frames by default.

The board sends stereo I²S to a DAC identified by Felucca's author as a CS4344.
Plugging in headphones mutes the built-in speaker. `board.headphones.is_inserted()`
reads the jack switch. The second 3.5 mm jack accepts MIDI. The published board
interfaces expose no microphone or analog line input. [Board inspection][teardown]
and [manufacturer specifications][specs].

Call `Output::render` from the ALNK0 interrupt. At the board's physical rate
of 24 MHz / 544, the application has 69,632 TIMER4 ticks (2.90 ms) per half.
The BSP reserves the last eighth of the service period for the DMA copy.
Render into the cleared block, adding sources in signed Q23 units. The
output applies a linear MASTER ramp and clips the mix once to the DAC range.
After `Refill::Written`, `Output::block` exposes that same mix for a meter,
oscilloscope or recording. A late render can cause the DAC to repeat old audio.

For a sequencer that prepares numbered blocks, use `Output::render_with_interval`.
After the first timed refill, `RefillInterval::Elapsed(n)` includes the current
half. Skip `n.get() - 1` expired blocks before selecting PCM for this one.
If the count is `Uncertain`, discard stale work and resynchronize the sequencer.
Publish any edit receipt or MIDI batch for the chosen block after `Refill::Written`.

```rust
use fm1_audio::{I24, MasterLevel, OUTPUT_FRAMES, Output, Refill, mix_stereo};

fn play(output: &mut Output, master: MasterLevel, left: &[I24; OUTPUT_FRAMES],
        right: &[I24; OUTPUT_FRAMES]) -> Refill {
    output.render(master, |mix| mix_stereo(mix, left, right))
}
```

### Mixing sources

Use the functions in `mix` on the block passed to `Output::render` or on an
application-owned bus. Each call mixes up to the shorter length and returns
the number of frames consumed. With separate planes the shortest of all three
slices sets the count. A voice can end partway through a block.

```rust
use fm1_audio::{MasterLevel, Output, Refill, mix::{self, Gain}};

fn play_voices(output: &mut Output, master: MasterLevel, kick: &[i16],
               stereo_bus: &[[i32; 2]]) -> Refill {
    output.render(master, |out| {
        mix::stereo_q23(out, stereo_bus);
        mix::mono_i16(out, kick, [Gain::UNITY, Gain::from_bits(8192)]);
    })
}
```

`mix::stereo_i16` accepts interleaved PCM16 with per-channel gains.
`mix::planar_q23` accepts native `i32` planes, including the same mono slice
for both channels. A frame is a native-endian array with left first, contiguous
channels and the alignment of one channel. PCM16 scales to Q23 before adding.
Both channels retain `i32` headroom until MASTER. Existing `I24` callers can
continue to use `mix_stereo`.

The default `dsp` feature assembles AC791N kernels on the FM-1 target with
the BSP's `PI32V2_BIN` tools.
They use independent 32-bit DSP lanes and repeat at most 16 frames at a time.
The [FM-1 repeat and alignment measurements][dsp-probe] document the chip
limits. A word-aligned stereo PCM16 source uses one packed load per frame.
A source aligned only to a halfword uses two halfword loads.
Other targets use the same arithmetic in Rust. For a pi32v2 emulator with
partial DSP instruction support, set `default-features = false` on the
`fm1-audio` dependency throughout the build to use that implementation.
Build `fm1/scripts/build-example.sh mix-bench` to reproduce the readings below.
On one FM-1, all six output blocks matched the scalar Rust reference. Each
column reports the best of 16 runs on 128 frames, in 24 MHz TIMER4 ticks
(24 ticks = 1 µs). The timed region covers adding one source to the mix.

| Source | Scalar Rust (ticks) | Mixer (ticks) | Speedup |
| --- | ---: | ---: | ---: |
| Mono PCM16 with left/right gains | 978 | 152 | 6.43× |
| Stereo PCM16, word-aligned | 1017 | 153 | 6.65× |
| Stereo PCM16, two bytes past a word boundary | 1022 | 178 | 5.74× |
| Interleaved stereo Q23 | 393 | 130 | 3.02× |
| Planar stereo Q23 | 410 | 126 | 3.25× |
| Planar `I24` via `mix_stereo` | 412 | 334 | 1.23× |

To measure an interrupt, read `fm1::time::cycles()` at entry and call
`Stats::record` after servicing any output taps. This tracks the longest
service time, the longest interval between serviced entries and observed
overruns. To also track the time to a successful DMA commit, sample TIMER4
again when `Output::render` returns and pass both timestamps to
`Stats::record_with_refill` at the end of service. The RTICX waveform and
piano examples display these measurements while HOME is held.

## License

Licensed under the [MIT License][mit].

This crate enables the BSP’s MPL-2.0 audio driver. See the
[BSP license section][bsp-license] for its upstream source.

### Contribution

Unless you explicitly state otherwise, you license contributions you
intentionally submit to this crate under the [MIT License][mit].

[dsp-probe]: https://github.com/jvitkauskas/pi32v2-isa/tree/058b89bb835d12b194a9f13f51ec153c1d6cc24b/hw/fm1-2026-11-15-excprobe
[crate-badge]: https://img.shields.io/crates/v/fm1-audio?logo=rust
[crate]: https://crates.io/crates/fm1-audio
[docs-badge]: https://docs.rs/fm1-audio/badge.svg
[docs]: https://docs.rs/fm1-audio/
[ci-badge]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-audio.yml/badge.svg?branch=main&event=push
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-audio.yml
[teardown]: https://note.com/kurogedelix/n/n225a8e2374cd?hl=en
[specs]: https://www.cuvave.com/product?id=fm-1
[mit]: https://github.com/fm1-rust/fm1-rust/blob/main/fm1-audio/LICENSE
[bsp-license]: https://github.com/fm1-rust/fm1-rust/blob/main/fm1/README.md#license
