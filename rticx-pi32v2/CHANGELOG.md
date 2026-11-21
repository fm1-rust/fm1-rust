# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.1.2 (2026-11-21)

### Changed
- [BREAKING] `asynchronous::channel` and `asynchronous::make_channel!` now
  use `rtic-sync` 1.6.1. If you name `rticx_async::channel` types directly,
  import them from `rticx_pi32v2::asynchronous::channel` instead.

### Fixed
- Reusing a channel no longer delivers the same owned value twice.
- Blocked sends use the latest waker, and receivers can drain queued messages
  after the last sender drops.

## 0.1.1 (2026-11-21) [YANKED]

Yanked because the re-exported `rticx-async` 0.2.0 channel could return the
same owned value twice after reuse, potentially causing a double free.

### Added
- Support `#[task(binds = USB0)]` on CPU0, including priority-ceiling masking
  for the USB controller interrupt.
- Add the TIMER2 interrupt entry used by the FM-1 deadline clock.

### Fixed
- Use wrapping pointer arithmetic for interrupt controller registers and RAM
  vector slots so address calculation does not require a backing Rust allocation.

## 0.1.0 (2026-11-20) [YANKED]

Yanked for the same `rticx-async` channel bug as 0.1.1. Reusing a channel
could give two owners the same value.

Initial release
