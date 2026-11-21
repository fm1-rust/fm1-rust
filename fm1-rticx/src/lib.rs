#![no_std]
#![doc = include_str!("../README.md")]

pub use fm1 as bsp;
pub use rticx_pi32v2 as rticx;
#[doc(inline)]
pub use rticx_pi32v2_macro::fm1_app as app;
pub mod time;

/// Text logging in SRAM using the SEGGER RTT format.
#[cfg(feature = "rtt")]
pub mod rtt {
    pub use rtt_target::UpChannel;

    /// Initialize a 256-byte nonblocking RTT up-channel on CPU0.
    /// Writes that do not fit are skipped. Use one RTT initializer per firmware.
    ///
    /// # Panics
    /// Panics if called more than once.
    #[must_use]
    pub fn init() -> UpChannel {
        let channels = rtt_target::rtt_init! { up: { 0: { size: 256, name: "Terminal" } } };
        channels.up.0
    }
}
