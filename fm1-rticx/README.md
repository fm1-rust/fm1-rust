# FM-1 RTICX Support

[![crates.io][crate-badge]][crate]
[![docs.rs][docs-badge]][docs]
[![Build status][ci-badge]][ci]

Concurrency, interrupt handling, and async support for the M-VAVE FM-1 using
the [RTICX] real-time framework.

## About

Write an FM-1 application as interrupt handlers and async Rust tasks on CPU0.
The crate provides the board entry point, the [FM-1 BSP][bsp] as `bsp`,
and a TIMER5 clock for delays and timeouts. The
[pi32v2 distribution][runtime] supplies the RTICX executor and
interrupt bindings. Both run without an allocator.

## Examples

With the [firmware toolchain environment][build-and-flash]
set, run the example builds from the workspace root.

```sh
./fm1/scripts/build-example.sh rticx-blink
./fm1/scripts/build-example.sh rticx-tone
./fm1/scripts/build-example.sh rticx-cpu1-audio
./fm1/scripts/build-example.sh rticx-controls
./fm1/scripts/build-example.sh rticx-piano
./fm1/scripts/build-example.sh rticx-waveform
./fm1/scripts/build-example.sh rticx-timing
./fm1/scripts/build-example.sh usb-log
```

The script writes the `.fwsc` packages to `target/firmware/`. You can also
download them from the `fm1-rticx-firmware` artifact on a successful
[workflow run][ci].

### Example tour

These screenshots show the examples running in [fm1-emu](https://github.com/joakimeriksson/fm1-emu).

| Screenshot | Example and controls |
| --- | --- |
| ![Front panel with the PLAY lamp lit](https://raw.githubusercontent.com/fm1-rust/fm1-rust/refs/tags/fm1-rticx/v0.1.1/docs/images/rticx-blink.png) | [`rticx-blink`][rticx-blink-example]<br>Watch the **PLAY** lamp blink. TIMER5 wakes one async task to toggle the lamp every 500 ms and another to sample the button every 10 ms. The LCD stays dark. |
| ![Tone playing with a green LCD above, muted with a red LCD below](https://raw.githubusercontent.com/fm1-rust/fm1-rust/refs/tags/fm1-rticx/v0.1.1/docs/images/rticx-tone.png) | [`rticx-tone`][rticx-tone-example]<br>Tap **PLAY/STOP** to mute the stereo square wave. The LCD changes from green to red and the PLAY indicator changes from green to amber. Tap again to resume. A magenta strip along the bottom marks a late DMA refill. |
| ![Controls display with SELECT at +3, PRESETS at +2 and the SAVE lamp enabled](https://raw.githubusercontent.com/fm1-rust/fm1-rust/refs/tags/fm1-rticx/v0.1.1/docs/images/rticx-controls.png) | [`rticx-controls`][rticx-controls-example]<br>Turn **SELECT** to change its count, tap **SAVE** to toggle its lamp, or hold a piano key to light its LED. **PLAY** cycles through green, red, both and off. TIMER5 scans one column every 100 µs while separate tasks read completed sweeps and paint the LCD. |
| ![Piano display with C4, E4 and G4 held and USB connected](https://raw.githubusercontent.com/fm1-rust/fm1-rust/refs/tags/fm1-rticx/v0.1.1/docs/images/rticx-piano.png) | [`rticx-piano`][rticx-piano-example]<br>Play a C-major chord. The held notes turn green on the LCD and their key LEDs light up. The keyboard spans F3–G5 (MIDI 53–79). Turn **MASTER** to set the volume, or [record the chord over USB-MIDI](#record-and-play-a-midi-clip). |
| ![Oscilloscope showing an A4 sine wave at the DAC output](https://raw.githubusercontent.com/fm1-rust/fm1-rust/refs/tags/fm1-rticx/v0.1.1/docs/images/rticx-waveform.png) | [`rticx-waveform`][rticx-waveform-example]<br>Turn **PRESETS** to cycle through sine, triangle, sawtooth and square waves. With **SEL DAC** displayed, press **FX** to halve the output and watch the trace shrink. **SEL** switches between the oscillator and DAC. Tap a key to latch F3–G5, transpose up to an octave with **SELECT**, set velocity (0–127) with **MASTER**, or mute and resume with **PLAY/STOP**. |
| ![Host terminal showing ready followed by two play+ and play- pairs](https://raw.githubusercontent.com/fm1-rust/fm1-rust/refs/tags/fm1-rticx/v0.1.1/docs/images/usb-log.png) | [`usb-log`][usb-log-example]<br>Tap and release **PLAY**, then [read the log over USB](#read-a-log-over-usb). The firmware records `ready` at startup, `play+` on press and `play-` on release. The LCD stays dark. |

The [`rticx-timing`][rticx-timing-example] diagnostic is a hardware-only timing
benchmark. It displays summary counters and [exports detailed results over USB](#hardware-timing-diagnostic).

Use the USB examples on a controlled USB bus. They activate USB0 under the
BSP's unsafe receive-DMA contract. [Physical measurements and deployment requirements](../docs/usb-dma/README.md#fix-and-remaining-hardware-contract)

To return to download mode, hold **PLAY** for two seconds in Blink, Tone or USB
Log. In Controls, Piano, Waveform or Timing, hold **OCT-** and **OCT+** together for
two seconds.

The tone, piano and waveform use [`fm1-audio`][audio] with 128 stereo
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
(`SCAN DROPS`). The example’s [`fm1-controls`][controls] FIFO holds eight sweeps.
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

### Read a log over USB

Run `./fm1/scripts/build-example.sh usb-log` and install
`target/firmware/usb-log.fwsc` with M-Flasher. Connect the USB data cable and
tap PLAY. Run the host reader from the workspace root.

```sh
cargo run -q -p fm1-cli -- log
```

Startup logs `ready`, a press logs `play+`, and release logs `play-`. The
screen stays dark. Hold PLAY for two seconds to return to download mode.
The reader checks the development USB ID and finds the MIDI pair that answers
the log request, even when macOS shows an earlier port name.

The `usb-log` feature forwards to the [BSP USB log][bsp-usb-log].
The independent `rtt` feature provides `fm1_rticx::rtt::init()`, which
allocates a 256-byte nonblocking RTT up-channel. This example uses both and
the host reader communicates entirely over USB-MIDI.

### Hardware timing diagnostic

In M-Flasher, choose **Install firmware (.fwsc)** and select `rticx-timing.fwsc`.
To build the image yourself, set up the [firmware toolchain][build-and-flash]
and run this command from the workspace root. The output is in `target/firmware/`.

```sh
./fm1/scripts/build-example.sh rticx-timing
```

On real hardware, this runs a one-second warmup followed by three five-second
measurements: **IDLE** (audio, 10 kHz panel scanning and periodic USB service,
LCD left alone), **DRAW** (the same interrupts while the low-priority task
repaints all 300 text cells), and **BUSY** (7 ms of preemptible spin work per
10 ms slot at the lowest priority). The output is silent. The screen labels
the current stage, shows the last completed stage's peak audio and panel
service times, gaps, input dispatch latency and loss counts, and finally shows
**DONE - USB REPORT**.

Connect the FM-1 with a USB data cable. When the LCD shows **DONE - USB REPORT**,
read the report from the workspace root.

```sh
cargo run -q -p fm1-cli -- timing
```

The first line should read `24MHZ AUDIO 128F PANEL 10KHZ`. With one FM-1 on
USB, the reader can find it even if macOS retained the name of another
example. To restrict the MIDI ports it tries, pass a unique name substring.

```sh
cargo run -q -p fm1-cli -- timing --port 'FM-1 MIDI Piano'
```

See [fm1-cli][host-cli] for standalone installation and Linux build prerequisites.

The reader uses MIDI CC 112 to select each of 25 report pages, then requests
that page using the existing USB log SysEx protocol. `UsbLog` retains at most
43 ASCII characters, so unpaged logging would silently evict measurements.
Times are reported in microseconds. `XR` counts a late refill, service lasting
at least one audio block, or an entry-to-entry gap of at least two blocks.
`JIT` measures the largest absolute difference between observed and nominal
**entry-to-entry** intervals. The timestamps come from software inside each
handler. `G2` counts intervals at least twice the nominal period. `DROP`
counts full input FIFO rejections. `W` reports TIMER2 async wake
lateness, and `M` counts wakes at least one full 5 ms period late. `LCD`
durations include DMA waits and interrupt preemption. The audio task refills a
silent buffer. Full synth DSP has a different workload. Peaks describe this
run, and a guaranteed worst-case bound needs a separate analysis.

To measure how long a scan queue operation masks interrupts, timestamp the
outermost disable and restore around enqueue and dequeue. To measure interrupt
response latency, capture the hardware event and entry into its handler on the
same time base.

`USB MAX` measures service time in microseconds. Normal MIDI advertises
64-byte EP1 OUT packets and reserves 2 KiB of staging each for EP0 and
EP1 OUT. On the black FM-1, a legal 64-byte packet wrote its CRC at
offsets 64–65, and a 69-byte diagnostic packet crossed the historical
68-byte allocation. The controller's maximum DMA write extent remains
unknown. [Board measurements and qualification requirements](../docs/usb-dma/README.md#fix-and-remaining-hardware-contract)

The report stays in RAM until the FM-1 restarts. Hold **OCT-** and **OCT+**
for two seconds to return to download mode.

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

## Persistent CPU1 audio probe

Build with `./fm1/scripts/build-example.sh rticx-cpu1-audio` and install
`target/firmware/rticx-cpu1-audio.fwsc` on the FM-1. Use the same qualified
board and ordinary USB-MIDI host setup as the USB log example. A quiet
tone and alternating blue LCD fills run for about 1.5 seconds. Steady green
PLAY and a green screen labeled `AUDIO PASS` mean the audio test passed. An
audio failure leaves PLAY amber and the screen red with the failure code.
The result and peak timings stay on the LCD while the USB readout continues
to update. Hold PLAY for two seconds to return to download mode.

```sh
fm1 log
```

If the Mac cannot find `1209:0001`, photograph the LCD before touching the
cable. The first audio-probe image produced a tone and a green result on an
FM-1 while the Mac's USB device tree had no FM-1 entry. The on-board readout
lets the same test distinguish the following observations.

| LCD | What the device observed |
| --- | --- |
| `USB WAIT RESET` | USB0 finished attaching. The driver has yet to see a bus reset or SETUP on this attempt. |
| `USB WAIT SETUP` | USB0 reported a bus reset. EP0 is waiting for its first SETUP packet. |
| `USB WAIT ADDRESS` / `USB WAIT CONFIG` | EP0 received SETUP / USB0 applied the host address after the status stage. |
| `USB ONLINE` / `USB SUSPENDED` | The host selected a configuration. `SUSPENDED` records a suspend indication or a stopped frame counter. |
| `USB DISCONNECTED` | USB0 signaled a disconnect. |
| `USB SIE TIMEOUT` | An indexed controller register access exceeded the driver's 100 µs bound. USB0 detached and will retry after 250 ms. A powered FM-1 with no data host can also show this. |

`RESETS` counts bus-reset indications and `RETRIES` counts controller
timeouts since boot. `REQ` shows the last SETUP's `bmRequestType`,
`bRequest` and `wValue` in hex. `IDX` and `LEN` give `wIndex` and
`wLength`. `TIMEOUT RD/WR` names the register access that last timed out.
`STALL` identifies the last request EP0 rejected, or the unexpected byte
count if EP0 was expecting SETUP. The request, timeout and stall survive
reconnects. For example, a successful configuration can show both
`USB ONLINE` and an earlier `TIMEOUT WR 01`. When comparing cold power-on,
a bootloader launch or cable reconnect, record which one produced the screen.

The LCD shows `BLOCKS` in decimal and rounds peak times up to whole
microseconds. A successful USB report has the following layout. Its times are hexadecimal
24 MHz TIMER4 ticks. Divide by 24 for microseconds. One 128-frame period is
69,632 ticks.

```text
C1 PASS 1FF 11 <commit> <gap> <worker>
```

| Field | Interpretation |
| --- | --- |
| `1FF` | 511 PCM blocks validated and accepted by ALNK0 out of 512 numbered jobs. |
| `11` | CPU0 submitted silence before releasing the halfway gate, then discarded that job's late completion. The one-slot pipeline submits silence again while requesting its replacement. |
| `commit` | Maximum time from ALNK0 handler entry through a successful refill. |
| `gap` | Maximum interval between serviced audio interrupt entries. |
| `worker` | Longest CPU1 render for a validated, played block. Excludes the deliberately stalled job. |

On a physical FM-1 connected to macOS, the LCD showed `AUDIO PASS`,
`USB ONLINE` and `RESETS 13`. The host received the following `fm1 log` reply.

```text
C1 PASS 1FF 11 0000099D 00011001 00000166
```

CPU0's peak entry-to-refill time was 102.54 µs. The longest interval between
serviced audio interrupts was 69,633 ticks (2,901.375 µs), one tick above
the 128-frame period. CPU1's peak render time for this test waveform was
14.92 µs. The reset count is cumulative. A second reading with the cable
still connected can show whether resets continue after configuration.

`RUN` is a live snapshot. `BT0/BT2/BT3/BT4` are the boot probe's timeout,
context, reply and fault statuses. `CPU1` means the live reply or debug status
changed. `SEQ`, `GATE` and `MISS` identify an unexpected completion number,
premature completion of the gated job, or a job unavailable at its deadline.
`DATA` means the samples or checksum disagreed. `DMA` means the refill was
late. `TIME` means the service time reached one period or the gap reached two
periods. `IRQ` means progress stopped for one second. `DAC` and `LCD` report
peripheral failures. `DRAW` means no LCD fill overlapped an accepted audio
block. A CPU0 exception can stop the reporter. The REC power-on download
gesture remains available.

CPU1 polls one sequence-tagged SRAM slot. It owns the samples until it has
published all 128 words, their checksum and its render time. CPU0 copies a
completed block, verifies every sample against its job and frame, then submits
the next job. At job 64 CPU1 pauses after 64 stores. Only a successful
silent refill releases it. The next normal job must pass the same checks.
CPU0 holds the peer on completion, failure and the download path. Boot and
handoff rely on aligned SRAM words, `csync` and the hardware hold. A pass
measures this exchange on the tested unit. Application linker scripts with
larger CPU0 stack reservations should check `__fm1_statics_end`, which
includes the CPU1 stacks and mailbox after `.bss`.

### Async patterns for audio and graphics

Keep the DSP state on CPU1 and the RTICX executor, ALNK0, USB and SPI1 on CPU0.
In this example ALNK0 has the highest priority. USB0 wakes a separate CPU0
async task that owns `MidiDevice`, with a 10 ms fallback for attachment,
retry and frame sampling. Both can preempt a full-screen fill. The
lower-priority `Monitor` copies the audio and USB snapshots under short
resource locks and releases them before touching SPI1 or awaiting its timer.
It fills the screen during the audio test, then repaints changed status cells.
For an async SPI transfer, keep its buffer and future in the CPU0 display task
through completion.
The [waveform example](examples/rticx-waveform.rs) shows CPU0 channels and
async display transfers.

For a synth, send bounded control messages to the DSP and return owned PCM
blocks tagged with a sequence and the settings revision. The ALNK0 interrupt
polls once, feeds ready audio to DMA and records the revision only after
`Refill::Written`. Send a small meter or scope snapshot from that interrupt
to the CPU0 display task with `try_send`. The task can `recv().await`.
A full telemetry queue may drop an update. Note edges and edit receipts need
an explicit overflow policy that preserves their ordering.

The following endpoints both belong to CPU0. A display task can skip a stale
meter reading without delaying audio. The application owns the choice to
drop the new reading when the channel is full.

```rust
use fm1_rticx::rticx::asynchronous::channel::{Receiver, Sender};

fn publish_meter(tx: &mut Sender<'_, [u16; 2], 1>, peaks: [u16; 2]) {
    let _ = tx.try_send(peaks);
}

async fn next_meter(rx: &mut Receiver<'_, [u16; 2], 1>) -> Option<[u16; 2]> {
    rx.recv().await.ok()
}
```

A future CPU0-facing completion API can share a nonblocking readiness check
with the audio interrupt. Keep the waiter on CPU0. Check for completion,
register the local waker and recheck before returning `Pending`. A CPU0
timer or interrupt observes the published sequence and wakes that task.
Use one consumer for each completion. Dropping a wait or reaching a timeout
abandons interest in the result. The worker retains its slot until it
acknowledges completion or CPU0 establishes a hardware hold. The existing
RTICX channels, resource ceilings and critical sections synchronize CPU0
tasks. A generic CPU1 executor would also need its own wake/dispatch
mechanism and an audit of every global synchronization backend reachable
from a submitted future. Construct and poll core-local futures on the core
that owns their state. A remote spawn handle could enqueue owned `Send`
commands. A `Send + 'static` future can still access a static protected by
the current CPU0-only critical section.

## Application setup

For a firmware project, add the prerelease to `Cargo.toml`.
Enable its `audio` feature to use the ALNK0 driver.

```toml
[dependencies]
fm1-rticx = "0.2.0-pre"
```

Annotate the application module with `#[fm1_rticx::app]`. In `#[init]`, call
`fm1_rticx::bsp::Board::take()` once and move the handles into your tasks
through `TaskInits`. RTICX enables interrupts after it initializes the tasks
and vectors. Use `#[post_init]` to spawn the first software tasks. The
[RTICX blink example][rticx-blink-example] includes a panic handler and
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

Task state, shared fields and spawn inputs cross interrupt contexts and must
implement `Send`. To hand off a board driver, store `bsp::Cpu0<DisplayPort>`
(or the corresponding driver type) and initialize it with `board.display.into()`.
Use `port.into_inner()` on CPU0 when a driver method consumes the raw port.
`fm1_audio::Output` and `Timer5` already support the handoff. Place an owned
`Cell` under the resource lock and perform the complete operation inside the
closure. A reference to a `Cell` cannot be handed to two tasks.

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

Each clock's `start` provides 16 slots. Count one slot for each delay or timeout
that has returned `Pending` and has yet to complete or be dropped. This includes
an expired delay whose task has yet to run. Dropping the future releases its
slot. Forgetting a registered future keeps its slot occupied. Polling a new
delay that needs a slot panics when all slots are in use.

Use `start_with_slots` to choose a different budget per clock. For example,
an application using `static_cell` can give TIMER2 room for 32 pending waits.

```rust,ignore
use fm1_rticx::time::{DeadlineClock, TimerSlot};
use static_cell::ConstStaticCell;

static SLOTS: ConstStaticCell<[TimerSlot; 32]> =
    ConstStaticCell::new([const { TimerSlot::EMPTY }; 32]);
DeadlineClock::start_with_slots::<3>(board.timers.take_alarm().unwrap(), SLOTS.take());
```

The queue scans the slot array with interrupts masked when finding the next
deadline. Larger arrays increase the worst-case time for a scan. The clock
owns the waker from the most recent poll and takes it out of the slot before
calling it, so a wake can immediately cancel the delay and reuse its storage.
Generic clock users should use the `Timebase` trait. The clocks no longer
implement `rtic_time`'s `TimerQueueBackend` or `TimerQueueBasedMonotonic`.

## License

Licensed under the [MIT License][mit].

The `audio` feature enables the BSP’s MPL-2.0 audio driver. The
[BSP license section][bsp-license] lists the drivers and their upstream sources.

### Contribution

Unless you explicitly state otherwise, you license contributions you
intentionally submit to this crate under the [MIT License][mit].

[RTICX]: https://github.com/rticx-rs/rticx
[bsp]: https://docs.rs/fm1/
[runtime]: https://docs.rs/rticx-pi32v2/
[audio]: https://docs.rs/fm1-audio/
[controls]: https://docs.rs/fm1-controls/
[build-and-flash]: ../fm1/README.md#build-and-flash
[bsp-usb-log]: ../fm1/README.md#usb-debug-log
[rticx-blink-example]: https://github.com/fm1-rust/fm1-rust/blob/862c16498a68b92a2d174140edf0e73e021aaf22/fm1-rticx/examples/rticx-blink.rs
[rticx-tone-example]: https://github.com/fm1-rust/fm1-rust/blob/862c16498a68b92a2d174140edf0e73e021aaf22/fm1-rticx/examples/rticx-tone.rs
[rticx-controls-example]: https://github.com/fm1-rust/fm1-rust/blob/862c16498a68b92a2d174140edf0e73e021aaf22/fm1-rticx/examples/rticx-controls.rs
[rticx-piano-example]: https://github.com/fm1-rust/fm1-rust/blob/862c16498a68b92a2d174140edf0e73e021aaf22/fm1-rticx/examples/rticx-piano.rs
[rticx-waveform-example]: https://github.com/fm1-rust/fm1-rust/blob/862c16498a68b92a2d174140edf0e73e021aaf22/fm1-rticx/examples/rticx-waveform.rs
[rticx-timing-example]: https://github.com/fm1-rust/fm1-rust/blob/main/fm1-rticx/examples/rticx-timing.rs
[usb-log-example]: https://github.com/fm1-rust/fm1-rust/blob/862c16498a68b92a2d174140edf0e73e021aaf22/fm1-rticx/examples/usb-log.rs
[standalone]: ../fm1/README.md#use-from-another-crate
[target-spec]: https://raw.githubusercontent.com/fm1-rust/fm1-rust/862c16498a68b92a2d174140edf0e73e021aaf22/fm1/targets/pi32v2-none.json
[crate-badge]: https://img.shields.io/crates/v/fm1-rticx?logo=rust
[crate]: https://crates.io/crates/fm1-rticx
[docs-badge]: https://docs.rs/fm1-rticx/badge.svg
[docs]: https://docs.rs/fm1-rticx/
[ci-badge]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-rticx.yml/badge.svg?branch=main&event=push
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-rticx.yml
[mit]: https://github.com/fm1-rust/fm1-rust/blob/main/fm1-rticx/LICENSE
[bsp-license]: https://github.com/fm1-rust/fm1-rust/blob/main/fm1/README.md#license

[host-cli]: https://github.com/fm1-rust/fm1-rust/tree/main/fm1-cli
