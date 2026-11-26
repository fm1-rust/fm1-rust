// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
//! AC791N USB0 device PHY, indexed MUSB registers and endpoint DMA.
//! The CPU0 USB handle serializes all SIE and DMA accesses.

pub(super) use crate::registers::csync as sync_tx;
use crate::{
    registers::{bus_address, clear, read, set, write},
    time::{Timers, cycles},
};

pub(super) const CON0: usize = 0x11800;
const CON1: usize = 0x11804;
const EP0_CNT: usize = 0x11808;
const EP1_CNT: usize = 0x1180c;
const EP0_ADR: usize = 0x11818;
const EP1_TX: usize = 0x1181c;
const EP1_RX: usize = 0x11820;
#[cfg(feature = "usb-audio-capture")]
const EP4_CNT: usize = 0x11834;
#[cfg(feature = "usb-audio-capture")]
const EP4_TX: usize = 0x11838;
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
pub(super) const SUSPEND: u8 = 1;
pub(super) const RESUME: u8 = 1 << 1;
pub(super) const RESET: u8 = 1 << 2;
pub(super) const DISCONNECT: u8 = 1 << 5;
const FRAME: u8 = 12;
pub(super) const INDEX: u8 = 14;
pub(super) const CSR0: u8 = 17;
const TXCSR: u8 = 17;
const RXCSR: u8 = 20;
const TX_MAX: u8 = 16;
const RX_MAX: u8 = 19;
pub(super) const COUNT: u8 = 22;
pub(super) const MAX_PACKET: usize = 64;
#[cfg(feature = "usb-audio-capture")]
static mut AUDIO_TX: Buffer<192> = Buffer([0; 192]);

#[cfg(feature = "usb-audio-capture")]
pub(super) fn capture_address() -> *mut u8 {
    (&raw mut AUDIO_TX).cast()
}
// Capacity of each normal USB0 receive destination, independent of the MIDI
// payload size. At RX_MAX=0xff the black FM-1 changed EP1 offsets 0..=65 for
// a legal transfer and reached offset 1023 under the oversized-packet probe.
// 2 KiB is the tested staging allocation. See UsbPort::assume_midi_dma for the
// separate platform obligation on the maximum DMA write before Rust can poll.
pub(super) const RX_DMA_BYTES: usize = 2048;
// Between targeted trials, stray EP1 traffic must also have a sacrificial destination.
#[cfg(feature = "usb-dma-stress")]
const STAGING: usize = super::dma_probe::BYTES;
#[cfg(not(feature = "usb-dma-stress"))]
const STAGING: usize = RX_DMA_BYTES;

// The controller receives the wrapper address and the CPU accesses its first field.
#[repr(C, align(8))]
#[cfg_attr(feature = "usb-midi-copy", repr(align(32)))]
struct Buffer<const N: usize>([u8; N]);
// Keep the first forbidden word in reserved RAM. The controller receives the
// address of data; its 2 KiB production budget excludes the trailing guard.
#[repr(C, align(8))]
#[cfg_attr(feature = "usb-midi-copy", repr(align(32)))]
struct RxStorage {
    data: [u8; STAGING],
    guard: [u32; 4],
}
impl RxStorage {
    const EMPTY: Self = Self {
        data: [0; STAGING],
        guard: [0; 4],
    };
}
const _: () = assert!(core::mem::offset_of!(RxStorage, guard) == STAGING);
pub(super) const RX_GUARD: [u32; 4] = [0x13a5_f96c, 0xc82e_715b, 0x6d03_b894, 0xfa57_20de];
static GUARDS_SEEDED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
// EP0 shares its RX and TX address. On the black board an eight-byte SETUP
// also stored its two CRC bytes. Both endpoints still need a general bound.
static mut CONTROL: RxStorage = RxStorage::EMPTY;
#[cfg(feature = "usb-midi-copy")]
const TX_STORAGE: usize = super::copied::BLOCK;
#[cfg(not(feature = "usb-midi-copy"))]
const TX_STORAGE: usize = MAX_PACKET;
static mut TX: Buffer<TX_STORAGE> = Buffer([0; TX_STORAGE]);
static mut RX: RxStorage = RxStorage::EMPTY;

#[derive(Clone, Copy)]
pub(super) enum Endpoint {
    Control,
    Out,
    In,
}

#[derive(Debug)]
pub(super) enum Buffers {
    Direct,
    #[cfg(feature = "usb-midi-copy")]
    Copied(super::copied::Engine),
}

impl Endpoint {
    pub(super) fn address(self) -> *mut u8 {
        match self {
            Self::Control => (&raw mut CONTROL).cast(),
            Self::Out => RxBuffer::Normal.ptr(),
            Self::In => (&raw mut TX).cast(),
        }
    }
}

impl Buffers {
    pub(super) fn init(&mut self) {
        #[cfg(feature = "usb-midi-copy")]
        {
            // A detached USB master can retain its previous address. No code
            // may reintroduce CPU accesses to endpoint storage later in a
            // copied boot, including through a legacy diagnostic constructor.
            static MODE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
            let which = match self {
                Self::Direct => 1,
                Self::Copied(_) => 2,
            };
            if !select_mode(&MODE, which) {
                rx_fault("restart before changing USB DMA mode", usize::from(which));
            }
        }
        match self {
            Self::Direct => init_rx_guards(),
            #[cfg(feature = "usb-midi-copy")]
            Self::Copied(engine) => engine.start(),
        }
    }
    pub(super) fn check(&mut self) {
        match self {
            Self::Direct => check_rx_guards(),
            #[cfg(feature = "usb-midi-copy")]
            Self::Copied(engine) => engine.guards(),
        }
    }
}

#[cfg(feature = "usb-midi-copy")]
fn select_mode(mode: &core::sync::atomic::AtomicU8, desired: u8) -> bool {
    use core::sync::atomic::Ordering;
    match mode.compare_exchange(0, desired, Ordering::AcqRel, Ordering::Acquire) {
        Ok(_) => true,
        Err(previous) => previous == desired,
    }
}

pub(super) type Result<T> = core::result::Result<T, super::RegisterTimeout>;

pub(super) fn sync_rx() {
    #[cfg(fm1)]
    // SAFETY: the target shim executes SSYNC. Bus ownership and the device's
    // write bounds are established separately before the CPU accesses SRAM.
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

fn guards() -> [(&'static str, *mut [u32; 4]); 2] {
    // SAFETY: project raw pointers to permanent fields without borrowing or
    // reading the payloads, which USB0 may own.
    unsafe {
        [
            ("EP0 changed trailing guard word", &raw mut CONTROL.guard),
            ("EP1 changed trailing guard word", &raw mut RX.guard),
        ]
    }
}

pub(super) fn init_rx_guards() {
    // UsbPort serializes owners. Preserve damage across a drop and reacquisition.
    use core::sync::atomic::Ordering;
    if GUARDS_SEEDED.load(Ordering::Relaxed) {
        check_rx_guards();
        return;
    }
    for (_, guard) in guards() {
        // SAFETY: the port owner initializes the two aligned static fields
        // while detached, before the first possible attach after startup.
        unsafe { guard.write_volatile(RX_GUARD) };
    }
    sync_tx();
    GUARDS_SEEDED.store(true, Ordering::Relaxed);
}

pub(super) fn check_rx_guards() {
    sync_rx();
    for (fault, guard) in guards() {
        for (i, expected) in RX_GUARD.into_iter().enumerate() {
            // SAFETY: all four words belong to the aligned guard field. Under
            // the DMA contract USB0 only writes data, so the CPU can inspect
            // this field while attached. Detection after a breach is best effort.
            if unsafe { guard.cast::<u32>().add(i).read_volatile() } != expected {
                rx_fault(fault, i);
            }
        }
    }
}

pub(super) fn check_packet_count(fault: &'static str, count: u16) {
    if usize::from(count) > MAX_PACKET {
        rx_fault(fault, usize::from(count));
    }
}

#[cold]
#[inline(never)]
#[allow(
    clippy::panic,
    reason = "A USB RX breach must stop this firmware session."
)]
fn rx_fault(fault: &'static str, value: usize) -> ! {
    detach();
    sync_tx();
    panic!("USB0 {fault} {value}");
}

pub(super) fn attach() -> Result<()> {
    clear(CLOCK, 3); // USB clock comes from the 48 MHz PLL.
    // SAFETY: install permanent, aligned RAM destinations before exposing D+.
    // Enumeration can write EP0 before a host application opens a MIDI port.
    // Activation must already cover control traffic and outstanding old writes.
    write(EP0_ADR, bus_address(Endpoint::Control.address()));
    for reg in [EP1_RX, 0x11828, 0x11830, 0x1183c] {
        write(reg, bus_address(Endpoint::Out.address()));
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
    write(EP0_ADR, bus_address(Endpoint::Control.address()));
    // Only EP0 participates until SET_CONFIGURATION.
    set(CON0, 0x1e << 19);
    clear(CON0, 1 << 19);
    wr(FADDR, 0)?;
    wr(USB_ENABLE, SUSPEND | RESUME | RESET | DISCONNECT)?;
    wr(TX_ENABLE, 1)?; // EP0
    wr(RX_ENABLE, 0)
}

#[derive(Clone, Copy)]
pub(super) enum RxBuffer {
    Normal,
    #[cfg(feature = "usb-dma-probe")]
    Live,
}
impl RxBuffer {
    pub(super) fn ptr(self) -> *mut u8 {
        match self {
            Self::Normal => (&raw mut RX).cast(),
            #[cfg(feature = "usb-dma-probe")]
            Self::Live => (&raw mut probe::LIVE).cast(),
        }
    }
}

pub(super) fn configure(enabled: bool) -> Result<()> {
    if !enabled {
        set(CON0, 1 << 20);
        wr(TX_ENABLE, 1)?;
        return wr(RX_ENABLE, 0);
    }
    write(EP1_TX, bus_address(Endpoint::In.address()));
    // SAFETY: the RX allocation is published before opening EP1's gate.
    // The owner established its write bound at activation. RX_MAX programs
    // the receiver. At 0x40 a rejected 1023-byte packet still changed SRAM
    // through byte 511 with no ready event on the black FM-1.
    write(EP1_RX, bus_address(Endpoint::Out.address()));
    wr(INDEX, 1)?;
    wr(TX_MAX, 0xff)?; // AC79 maximum packet register, as in the SDK.
    wr(TXCSR, 0x48)?; // Flush FIFO and reset data toggle.
    wr(TXCSR + 1, 0)?;
    wr(RX_MAX, 0xff)?;
    wr(RXCSR, 0x90)?;
    wr(RXCSR + 1, 0)?;
    wr(TX_ENABLE, 3)?; // EP0 and completion of a queued EP1 IN packet.
    wr(RX_ENABLE, 2)?;
    clear(CON0, 1 << 20);
    Ok(())
}

#[cfg(feature = "usb-audio-capture")]
pub(super) fn configure_capture(active: bool, midi_configured: bool) -> Result<()> {
    let midi_irqs = if midi_configured { 3 } else { 1 };
    // Gate EP4 before touching its bank. Keep the separate guarded RX address
    // installed by attach(). There is no audio OUT interface or RX interrupt.
    set(CON0, 1 << 23);
    wr(TX_ENABLE, midi_irqs)?;
    wr(INDEX, 4)?;
    wr(TXCSR, 0x48)?; // Flush the last stream's packet.
    wr(TXCSR + 1, 0)?;
    wr(TXCSR, 0)?;
    if active {
        write(EP4_TX, bus_address(capture_address()));
        wr(TX_MAX, 0xff)?;
        wr(TXCSR + 1, 0x40)?; // Isochronous IN.
        wr(TX_ENABLE, midi_irqs | 0x10)?;
        clear(CON0, 1 << 23);
    }
    Ok(())
}

#[cfg(feature = "usb-audio-capture")]
pub(super) fn service_capture(
    stream: &mut super::capture::Stream,
    buffers: &mut Buffers,
    now: u32,
) -> Result<()> {
    wr(INDEX, 4)?;
    if rd(TXCSR)? & 1 != 0 {
        return Ok(());
    }
    let mut data = [0; super::capture::MAX_BYTES];
    let n = stream.packet(now, &mut data);
    let Buffers::Copied(engine) = buffers else {
        unreachable!("capture uses copied DMA")
    };
    engine.publish_capture(&data);
    sync_tx();
    write(EP4_CNT, u32::from(n));
    sync_tx();
    // Set TXPKTRDY and clear a previous isochronous underrun indication.
    wr(TXCSR, 1)
}

// Keep this transaction out of line. A powered FM-1 without a data host can
// leave DONE clear indefinitely. Bound one transaction, then back off.
#[inline(never)]
fn transaction(command: u32) -> Option<u8> {
    write(CON1, command);
    sync_tx();
    let start = cycles();
    loop {
        // The audio ISR can preempt a poll for longer than the timeout. Read DONE
        // after sampling the deadline so a completed transfer always wins.
        let expired = cycles().wrapping_sub(start) >= Timers::CRYSTAL_HZ / 10_000;
        let value = read(CON1);
        if value & 0x8000 != 0 {
            return Some(value.to_le_bytes()[0]);
        }
        if expired {
            return None;
        }
    }
}

pub(super) fn rd(reg: u8) -> Result<u8> {
    transaction((u32::from(reg) << 8) | 0x4000).ok_or(super::RegisterTimeout::Read(reg))
}
pub(super) fn wr(reg: u8, val: u8) -> Result<()> {
    transaction((u32::from(reg) << 8) | u32::from(val))
        .map(|_| ())
        .ok_or(super::RegisterTimeout::Write(reg))
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

pub(super) fn setup(buffers: &mut Buffers) -> ([u8; 8], bool) {
    match buffers {
        Buffers::Direct => {
            sync_rx();
            // SAFETY: Direct requires eight initialized CPU-owned bytes until
            // this copy completes, independently of the sampled ready flag.
            (
                core::array::from_fn(|i| unsafe {
                    (&raw mut CONTROL.data).cast::<u8>().add(i).read_volatile()
                }),
                true,
            )
        }
        #[cfg(feature = "usb-midi-copy")]
        Buffers::Copied(engine) => {
            let (packet, valid) = engine.packet(Endpoint::Control, 8);
            (core::array::from_fn(|i| packet[i]), valid)
        }
    }
}

pub(super) fn control_in(buffers: &mut Buffers, bytes: &[u8], last: bool) -> Result<()> {
    publish(buffers, Endpoint::Control, bytes);
    wr(CSR0, if last { 0x0a } else { 0x02 })
}

pub(super) fn receive(buffers: &mut Buffers, events: &mut super::PacketBuffer) -> Result<usize> {
    wr(INDEX, 1)?;
    let csr = rd(RXCSR)?;
    if csr & 1 == 0 {
        return Ok(0);
    }
    let count = rx_count()?;
    check_packet_count("EP1 byte count", count);
    let count = usize::from(count);
    let n = count / 4;
    let delivered = match buffers {
        Buffers::Direct => {
            sync_rx();
            for (i, b) in events[..n].as_flattened_mut().iter_mut().enumerate() {
                // SAFETY: count bounds this prefix. Direct requires it to
                // remain CPU-owned through the acknowledgement.
                *b = unsafe { RxBuffer::Normal.ptr().add(i).read_volatile() };
            }
            n
        }
        #[cfg(feature = "usb-midi-copy")]
        Buffers::Copied(engine) => {
            let (frame, valid) = engine.packet(Endpoint::Out, count);
            if valid && count.is_multiple_of(4) {
                events[..n]
                    .as_flattened_mut()
                    .copy_from_slice(&frame[..count]);
                n
            } else {
                0
            }
        }
    };
    acknowledge_rx(csr)?;
    Ok(delivered)
}

/// Check a newly signaled normal endpoint even if the application never reads MIDI.
pub(super) fn check_ready_midi_count() -> Result<()> {
    wr(INDEX, 1)?;
    if rd(RXCSR)? & 1 != 0 {
        check_packet_count("EP1 byte count", rx_count()?);
    }
    Ok(())
}

fn rx_count() -> Result<u16> {
    Ok(u16::from_le_bytes([rd(COUNT)?, rd(COUNT + 1)?]))
}

/// Acknowledge a ready packet with EP1 selected and its sampled RXCSR.
pub(super) fn acknowledge_rx(csr: u8) -> Result<()> {
    wr(RXCSR, (csr & !0x64) | 0x10)?;
    wr(RXCSR + 1, 0)
}

#[cfg(feature = "usb-dma-probe")]
pub(super) mod probe {
    use super::*;

    use super::super::dma_probe::BYTES;
    pub(super) static mut LIVE: Buffer<BYTES> = Buffer([0; BYTES]);

    pub(in super::super) fn gate() {
        set(CON0, 1 << 20); // Same endpoint gate as configure(false). EP0 stays live.
        sync_tx();
    }

    pub(in super::super) fn enable() {
        clear(CON0, 1 << 20);
        sync_tx();
    }

    pub(in super::super) fn point_at(buffer: RxBuffer) {
        write(EP1_RX, bus_address(buffer.ptr()));
        sync_tx();
    }

    pub(in super::super) fn rx_max(value: u8) -> Result<u8> {
        wr(INDEX, 1)?;
        wr(RX_MAX, value)?;
        rd(RX_MAX)
    }

    pub(in super::super) fn observe() -> Result<(u8, u16)> {
        wr(INDEX, 1)?;
        Ok((rd(RXCSR)?, rx_count()?))
    }
}

pub(super) fn discard_rx() -> Result<()> {
    wr(INDEX, 1)?;
    wr(RXCSR, 0x10) // FlushFIFO; keep the host's data toggle.
}

pub(super) fn send(buffers: &mut Buffers, events: &[super::EventPacket]) -> Result<usize> {
    let n = events.len().min(super::MAX_EVENTS);
    if n == 0 {
        return Ok(0);
    }
    wr(INDEX, 1)?;
    let csr = rd(TXCSR)?;
    if csr & 1 != 0 {
        return Ok(0);
    }
    publish(buffers, Endpoint::In, events[..n].as_flattened());
    wr(TXCSR, csr | 1)?;
    Ok(n)
}

fn publish(buffers: &mut Buffers, endpoint: Endpoint, bytes: &[u8]) {
    assert!(bytes.len() <= MAX_PACKET);
    let (address, count) = match endpoint {
        Endpoint::Control => (EP0_ADR, EP0_CNT),
        Endpoint::In => (EP1_TX, EP1_CNT),
        Endpoint::Out => unreachable!("receive-only endpoint"),
    };
    match buffers {
        Buffers::Direct => {
            let buffer = endpoint.address();
            for (i, &byte) in bytes.iter().enumerate() {
                // SAFETY: Direct requires exclusive CPU ownership over this
                // idle endpoint while filling. Both allocations fit MAX_PACKET.
                unsafe { buffer.add(i).write_volatile(byte) };
            }
            sync_tx();
            write(address, bus_address(buffer));
        }
        #[cfg(feature = "usb-midi-copy")]
        Buffers::Copied(engine) => {
            // The address installed at attach/configure stays fixed even when
            // a replacement SETUP overlaps the bus-to-bus payload copy.
            engine.publish(endpoint, bytes);
        }
    }
    write(
        count,
        u32::try_from(bytes.len()).expect("one USB packet fits"),
    );
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::usb_midi::{Identity, UsbError};

    // The host MMIO fixture is thread-local. The USB claim and buffers are shared.
    static USB: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const ID: Identity = Identity {
        vendor_id: 0x1209,
        product_id: 1,
        manufacturer: "fm1-rust",
        product: "test",
    };

    #[cfg(feature = "usb-midi-copy")]
    #[test]
    fn selected_mode_survives_reacquisition_and_rejects_an_incompatible_owner() {
        for (first, other) in [(1, 2), (2, 1)] {
            let mode = core::sync::atomic::AtomicU8::new(0);
            assert!(select_mode(&mode, first));
            assert!(!select_mode(&mode, other));
            assert!(select_mode(&mode, first));
        }
    }

    #[test]
    fn checked_start_detaches_and_releases_the_port_on_error_or_panic() {
        let _usb = USB.lock().expect("exclusive test port");
        let board = crate::Board::init();
        let claim = || board.take_usb().expect("port is available");

        // Model a bootloader that left D+ asserted with its own RX destinations.
        write(EP0_ADR, 0x1230);
        write(EP1_RX, 0x4560);
        let attached = || {
            write(CON0, 1);
            write(IO, 0x40);
        };
        let assert_detached = || {
            assert_eq!((read(CON0), read(IO)), (0, 0x0c));
            assert_eq!((read(EP0_ADR), read(EP1_RX)), (0x1230, 0x4560));
        };

        attached();
        assert_eq!(
            claim().try_into_midi(ID).expect_err("unqualified AC791N"),
            UsbError::DmaProtectionUnavailable
        );
        assert_detached();

        let port = claim();
        attached();
        assert!(std::panic::catch_unwind(|| port.into_midi(ID)).is_err());
        assert_detached();
        drop(claim());
    }

    #[test]
    fn bench_qualification_still_starts_midi_and_diagnostics() {
        let _usb = USB.lock().expect("exclusive test port");
        let board = crate::Board::init();
        let port = board.take_usb().expect("port is available");
        // SAFETY: the host fixture has no DMA master. Only this test owns USB0.
        let device = unsafe { port.assume_midi_dma() }.into_midi(ID);
        assert!(board.take_usb().is_none());
        assert_eq!(
            device.status().stage,
            crate::usb_midi::EnumerationStage::Starting
        );
        #[cfg(feature = "usb-dma-probe")]
        assert!(device.probe_display().is_none());
        drop(device);

        #[cfg(feature = "usb-dma-probe")]
        {
            let port = board.take_usb().expect("previous device released the port");
            // SAFETY: the fixture cannot transfer USB data to either probe arena.
            let probe = unsafe { port.into_dma_probe(ID, crate::usb_midi::ProbeProfile::Iso) };
            assert!(probe.probe_display().is_some());
        }
    }

    #[test]
    fn receive_destinations_have_space_for_full_speed_data_and_its_trailer() {
        fn capacity<const N: usize>(_: *const [u8; N]) -> usize {
            N
        }
        let storage = RxStorage::EMPTY;
        assert!(capacity(&raw const storage.data) >= 1023 + 2);
        assert_eq!(MAX_PACKET, 64); // Keep the advertised MIDI payload independent.
    }

    #[test]
    fn reacquisition_preserves_guard_damage_and_detaches_before_panicking() {
        let _usb = USB.lock().expect("exclusive test port");
        // No USB DMA master runs in the host fixture. Hold the same exclusive
        // claim as a real caller while changing the guard and restoring it.
        let _port = crate::Board::init().take_usb().expect("fixture USB port");
        init_rx_guards();
        let last = RX_GUARD.len() - 1;
        // SAFETY: the fixture owns the static guard for this test. The chosen
        // index is the final aligned word of EP0's guard.
        let word = unsafe {
            let word = guards()[0].1.cast::<u32>().add(last);
            word.write_volatile(!RX_GUARD[last]);
            word
        };
        write(CON0, 1);
        let detected = std::panic::catch_unwind(init_rx_guards);
        // SAFETY: restore the same word while the fixture is still idle.
        unsafe { word.write_volatile(RX_GUARD[last]) };
        assert!(detected.is_err());
        assert_eq!(read(CON0), 0);
        init_rx_guards();

        write(CON0, 1);
        check_packet_count("EP0 byte count", 64);
        assert_eq!(read(CON0), 1);
        assert!(std::panic::catch_unwind(|| check_packet_count("EP0 byte count", 65)).is_err());
        assert_eq!(read(CON0), 0);
    }

    #[test]
    fn a_completed_sie_read_wins_after_preemption() {
        crate::test_board::complete_usb_on_timer_read(42);
        assert_eq!(rd(INDEX).ok(), Some(42));
        assert!(rd(INDEX).is_err()); // A controller that stays busy still times out.
    }
}
