# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.1.1 (2026-11-21)

### Changed
- [BREAKING] Require `rticx-pi32v2` 0.1.2. The channel types and
  `make_channel!` re-exported through `rticx::asynchronous` now come from
  `rtic-sync`. If you name `rticx_async::channel` types directly, import
  them from `fm1_rticx::rticx::asynchronous::channel` instead.

### Added
- Ship the self-contained `rticx-waveform` example in the crate archive.
- Add `usb-log` and `rtt` features. The `usb-log` example records PLAY button
  events for readback over USB-MIDI and mirrors them to RTT.

## 0.1.0 (2026-11-21) [YANKED]

Initial release. Includes the `#[fm1_rticx::app]` entry point, the FM-1 BSP
and pi32v2 RTICX runtime, a 100 Hz TIMER5 clock, and a 24 MHz deadline clock.

Yanked because the published lockfile selected `rticx-pi32v2` 0.1.1,
whose re-exported channel could return the same owned value twice. It also
selected `fm1-controls` 0.1.0 for the examples. Downstream builds of 0.1.0
that resolved `rticx-pi32v2` 0.1.2 already use the fixed channel.
