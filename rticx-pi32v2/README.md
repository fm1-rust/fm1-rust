# rticx-pi32v2

A single-core [RTICX](https://github.com/rticx-rs/rticx) distribution for the
JieLi AC791N interrupt controller. FM-1 applications can use the
[fm1-rticx board crate](../fm1-rticx/). The distribution can also be used
directly with `#[rticx_pi32v2::app(device = fm1, dispatchers = [SOFT0])]`.

## Interrupts and priorities

CPU0 owns the interrupt controller and CPU1 stays parked. RTICX priorities
1 through 8 map to hardware priorities 0 through 7. Priority zero runs in
the idle context. The application supplies its startup code, CPU0 stacks
and compiler-rt builtins through the BSP. `PI32V2_BIN` selects the same
assembler as the firmware build.

The supported peripheral vectors are ALNK0, SPI1, UART1, SARADC, SPI2 and
TIMER5. The FM-1 deadline clock can independently claim TIMER2 (IRQ 6) at its
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

[nesting]: https://github.com/jvitkauskas/pi32v2-isa/tree/701a76faf103d1889d9bdf67987b872899c27257/hw/fm1-2026-10-24-app3
[abi]: https://github.com/jvitkauskas/pi32v2-isa/blob/701a76faf103d1889d9bdf67987b872899c27257/docs/ABI.md
