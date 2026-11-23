# rticx-pi32v2

[![crates.io][crate-badge]][crate]
[![docs.rs][docs-badge]][docs]
[![Build status][ci-badge]][ci]

A single-core [RTICX](https://github.com/rticx-rs/rticx) distribution for the
JieLi AC791N interrupt controller. FM-1 applications can use the
[fm1-rticx board crate][board]. The distribution can also be used
directly with `#[rticx_pi32v2::app(device = fm1, dispatchers = [SOFT0])]`.

## Interrupts and priorities

CPU0 owns the interrupt controller and CPU1 stays parked. RTICX priorities
1 through 8 map to hardware priorities 0 through 7. Priority zero runs in
the idle context. The application supplies its startup code, CPU0 stacks
and compiler-rt builtins through the BSP. `PI32V2_BIN` selects the same
assembler as the firmware build.

Hardware tasks can bind USB0 (controller IRQ 9), ALNK0, SPI1, UART1, SARADC,
SPI2 and TIMER5. The FM-1 deadline clock can independently claim TIMER2 (IRQ 6) at its
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

Spawn async work with the generated `Task::spawn` method. Startup installs the
slot storage once. Only its owning interrupt or idle dispatcher may install
and poll a future. A wake marks the slot pending, even if it arrives before
the future is installed.

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
[mit]: https://github.com/fm1-rust/fm1-rust/blob/main/rticx-pi32v2/LICENSE
[nesting]: https://github.com/jvitkauskas/pi32v2-isa/tree/701a76faf103d1889d9bdf67987b872899c27257/hw/fm1-2026-10-24-app3
[abi]: https://github.com/jvitkauskas/pi32v2-isa/blob/701a76faf103d1889d9bdf67987b872899c27257/docs/ABI.md
