#![no_std]
#![doc = include_str!("../README.md")]

pub use fm1 as bsp;
pub use rticx_pi32v2 as rticx;
#[doc(inline)]
pub use rticx_pi32v2_macro::fm1_app as app;
pub mod time;
