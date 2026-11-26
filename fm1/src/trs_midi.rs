// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
//! Byte input from the FM-1's TRS MIDI jack on PH8 (UART1, 31,250 baud).
//! The board wiring and PLL setup follow Felucca's `chip/fm1_uart.h`.

use crate::{Board, Token, registers, time};
use core::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};
use embedded_hal_nb::{nb, serial};

#[cfg(feature = "midly")]
mod events;
#[cfg(feature = "midly")]
pub use events::{MidlyRx, midly};
#[cfg(feature = "embedded-io")]
mod blocking;
#[cfg(feature = "embedded-io")]
pub use blocking::Blocking;

const CON: usize = 0x12100;
const RX_COUNT: usize = CON + 0x24;
const RING: u32 = 1 << u8::BITS;
const WATERMARK: u32 = 8;
const READ_CHUNK: usize = 32;
const RX_PENDING: u32 = 1 << 16; // Packed alongside HRXCNT by capture().
static CLAIMED: AtomicBool = AtomicBool::new(false);

/// A discontinuity in the incoming serial stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxError {
    /// Unread bytes may have been overwritten or the receive count was
    /// ambiguous. The receiver discards the backlog and restarts at a known
    /// position. Discard the parser's running status and any held notes.
    Overrun,
}

impl fmt::Display for RxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TRS MIDI receive overrun")
    }
}
impl core::error::Error for RxError {}

impl serial::Error for RxError {
    fn kind(&self) -> serial::ErrorKind {
        serial::ErrorKind::Overrun
    }
}

#[derive(Debug)]
struct Stamp {
    cycles: u32,
    ticks: u32,
}

impl Stamp {
    fn now() -> Self {
        Self {
            cycles: time::cycles(),
            ticks: time::ticks(),
        }
    }

    fn count_may_have_wrapped(&self, now: &Self) -> bool {
        // A 16-bit count wraps after ~21 s at full wire rate. The coarse
        // clock also covers absences spanning an entire TIMER4 revolution.
        now.cycles.wrapping_sub(self.cycles) >= 16 * time::Timers::CRYSTAL_HZ
            || now.ticks.wrapping_sub(self.ticks) >= 16 * time::Timers::TICK_HZ
    }
}

/// The exclusive CPU0 reader for UART1. Input goes into permanent BSP SRAM.
#[derive(Debug)]
pub struct TrsMidiRx {
    _core: Token,
    cursor: u8,
    available: u32,
    checked: Stamp,
}

impl Board {
    /// Start the TRS MIDI input and claim it until its handle is dropped.
    /// UART1 receives independently of the USB interface.
    #[must_use]
    pub fn take_trs_midi(&self) -> Option<TrsMidiRx> {
        if CLAIMED.swap(true, Ordering::Acquire) {
            return None;
        }
        use registers::{PH, modify, write};
        write(CON, 0x3400); // Disable UART1 and clear the pending flags.
        write(CON + 4, 0); // No flow control.
        modify(0x10010, 3 << 10, 1 << 10); // PLL48M UART clock.
        modify(0x51028, 0xf0, 5 << 4); // UART1 RX from input channel 1.
        modify(0x51024, 0x3f00, 49 << 8); // Input channel 1 from PH8.
        registers::pull_up_inputs(PH, 1 << 8);
        write(CON + 8, 383); // 48 MHz / (4 * 31,250) - 1.
        write(CON + 0x10, 60_000); // Timeout after 1.25 ms of silence.
        let mut rx = TrsMidiRx {
            _core: Token::new(),
            cursor: 0,
            available: 0,
            checked: Stamp::now(),
        };
        rx.restart();
        Some(rx)
    }
}

impl TrsMidiRx {
    fn restart(&mut self) {
        use registers::{csync, write};
        write(CON, 0x3400);
        csync();
        hw::set_ring();
        write(RX_COUNT, WATERMARK);
        write(CON, 0x14c0); // RX DMA, reset count, clear RX and timeout flags.
        csync();
        write(CON, 0x41); // Enable last, with UART interrupts masked.
        self.cursor = 0;
        self.available = 0;
        self.checked = Stamp::now();
    }

    fn collect(&mut self) -> Result<(), RxError> {
        let before = Stamp::now();
        let count = hw::capture();
        let bytes = count & 0xffff;
        let watermark = count & RX_PENDING != 0;
        let uncertain =
            watermark && (bytes < WATERMARK || self.checked.count_may_have_wrapped(&Stamp::now()));
        self.checked = before;
        self.available = self.available.saturating_add(bytes);
        // Up to WATERMARK-1 newer bytes can still be in DMA without either
        // pending flag. Leave space for all of them, including during a copy.
        if uncertain || self.available > RING - (WATERMARK - 1) {
            self.restart();
            return Err(RxError::Overrun);
        }
        Ok(())
    }

    /// Copy up to 32 available bytes in wire order. `Ok(0)` means that the
    /// destination is empty or there is no completed batch. The ring holds
    /// about 80 ms of continuous traffic. If a custom runtime does not
    /// advance the BSP's TIMER5 clock, poll at least once per TIMER4 cycle
    /// (about 179 s) for wrap detection, including when the line is idle.
    ///
    /// # Errors
    /// Returns [`RxError::Overrun`] before publishing bytes across a gap.
    /// The destination is unchanged on error.
    pub fn try_read(&mut self, out: &mut [u8]) -> Result<usize, RxError> {
        if out.is_empty() {
            return Ok(0);
        }
        self.collect()?;
        let n = out.len().min(READ_CHUNK).min(self.available as usize);
        if n != 0 {
            let mut stage = [0; READ_CHUNK];
            hw::copy(self.cursor, &mut stage[..n]);
            // A higher-priority task can preempt the copy long enough to lap
            // our original cursor. Retain that cursor until validation ends.
            self.collect()?;
            out[..n].copy_from_slice(&stage[..n]);
            #[allow(
                clippy::cast_possible_truncation,
                reason = "n is at most READ_CHUNK (32)"
            )]
            let taken = n as u8;
            self.cursor = self.cursor.wrapping_add(taken);
            self.available = self.available.saturating_sub(u32::from(taken));
        }
        Ok(n)
    }

    /// Decode live MIDI events with a small fixed buffer.
    #[cfg(feature = "midly")]
    #[must_use]
    pub fn into_midly(self) -> MidlyRx {
        MidlyRx::new(self)
    }

    /// Borrow the receiver for blocking `embedded_io::Read`.
    #[cfg(feature = "embedded-io")]
    #[must_use]
    pub fn blocking(&mut self) -> Blocking<'_> {
        Blocking(self)
    }
}

impl serial::ErrorType for TrsMidiRx {
    type Error = RxError;
}

impl serial::Read<u8> for TrsMidiRx {
    fn read(&mut self) -> nb::Result<u8, RxError> {
        let mut byte = [0];
        match self.try_read(&mut byte)? {
            0 => Err(nb::Error::WouldBlock),
            _ => Ok(byte[0]),
        }
    }
}

impl Drop for TrsMidiRx {
    fn drop(&mut self) {
        stop();
        CLAIMED.store(false, Ordering::Release);
    }
}

pub(crate) fn stop() {
    registers::write(CON, 0x3400);
    registers::csync();
}

#[cfg(fm1)]
mod hw {
    unsafe extern "C" {
        safe fn fm1_trs_rx_set_ring();
        safe fn fm1_trs_rx_capture() -> u32;
        fn fm1_trs_rx_copy(start: u8, out: *mut u8, len: usize);
    }
    pub(super) fn set_ring() {
        fm1_trs_rx_set_ring();
    }
    pub(super) fn capture() -> u32 {
        fm1_trs_rx_capture()
    }
    pub(super) fn copy(start: u8, out: &mut [u8]) {
        // SAFETY: the C shim issues one volatile byte load per ring slot and
        // bounds every index to 0..256. DMA can only write that permanent,
        // disjoint allocation. The output slice is exclusive for this call.
        unsafe { fm1_trs_rx_copy(start, out.as_mut_ptr(), out.len()) }
    }
}

#[cfg(all(not(fm1), not(test)))]
mod hw {
    // A host cannot acquire a Board.
    pub(super) fn set_ring() {}
    pub(super) fn capture() -> u32 {
        0
    }
    pub(super) fn copy(_: u8, _: &mut [u8]) {}
}

#[cfg(test)]
mod tests;
#[cfg(test)]
use tests as hw;
