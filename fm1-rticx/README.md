# fm1-rticx

Write an FM-1 application as interrupt handlers and async Rust tasks on CPU0.
The crate provides the board entry point, the [FM-1 BSP](../fm1/) as `bsp`,
and a TIMER5 clock for delays and timeouts. The
[pi32v2 distribution](../rticx-pi32v2/) supplies the RTICX executor and
interrupt bindings. Both run without an allocator.

## Examples

With the [firmware toolchain environment](../fm1/README.md#build-and-flash)
set, run the example builds from the workspace root.

```sh
./fm1/scripts/build-example.sh rticx-blink
./fm1/scripts/build-example.sh rticx-tone
./fm1/scripts/build-example.sh rticx-controls
./fm1/scripts/build-example.sh rticx-piano
./fm1/scripts/build-example.sh rticx-waveform
```

| Firmware | Tasks and controls |
| --- | --- |
| [rticx-blink](examples/rticx-blink.rs) | TIMER5 wakes two periodic async tasks. One toggles the PLAY LED every 500 ms. The other samples the button every 10 ms. Hold PLAY for two seconds to return to download mode. |
| [rticx-tone](examples/rticx-tone.rs) | Starts with a stereo tone, green screen and green PLAY indicator. Tap PLAY/STOP for silence, a red screen and an amber indicator. Tap again to resume. Hold for two seconds to return to download mode. A magenta strip along the bottom records a late DMA refill. |
| [rticx-controls](examples/rticx-controls.rs) | TIMER5 scans one column every 100 µs while a lower-priority task paints the LCD. Tap a button to toggle its lamp. PLAY cycles green, red, both and off. Each piano key lights its own indicator while held. Hold both octave buttons for two seconds to return to USB download mode. |
| [rticx-piano](examples/rticx-piano.rs) | Play F3 through G5 (MIDI notes 53–79), including chords. Each held key sounds and lights its own LED. MASTER sets the volume. Hold both octave buttons for two seconds to return to USB download mode. |
| [rticx-waveform](examples/rticx-waveform.rs) | Play a monophonic oscillator and see the samples sent to the DAC. PRESETS cycles sine, triangle, sawtooth and square. Tap a piano key to latch F3–G5. SELECT transposes up to an octave either way, MASTER sets velocity (0–127), and PLAY/STOP mutes or resumes. Hold both octave buttons for two seconds to return to USB download mode. |

The waveform starts on A4. The graph shows 224 consecutive left-channel samples
(about 5.1 ms) on a fixed vertical scale. A rising zero crossing triggers each
trace. Audio runs at priority 3, panel scanning at 2 and drawing at 1. The
renderer replaces changed tiles and prepares the next strip while SPI1 sends
the previous one. A red LATE indicator records an audio DMA refill that crossed
a buffer boundary.

The controls display shows MASTER, signed counts for all encoders, a press count
for each button, and the last piano key (1–27). Green text means held, cyan means
previously pressed, and an asterisk marks an enabled lamp.

On the piano display, held notes are green and notes played since boot stay
cyan after release. `SEEN` counts distinct keys. `LAST` keeps the most recent
note name and MIDI number on screen. A red `LATE` records a DMA boundary
crossed during an audio refill. The piano uses digital key events at fixed
velocity 127 and produces stereo audio locally.

### Record and play a MIDI clip

Connect the piano to a computer with a USB data cable. The USB indicator turns
green when the host configures **FM-1 MIDI Piano**. To try it in Ableton Live, record and play back a chord.

1. Under **Settings → Link, Tempo & MIDI**, enable **Track** for the FM-1 input
   and output ports.
2. On an empty MIDI track, choose **MIDI From → FM-1 MIDI Piano → Ch. 1**.
   Leave **MIDI To** set to **No Output**, arm the track, and record a chord.
   The FM-1 plays the keys locally while Live records their lengths.
3. Stop recording, disarm the track, choose **MIDI To → FM-1 MIDI Piano →
   Ch. 1**, and play the clip. The matching key LEDs light up and the FM-1's
   speaker or headphones play the chord. Edit a note's velocity to change its
   playback level.

Incoming notes span MIDI numbers 53–79. Incoming notes and physical keys
have independent lifetimes, including at the same pitch. The MIDI task runs
once per panel sweep below the audio interrupt. The example uses the shared
[pid.codes test identity](https://pid.codes/1209/0001/) for bench use. A distributed product needs its own USB identity.

The packages are written to `target/firmware/`. Download the `.fwsc` files from the `fm1-rticx-firmware`
artifact on a successful [fm1-rticx workflow run][ci].

## Application setup

For a firmware project next to an `fm1-rust` checkout, add one dependency.
Enable its `audio` feature to use the ALNK0 driver.

```toml
[dependencies]
fm1-rticx = { path = "../fm1-rust/fm1-rticx" }
```

Annotate the application module with `#[fm1_rticx::app]`. In `#[init]`, call
`fm1_rticx::bsp::Board::take()` once and move the handles into your tasks
through `TaskInits`. RTICX enables interrupts after it initializes the tasks
and vectors. Use `#[post_init]` to spawn the first software tasks. The
[RTICX blink example](examples/rticx-blink.rs) includes a panic handler and
can serve as your `src/main.rs`.

Copy `../fm1-rust/fm1/targets/pi32v2-none.json` to your application root.
Then use the `cargo build` and `fm1app.py` commands in the
[BSP's standalone build instructions](../fm1/README.md#use-from-another-crate).

Bind hardware tasks with `#[task(binds = ALNK0, priority = 3)]`. For
`#[async_task]`, list one software interrupt per distinct positive software
priority, as in `#[fm1_rticx::app(dispatchers = [SOFT0, SOFT1])]`. Higher
numbers can preempt lower numbers. Tasks at the same priority yield to one
another at await points or when they finish. A shared peripheral belongs in
`#[shared]` and is accessed through its field's `lock` closure.

For timed work, pass `board.timers` to `time::Clock::start` and give the
returned handle to a TIMER5 task. Its handler calls `on_interrupt()`.
Import `time::Timebase` for `Clock::now()`, `delay`, `delay_until` and
timeouts. The clock wraps after 2³² ticks of 10 ms. Keep deadlines less
than 2³¹ ticks ahead. The blink example advances an absolute deadline each
period so work done in the task does not accumulate as drift.

[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-rticx.yml
