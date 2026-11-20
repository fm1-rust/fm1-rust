// SPDX-FileCopyrightText: 2026 Julius Vitkauskas
// SPDX-License-Identifier: MIT-0
//! The 240 × 240 ST7789 on SPI1. Transfers block and accept RGB565 bytes in
//! wire order (most significant byte first). Frames can live in flash.

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
const CHUNK: usize = SIZE as usize * 2 * 8;

#[repr(align(4))]
struct Staging([u8; CHUNK]);
static mut DMA_BUFFER: Staging = Staging([0; CHUNK]);
unsafe extern "C" {
    fn fm1_spi1_dma(bytes: *const u8, len: u32);
}

/// Exclusive SPI1 and display pins before ST7789 initialization.
#[derive(Debug)]
pub struct DisplayPort(pub(crate) Token);
/// An initialized panel.
#[derive(Debug)]
pub struct Display {
    _port: DisplayPort,
    failed: bool,
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
        set(0x51020, 0x10);
        set(PC + OUT, CS);
        clear(PC + OUT, DC | (1 << 9) | (1 << 10));
        clear(PC + DIR, CS | DC | (1 << 9) | (1 << 10));
        write(CON, 0x4021);
        write(CON + 4, 4);
        let mut lcd = Display {
            _port: self,
            failed: false,
        };
        wait_cycles(100 * 24_000);
        lcd.command(0x01)?;
        wait_cycles(150 * 24_000);
        lcd.command(0x11)?;
        wait_cycles(120 * 24_000);
        lcd.command_data(0x3a, &[0x55])?; // RGB565
        lcd.command_data(0x36, &[0])?; // RGB, origin at top left
        lcd.command(0x21)?; // IPS inversion
        lcd.command(0x13)?; // normal mode
        Ok(lcd)
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
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "Validation bounds both nonempty ranges to 0..240."
    )]
    pub fn write_rect(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        pixels: &[u8],
    ) -> Result<(), Error> {
        if self.failed {
            return Err(Error::Timeout);
        }
        validate(x, y, width, height, pixels.len())?;
        if pixels.is_empty() {
            return Ok(());
        }
        let window = |start: u16, len: u16| {
            let [a, b] = start.to_be_bytes();
            let [c, d] = (start + len - 1).to_be_bytes();
            [a, b, c, d]
        };
        self.command_data(0x2a, &window(x, width))?;
        self.command_data(0x2b, &window(y, height))?;
        self.command_data(0x2c, pixels)?;
        self.command(0x29)
    }

    fn begin(&mut self, data: bool) {
        level(PC + OUT, DC, data);
        clear(PC + OUT, CS);
        set(CON, ACK);
    }
    fn finish(&mut self) -> Result<(), Error> {
        let start = cycles();
        while read(CON) & DONE == 0 {
            if cycles().wrapping_sub(start) >= 50 * 24_000 {
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
        self.begin(false);
        write(CON + 8, u32::from(command));
        self.finish()
    }
    fn command_data(&mut self, command: u8, data: &[u8]) -> Result<(), Error> {
        self.command(command)?;
        for chunk in data.chunks(CHUNK) {
            // SAFETY: the single Display owns SPI1 and this static SRAM buffer.
            // A completed transfer releases it. A timeout permanently faults
            // the handle, so neither the hardware nor an error path can race a
            // later copy. The buffer remains allocated even if Display is dropped.
            let buffer = unsafe { (&raw mut DMA_BUFFER.0).cast::<u8>() };
            unsafe { core::ptr::copy_nonoverlapping(chunk.as_ptr(), buffer, chunk.len()) };
            self.begin(true);
            #[allow(
                clippy::cast_possible_truncation,
                reason = "A chunk contains at most 3,840 bytes."
            )]
            unsafe {
                fm1_spi1_dma(buffer, chunk.len() as u32)
            };
            self.finish()?;
        }
        Ok(())
    }
}
#[allow(
    clippy::arithmetic_side_effects,
    reason = "Short-circuit bounds checks limit each dimension to 240 before multiplication."
)]
fn validate(x: u16, y: u16, w: u16, h: u16, bytes: usize) -> Result<(), Error> {
    if x > SIZE
        || y > SIZE
        || w > SIZE - x
        || h > SIZE - y
        || bytes != usize::from(w) * usize::from(h) * 2
    {
        return Err(Error::InvalidRegion);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_checked_before_io() {
        assert_eq!(validate(0, 0, 240, 240, 115200), Ok(()));
        assert_eq!(validate(239, 239, 1, 1, 2), Ok(()));
        assert_eq!(validate(240, 240, 0, 0, 0), Ok(()));
        for (x, y, w, h, n) in [
            (239, 239, 2, 1, 4),
            (0, 0, 1, 1, 1),
            (0, 0, 1, 1, 3),
            (65535, 1, 2, 1, 4),
            (0, 0, 65535, 65535, 0),
        ] {
            assert_eq!(validate(x, y, w, h, n), Err(Error::InvalidRegion));
        }
    }
}
