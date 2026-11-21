# FM-1 Controls

[![crates.io][crate-badge]][crate]
[![docs.rs][docs-badge]][docs]
[![Build status][ci-badge]][ci]

Allocation-free front-panel input for the M-VAVE FM-1.

## About

The crate re-exports the BSP's panel types, including `Controls`, `Scan`,
`Event` and `Leds`.

Split a `ScanQueue<N>` during startup. In the panel timer interrupt, call
`Controls::scan(&leds)` once per 100 µs. When it returns a complete sweep
(every 11 columns), call `sender.try_send(scan, fm1::time::cycles())`. In an
async task, await `receiver.recv()` and iterate over `frame.scan.events()`.
The queue uses the application's `critical-section` implementation.

The FIFO preserves button edges and signed encoder deltas in order while it
has space. If the consumer falls behind, the producer drops new sweeps and
the next accepted frame reports their count in `missed_before`. The consumer
can recover current switch levels and MASTER from that frame. Presses and
detents that occurred entirely in a gap cannot be reconstructed.

The `rticx-controls` example uses eight slots, about 8.8 ms of sweeps from an
empty queue at the standard scan rate. Hold HOME to see timing and drops.

## License

Licensed under the [MIT License][mit].

[crate-badge]: https://img.shields.io/crates/v/fm1-controls?logo=rust
[crate]: https://crates.io/crates/fm1-controls
[docs-badge]: https://docs.rs/fm1-controls/badge.svg
[docs]: https://docs.rs/fm1-controls/
[ci-badge]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-controls.yml/badge.svg?branch=main&event=push
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-controls.yml
[mit]: https://github.com/fm1-rust/fm1-rust/blob/main/fm1-controls/LICENSE
