// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
//! AC791N USB0 device PHY, indexed MUSB registers and endpoint DMA.
//! The CPU0 USB handle serializes all SIE and DMA accesses.

use crate::{
    registers::{bus_address, clear, read, set, write},
    time::{Timers, cycles},
};

const CON0: usize = 0x11800;
const CON1: usize = 0x11804;
const EP0_CNT: usize = 0x11808;
const EP1_CNT: usize = 0x1180c;
const EP0_ADR: usize = 0x11818;
const EP1_TX: usize = 0x1181c;
const EP1_RX: usize = 0x11820;
const IO: usize = 0x51000;
const CLOCK: usize = 0x10010;

pub(super) const FADDR: u8 = 0;
pub(super) const INTR_TX: u8 = 2;
pub(super) const INTR_RX: u8 = 4;
pub(super) const INTR_USB: u8 = 6;
const POWER: u8 = 1;
const TX_ENABLE: u8 = 7;
const RX_ENABLE: u8 = 9;
const USB_ENABLE: u8 = 11;
const FRAME: u8 = 12;
pub(super) const INDEX: u8 = 14;
pub(super) const CSR0: u8 = 17;
const TXCSR: u8 = 17;
const RXCSR: u8 = 20;
const TX_MAX: u8 = 16;
const RX_MAX: u8 = 19;
pub(super) const COUNT: u8 = 22;
pub(super) const MAX_PACKET: usize = 64;

#[repr(align(4))]
struct Buffer<const N: usize>([u8; N]);
// The AC79 SDK reserves one extra word after every receive buffer.
static mut CONTROL: Buffer<68> = Buffer([0; 68]);
static mut TX: Buffer<64> = Buffer([0; 64]);
static mut RX: Buffer<68> = Buffer([0; 68]);

#[derive(Debug)]
pub(super) struct Timeout;
pub(super) type Result<T> = core::result::Result<T, Timeout>;

fn sync_tx() {
    #[cfg(fm1)]
    unsafe {
        unsafe extern "C" {
            fn fm1_csync();
        }
        fm1_csync();
    }
}

fn sync_rx() {
    #[cfg(fm1)]
    unsafe {
        unsafe extern "C" {
            fn fm1_ssync();
        }
        fm1_ssync();
    }
}

pub(super) fn detach() {
    write(CON0, 0);
    write(CON1, 0);
    write(IO, 0x0c);
}

pub(super) fn attach() -> Result<()> {
    clear(CLOCK, 3); // USB clock comes from the 48 MHz PLL.
    write(EP0_ADR, bus_address(&raw mut CONTROL));
    for reg in [EP1_RX, 0x11828, 0x11830, 0x1183c] {
        write(reg, bus_address(&raw mut RX));
    }
    set(IO, (1 << 10) | (1 << 9));
    clear(IO, 1 << 11);
    set(IO, 1 << 12);
    clear(CON0, 0x4035);
    write(CON1, 0);
    set(CON0, 1);
    clear(IO, 0xf0);
    set(IO, 0x40); // Full-speed attach on D+.
    set(CON0, 0x3c);
    wr(POWER, 0x60)?;
    wr(TX_ENABLE + 1, 0)?;
    wr(RX_ENABLE + 1, 0)?;
    reset()
}

pub(super) fn reset() -> Result<()> {
    write(EP0_ADR, bus_address(&raw mut CONTROL));
    // Only EP0 participates until SET_CONFIGURATION.
    set(CON0, 0x1e << 19);
    clear(CON0, 1 << 19);
    wr(FADDR, 0)?;
    wr(USB_ENABLE, 7)?; // suspend, resume and reset
    wr(TX_ENABLE, 1)?; // EP0
    wr(RX_ENABLE, 0)
}

pub(super) fn configure(enabled: bool) -> Result<()> {
    if !enabled {
        set(CON0, 1 << 20);
        return wr(RX_ENABLE, 0);
    }
    write(EP1_TX, bus_address(&raw mut TX));
    write(EP1_RX, bus_address(&raw mut RX));
    wr(INDEX, 1)?;
    wr(TX_MAX, 0xff)?; // AC79 maximum packet register, as in the SDK.
    wr(TXCSR, 0x48)?; // Flush FIFO and reset data toggle.
    wr(TXCSR + 1, 0)?;
    wr(RX_MAX, 0xff)?;
    wr(RXCSR, 0x90)?;
    wr(RXCSR + 1, 0)?;
    wr(RX_ENABLE, 2)?;
    clear(CON0, 1 << 20);
    Ok(())
}

// Keep this transaction out of line. A powered FM-1 without a data host can
// leave DONE clear indefinitely. Bound one transaction, then back off.
#[inline(never)]
fn transaction(command: u32) -> Result<u8> {
    let start = cycles();
    write(CON1, command);
    sync_tx();
    loop {
        let value = read(CON1);
        if value & 0x8000 != 0 {
            return Ok(value.to_le_bytes()[0]);
        }
        if cycles().wrapping_sub(start) >= Timers::CRYSTAL_HZ / 10_000 {
            return Err(Timeout);
        }
    }
}

pub(super) fn rd(reg: u8) -> Result<u8> {
    transaction((u32::from(reg) << 8) | 0x4000)
}
pub(super) fn wr(reg: u8, val: u8) -> Result<()> {
    transaction((u32::from(reg) << 8) | u32::from(val)).map(|_| ())
}

pub(super) fn frame_number() -> Result<u16> {
    Ok(u16::from_le_bytes([rd(FRAME)?, rd(FRAME + 1)? & 7]))
}

pub(super) fn halt(input: bool, halted: bool) -> Result<()> {
    wr(INDEX, 1)?;
    // SENDSTALL, or flush the endpoint and reset its toggle to DATA0.
    let (reg, value) = match (input, halted) {
        (true, true) => (TXCSR, 0x10),
        (true, false) => (TXCSR, 0x48),
        (false, true) => (RXCSR, 0x20),
        (false, false) => (RXCSR, 0x90),
    };
    wr(reg, value)
}

pub(super) fn setup() -> [u8; 8] {
    sync_rx();
    // SAFETY: an 8-byte setup packet is ready. DMA waits for ServicedRxPktRdy.
    core::array::from_fn(|i| unsafe { (&raw mut CONTROL.0).cast::<u8>().add(i).read_volatile() })
}

pub(super) fn control_in(bytes: &[u8], last: bool) -> Result<()> {
    // SAFETY: USB0 owns CONTROL, and the previous IN packet has completed.
    unsafe { publish((&raw mut CONTROL).cast(), EP0_ADR, EP0_CNT, bytes) };
    wr(CSR0, if last { 0x0a } else { 0x02 })
}

pub(super) fn receive(events: &mut super::PacketBuffer) -> Result<usize> {
    wr(INDEX, 1)?;
    let csr = rd(RXCSR)?;
    if csr & 1 == 0 {
        return Ok(0);
    }
    let count = u16::from_le_bytes([rd(COUNT)?, rd(COUNT + 1)?]);
    let n = usize::from(count).min(MAX_PACKET) / 4;
    sync_rx();
    for (i, b) in events[..n].as_flattened_mut().iter_mut().enumerate() {
        // SAFETY: RXPKTRDY holds the DMA buffer until the acknowledgement below.
        *b = unsafe { (&raw mut RX.0).cast::<u8>().add(i).read_volatile() };
    }
    wr(RXCSR, (csr & !0x64) | 0x10)?;
    wr(RXCSR + 1, 0)?;
    Ok(n)
}

pub(super) fn discard_rx() -> Result<()> {
    wr(INDEX, 1)?;
    wr(RXCSR, 0x10) // FlushFIFO; keep the host's data toggle.
}

pub(super) fn send(events: &[super::EventPacket]) -> Result<usize> {
    let n = events.len().min(super::MAX_EVENTS);
    if n == 0 {
        return Ok(0);
    }
    wr(INDEX, 1)?;
    let csr = rd(TXCSR)?;
    if csr & 1 != 0 {
        return Ok(0);
    }
    // SAFETY: USB0 owns TX and TXPKTRDY is clear until the send below.
    unsafe {
        publish(
            (&raw mut TX).cast(),
            EP1_TX,
            EP1_CNT,
            events[..n].as_flattened(),
        )
    };
    wr(TXCSR, csr | 1)?;
    Ok(n)
}

// The caller owns at least MAX_PACKET bytes of writable, idle, aligned DMA memory.
unsafe fn publish(buffer: *mut u8, address_reg: usize, count_reg: usize, bytes: &[u8]) {
    assert!(bytes.len() <= MAX_PACKET);
    for (i, &byte) in bytes.iter().enumerate() {
        unsafe { buffer.add(i).write_volatile(byte) };
    }
    sync_tx();
    write(address_reg, bus_address(buffer));
    write(
        count_reg,
        u32::try_from(bytes.len()).expect("one USB packet fits"),
    );
}
