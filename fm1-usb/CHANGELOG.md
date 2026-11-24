# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 (Unreleased)

### Added

- Add host transport for the WND1 and WND3 EP0 SETUP and EP1 OUT write-range probes.

- Discover an FM-1 over USB, connect to its system MIDI ports, and read
  firmware log snapshots and timing reports. Named MIDI ports can also be
  used with instruments that do not implement the log protocol.
- Add the optional `dma-probe` transport for RXD3 and RXS1 firmware. It
  reads frozen EP1 snapshots over EP0 and supports bulk and isochronous
  test traffic.
- Report the protocol identifier and returned length when a diagnostic
  handshake is truncated or comes from older firmware.
