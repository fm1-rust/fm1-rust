# rticx-pi32v2-macro

[![crates.io][crate-badge]][crate]
[![docs.rs][docs-badge]][docs]
[![Build status][ci-badge]][ci]

This crate provides the RTICX application macros for CPU0 of the JieLi AC791N.
Use `#[rticx_pi32v2::app]` through the [pi32v2 runtime][runtime] or
`#[fm1_rticx::app]` through the [FM-1 board crate][board].

## License

Licensed under the [MIT License][mit].

[crate-badge]: https://img.shields.io/crates/v/rticx-pi32v2-macro?logo=rust
[crate]: https://crates.io/crates/rticx-pi32v2-macro
[docs-badge]: https://docs.rs/rticx-pi32v2-macro/badge.svg
[docs]: https://docs.rs/rticx-pi32v2-macro/
[ci-badge]: https://github.com/fm1-rust/fm1-rust/actions/workflows/rticx-pi32v2.yml/badge.svg?branch=main&event=push
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/rticx-pi32v2.yml
[mit]: https://github.com/fm1-rust/fm1-rust/blob/main/rticx-pi32v2-macro/LICENSE
[runtime]: https://docs.rs/rticx-pi32v2/
[board]: https://github.com/fm1-rust/fm1-rust/tree/main/fm1-rticx
