# fm1-rticx

Write an FM-1 application as interrupt handlers and async Rust tasks on CPU0.
The crate provides the board entry point, the [FM-1 BSP](../fm1/) as `bsp`,
and a TIMER5 clock for delays and timeouts. The
[pi32v2 distribution](../rticx-pi32v2/) supplies the RTICX executor and
interrupt bindings. Both run without an allocator.

## Examples

With the [firmware toolchain environment](../fm1/README.md#build-and-flash)
set, run either command from the workspace root.

```sh
./fm1/scripts/build-example.sh rticx-blink
./fm1/scripts/build-example.sh rticx-tone
```

| Firmware | Tasks and controls |
| --- | --- |
| [rticx-blink](examples/rticx-blink.rs) | TIMER5 wakes two periodic async tasks. One toggles the PLAY LED every 500 ms. The other samples the button every 10 ms. Hold PLAY for two seconds to return to download mode. |
| [rticx-tone](examples/rticx-tone.rs) | Starts with a stereo tone, green screen and green PLAY indicator. Tap PLAY/STOP for silence, a red screen and an amber indicator. Tap again to resume. Hold for two seconds to return to download mode. A magenta strip along the bottom records a late DMA refill. |

The packages are written to `target/firmware/`. Both have been exercised in
the FM-1 emulator. Download both `.fwsc` files from the `fm1-rticx-firmware`
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
