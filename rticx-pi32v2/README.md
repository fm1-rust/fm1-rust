# rticx-pi32v2

[![crates.io][crate-badge]][crate]
[![docs.rs][docs-badge]][docs]
[![Build status][ci-badge]][ci]

This [RTICX](https://github.com/rticx-rs/rticx) distribution runs interrupt
handlers and async tasks on CPU0 of the JieLi AC791N. On the FM-1, preliminary
multicore support lets a dedicated DSP worker run on CPU1 alongside those tasks.

Use the [fm1-rticx board crate][board] for FM-1 applications or invoke
`#[rticx_pi32v2::app(device = fm1, dispatchers = [SOFT0])]` directly.

## CPU1 audio worker

The [FM-1 audio-block probe][cpu1-probe] and [WaveGraph][wavegraph-worker]
use board-specific startup code to launch their workers. CPU1 renders audio
blocks and hands them back through a shared SRAM mailbox. CPU0's ALNK0
interrupt handler uses completed blocks to refill the audio DMA buffer.

RTICX task attributes accept only `core = 0`. Shared-resource locks and async
channels are local to CPU0 because the runtime implements critical sections
by masking interrupts on that core.

## Interrupts and priorities

RTICX priorities 1 through 8 map to CPU0 hardware priorities 0 through 7.
Priority zero runs in the idle context. The application supplies its startup
code, CPU0 stacks and compiler-rt builtins through the BSP. `PI32V2_BIN`
selects the same assembler as the firmware build.

Hardware tasks can bind USB0 (controller IRQ 9), ALNK0, SPI1, UART1, SARADC,
SPI2 and TIMER5. The same `Interrupt` variant selects the installed vector and
the source masked by a resource lock. Downstream matches need a wildcard arm
so the runtime can add support for more sources.

The FM-1 deadline clock can independently claim TIMER2 (IRQ 6) at its
chosen priority. Its handler services only the timer queue. Peripheral handlers must acknowledge their interrupt at its source.
SOFT0 through SOFT7 can dispatch software tasks. The runtime clears the
software latch on entry so a wake during dispatch can pend it again.

The entry stubs save the interrupted C ABI state and open both interrupt
gates for higher priorities. The controller defers equal and lower priorities.
A resource lock temporarily disables the registered sources at or below that
field's ceiling. Global interrupts are masked only while the enable bits and
borrow state are updated. Nested locks restore exactly the bits each cleared.
See the [hardware nesting measurements][nesting] and the [interrupt ABI][abi].

The application macro uses the published RTICX core and async compilation
passes. With the pinned 0.2 releases, it keeps each executor slot in the
non-returning entry frame and projects shared fields through raw pointers
before locking. Size task futures for the application's startup stack.

Implement interrupt handlers in `RticTask::exec` and idle handlers in
`RticIdleTask::exec`. The runtime invokes these trait methods with a fresh
mutable borrow for each call, even if the task type has an inherent `exec`
method.

### Values crossing interrupt priorities

Task and idle state, each shared field, and each async spawn input must implement
`Send`. A shared `Cell<u32>` can be used inside its lock. A shared
`&Cell<u32>` would let both priorities copy the reference and access the cell
after releasing the lock. The macro rejects that type in both shared storage
and task initializers. The future created by an async task stays on its owning
dispatcher and may keep task-local, non-`Send` values across an await.
Both application attributes resolve the handoff bound in the defining runtime.
Re-exporting the runtime or naming another crate `core` cannot change the
trait used by this check.

Spawn async work with the generated `Task::spawn` method. Startup installs the
slot storage once. Only its owning interrupt or idle dispatcher may install
and poll a future. A wake marks the slot pending, even if it arrives before
the future is installed.

### Async channels

Use `rticx_pi32v2::asynchronous::make_channel!(T, N)` once at startup to
create a bounded FIFO with cloneable producers and one receiver. Each producer
calls `try_send`. On `Full(value)` the caller retains the value and decides
whether to coalesce, drop or retry it later. Values move under the interrupt
mask. For large payloads, send a handle, as the waveform example does for
render jobs. The receiver can use `recv().await` or `try_recv()` and drains
buffered values after the last sender drops.

In v0.2, import endpoints from `asynchronous::channel`. The previous
`rtic-sync` endpoint types were exposed directly and supported `send().await`.
A custom waker could cancel a blocked sender while its intrusive queue node
was being accessed. A waker clone could also send while the receiver was
registering. The replacement keeps only a receiver waker and runs waker
callbacks and payload destructors outside the channel's critical section.

## License

Licensed under the [MIT License][mit].

### Contribution

Unless you explicitly state otherwise, you license contributions you
intentionally submit to this crate under the [MIT License][mit].

[crate-badge]: https://img.shields.io/crates/v/rticx-pi32v2?logo=rust
[crate]: https://crates.io/crates/rticx-pi32v2
[docs-badge]: https://docs.rs/rticx-pi32v2/badge.svg
[docs]: https://docs.rs/rticx-pi32v2/
[ci-badge]: https://github.com/fm1-rust/fm1-rust/actions/workflows/rticx-pi32v2.yml/badge.svg?branch=main&event=push
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/rticx-pi32v2.yml
[board]: https://docs.rs/fm1-rticx/
[cpu1-probe]: https://github.com/fm1-rust/fm1-rust/blob/c4717283bff5f1fabfaf7887fc62e90aba24c882/fm1-rticx/README.md#persistent-cpu1-audio-probe
[wavegraph-worker]: https://github.com/fm1-rust/wavegraph/blob/0e3e6adc225ce6337518053fa8ceeff0ed344525/wavegraph-fm1/README.md#the-cpu1-render-worker
[mit]: https://github.com/fm1-rust/fm1-rust/blob/main/rticx-pi32v2/LICENSE
[nesting]: https://github.com/jvitkauskas/pi32v2-isa/tree/701a76faf103d1889d9bdf67987b872899c27257/hw/fm1-2026-10-24-app3
[abi]: https://github.com/jvitkauskas/pi32v2-isa/blob/701a76faf103d1889d9bdf67987b872899c27257/docs/ABI.md
