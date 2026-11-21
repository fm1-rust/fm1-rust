// SPDX-FileCopyrightText: 2026 Julius Vitkauskas
// SPDX-License-Identifier: MIT-0
//! The 240 × 240 ST7789 on SPI1. RGB565 bytes are sent most significant byte first.
//! DMA uses a private SRAM strip, so pixels may come from RAM or flash.

#[cfg(feature = "async-display")]
mod asynchronous;
#[cfg(feature = "async-display")]
pub use asynchronous::{AsyncDisplay, on_spi1_interrupt};

#[cfg(feature = "embedded-graphics")]
pub mod graphics;

use crate::{
    Token,
    registers::*,
    time::{cycles, wait_cycles},
};

/// Width and height of the addressable panel area, in pixels.
pub const SIZE: u16 = 240;
const CON: usize = 0x11d00;
const CS: u32 = 1 << 7;
const DC: u32 = 1 << 8;
const DONE: u32 = 1 << 15;
const ACK: u32 = 1 << 14;
const IRQ_ENABLE: u32 = 1 << 13;
const TIMEOUT_CYCLES: u32 = 50 * 24_000;
const CHUNK: usize = SIZE as usize * 2 * 8;

#[repr(align(4))]
struct Staging([u8; CHUNK]);
static mut DMA_BUFFER: Staging = Staging([0; CHUNK]);
unsafe extern "C" {
    fn fm1_spi1_dma(bytes: *const u8, len: u32);
}

/// SPI1 clock relative to the peripheral (LSB) clock inherited from the bootloader.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[repr(u32)]
pub enum SpiClock {
    /// LSB / 5, the original BSP setting.
    #[default]
    Div5 = 4,
    /// Full LSB clock, as used by DUB-1 for pixel transfers.
    Div1 = 0,
}

/// Exclusive SPI1 and display pins before ST7789 initialization.
#[derive(Debug)]
pub struct DisplayPort(pub(crate) Token);
/// An initialized panel. Dropping it waits for pending DMA. Use [`Display::flush`]
/// to observe a transfer error before releasing the handle.
#[derive(Debug)]
pub struct Display {
    spi: Spi1,
    visible: bool,
}

#[derive(Debug)]
struct Spi1 {
    _port: DisplayPort,
    failed: bool,
    pending_since: Option<u32>,
}

/// Display validation and transport errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The rectangle is outside the screen or its RGB565 byte length is incorrect.
    InvalidRegion,
    /// SPI1 failed to finish within 50 ms. The driver stops SPI1 until reset.
    Timeout,
}

impl DisplayPort {
    /// Configure PC7–PC10 and wake the ST7789. Uses TIMER4 for reset delays.
    ///
    /// # Errors
    /// Returns [`Error::Timeout`] if SPI1 fails to complete an initialization command.
    pub fn init(self) -> Result<Display, Error> {
        self.init_with_clock(SpiClock::default())
    }

    /// Wake the panel at the original clock, then select the drawing clock.
    ///
    /// # Errors
    /// Returns [`Error::Timeout`] if an initialization command stalls.
    pub fn init_with_clock(self, clock: SpiClock) -> Result<Display, Error> {
        let mut spi = self.start_spi();
        spi.command(0x01)?;
        wait_cycles(150 * 24_000);
        spi.command(0x11)?;
        wait_cycles(120 * 24_000);
        spi.command_data(0x3a, &[0x55])?; // RGB565
        spi.command_data(0x36, &[0])?; // RGB, origin at top left
        spi.command(0x21)?; // IPS inversion
        spi.command(0x13)?; // normal mode
        write(CON + 4, clock as u32);
        Ok(Display {
            spi,
            visible: false,
        })
    }

    fn start_spi(self) -> Spi1 {
        set(0x51020, 0x10);
        set(PC + OUT, CS);
        clear(PC + OUT, DC | (1 << 9) | (1 << 10));
        clear(PC + DIR, CS | DC | (1 << 9) | (1 << 10));
        write(CON, 0x4021);
        write(CON + 4, SpiClock::Div5 as u32);
        wait_cycles(100 * 24_000);
        Spi1 {
            _port: self,
            failed: false,
            pending_since: None,
        }
    }
}

impl Display {
    /// Draw a rectangle at (x, y) from exactly width × height × 2 row-major
    /// bytes. Empty in-bounds rectangles are no-ops. On timeout the screen
    /// can contain a partial update; subsequent transfers return `Timeout`.
    ///
    /// # Errors
    /// Returns [`Error::InvalidRegion`] for an out-of-bounds rectangle or a
    /// mismatched buffer length, and [`Error::Timeout`] on a stalled transfer.
    pub fn write_rect(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        pixels: &[u8],
    ) -> Result<(), Error> {
        self.write_rect_buffered(x, y, width, height, pixels)?;
        if pixels.is_empty() {
            return Ok(());
        }
        self.flush()
    }

    /// Write a rectangle, leaving its last DMA transfer in progress. All input bytes
    /// have been copied on return. Render into the caller's buffer while SPI1 sends
    /// the staged pixels. The next write waits before reusing the DMA buffer.
    /// Call [`Self::flush`] to wait for the last pixels and reveal the first image.
    ///
    /// # Errors
    /// Returns the same errors as [`Self::write_rect`]. A pending transfer can
    /// also report a timeout on the next write or flush.
    pub fn write_rect_buffered(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        pixels: &[u8],
    ) -> Result<(), Error> {
        self.validate_pixels(x, y, width, height, pixels)?;
        if !pixels.is_empty() {
            self.window(x, y, width, height)?;
            self.spi.write_bytes_buffered(pixels)?;
        }
        Ok(())
    }

    /// Fill one panel window with a native RGB565 color. Only one strip is
    /// prepared in RAM, then replayed by DMA as often as needed.
    ///
    /// # Errors
    /// Returns [`Error::InvalidRegion`] for an out-of-bounds window and
    /// [`Error::Timeout`] if SPI1 stalls.
    pub fn fill_rect(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        rgb565: u16,
    ) -> Result<(), Error> {
        let bytes = self.region_bytes(x, y, width, height)?;
        if bytes == 0 {
            return Ok(());
        }
        self.window(x, y, width, height)?;
        self.spi.repeat_rgb565(rgb565.to_be_bytes(), bytes / 2)?;
        self.flush()
    }

    /// Finish the last buffered rectangle. The first completed rectangle turns on the panel.
    ///
    /// # Errors
    /// Returns [`Error::Timeout`] if SPI1 stalls or a previous transfer failed.
    pub fn flush(&mut self) -> Result<(), Error> {
        if self.spi.failed {
            return Err(Error::Timeout);
        }
        if self.spi.pending_since.is_some() {
            self.spi.wait_dma()?;
            if !self.visible {
                self.spi.command(0x29)?;
                self.visible = true;
            }
        }
        Ok(())
    }

    fn validate_pixels(
        &self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        pixels: &[u8],
    ) -> Result<(), Error> {
        if self.region_bytes(x, y, width, height)? != pixels.len() {
            return Err(Error::InvalidRegion);
        }
        Ok(())
    }

    fn region_bytes(&self, x: u16, y: u16, width: u16, height: u16) -> Result<usize, Error> {
        if self.spi.failed {
            return Err(Error::Timeout);
        }
        region_bytes(x, y, width, height)
    }

    #[allow(
        clippy::arithmetic_side_effects,
        reason = "Both nonempty axes were validated against SIZE."
    )]
    fn window(&mut self, x: u16, y: u16, width: u16, height: u16) -> Result<(), Error> {
        self.flush()?;
        let endpoints = |start: u16, len: u16| {
            let [a, b] = start.to_be_bytes();
            let [c, d] = (start + len - 1).to_be_bytes();
            [a, b, c, d]
        };
        self.spi.command_data(0x2a, &endpoints(x, width))?;
        self.spi.command_data(0x2b, &endpoints(y, height))?;
        self.spi.command(0x2c)
    }
}

impl Spi1 {
    fn write_bytes_buffered(&mut self, bytes: &[u8]) -> Result<(), Error> {
        for chunk in bytes.chunks(CHUNK) {
            self.wait_dma()?;
            self.stage_bytes(chunk, false);
        }
        Ok(())
    }

    // Prepare one strip, replay it, and leave its final DMA in flight.
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "A strip holds at most CHUNK / 2 pixels, and each count is bounded by remaining."
    )]
    fn repeat_rgb565(&mut self, pixel: [u8; 2], mut remaining: usize) -> Result<(), Error> {
        self.wait_dma()?;
        let strip = remaining.min(CHUNK / 2);
        self.staging()[..strip * 2]
            .as_chunks_mut::<2>()
            .0
            .fill(pixel);
        while remaining != 0 {
            self.wait_dma()?;
            let count = remaining.min(strip);
            self.start_dma(count * 2, false);
            remaining -= count;
        }
        Ok(())
    }

    fn staging(&mut self) -> &mut [u8; CHUNK] {
        debug_assert!(!self.failed && self.pending_since.is_none());
        // SAFETY: the unique SPI1 transport owns this permanent SRAM allocation. Callers
        // complete any preceding transfer and stop on timeout before borrowing it.
        let buffer = unsafe { &raw mut DMA_BUFFER.0 };
        unsafe { &mut *buffer }
    }

    fn stage_bytes(&mut self, bytes: &[u8], interrupt: bool) {
        self.staging()[..bytes.len()].copy_from_slice(bytes);
        self.start_dma(bytes.len(), interrupt);
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "All callers pass 1..=CHUNK bytes."
    )]
    fn start_dma(&mut self, len: usize, interrupt: bool) {
        debug_assert!((1..=CHUNK).contains(&len));
        self.begin(true, interrupt);
        self.pending_since = Some(cycles());
        // SAFETY: DMA reads only initialized bytes from the transport's static SRAM.
        unsafe { fm1_spi1_dma((&raw const DMA_BUFFER.0).cast(), len as u32) };
    }

    fn wait_dma(&mut self) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Timeout);
        }
        if let Some(start) = self.pending_since.take() {
            self.finish(start)?;
        }
        Ok(())
    }

    fn begin(&mut self, data: bool, interrupt: bool) {
        level(PC + OUT, DC, data);
        clear(PC + OUT, CS);
        modify(
            CON,
            IRQ_ENABLE,
            ACK | if interrupt { IRQ_ENABLE } else { 0 },
        );
    }
    fn finish(&mut self, start: u32) -> Result<(), Error> {
        loop {
            let expired = cycles().wrapping_sub(start) >= TIMEOUT_CYCLES;
            if read(CON) & DONE != 0 {
                break;
            }
            if expired {
                clear(CON, 1);
                set(PC + OUT, CS);
                self.failed = true;
                return Err(Error::Timeout);
            }
        }
        set(CON, ACK);
        set(PC + OUT, CS);
        Ok(())
    }
    fn command(&mut self, command: u8) -> Result<(), Error> {
        self.wait_dma()?;
        self.begin(false, false);
        write(CON + 8, u32::from(command));
        self.finish(cycles())
    }
    fn command_data(&mut self, command: u8, data: &[u8]) -> Result<(), Error> {
        self.command(command)?;
        self.write_bytes_buffered(data)?;
        self.wait_dma()
    }
}
impl Drop for Display {
    fn drop(&mut self) {
        let _ = self.flush();
    }
}

#[allow(
    clippy::arithmetic_side_effects,
    reason = "Short-circuit bounds checks limit each dimension to 240 before multiplication."
)]
fn region_bytes(x: u16, y: u16, w: u16, h: u16) -> Result<usize, Error> {
    if x > SIZE || y > SIZE || w > SIZE - x || h > SIZE - y {
        return Err(Error::InvalidRegion);
    }
    Ok(usize::from(w) * usize::from(h) * 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_checked_before_io() {
        assert_eq!(region_bytes(0, 0, 240, 240), Ok(115200));
        assert_eq!(region_bytes(239, 239, 1, 1), Ok(2));
        assert_eq!(region_bytes(240, 240, 0, 0), Ok(0));
        for (x, y, w, h) in [(239, 239, 2, 1), (65535, 1, 2, 1), (0, 0, 65535, 65535)] {
            assert_eq!(region_bytes(x, y, w, h), Err(Error::InvalidRegion));
        }
    }
}
