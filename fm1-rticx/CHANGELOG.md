# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.1.0 (2026-11-21)

Initial release. Includes the `#[fm1_rticx::app]` entry point, the FM-1 BSP
and pi32v2 RTICX runtime, a 100 Hz TIMER5 clock, and a 24 MHz deadline clock.

The re-exported `rticx-async` 0.2.0 channel has known wakeup and memory-safety
bugs. This release establishes the existing channel API.
