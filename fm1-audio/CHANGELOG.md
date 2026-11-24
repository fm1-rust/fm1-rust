# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.1 (2026-11-24)

### Added

- Add gain-controlled PCM16 mono and stereo mixing and unity-gain interleaved
  and planar Q23 mixing. The layout is public and the FM-1 uses DSP kernels.
- Add the `mix-bench` firmware example to check the kernels against scalar Rust
  and compare ticks per 128-frame block on the LCD.

### Changed

- Reduce branching in the existing FM-1 planar `I24` mixer.

## 0.2.0 (2026-11-24)

### Added

- Add `Output::render_with_interval` so a sequencer can recognize missed DAC
  halves before choosing the next block of samples.

### Changed

- `Output` now holds its DMA stream in `fm1::Cpu0`, so an RTICX interrupt
  task can own the audio output under the new `Send` requirement.

## 0.1.0 (2026-11-21)

Initial release. Re-exports the `fm1::audio` API and adds 128-frame stereo
output, Q23 mixing, MASTER gain ramps, and audio interrupt timing statistics.
