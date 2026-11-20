// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
// Copyright (C) 2026 Kerem Kilic (Ellic Studio)
// AC791N ALNK0 initialization ported from felucca-bsp/boards/fm1/fm1_audio.h.
//! Polled stereo I²S output on PC0, PC1, PC2 and PC6. The DMA cycles through
//! two fixed SRAM halves. Service it about every 5.8 ms to keep both filled.

use crate::{
    Token,
    registers::{self, DIE, DIR, OUT, PC},
    time::{cycles, wait_cycles},
};

const FRAMES_PER_HALF: u16 = 256;

/// Frames in one DMA half.
pub const BLOCK_FRAMES: usize = FRAMES_PER_HALF as usize;
/// A block of left/right signed Q23 samples. Values outside the range are saturated on copy.
pub type Block = [[i32; 2]; BLOCK_FRAMES];
/// Physical frame rate in hertz as a numerator and denominator.
pub const FRAME_RATE: (u32, u32) = (24_000_000, 544);

const CON0: usize = 0x12e00;
const CON1: usize = CON0 + 4;
const STATUS: usize = CON0 + 8;
const CON3: usize = CON0 + 12;
const ADDR3: usize = CON0 + 0x1c;
const LEN: usize = CON0 + 0x20;
const RUN: u16 = 1 << 11;
const DATA: u32 = 1 << 6;

static mut DMA: [Block; 2] = [[[0; 2]; BLOCK_FRAMES]; 2];

#[cfg(not(test))]
fn read16(addr: usize) -> u16 {
    unsafe { core::ptr::read_volatile(addr as *const u16) }
}
#[cfg(not(test))]
fn write16(addr: usize, value: u16) {
    unsafe { core::ptr::write_volatile(addr as *mut u16, value) }
}
#[cfg(not(test))]
fn pending() -> u8 {
    unsafe { core::ptr::read_volatile(STATUS as *const u8) }
}
#[cfg(not(test))]
fn write8(addr: usize, value: u8) {
    unsafe { core::ptr::write_volatile(addr as *mut u8, value) }
}

#[cfg(test)]
fn read16(addr: usize) -> u16 {
    u16::try_from(registers::read(addr)).expect("16-bit mock register")
}
#[cfg(test)]
fn write16(addr: usize, value: u16) {
    registers::write(addr, value.into());
}
#[cfg(test)]
fn pending() -> u8 {
    u8::try_from(registers::read(STATUS)).expect("8-bit mock register")
}
#[cfg(test)]
fn write8(addr: usize, value: u8) {
    registers::write(addr, value.into());
}

fn sync() {
    #[cfg(not(test))]
    {
        unsafe extern "C" {
            fn fm1_csync();
        }
        unsafe { fm1_csync() };
    }
}

fn pin(mask: u32, high: bool) {
    registers::set(PC + DIE, mask);
    registers::level(PC + OUT, mask, high);
    registers::clear(PC + DIR, mask);
}

/// ALNK0 and the four codec pins. Starts only on explicit request.
#[derive(Debug)]
pub struct AudioPort(pub(crate) Token);

/// Conditions under which the stream cannot start.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum StartError {
    /// The audio PLL has a configuration outside the FM-1's characterized stock settings.
    Clock,
    /// ALNK0 DMA is already running.
    Busy,
}

impl AudioPort {
    /// Start both halves with silence. Leaves the CPU clock and shared PLL alone.
    ///
    /// # Errors
    /// Returns [`StartError::Busy`] if DMA is running, or [`StartError::Clock`]
    /// if the PLL settings differ from those used by the FM-1.
    pub fn start(&mut self) -> Result<Stream<'_>, StartError> {
        if read16(CON0) & RUN != 0 {
            return Err(StartError::Busy);
        }
        if registers::read(0x119a0) != 0x4540_0203
            || registers::read(0x119a4) & 0x07ff_ffff != 0x0750_3026
        {
            return Err(StartError::Clock);
        }
        // Polled operation. TIMER5, the BSP's only ISR, never accesses ALNK0 or port C.
        let dma = (&raw mut DMA).cast::<i32>();
        for i in 0..BLOCK_FRAMES * 4 {
            // SAFETY: DMA is stopped and this port exclusively owns the static array.
            unsafe { dma.add(i).write_volatile(0) };
        }
        pin(DATA, false);
        wait_cycles(5 * 24_000);
        write16(CON0, 0);
        write16(CON1, 0);
        write8(STATUS, 0);
        write8(CON3, 0);
        registers::clear(0x51030, 0xc0); // ALNK0 group on port C
        write8(CON3, 3);
        pin(1, true); // MCLK
        write16(CON0, 0x100);
        pin(1 << 2, true); // LRCK
        pin(1 << 1, true); // BCLK
        write16(CON0, 0x180);
        write16(LEN, FRAMES_PER_HALF * 2); // words per half
        registers::clear(0x10014, 0x0f00); // stock 44.1 kHz clock family
        write8(CON3, 0x83);
        write16(CON1, 0x5000); // channel 3 transmit, 24-bit samples
        registers::write(ADDR3, registers::bus_address(dma));
        pin(DATA, true);
        write8(STATUS, 0x0f);
        sync();
        write16(CON0, 0x180 | RUN);
        wait_cycles(5 * 24_000);
        pin(DATA, true);
        Ok(Stream { _port: self })
    }
}

/// A running stereo output. Dropping it stops DMA and drives the data line low.
/// Missed refill deadlines can repeat old samples.
#[derive(Debug)]
pub struct Stream<'a> {
    _port: &'a mut AudioPort,
}

/// Whether DMA accepted a block and whether it crossed a half-buffer boundary during the copy.
#[derive(Debug, PartialEq, Eq, Copy, Clone)]
pub enum Refill {
    /// DMA has not released a half since the last submission.
    WouldBlock,
    /// The complete block was copied and published before the next observed boundary.
    Written,
    /// The DMA boundary moved during the copy. The DAC may have consumed a partial block.
    Late,
}

impl Stream<'_> {
    /// Try to submit one pre-rendered block. Never hands out references to DMA-owned memory.
    pub fn try_write(&mut self, input: &Block) -> Refill {
        if pending() & 0x80 == 0 {
            return Refill::WouldBlock;
        }
        let start = cycles();
        let active = read16(CON0) >> 15;
        let free = usize::from(active ^ 1);
        // SAFETY: the port exclusively owns the array; its two fixed-size halves are
        // in SRAM for the life of the program. DMA only reads it. CPU writes are
        // full, aligned volatile sample words, including when firmware misses a deadline.
        let dst = unsafe { (&raw mut DMA).cast::<Block>().add(free).cast::<[i32; 2]>() };
        for (i, frame) in input.iter().enumerate() {
            for (ch, sample) in frame.iter().enumerate() {
                unsafe {
                    dst.add(i)
                        .cast::<i32>()
                        .add(ch)
                        .write_volatile((*sample).clamp(-8_388_608, 8_388_607))
                };
            }
        }
        sync();
        let late = read16(CON0) >> 15 != active
            || cycles().wrapping_sub(start) >= 544 * u32::from(FRAMES_PER_HALF);
        write8(STATUS, 0x08);
        if late { Refill::Late } else { Refill::Written }
    }
}

impl Drop for Stream<'_> {
    fn drop(&mut self) {
        write16(CON0, read16(CON0) & !RUN);
        sync();
        write8(STATUS, 0x0f);
        pin(DATA, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn refuses_unknown_clocks_and_refills_only_the_free_half() {
        let mut audio = AudioPort(Token::new());
        assert!(matches!(audio.start(), Err(StartError::Clock)));
        assert_eq!(read16(CON0), 0);
        registers::write(0x119a0, 0x4540_0203);
        registers::write(0x119a4, 0x3f50_3026);
        write16(CON0, RUN);
        assert!(matches!(audio.start(), Err(StartError::Busy)));
        write16(CON0, 0);
        let mut stream = audio.start().expect("valid PLL with idle DMA");
        assert_eq!(read16(LEN), 512);
        let block = [[i32::MIN, i32::MAX]; BLOCK_FRAMES];
        assert_eq!(stream.try_write(&block), Refill::WouldBlock);
        registers::write(STATUS, 0x80);
        assert_eq!(stream.try_write(&block), Refill::Written);
        // The mock DMA is idle. Check every frame in both halves.
        let ptr = (&raw const DMA).cast::<[i32; 2]>();
        for i in 0..BLOCK_FRAMES {
            unsafe {
                assert_eq!(ptr.add(i).read_volatile(), [0; 2]);
                assert_eq!(
                    ptr.add(BLOCK_FRAMES + i).read_volatile(),
                    [-8_388_608, 8_388_607]
                );
            }
        }
        drop(stream);
        assert_eq!(read16(CON0) & RUN, 0);
        assert_eq!(registers::read(PC + OUT) & DATA, 0);
        assert!(audio.start().is_ok());
    }
}
