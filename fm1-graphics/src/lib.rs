#![no_std]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

mod text;
mod waveform;

pub use embedded_graphics_core::pixelcolor::Rgb565;
pub use text::TextGrid;
pub use waveform::{Waveform, WaveformStyle};

/// RGB565 colors used by the FM-1 example dashboards.
pub mod palette {
    use super::Rgb565;
    use embedded_graphics_core::pixelcolor::RgbColor;

    /// Secondary labels.
    pub const MUTED: Rgb565 = Rgb565::new(19, 38, 19);
    /// Primary labels.
    pub const WHITE: Rgb565 = Rgb565::WHITE;
    /// Readouts and selection.
    pub const CYAN: Rgb565 = Rgb565::new(11, 52, 31);
    /// Active indicators.
    pub const GREEN: Rgb565 = Rgb565::new(11, 59, 12);
    /// Paused indicators.
    pub const AMBER: Rgb565 = Rgb565::new(31, 49, 8);
    /// Error indicators.
    pub const RED: Rgb565 = Rgb565::new(31, 14, 7);
}
