# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 (Unreleased)

### Added

- Add the `rticx-timing` diagnostic for measuring interrupt and async-task
  timing on the board. It reports over USB and requires a bench setup that
  satisfies the BSP's receive DMA contract.
- Add `rticx-cpu1-audio`, which runs a fixed audio worker on CPU1. CPU0
  mixes a local PCM16 voice into each completed block with `fm1-audio` 0.2.1
  and submits the checked stereo mix from the ALNK0 interrupt.

### Changed

- `rticx-piano` uses `UsbPort::try_into_midi` at startup. On AC791N the
  keyboard plays locally when the BSP returns `UsbError::DmaProtectionUnavailable`.
- [BREAKING] `Clock` and `DeadlineClock` own their pending timer registrations.
  This fixes a use-after-free during expiry and wakes the most recent waker
  after a repoll. Use `rtic_time::Timebase` in generic clock bounds. Each clock
  provides 16 slots by default. Use `start_with_slots` for a different capacity.
- [BREAKING] The async channel now uses local endpoints with nonblocking
  `try_send` and cancellable `recv().await`. Update explicit `rtic_sync` endpoint
  types to the runtime channel types. Callers of `send().await` must handle
  `TrySendError::Full(value)` or arrange a separate capacity notification.
- [BREAKING] `#[fm1_rticx::app]` requires `Send` for shared fields, task and
  idle state, and async spawn inputs. Wrap supported owned BSP handles in
  `bsp::Cpu0` when handing them to interrupt tasks. Task-local futures may
  keep non-`Send` values across an await.

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
