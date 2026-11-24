//! USB0 owns its endpoint arenas for the lifetime of this firmware. A separate
//! WL82 copy engine transfers fixed-size payloads and guards through private staging.
use super::{controller as hw, controller::Endpoint};
use crate::{
    registers::{bus_address, read, write},
    time::{Timers, cycles},
};
use core::sync::atomic::{AtomicBool, Ordering, compiler_fence};

const CON: usize = 0x41300;
const TASK_ADDRESS: usize = 0x41304;
const DONE: u32 = 1 << 7;
const ERROR: u32 = 1 << 5;
// Payload copies always cover a full packet and its wire CRC.
pub(super) const BLOCK: usize = 68;
const GUARD_BYTES: usize = size_of_val(&hw::RX_GUARD);
const GUARD: [u8; BLOCK] = {
    let mut g = [0; BLOCK];
    let mut i = 0;
    let mut byte = 0u8;
    while i < BLOCK {
        g[i] = byte ^ 0xa5;
        byte = byte.wrapping_add(29);
        i += 1;
    }
    g
};

#[repr(C, align(32))]
struct Aligned<T>(T);
// USB0 is never given either private address. Both outlive a failed job.
static mut STAGE: Aligned<[u8; BLOCK]> = Aligned([0; BLOCK]);
static mut TASK: Aligned<[u32; 8]> = Aligned([0; 8]);
static POISONED: AtomicBool = AtomicBool::new(false);
static SEEDED: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
enum Area {
    Endpoint(Endpoint),
    ControlGuard,
    OutGuard,
}
impl Area {
    fn count(self) -> u32 {
        let n = match self {
            Self::Endpoint(_) => BLOCK,
            Self::ControlGuard | Self::OutGuard => GUARD_BYTES,
        };
        u32::try_from(n).expect("fixed hardware bounds")
    }
    fn ptr(self) -> *mut u8 {
        match self {
            Self::Endpoint(e) => e.address(),
            // Both arena pointers have allocation-wide provenance. The layout
            // reserves 16 accessible bytes from offset 2048 in both normal
            // and stress builds. The mode latch excludes direct users.
            Self::ControlGuard => Endpoint::Control.address().wrapping_add(hw::RX_DMA_BYTES),
            Self::OutGuard => Endpoint::Out.address().wrapping_add(hw::RX_DMA_BYTES),
        }
    }
}

#[derive(Debug)]
pub(super) struct Engine(());
impl Engine {
    /// # Safety
    /// The owner must supply the engine exclusivity, bounded USB and copy
    /// accesses, ordering and retirement required by into_midi_with_dma_copy.
    pub(super) unsafe fn new() -> Self {
        Self(())
    }

    // A panic may be caught, and a safe caller may drop or forget the device.
    // Check the sticky fault before touching either private DMA allocation.
    fn idle(&mut self) {
        if POISONED.load(Ordering::Acquire) {
            fault("copy engine quarantined", 0);
        }
        let status = read(CON);
        if status & ERROR != 0 {
            fault("copy bus error", status);
        }
        if status & DONE != 0 {
            write(CON, (status & !2) | 0x40);
            hw::sync_tx();
            let cleared = read(CON);
            if cleared & (ERROR | DONE) != 0 {
                fault("copy completion stuck", cleared);
            }
        }
    }

    // The private caller checked idle() before touching STAGE. Both addresses
    // are selected here from dedicated permanent allocations. Each supports a
    // complete transfer for its area, even for a zero-length reply. No USB or MIDI data
    // can contribute a descriptor word.
    fn copy(&mut self, area: Area, to_device: bool) {
        let device = bus_address(area.ptr());
        let stage = bus_address(&raw mut STAGE);
        let (src, dst) = if to_device {
            (stage, device)
        } else {
            (device, stage)
        };
        let desc = [area.count(), src, 0, dst, 0, 0, 0, 0];
        // SAFETY: the constructor establishes one idle engine at entry; each
        // successful previous job retired before returning. TASK is exclusively
        // owned at this point, aligned and large enough for all eight words.
        unsafe { (&raw mut TASK).cast::<[u32; 8]>().write_volatile(desc) };
        compiler_fence(Ordering::SeqCst);
        hw::sync_tx();
        write(TASK_ADDRESS, bus_address(&raw const TASK));
        write(CON, read(CON) | 0x103);
        hw::sync_tx();
        let start = cycles();
        loop {
            // Check completion after measuring the deadline so a preempting
            // audio interrupt cannot turn a completed copy into a timeout.
            let expired = cycles().wrapping_sub(start) >= Timers::CRYSTAL_HZ / 500;
            let s = read(CON);
            if s & ERROR != 0 {
                fault("copy bus error", s);
            }
            if s & DONE != 0 {
                break;
            }
            if expired {
                fault("copy timeout", s);
            }
        }
        hw::sync_rx();
        compiler_fence(Ordering::SeqCst);
    }

    fn store(&mut self, area: Area, data: [u8; BLOCK]) {
        self.idle();
        // SAFETY: private STAGE is initialized and CPU-owned between jobs.
        // No USB register points to it. We publish only after this store.
        unsafe { (&raw mut STAGE).cast::<[u8; BLOCK]>().write_volatile(data) };
        self.copy(area, true);
    }
    fn load(&mut self, area: Area) -> [u8; BLOCK] {
        self.idle();
        self.copy(area, false);
        // SAFETY: fresh DONE and SSYNC retire this fixed-size job under the
        // constructor's contract. No other task or endpoint can touch STAGE.
        // Every possible copied bit pattern is a valid initialized byte.
        unsafe { (&raw const STAGE).cast::<[u8; BLOCK]>().read_volatile() }
    }

    pub(super) fn start(&mut self) {
        for pattern in [0x3c, 0xa5] {
            let data = GUARD.map(|byte| byte ^ pattern);
            // The IN arena is device-only even during reactivation. USB0 may
            // still fetch there; its accesses are read-only and bounded.
            let calibration = Area::Endpoint(Endpoint::In);
            self.store(calibration, data);
            self.idle();
            // SAFETY: the preceding job retired and STAGE is private. A stale
            // result cannot pass the comparison against this inverted pattern.
            unsafe {
                (&raw mut STAGE)
                    .cast::<[u8; BLOCK]>()
                    .write_volatile(data.map(|b| !b))
            };
            if self.load(calibration) != data {
                fault("copy calibration", pattern.into());
            }
        }
        if !SEEDED.load(Ordering::Relaxed) {
            for guard in [Area::ControlGuard, Area::OutGuard] {
                self.store(guard, GUARD);
            }
            SEEDED.store(true, Ordering::Relaxed);
        }
        self.guards();
    }
    pub(super) fn guards(&mut self) {
        for (area, why) in [
            (Area::ControlGuard, "EP0 copy guard"),
            (Area::OutGuard, "EP1 copy guard"),
        ] {
            let got = self.load(area);
            if let Some(i) = GUARD[..GUARD_BYTES]
                .iter()
                .zip(got)
                .position(|(&a, b)| a != b)
            {
                fault(why, u32::try_from(i).expect("16-byte guard"));
            }
        }
    }
    pub(super) fn publish(&mut self, ep: Endpoint, bytes: &[u8]) {
        let mut packet = [0; BLOCK];
        packet[..bytes.len()].copy_from_slice(bytes);
        self.store(Area::Endpoint(ep), packet);
    }

    // A USB writer may overwrite a packet during the bus copy. Treat the
    // private bytes as untrusted. CRC detects some tears and is never used
    // to prove ownership. Try once more if the trailer did not match.
    pub(super) fn packet(&mut self, ep: Endpoint, count: usize) -> ([u8; BLOCK], bool) {
        debug_assert!(count <= hw::MAX_PACKET);
        let data = self.load(Area::Endpoint(ep));
        if valid_crc(&data, count) {
            return (data, true);
        }
        let data = self.load(Area::Endpoint(ep));
        let valid = valid_crc(&data, count);
        (data, valid)
    }
}
fn valid_crc(data: &[u8; BLOCK], count: usize) -> bool {
    let mut crc = u16::MAX;
    for &byte in &data[..count] {
        crc ^= u16::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 0 {
                crc >> 1
            } else {
                (crc >> 1) ^ 0xa001
            };
        }
    }
    data[count..].starts_with(&(!crc).to_le_bytes())
}
#[cold]
#[allow(
    clippy::panic,
    reason = "Release no storage to Rust after an uncertain DMA completion."
)]
fn fault(why: &str, value: u32) -> ! {
    POISONED.store(true, Ordering::Release);
    hw::detach();
    hw::sync_tx();
    panic!("USB0 {why} {value:#x}");
}

#[cfg(test)]
mod tests {
    use super::{BLOCK, valid_crc};

    #[test]
    fn a_torn_setup_fails_the_wire_crc_check() {
        // Fresh SETUP from the black FM-1 EPH3 run, token e33a.
        let mut packet = [0u8; BLOCK];
        packet[..10].copy_from_slice(&[0x40, 0x7d, 0xc4, 0x5b, 0x3a, 0xe3, 0, 0, 0x9e, 0xa4]);
        assert!(valid_crc(&packet, 8));
        packet[8..10].fill(0xa5);
        assert!(!valid_crc(&packet, 8));
        // A legal empty USB data packet has a zero wire CRC.
        assert!(valid_crc(&[0; BLOCK], 0));
    }
}
