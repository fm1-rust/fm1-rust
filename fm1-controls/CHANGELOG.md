# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## Unreleased

- [BREAKING] `ScanReceiver` is now a concrete `fm1-controls` type. Code that
  names `rtic_sync::channel::Receiver` for panel input must use
  `fm1_controls::ScanReceiver` instead. Its receive methods and error types are preserved.
- Cancelling a pending panel receive releases its task's registered waker.

## 0.1.0 (2026-11-21)

Initial release. Re-exports the BSP front-panel types and adds an allocation-free
queue that delivers timestamped panel scans to async tasks and reports dropped sweeps.
