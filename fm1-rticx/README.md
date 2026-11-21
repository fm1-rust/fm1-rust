# FM-1 RTICX Support

[![crates.io][crate-badge]][crate]
[![docs.rs][docs-badge]][docs]
[![Build status][ci-badge]][ci]

Concurrency, interrupt handling, and async support for the M-VAVE FM-1 using
the [RTICX] realtime framework.

## About

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
| [rticx-controls](examples/rticx-controls.rs) | TIMER5 scans one column every 100 µs. An async task consumes completed sweeps and updates the controls while another task paints the LCD. Tap a button to toggle its lamp. PLAY cycles green, red, both and off. Each piano key lights its own indicator while held. Hold both octave buttons for two seconds to return to USB download mode. |
| [rticx-piano](examples/rticx-piano.rs) | Play F3 through G5 (MIDI notes 53–79), including chords. Each held key sounds and lights its own LED. MASTER sets the volume. Hold both octave buttons for two seconds to return to USB download mode. |
| [rticx-waveform](examples/rticx-waveform.rs) | Play a local four-waveform oscillator. FX switches between dry and half gain. The SEL button switches the scope between oscillator and DAC output. PRESETS cycles sine, triangle, sawtooth and square. Tap a piano key to latch F3–G5. SELECT transposes up to an octave either way, MASTER sets velocity (0–127), and PLAY/STOP mutes or resumes. Hold both octave buttons for two seconds to return to USB download mode. |

The tone, piano and waveform use [`fm1-audio`](../fm1-audio/) with 128 stereo
frames per DMA half (2.90 ms). Their LATE indicators latch when an audio
interrupt overruns its budget or a refill deadline is missed. The piano also
shows peak audio service time in microseconds (`AUD`) and observed audio
overruns (`XR`). Its MIDI clock timing fields are independent.

Hold **HOME** in the waveform or piano demo to see peak interrupt timings while
playing. Release it to restore the usual display. The peaks accumulate from
startup using the 24 MHz TIMER4 and are displayed in microseconds, rounded up.

| Field | Measurement |
| --- | --- |
| `WORK` | Longest elapsed time from Rust handler entry to its final timing sample, including preemption. The audio sample follows the output taps. The panel sample includes the complete column service, including end-of-sweep processing. A value turns red if it reaches the period. |
| `GAP` | Longest interval between consecutive serviced entries. Audio normally has 69,632 ticks (2.901 ms) between refills. The panel has 2,400 ticks (100 µs) between columns. Entry jitter can lengthen a gap even when both jobs finish on time. |
| `COMMIT` | Longest time from audio handler entry until a successful DMA submission returned. Includes the input snapshot, preparing or copying the mix, MASTER gain and the DMA copy. Starts at zero until the first successful submission. |
| `XR` | Serviced audio interrupts that reported a late refill, used a full period, or arrived at least two periods after the previous service. Several coalesced events can increment this counter once. |
| `HIT` | Waveform only. Peak entry-to-commit time for successful refills that used a prepared oscillator block. Starts at zero until the first hit. |
| `MISS` | Waveform only. Number of refills that rendered the oscillator inline. Startup, control changes and a busy producer can each cause a miss. |

The examples exercise different priority orders on CPU0. In the waveform demo,
audio (4) can interrupt the panel (3). In the piano demo, the panel (4) can
interrupt audio (3), and up to 30 panel ticks can arrive in a 128-frame block. The
piano's independent TIMER2 alarm also runs at 3 and can delay audio entry.
These readings describe observed runs. A worst-case response-time bound also
needs bounds on shared-resource and interrupt masking time, interrupt overhead
and task execution on the chip. TIMER4 sampling at entry cannot measure how
long an interrupt was pending before the handler ran.

The waveform starts on A4 with the dry route and DAC tap selected. While a note
sounds, press **FX** to insert a half-gain stage. Press the **SEL button** to
see the oscillator at full amplitude while the speaker still plays at half
level. Press FX again to bypass the stage. The SELECT encoder still transposes.
The audio interrupt reads the current gain and scope tap together at each refill.

The scope shows 224 consecutive samples
(about 5.1 ms) on a fixed vertical scale. A rising zero crossing triggers each
trace. A priority-2 async task renders one 128-frame mono block ahead. The
priority-4 audio interrupt uses it when the block number, starting phase and
voice controls still match. Otherwise it renders inline. It applies the current
FX gain as it copies the oscillator into both stereo channels. The scope can
therefore sample either the raw oscillator or the post-MASTER DAC output on
that same refill.

After a successful refill the interrupt offers the selected mono signal to a two-block queue.
A priority-2 async task finds the trigger and assembles the trace. If the queue
fills, a refill is late, or an edit takes effect, capture re-arms. The trace
and its gain and tap labels come from the same submitted audio blocks.
On HOME, `EDIT` and `HIT` share a row and are in microseconds. `EDIT` is the
peak time from an FX or SEL change to its first successful DMA submission.
`AGE` is the peak time from submission of the trigger block to completion of
its trace paint. `GAP` on that row is the longest interval between completed
paints of distinct traces. Both display measurements are in milliseconds.
The HOME view's `DROP` counts submitted audio blocks the observer missed
because its queue was full. Panel scanning runs at priority 3 and drawing at 1,
so capture can continue during LCD startup and redraws. The renderer
replaces changed tiles and prepares the next strip while SPI1 sends
the previous one. When the strip is still in flight, the renderer awaits the
SPI1 completion interrupt at priority 1. TIMER5 supplies its stall timeout.

The controls display shows MASTER, signed counts for all encoders, a press count
for each button, and the last piano key (1–27). Green text means held, cyan means
previously pressed, and an asterisk marks an enabled lamp. Hold HOME for the
peak TIMER5 handler time (`IRQ US`), peak time from a completed sweep to the
input task receiving it (`DISPATCH US`), and the number of dropped sweeps
(`SCAN DROPS`). The [`fm1-controls`](../fm1-controls/) FIFO holds eight sweeps.
Only the display coalesces updates during normal operation. A full input FIFO
drops the newest sweep. The next accepted one restores held levels, key LEDs
and MASTER. The diagnostic counts only button presses and encoder detents it
actually received.

On the piano display, held notes are green and notes played since boot stay
cyan after release. `SEEN` counts distinct keys. `LAST` keeps the most recent
note name and MIDI number on screen. `VEL` shows the last strike velocity. `PRESS`
shows how hard that note is being held, or `--` after release and for a physical
key without pressure sensing. Pressing harder brings that voice up in the local
stereo mix.

### Record and play a MIDI clip

Connect the piano to a computer with a USB data cable. The USB indicator changes from `USB-` to
green `USB+` when the host configures **FM-1 MIDI Piano**. To try it in Ableton Live, record and play back a chord.

1. Under **Settings → Link, Tempo & MIDI**, enable **Track** for the FM-1 input
   and output ports.
2. On an empty MIDI track, choose **MIDI From → FM-1 MIDI Piano → Ch. 1**.
   Leave **MIDI To** set to **No Output**, arm the track, and record a chord.
   The FM-1 plays the keys locally and sends their strike velocities. Hold two
   keys between F3 and C5 and press harder on one. The piano sends MIDI
   Polyphonic Key Pressure (0xA0) for that note. The top seven keys send strike
   velocity only.
3. Stop recording, disarm the track, choose **MIDI To → FM-1 MIDI Piano →
   Ch. 1**, and play the clip. The matching key LEDs light up and the FM-1's
   speaker or headphones play the chord. The synth responds to incoming note
   velocity and polyphonic pressure.

Incoming notes span MIDI numbers 53–79. Incoming notes and physical keys
have independent lifetimes, including at the same pitch. USB0 wakes the MIDI
task for bus and endpoint activity. Physical notes, pressure and PLAY changes
wake it from the panel. TIMER2 schedules MIDI Clock pulses. Panel scanning and
audio have higher task priorities. The example uses the shared
[pid.codes test identity](https://pid.codes/1209/0001/) for bench use. A distributed product needs its own USB identity.

To check unplug detection, sustain a note from the host and pull the data cable
while the FM-1 stays powered. The remote note should release and the indicator
should show `USB-`. Physical keys should keep playing. Reconnect and play
another note after `USB+` returns. The indicator tracks a configured, active
host session. Host suspend also shows `USB-`. If removal raises no interrupt,
a 100 ms frame-counter check runs while a remote note is held. An idle instrument
can keep showing its previous status until the next controller event.

### Follow the piano’s MIDI Clock in Ableton Live

Enable **Sync** for the **FM-1 MIDI Piano** input in Live’s MIDI settings and
turn on **EXT** in the control bar. Tap PLAY/STOP on the FM-1. The green PLAY
lamp indicates the transport is running. The piano sends MIDI Start, 24 timing
pulses per quarter note at 120 BPM, and MIDI Stop on the next press. Keys
still play and send notes during synchronization.

`MAX US` shows the worst lateness of the firmware’s clock task since Start.
`MISSED` counts pulses skipped when the task or USB endpoint falls a full pulse
behind. The audio `LATE` indicator continues to report DMA refill overruns.
USB frame scheduling and host-side latency are outside the `MAX US` measurement.

The packages are written to `target/firmware/`. Download the `.fwsc` files from the `fm1-rticx-firmware`
artifact on a successful [fm1-rticx workflow run][ci].

## Application setup

For a firmware project, add the published crate to `Cargo.toml`.
Enable its `audio` feature to use the ALNK0 driver.

```toml
[dependencies]
fm1-rticx = "0.1.0"
```

Annotate the application module with `#[fm1_rticx::app]`. In `#[init]`, call
`fm1_rticx::bsp::Board::take()` once and move the handles into your tasks
through `TaskInits`. RTICX enables interrupts after it initializes the tasks
and vectors. Use `#[post_init]` to spawn the first software tasks. The
[RTICX blink example](examples/rticx-blink.rs) includes a panic handler and
can serve as your `src/main.rs`.

Copy the [pi32v2 target specification][target-spec] to your application root.
Then use the `cargo build` and `fm1app.py` commands in the
[BSP's standalone build instructions][standalone].

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

For finer deadlines, take TIMER2 before moving the board timers into the
panel or 100 Hz clock task. The deadline clock owns its interrupt handler.
The const argument sets its RTICX priority.

```rust,ignore
use fm1_rticx::time::{DeadlineClock, ExtU32, Timebase};

DeadlineClock::start::<3>(board.timers.take_alarm().expect("one TIMER2 alarm"));
// In an async task:
let deadline = DeadlineClock::now() + 250.micros();
DeadlineClock::delay_until(deadline).await;
```

`DeadlineClock` reads the 24 MHz TIMER4 counter and arms TIMER2 for the next
deadline. TIMER2 stops while the queue is empty. TIMER5 and panel scanning are
optional. Schedule less than 2³¹ crystal cycles (about 89 s) ahead and advance
periodic work from its previous deadline to carry phase across counter wrap.

## License

`fm1-rticx` is licensed under the [MIT License][mit]. Its `audio` feature
enables the BSP’s MPL-2.0 audio driver. The [BSP license section][bsp-license]
lists the drivers and their upstream sources.

[RTICX]: https://github.com/rticx-rs/rticx
[standalone]: https://github.com/fm1-rust/fm1-rust/blob/fm1-rticx/v0.1.0/fm1/README.md#use-from-another-crate
[target-spec]: https://raw.githubusercontent.com/fm1-rust/fm1-rust/refs/tags/fm1-rticx/v0.1.0/fm1/targets/pi32v2-none.json
[crate-badge]: https://img.shields.io/crates/v/fm1-rticx?logo=rust
[crate]: https://crates.io/crates/fm1-rticx
[docs-badge]: https://docs.rs/fm1-rticx/badge.svg
[docs]: https://docs.rs/fm1-rticx/
[ci-badge]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-rticx.yml/badge.svg?branch=main&event=push
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-rticx.yml
[mit]: https://github.com/fm1-rust/fm1-rust/blob/main/fm1-rticx/LICENSE
[bsp-license]: https://github.com/fm1-rust/fm1-rust/blob/main/fm1/README.md#license
