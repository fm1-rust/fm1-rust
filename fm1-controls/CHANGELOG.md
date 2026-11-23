# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 (Unreleased)

### Changed

- Use the v0.2 BSP for the front-panel types re-exported by `fm1-controls`.

## 0.1.1 (2026-11-21)

### Changed
- [BREAKING] `ScanReceiver` is now a concrete type. Replace explicit
  `rticx_async::channel::Receiver` annotations with `fm1_controls::ScanReceiver`.
  Import `ReceiveError` and `TrySendError` from `fm1_controls`. Their underlying
  types now come from `rtic-sync`.

### Fixed
- Cancelling a pending panel receive releases its task's registered waker.

## 0.1.0 (2026-11-21) [YANKED]

Yanked because the panel queue's `rticx-async` 0.2.0 channel retained the
waker of a cancelled `recv()`. A later scan could wake a task that was no
longer waiting for input.

Initial release. Re-exports the BSP front-panel types and adds an allocation-free
queue that delivers timestamped panel scans to async tasks and reports dropped sweeps.
