//! RGB565 drawing with the `embedded-graphics` feature.
//!
//! Draw directly on a [`MipidsiDisplay`] or draw into a [`Framebuffer`] and
//! send its changed rows with [`Display::write_framebuffer_buffered`].

pub use embedded_graphics;
pub use mipidsi;
pub use oxstack_gfx::Framebuffer;

use super::{CHUNK, CON, Display, DisplayPort, Error, SIZE, Spi1, SpiClock};
use crate::registers::write;
use core::convert::Infallible;
use embedded_hal::delay::DelayNs;
use mipidsi::{
    Builder, InitError, NoResetPin,
    interface::{Interface, InterfaceKind},
    models::ST7789,
    options::ColorInversion,
};

/// The upstream ST7789 driver configured for the FM-1's 240 × 240 panel.
pub type MipidsiDisplay = mipidsi::Display<SpiInterface, ST7789, NoResetPin>;

/// The FM-1's four-wire SPI1 display interface. Pixel transfers use DMA.
#[derive(Debug)]
pub struct SpiInterface(Spi1);

impl DisplayPort {
    /// Initialize the ST7789 as an RGB565 `DrawTarget`. Pass the board timers
    /// for the controller's reset and wake delays. Drawing waits for SPI1 to finish.
    ///
    /// # Errors
    /// Returns [`InitError::Interface`] if an initialization transfer times out.
    pub fn init_mipidsi(
        self,
        clock: SpiClock,
        delay: &mut impl DelayNs,
    ) -> Result<MipidsiDisplay, InitError<Error, Infallible>> {
        let display = Builder::new(ST7789, SpiInterface(self.start_spi()))
            .display_size(SIZE, SIZE)
            .invert_colors(ColorInversion::Inverted)
            .init(delay)?;
        write(CON + 4, clock as u32);
        Ok(display)
    }
}

impl Display {
    /// Send the run of changed rows of a canvas positioned at `(x, y)`.
    /// Rows span the canvas width. A clean canvas sends nothing. Call
    /// [`Framebuffer::mark_all_dirty`] to repaint a clean canvas at a new origin.
    ///
    /// The canvas can be drawn into again immediately. Call [`Self::flush`]
    /// to wait for the last DMA transfer and reveal the first image.
    ///
    /// # Errors
    /// Returns [`Error::InvalidRegion`] if the entire canvas does not fit, keeping
    /// its dirty rows. Returns [`Error::Timeout`] if a transfer stalls. Mark the
    /// canvas dirty before repainting on a reinitialized display.
    pub fn write_framebuffer_buffered<const W: usize, const H: usize>(
        &mut self,
        x: u16,
        y: u16,
        canvas: &mut Framebuffer<W, H>,
    ) -> Result<(), Error> {
        let width = u16::try_from(W).map_err(|_| Error::InvalidRegion)?;
        let height = u16::try_from(H).map_err(|_| Error::InvalidRegion)?;
        self.region_bytes(x, y, width, height)?;
        if let Some(dirty) = canvas.take_dirty() {
            #[allow(
                clippy::arithmetic_side_effects,
                reason = "The entire canvas, including the last dirty row, fits on the panel."
            )]
            let top = y + dirty.y;
            if let Err(error) = self.write_rect_buffered(x, top, width, dirty.rows, dirty.bytes) {
                canvas.mark_all_dirty();
                return Err(error);
            }
        }
        Ok(())
    }
}

impl Interface for SpiInterface {
    type Word = u8;
    type Error = Error;
    const KIND: InterfaceKind = InterfaceKind::Serial4Line;

    fn send_command(&mut self, command: u8, args: &[u8]) -> Result<(), Error> {
        self.0.command_data(command, args)
    }

    fn send_pixels<const N: usize>(
        &mut self,
        pixels: impl IntoIterator<Item = [u8; N]>,
    ) -> Result<(), Error> {
        self.0.wait_dma()?;
        if N == 0 {
            return Ok(());
        }
        let mut bytes = pixels.into_iter().flatten();
        loop {
            let mut len = 0;
            #[allow(
                clippy::arithmetic_side_effects,
                reason = "The strip has at most CHUNK slots."
            )]
            for (slot, byte) in self.0.staging().iter_mut().zip(&mut bytes) {
                *slot = byte;
                len += 1;
            }
            if len != 0 {
                self.0.start_dma(len);
                self.0.wait_dma()?;
            }
            if len < CHUNK {
                return Ok(());
            }
        }
    }

    fn send_repeated_pixel<const N: usize>(
        &mut self,
        pixel: [u8; N],
        count: u32,
    ) -> Result<(), Error> {
        if let [hi, lo] = pixel.as_slice() {
            self.0.repeat_rgb565([*hi, *lo], count as usize)?;
            self.0.wait_dma()
        } else {
            self.send_pixels((0..count).map(|_| pixel))
        }
    }
}
