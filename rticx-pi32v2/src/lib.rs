#![no_std]
#![doc = include_str!("../README.md")]

pub mod asynchronous;
#[doc(hidden)]
pub mod executor;
#[doc(hidden)]
#[cfg(pi32v2)]
pub mod export;
#[cfg(any(pi32v2, test))]
mod mask;
#[cfg(any(pi32v2, test))]
mod queue;
#[cfg(any(pi32v2, test))]
mod registers;
pub use rticx_pi32v2_macro::app;
#[doc(hidden)]
pub use rticx_pi32v2_macro::{__entry, __generate_app, __interrupt, fm1_app};

/// Expand an application using the runtime that defined this macro.
#[doc(hidden)]
#[macro_export]
macro_rules! __expand_app {
    ([$($args:tt)*] $($app:tt)*) => {
        // SAFETY: `$crate` identifies this runtime through renames and re-exports.
        $crate::__generate_app! {
            $crate [unsafe { __rticx_origin::assume_genuine() }] [$($args)*] $($app)*
        }
    };
}

/// CPU0 interrupt sources currently supported on the AC791N.
/// The discriminants are indices in the SoC's RAM vector table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Interrupt {
    /// Audio link 0.
    ALNK0 = 11,
    /// SPI controller 1.
    SPI1 = 16,
    /// UART controller 1.
    UART1 = 20,
    /// Successive approximation ADC.
    SARADC = 24,
    /// SPI controller 2.
    SPI2 = 37,
    /// General-purpose timer 5.
    TIMER5 = 63,
    /// Software interrupt 0.
    SOFT0 = 120,
    /// Software interrupt 1.
    SOFT1,
    /// Software interrupt 2.
    SOFT2,
    /// Software interrupt 3.
    SOFT3,
    /// Software interrupt 4.
    SOFT4,
    /// Software interrupt 5.
    SOFT5,
    /// Software interrupt 6.
    SOFT6,
    /// Software interrupt 7.
    SOFT7,
}
