# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.1.1 (2026-11-20)
### Added
- `Board::take`, `Timers::on_interrupt`, and `time::ticks` for interrupt-driven applications.
- `AudioPort::into_stream` so an audio task can own its DMA stream.
- Support both PLAY/STOP indicator colors with `PlayLed::set_color`.

## 0.1.0 (2026-11-20)
Initial release
