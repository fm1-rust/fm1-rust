# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 (Unreleased)

### Added

- Add `AsyncDisplay::fill_rect` for interrupt-driven RGB565 fills using one
  panel window and the BSP's existing SRAM strip.
- Add `audio::Stream::try_write_with_interval` to estimate how many DAC halves
  elapsed before a refill and report ambiguous counts explicitly.
- Add `Cpu0<T>` for passing an owned BSP peripheral between tasks on CPU0.
  Access, extraction and drop check which core is executing.
- Add the opt-in `cpu1-probe` boot example and linker support for a fixed
  SRAM worker on CPU1.
- Add `usb-dma-probe` and `usb-dma-stress` firmware support for measuring
  EP1 OUT writes on a qualified board.

### Changed

- [BREAKING] `UsbPort::into_midi` now panics if the BSP cannot guarantee receive
  DMA bounds and buffer ownership. Use `UsbPort::try_into_midi` to keep running
  without USB. On AC791N it detaches USB0 and returns
  `UsbError::DmaProtectionUnavailable` before arming receive DMA. Bench firmware
  that upholds the port's DMA contract can start MIDI with
  `unsafe { port.assume_midi_dma() }.into_midi(identity)`.

### Fixed

- Give EP0 and normal EP1 OUT separate, guarded 2 KiB receive buffers. On the
  black FM-1, EP1 DMA wrote beyond the former 68-byte allocation. The maximum
  write extent remains a board qualification requirement.
  [Hardware measurements](../docs/usb-dma/README.md#fix-and-remaining-hardware-contract)
- Acknowledge EP1 OUT during a diagnostic trial without reading the live
  payload. Gate and retarget DMA, allow the old destination to drain, then
  take the snapshot. Deferred acknowledgements and capture deadlines now
  make progress even on a quiet bus.

## 0.1.3 (2026-11-21)

### Added
- Add `usb_midi::UsbLog` behind the `usb-log` feature. It keeps complete ASCII
  lines in a 44-byte buffer. Read a snapshot over USB-MIDI SysEx with
  `scripts/read-usb-log.py`.

### Fixed
- Drop the unused waker outside the critical section when USB0 has already
  signalled activity.
- Give the USB controller buffers and the SPI display staging buffer an
  explicit layout so their byte arrays start at the four-byte-aligned DMA address.

## 0.1.2 (2026-11-21)
### Added
- Add typed front-panel scanning for buttons, piano keys, encoders, LEDs, and the
  MASTER potentiometer. `TouchControls` reports strike velocity on all 27 piano
  keys and polyphonic aftertouch on the lower 20.
- Add bidirectional USB-MIDI 1.0 device support with host session tracking.
  Enable `async-usb` to await USB0 interrupts.
- Add buffered ST7789 rectangle writes, solid-color fills, and selectable SPI
  clocks. Enable `async-display` to await SPI1 DMA completion or
  `embedded-graphics` to draw directly to the LCD or into a framebuffer.
- Add configurable audio DMA half-buffers of 1–256 stereo frames and
  `Stream::try_write_with` to render when a half-buffer becomes available.
- Expose the 24 MHz cycle counter through `time::cycles`, add an independent
  TIMER2 alarm, and support 100 µs panel scans while preserving the 10 ms tick.

### Fixed
- Avoid reporting an SPI timeout when DMA completed while the caller was
  preempted.

## 0.1.1 (2026-11-20)
### Added
- `Board::take`, `Timers::on_interrupt`, and `time::ticks` for interrupt-driven
  applications.
- `AudioPort::into_stream` so an audio task can own its DMA stream.
- Support both PLAY/STOP indicator colors with `PlayLed::set_color`.

## 0.1.0 (2026-11-20)
Initial release
