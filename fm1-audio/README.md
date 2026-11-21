# FM-1 audio

[![Build status][ci-badge]][ci]

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

```rust
use fm1_audio::{I24, MasterLevel, OUTPUT_FRAMES, Output, Refill, mix_stereo};

fn play(output: &mut Output, master: MasterLevel, left: &[I24; OUTPUT_FRAMES],
        right: &[I24; OUTPUT_FRAMES]) -> Refill {
    output.render(master, |mix| mix_stereo(mix, left, right))
}
```

To measure an interrupt, read `fm1::time::cycles()` at entry and call
`Stats::record` after rendering, submitting and servicing any output taps.
The RTICX tone, piano and waveform examples use this path. The piano displays
its observed overrun count and longest audio service time.

[ci-badge]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-audio.yml/badge.svg?branch=main&event=push
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-audio.yml
[teardown]: https://note.com/kurogedelix/n/n225a8e2374cd?hl=en
[specs]: https://www.cuvave.com/product?id=fm-1
