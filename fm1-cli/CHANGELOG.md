# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 (Unreleased)

### Added

- Add `fm1 dma-window` to capture, recover and replay the EP0/EP1 write-range experiments, including WND1 archives and WND3 destination checks in SRAM.
  Export one or more completed runs as a single JSON file with `--pack`, and pass the file to `--replay` for offline verification.

- Add the `fm1` host command. Use `fm1 log` for a firmware log snapshot or
  diagnostic trial history, and `fm1 timing` for the board's timing report.
- Add `fm1 dma-probe` and `fm1 dma-stress` to run EP1 OUT trials and save
  the evidence. Use `dma-stress --replay` to check saved captures offline.
- Require changed bytes in frozen SRAM before reporting a write for a
  nonempty RXD3 trial. A quiet control with an EP1 RX interrupt is
  inconclusive. A crossing of the historical 68-byte boundary is retained
  even when the host transfer reports an error.
