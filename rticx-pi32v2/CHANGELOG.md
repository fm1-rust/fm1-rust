# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.1.1 (2026-11-21)
### Added
- Support `#[task(binds = USB0)]` on CPU0, including priority-ceiling masking
  for the USB controller interrupt.
- Add the TIMER2 interrupt entry used by the FM-1 deadline clock.

### Fixed
- Use wrapping pointer arithmetic for interrupt controller registers and RAM
  vector slots so address calculation does not require a backing Rust allocation.

## 0.1.0 (2026-11-20)
Initial release
