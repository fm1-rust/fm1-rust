# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
