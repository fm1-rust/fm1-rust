# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 (Unreleased)

### Changed

- [BREAKING] The async channel now uses local endpoints with nonblocking
  `try_send` and cancellable `recv().await`. Update explicit `rtic_sync` endpoint
  types to the runtime channel types. Callers of `send().await` must handle
  `TrySendError::Full(value)` or arrange a separate capacity notification.
- [BREAKING] Remove the `asynchronous::executor` re-export. Submit work with
  the generated `Task::spawn` method.
- [BREAKING] Add `Interrupt::USB0` for vector 9 and mark `Interrupt`
  non-exhaustive. Downstream matches need a wildcard arm.

### Fixed

- Track executor slot reservation separately from initialization so an early or
  stale wake cannot poll uninitialized storage. A completed future is dropped
  in place before its slot can be reused.

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
