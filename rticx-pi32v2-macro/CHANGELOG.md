# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 0.2.0 (Unreleased)

### Changed

- [BREAKING] Require `Send` for each shared field, task and idle state, and
  async spawn input. Futures still run on their owning dispatcher and may
  hold non-`Send` locals across an await.
- [BREAKING] Restrict construction of the generated IRQ dispatcher and
  priority-zero idle executor. Use `Task::spawn` to schedule work.

### Fixed

- Check `Send` against the compiled shared field and require its type to match
  the lock proxy. A conditional declaration could hide a non-`Send` field
  behind a dereference coercion.
- Bind interrupt and idle dispatch to the generated task traits. A safe application
  could shadow `RticTask` and retain a mutable task reference across interrupts.
- Require a direct struct field for each shared resource lock. A field removed
  by `cfg` could previously resolve through `DerefMut` and alias another lock.
- Both application attributes enforce handoffs using the defining runtime's
  `Send` bound, even if application code supplies its own `core::marker::Send`.
- Invoke the interrupt and idle task traits explicitly. Bind generated storage
  and executor calls to the runtime. Previously an application method could
  retain a task reference or poll a future from its wake callback.
- Resolve interrupt names as runtime enum variants for both vector installation
  and ceiling masks. Application conversions and trait constants could
  previously select the wrong vector.

### Known issues

- A declaration inside the application module with the same name as generated
  startup, wake or storage code can interfere with these checks, even under
  `#[cfg(any())]`. Avoid declaring such items alongside the generated ones.

## 0.1.1 (2026-11-21)
### Added
- Support `#[task(binds = USB0)]` with a pi32v2 runtime that provides the USB0 vector.

### Fixed
- Let rustc reject unaligned borrows of packed shared fields in generated resource locks.

## 0.1.0 (2026-11-20)
Initial release
