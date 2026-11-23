# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 (Unreleased)

### Changed

- Use the v0.2 BSP display driver for `TextGrid` and `Waveform`.
- Enforce `#![forbid(unsafe_code)]` throughout the graphics crate.

## 0.1.0 (2026-11-21)

Initial release. `TextGrid` and `Waveform` repaint changed regions of the FM-1
display without allocating. Enable `async-display` to await SPI1 DMA completion
when painting either widget.
