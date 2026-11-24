# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 (Unreleased)

### Added

- Add `Output::render_with_interval` so a sequencer can recognize missed DAC
  halves before choosing the next block of samples.

### Changed

- `Output` now holds its DMA stream in `fm1::Cpu0`, so an RTICX interrupt
  task can own the audio output under the new `Send` requirement.

## 0.1.0 (2026-11-21)

Initial release. Re-exports the `fm1::audio` API and adds 128-frame stereo
output, Q23 mixing, MASTER gain ramps, and audio interrupt timing statistics.
