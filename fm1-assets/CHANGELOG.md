# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 (2026-11-24)

### Added

- Embed read-only assets in FM-1 flash with `flash_asset!` and the
  `fm1-assets.ld` linker script. The ELF packager includes the assets in
  emulator and M-Flasher images.
- Stream packed mono PCM16 from flash with `pcm::PackedPcm16`. The optional
  `encode` feature adds a host-side encoder.
- Stream mono FLAC with the optional `flac` feature.
