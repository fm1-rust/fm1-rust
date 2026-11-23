//! EP1 OUT measurements for the opt-in timing firmware. One CPU0 USB task owns
//! both DMA destinations and the frozen copy. No arena is stored in its future.
use super::{
    Identity,
    control::{Control, Interface, Reply},
    controller::{self, RxBuffer, probe as hw},
};
use crate::time::{Timers, cycles, wait_cycles};

const ARENA_LENGTH: u16 = 2048;
#[cfg(not(feature = "usb-dma-stress"))]
pub(super) const BYTES: usize = ARENA_LENGTH as usize;
#[cfg(feature = "usb-dma-stress")]
pub(super) const BYTES: usize = 8192;
// Fixed RXD3 comparison with the allocation at commit 2721022. It must not
// grow when the normal receive allocation is hardened.
const ORIGINAL_RX_BYTES: u16 = 68;
const TICKS_PER_MS: u32 = Timers::CRYSTAL_HZ / 1000;
const DRAIN_TICKS: u32 = 3 * TICKS_PER_MS;
const WINDOW_MS: u16 = 500;
const DELAY_MS: u32 = 50;
const PAGE: usize = 64;

static mut FROZEN: [u8; BYTES] = [0; BYTES];

/// USB descriptors used by a diagnostic image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ProbeProfile {
    /// Stock timing descriptors, including the MIDI class interfaces.
    Midi,
    /// Vendor interface, alternate 1 is 64-byte bulk OUT.
    Bulk,
    /// Adds alternate 2 for one full-speed isochronous OUT packet up to 1023 bytes.
    Iso,
    /// Vendor descriptor with the 2 KiB production guard inside an 8 KiB arena.
    #[cfg(feature = "usb-dma-stress")]
    Stress,
}

impl ProbeProfile {
    fn stress(self) -> bool {
        #[cfg(feature = "usb-dma-stress")]
        {
            self == Self::Stress
        }
        #[cfg(not(feature = "usb-dma-stress"))]
        {
            false
        }
    }

    fn length(self) -> u16 {
        if self.stress() { 8192 } else { ARENA_LENGTH }
    }

    fn boundary(self) -> u16 {
        if self.stress() {
            u16::try_from(controller::RX_DMA_BYTES).expect("USB staging fits u16")
        } else {
            ORIGINAL_RX_BYTES
        }
    }
}

// Alternate zero consumes no periodic bandwidth. Both active alternates use
// the same EP1 address, in mutually exclusive settings of the SAME interface.
#[rustfmt::skip]
const VENDOR: [u8; 50] = [
    9, 2, 50, 0, 1, 1, 0, 0x80, 50,
    9, 4, 0, 0, 0, 0xff, 0, 0, 0,
    9, 4, 0, 1, 1, 0xff, 0, 0, 0,
    7, 5, 1, 2, 64, 0, 0,
    9, 4, 0, 2, 1, 0xff, 0, 0, 0,
    7, 5, 1, 1, 0xff, 3, 1,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
enum Phase {
    Idle,
    Armed,
    Settling,
    Frozen,
}

#[derive(Clone, Copy, Debug, Default)]
struct Observation {
    csr: u8,
    count: u16,
    ms: u16,
}
impl Observation {
    fn encode(self, dest: &mut [u8]) {
        dest[0] = self.csr;
        dest[1..3].copy_from_slice(&self.count.to_le_bytes());
        dest[3..5].copy_from_slice(&self.ms.to_le_bytes());
    }
}

/// A trial as seen by the board, independent of host transfer completion.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProbeReading {
    /// Trial generation.
    pub token: u16,
    /// Host-declared transfer length. A locally armed capture has no declared length.
    pub requested: Option<u16>,
    /// Active USB alternate setting.
    pub alt: u8,
    /// RX_MAX value written and value read back.
    pub rx_max: [u8; 2],
    /// Byte used to fill SRAM.
    pub fill: u8,
    /// Whether the first service was delayed.
    pub delayed: bool,
    /// Number of ready packets observed.
    pub packets: u8,
    /// Whether the endpoint RX interrupt was seen.
    pub irq: bool,
    /// Raw COUNT for each of the first two ready packets.
    pub counts: [u16; 2],
    /// COUNT sampled before gating EP1.
    pub gate_count: u16,
    /// RXCSR sampled before gating EP1.
    pub gate_csr: u8,
    /// Last changed byte anywhere in the arena.
    pub highest: Option<u16>,
    /// First changed byte at or beyond the profile’s comparison boundary.
    pub first_guard: Option<u16>,
    /// Observed value at the first changed guard byte, when present.
    pub guard_byte: u8,
}

/// Values that can be displayed without opening a USB log connection.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ProbeDisplay {
    /// Highest enumeration stage reached (attach, SETUP, address, configuration).
    pub stage: u8,
    /// Cumulative SIE retries.
    pub retries: u16,
    /// Whether the USB session is currently active.
    pub online: bool,
    /// True while acquiring or draining the current trial.
    pub active: bool,
    /// Current trial while acquiring, otherwise the last completed trial.
    pub reading: Option<ProbeReading>,
    /// First guard change since boot, retained across subsequent trials and USB resets.
    pub first_hit: Option<ProbeReading>,
    /// Most recent trial interrupted before capture, zero until the first interruption.
    pub aborted: u16,
}

const LOG_CAPACITY: u8 = 64;
static mut JOURNAL: [[u8; 32]; LOG_CAPACITY as usize] = [[0; 32]; LOG_CAPACITY as usize];

#[derive(Debug, Default, Clone, Copy)]
struct History {
    total: u32,
    count: u8,
    last: Option<ProbeReading>,
    first_hit: Option<ProbeReading>,
    aborted: u16,
}

#[derive(Debug)]
pub(super) struct Probe {
    pub(super) profile: ProbeProfile,
    pub(super) alt: u8,
    pub(super) stage: u8,
    pub(super) retries: u16,
    phase: Phase,
    token: u16,
    sentinel: u8,
    max: u8,
    delayed: bool,
    started: u32,
    gated: u32,
    live_needs_drain: bool,
    pending_at: Option<u32>,
    requested: Option<u16>,
    written: u8,
    history: History,
    packets: u8,
    saw_irq: bool,
    samples: [Observation; 2],
    last: Observation,
}
impl Probe {
    pub(super) fn new(profile: ProbeProfile) -> Self {
        Self {
            profile,
            alt: 0,
            stage: 0,
            retries: 0,
            phase: Phase::Idle,
            token: 0,
            sentinel: 0,
            max: 0,
            delayed: false,
            started: 0,
            gated: 0,
            live_needs_drain: false,
            pending_at: None,
            requested: None,
            written: 0,
            history: History::default(),
            packets: 0,
            saw_irq: false,
            samples: [Observation::default(); 2],
            last: Observation::default(),
        }
    }

    pub(super) fn reset(&mut self) {
        if self.active() {
            self.history.aborted = self.token;
        }
        *self = Self {
            history: self.history,
            live_needs_drain: self.live_needs_drain,
            token: self.token,
            stage: self.stage,
            retries: self.retries,
            ..Self::new(self.profile)
        };
    }

    pub(super) fn active(&self) -> bool {
        matches!(self.phase, Phase::Armed | Phase::Settling)
    }

    pub(super) fn endpoint_enabled(&self) -> bool {
        self.profile == ProbeProfile::Midi || self.alt != 0
    }

    pub(super) fn arm(
        &mut self,
        token: u16,
        max: u8,
        flags: u8,
        requested: Option<u16>,
    ) -> controller::Result<()> {
        hw::gate();
        hw::point_at(RxBuffer::Normal);
        if self.live_needs_drain {
            // A reset, suspend or SET_INTERFACE interrupted the preceding
            // capture. Drain before reusing LIVE even if no IRQ was delivered.
            wait_cycles(DRAIN_TICKS);
            controller::sync_rx();
            self.live_needs_drain = false;
        }
        controller::discard_rx()?; // Flush stale data, preserve the bulk data toggle.
        self.requested = requested;
        self.written = max;
        self.max = hw::rx_max(max)?;
        self.sentinel = if flags & 1 == 0 { 0xa5 } else { 0x5a };
        let live = RxBuffer::Live.ptr();
        for i in 0..usize::from(self.profile.length()) {
            // SAFETY: EP1 is gated and points elsewhere. After an interrupted
            // capture the probe contract bounds the drain above before LIVE
            // is reused. The sole CPU owner fills at most BYTES here.
            unsafe { live.add(i).write_volatile(self.sentinel) };
        }
        controller::sync_tx();
        hw::point_at(RxBuffer::Live);
        self.phase = Phase::Armed;
        self.token = token;
        self.delayed = flags & 2 != 0;
        self.pending_at = None;
        self.packets = 0;
        self.saw_irq = false;
        self.samples = [Observation::default(); 2];
        self.last = Observation::default();
        self.started = cycles();
        self.live_needs_drain = true;
        hw::enable();
        Ok(())
    }

    pub(super) fn cancel(&mut self) -> controller::Result<()> {
        if self.active() {
            self.history.aborted = self.token;
            hw::gate();
            hw::point_at(RxBuffer::Normal);
            self.restore()?;
        }
        self.phase = Phase::Idle;
        Ok(())
    }

    fn restore(&self) -> controller::Result<()> {
        controller::discard_rx()?;
        hw::rx_max(0xff)?;
        hw::enable();
        Ok(())
    }

    pub(super) fn advance(&mut self, now: u32, rx_pending: &mut bool) -> controller::Result<()> {
        match self.phase {
            Phase::Armed => {
                // Read before service, acknowledgement, gate or flush. Check even
                // if the controller never raised the RX interrupt.
                self.saw_irq |= *rx_pending;
                let (csr, count) = hw::observe()?;
                if csr & 1 == 0 {
                    *rx_pending = false;
                }
                let ms = now.wrapping_sub(self.started) / TICKS_PER_MS;
                let obs = Observation {
                    csr,
                    count,
                    ms: u16::try_from(ms).unwrap_or(u16::MAX),
                };
                if csr & 1 != 0 && self.pending_at.is_none() {
                    if let Some(slot) = self.samples.get_mut(usize::from(self.packets)) {
                        *slot = obs;
                    }
                    self.packets = self.packets.saturating_add(1);
                    self.pending_at = Some(now);
                }
                // PLAY on the MIDI image captures the first ready packet, or three
                // seconds of silence. Leave that packet unacknowledged until gating.
                let stop = match self.requested {
                    None => csr & 1 != 0 || ms >= 3000,
                    Some(_) => ms >= u32::from(WINDOW_MS),
                };
                if stop {
                    self.last = obs;
                    *rx_pending = false;
                    hw::gate();
                    hw::point_at(RxBuffer::Normal);
                    self.gated = cycles();
                    self.phase = Phase::Settling;
                } else if *rx_pending
                    && csr & 1 != 0
                    && self.pending_at.is_some_and(|first| {
                        !self.delayed
                            || self.packets != 1
                            || now.wrapping_sub(first) / TICKS_PER_MS >= DELAY_MS
                    })
                {
                    // Collect register samples while armed. Read LIVE only
                    // after redirecting EP1, waiting for the drain and syncing.
                    controller::sync_rx();
                    controller::acknowledge_rx(csr)?;
                    self.pending_at = None;
                    *rx_pending = false;
                }
            }
            Phase::Settling if now.wrapping_sub(self.gated) >= DRAIN_TICKS => {
                // EP1 is gated and retargeted. Allow several FS frame times for
                // an in-flight packet, then use the BSP's DMA acquire barrier.
                controller::sync_rx();
                self.live_needs_drain = false;
                let mut reading = self.reading();
                for offset in 0..self.profile.length() {
                    let i = usize::from(offset);
                    // SAFETY: i is below BYTES in disjoint static arrays. EP1
                    // is gated, points elsewhere, and the qualified drain and
                    // SSYNC have completed. Only this USB owner accesses FROZEN.
                    let byte = unsafe {
                        let byte = RxBuffer::Live.ptr().add(i).read_volatile();
                        (&raw mut FROZEN).cast::<u8>().add(i).write(byte);
                        byte
                    };
                    if byte != self.sentinel {
                        reading.highest = Some(offset);
                        if offset >= self.profile.boundary() && reading.first_guard.is_none() {
                            reading.first_guard = Some(offset);
                            reading.guard_byte = byte;
                        }
                    }
                }
                self.phase = Phase::Frozen;
                self.remember(reading); // Commit before touching the SIE again.
                self.restore()?;
            }
            _ => {}
        }
        Ok(())
    }

    pub(super) fn next_token(&self) -> u16 {
        self.token.wrapping_add(1).max(1)
    }

    fn reading(&self) -> ProbeReading {
        ProbeReading {
            token: self.token,
            requested: self.requested,
            alt: self.alt,
            rx_max: [self.written, self.max],
            fill: self.sentinel,
            delayed: self.delayed,
            packets: self.packets,
            irq: self.saw_irq,
            counts: self.samples.map(|s| s.count),
            gate_count: self.last.count,
            gate_csr: self.last.csr,
            ..ProbeReading::default()
        }
    }

    pub(super) fn display(&self, online: bool) -> ProbeDisplay {
        ProbeDisplay {
            stage: self.stage,
            retries: self.retries,
            online,
            active: self.active(),
            reading: if self.active() {
                Some(self.reading())
            } else {
                self.history.last
            },
            first_hit: self.history.first_hit,
            aborted: self.history.aborted,
        }
    }

    fn remember(&mut self, reading: ProbeReading) {
        let mut record = [0; 32];
        record[..24].copy_from_slice(&self.status());
        record[24..26].copy_from_slice(&reading.requested.unwrap_or(u16::MAX).to_le_bytes());
        record[26] = self.written;
        record[27] = reading.guard_byte;
        record[28..30].copy_from_slice(&reading.highest.unwrap_or(u16::MAX).to_le_bytes());
        record[30..32].copy_from_slice(&reading.first_guard.unwrap_or(u16::MAX).to_le_bytes());
        self.history.total = self.history.total.wrapping_add(1);
        self.history.count = self.history.count.saturating_add(1).min(LOG_CAPACITY);
        let slot = usize::from(self.history.total.to_le_bytes()[0] % LOG_CAPACITY);
        // SAFETY: CPU0's sole USB task owns JOURNAL. The slot is in its 64-entry allocation.
        unsafe {
            (&raw mut JOURNAL)
                .cast::<[u8; 32]>()
                .add(slot)
                .write(record);
        }
        self.history.last = Some(reading);
        if reading.first_guard.is_some() {
            self.history.first_hit.get_or_insert(reading);
        }
    }

    fn status(&self) -> [u8; 24] {
        let mut data = [0; 24];
        data[..2].copy_from_slice(&self.token.to_le_bytes());
        data[2..8].copy_from_slice(&[
            self.phase as u8,
            self.sentinel,
            self.max,
            u8::from(self.delayed),
            self.packets,
            self.alt,
        ]);
        self.samples[0].encode(&mut data[8..13]);
        self.samples[1].encode(&mut data[13..18]);
        self.last.encode(&mut data[18..23]);
        data[23] = u8::from(self.saw_irq);
        data
    }

    pub(super) fn request(
        &self,
        c: &mut Control,
        id: &Identity,
        state: Interface,
        setup: [u8; 8],
    ) -> Option<Reply> {
        let [kind, request, vl, vh, il, ih, ll, lh] = setup;
        let value = u16::from_le_bytes([vl, vh]);
        let index = u16::from_le_bytes([il, ih]);
        let len = u16::from_le_bytes([ll, lh]);
        let reply = match (kind, request) {
            (0xc0, 0x50) if state.configured && (value, index, len) == (0, 0, 12) => {
                let [lo, hi] = WINDOW_MS.to_le_bytes();
                let arena = self.profile.length().to_le_bytes();
                let bound = self.profile.boundary().to_le_bytes();
                let magic = if self.profile.stress() {
                    *b"RXS1"
                } else {
                    *b"RXD3"
                };
                c.data(&[
                    magic[0],
                    magic[1],
                    magic[2],
                    magic[3],
                    self.profile as u8,
                    self.alt,
                    arena[0],
                    arena[1],
                    bound[0],
                    bound[1],
                    lo,
                    hi,
                ])
            }
            // Compare-and-arm with a device generation. Competing readers
            // cannot splice frozen pages from two trials with the same token.
            (0xc0, 0x54) if state.configured && (value, index, len) == (0, 0, 2) => {
                c.data(&self.next_token().to_le_bytes())
            }
            // wIndex is the generation. wValue carries the total wire payload
            // and polarity. The request selects RX_MAX and prompt/delayed service.
            (0x40, 0x60..=0x69)
                if self.profile.stress()
                    && state.configured
                    && self.alt != 0
                    && !self.active()
                    && !state.out_halted
                    && len == 0
                    && index == self.next_token()
                    && value & 0x7fff <= 4096 =>
            {
                let maxima = [0xff, 0x40, 0x08, 0x09, 0x20];
                let max = maxima[usize::from((request & 0x0f) >> 1)];
                Reply::Arm {
                    token: index,
                    max,
                    flags: ((vh >> 7) & 1) | ((request & 1) << 1),
                    requested: value & 0x7fff,
                }
            }
            (0x40, 0x51)
                if !self.profile.stress()
                    && state.configured
                    && self.endpoint_enabled()
                    && !self.active()
                    && !state.out_halted
                    && len == 0
                    && index == self.next_token()
                    && value >> 14 == 0 =>
            {
                let max = match (value >> 10) & 3 {
                    0 => 0xff,
                    1 => 0x20,
                    2 => 0x40,
                    _ => return Some(Reply::Stall),
                };
                Reply::Arm {
                    token: index,
                    max,
                    flags: vh >> 4,
                    requested: value & 1023,
                }
            }
            (0xc0, 0x55) if state.configured && (value, index, len) == (0, 0, 16) => {
                let mut h = [0; 16];
                h[..4].copy_from_slice(b"DLG3");
                h[4..8].copy_from_slice(&self.history.total.to_le_bytes());
                h[8..12].copy_from_slice(&[
                    self.history.count,
                    self.profile as u8,
                    self.stage,
                    self.phase as u8,
                ]);
                h[12..14].copy_from_slice(&self.retries.to_le_bytes());
                h[14..].copy_from_slice(&self.history.aborted.to_le_bytes());
                c.data(&h)
            }
            (0xc0, 0x56) if state.configured && len == 32 => {
                let sequence = u32::from_le_bytes([vl, vh, il, ih]);
                if self.history.total.wrapping_sub(sequence) >= u32::from(self.history.count) {
                    return Some(Reply::Stall);
                }
                let slot = usize::from(vl % LOG_CAPACITY);
                // SAFETY: only this task owns the journal. This sequence is still retained.
                let record = unsafe { (&raw const JOURNAL).cast::<[u8; 32]>().add(slot).read() };
                c.data(&record)
            }
            (0xc0, 0x52)
                if state.configured
                    && value == 0
                    && index != 0
                    && index == self.token
                    && len == 24 =>
            {
                c.data(&self.status())
            }
            (0xc0, 0x53)
                if self.phase == Phase::Frozen
                    && index == self.token
                    && len == 64
                    && usize::from(value) < usize::from(self.profile.length()) / PAGE =>
            {
                let offset = usize::from(value) * PAGE;
                // SAFETY: host reads only after a complete freeze. EP1 never
                // points here and the next arm cannot run during this copy.
                let page = unsafe {
                    core::slice::from_raw_parts((&raw const FROZEN).cast::<u8>().add(offset), PAGE)
                };
                c.data(page)
            }
            _ if self.profile == ProbeProfile::Midi => return None,
            (0x80, 6) if value == 0x0100 && index == 0 => c.device(id, self.profile as u8),
            (0x80, 6) if value == 0x0200 && index == 0 => {
                let n: u8 = if self.profile == ProbeProfile::Bulk {
                    34
                } else {
                    50
                };
                let mut bytes = VENDOR;
                bytes[2] = n;
                c.data(&bytes[..usize::from(n)])
            }
            (0x81, 0) if state.configured && index == 0 && value == 0 && len == 2 => {
                c.data(&[0, 0])
            }
            (0x81, 10) if state.configured && index == 0 && value == 0 && len == 1 => {
                c.data(&[self.alt])
            }
            (1, 11)
                if state.configured
                    && index == 0
                    && len == 0
                    && value <= u16::from((self.profile as u8).min(2)) =>
            {
                Reply::Alternate(vl)
            }
            (0x82, 0)
                if index == 1 && state.configured && self.alt != 0 && len == 2 && value == 0 =>
            {
                c.data(&[u8::from(state.out_halted), 0])
            }
            (2, 1 | 3)
                if index == 1 && state.configured && self.alt == 1 && value == 0 && len == 0 =>
            {
                Reply::Halt {
                    input: false,
                    halted: request == 3,
                }
            }
            (0x82, 0) if matches!(index, 0 | 0x80) => return None,
            (0x81 | 0x82 | 1 | 2, _) => Reply::Stall,
            _ => return None,
        };
        Some(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: Identity = Identity {
        vendor_id: 0x1209,
        product_id: 1,
        manufacturer: "fm1-rust",
        product: "FM-1 Timing",
    };
    const CFG: Interface = Interface {
        configured: true,
        in_halted: false,
        out_halted: false,
    };

    #[test]
    fn personalities_expose_only_their_own_alternates() {
        for (profile, expected) in [
            (ProbeProfile::Midi, 0),
            (ProbeProfile::Bulk, 34),
            (ProbeProfile::Iso, 50),
        ] {
            let mut c = Control::new();
            let p = Probe::new(profile);
            let desc = p.request(&mut c, &ID, CFG, [0x80, 6, 0, 2, 0, 0, 255, 0]);
            if profile == ProbeProfile::Midi {
                assert_eq!(desc, None);
                continue;
            }
            assert_eq!(desc, Some(Reply::Data(expected)));
            c.start(expected, 255);
            let (config, last) = c.chunk();
            assert!(last);
            assert_eq!(usize::from(config[2]), expected);
            assert_eq!(&config[9..18], &VENDOR[9..18]); // zero endpoints on alt zero
            let mut offset = 0;
            for len in [9, 9, 9, 7, 9, 7] {
                if offset == expected {
                    break;
                }
                assert_eq!(usize::from(config[offset]), len);
                offset = offset.saturating_add(len);
            }
            assert_eq!(offset, expected);
            let iso_set = p.request(&mut c, &ID, CFG, [1, 11, 2, 0, 0, 0, 0, 0]);
            assert_eq!(
                iso_set,
                Some(if profile == ProbeProfile::Iso {
                    Reply::Alternate(2)
                } else {
                    Reply::Stall
                })
            );
            assert_eq!(
                p.request(&mut c, &ID, CFG, [0x40, 0x51, 0xff, 0, 1, 0, 0, 0]),
                None
            ); // EP1 disabled
        }
    }

    #[test]
    fn first_crossing_and_completed_log_survive_wrap_and_bus_reset() {
        let mut p = Probe::new(ProbeProfile::Midi);
        let mut c = Control::new();
        for token in 1..=70 {
            p.token = token;
            p.phase = Phase::Frozen;
            let reading = ProbeReading {
                first_guard: (token == 2 || token == 65).then_some(68),
                ..p.reading()
            };
            p.remember(reading);
        }
        p.token = 71;
        p.phase = Phase::Armed;
        p.live_needs_drain = true;
        p.reset();
        assert!(p.live_needs_drain);
        let display = p.display(false);
        assert_eq!(display.first_hit.map(|r| r.token), Some(2));
        assert_eq!(display.reading.map(|r| r.token), Some(70));
        assert_eq!(display.aborted, 71);
        assert!(!display.online);
        assert_eq!(p.history.count, 64);
        assert_eq!(p.next_token(), 72);
        assert_eq!(
            p.request(&mut c, &ID, CFG, [0xc0, 0x56, 6, 0, 0, 0, 32, 0]),
            Some(Reply::Stall)
        );
        for token in [7, 65, 70] {
            assert_eq!(
                p.request(&mut c, &ID, CFG, [0xc0, 0x56, token, 0, 0, 0, 32, 0]),
                Some(Reply::Data(32))
            );
            c.start(32, 32);
            let (record, _) = c.chunk();
            assert_eq!(&record[..2], &[token, 0]);
            if token == 65 {
                assert_eq!(&record[30..32], &68u16.to_le_bytes());
            }
        }
    }

    #[test]
    fn old_tokens_and_unfrozen_pages_cannot_alias_the_next_snapshot() {
        let mut c = Control::new();
        let mut p = Probe::new(ProbeProfile::Midi);
        p.token = 12;
        p.phase = Phase::Armed;
        assert_eq!(
            p.request(&mut c, &ID, CFG, [0xc0, 0x53, 0, 0, 12, 0, 64, 0]),
            None
        );
        p.phase = Phase::Frozen;
        assert_eq!(
            p.request(&mut c, &ID, CFG, [0xc0, 0x53, 0, 0, 11, 0, 64, 0]),
            None
        );
        assert_eq!(
            p.request(&mut c, &ID, CFG, [0xc0, 0x53, 32, 0, 12, 0, 64, 0]),
            None
        );
        p.reset();
        assert_eq!(
            p.request(&mut c, &ID, CFG, [0x40, 0x51, 0xff, 0, 12, 0, 0, 0]),
            None
        );
        assert_eq!(
            p.request(&mut c, &ID, CFG, [0x40, 0x51, 0xff, 0, 13, 0, 0, 0]),
            Some(Reply::Arm {
                token: 13,
                max: 0xff,
                requested: 255,
                flags: 0
            })
        );
    }
}
