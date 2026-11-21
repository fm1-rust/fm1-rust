# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.1.1 (2026-11-21)
### Added
- Support `#[task(binds = USB0)]` with a pi32v2 runtime that provides the USB0 vector.

### Fixed
- Let rustc reject unaligned borrows of packed shared fields in generated resource locks.

## 0.1.0 (2026-11-20)
Initial release
