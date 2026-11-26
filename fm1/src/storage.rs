//! Small, independently replaceable byte records on the FM-1's SPI NOR.
//! Link `fm1-storage.x` alongside the application script. Each slot owns two
//! 4 KiB sectors. Reads validate both copies and saves commit to the older
//! copy. Run these blocking operations with transport stopped on CPU0.
//! With a live CPU1, each raw transfer can wait up to 5 ms with CPU0
//! interrupts masked. Even a probe or read can miss a 2.90 ms audio refill
//! deadline and return [`Error::Busy`] before issuing a flash command.
//!
//! Choose a stable four-byte application identifier. Version and validate
//! the application's byte encoding separately from this storage format.
use crate::{Board, Token};
use core::sync::atomic::{AtomicBool, Ordering};

mod chip;
#[cfg(test)]
mod tests;

const PAGE: usize = 256;
const SECTOR: usize = 4096;
const COMMIT: usize = SECTOR - PAGE;
/// Number of independent records in the reserved 16 KiB partition.
pub const SLOT_COUNT: u8 = 2;
/// Largest encoded record. The first and last pages hold ownership and commit metadata.
pub const MAX_LEN: usize = SECTOR - 2 * PAGE;

/// A storage operation failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// CPU1 did not reach a checkpoint in time, or the flash controller was busy.
    Busy,
    /// The platform, flash part, or XIP mapping is unsupported.
    Unsupported,
    /// The device rejected an operation or its bytes failed readback.
    Io,
    /// The slot index or payload length is outside the reservation.
    OutOfRange,
    /// The destination needs at least this many bytes. It has not been modified.
    BufferTooSmall(usize),
    /// A nonblank sector has an unrecognized owner or storage format.
    Occupied,
    /// An owned slot contains data but has no intact committed copy.
    Corrupt,
}

/// CPU0's exclusive storage peripheral, acquired with the board.
#[derive(Debug)]
pub struct StoragePort(Token);

static CLAIMED: AtomicBool = AtomicBool::new(false);

/// Let CPU0 use flash while this worker is between jobs.
///
/// Call in CPU1's idle/dispatch loop, after publishing a completed job and
/// before taking the next. Also poll while waiting for CPU0 to finish setup
/// if it needs to open storage then. On CPU0 or a host this does nothing.
/// On CPU1 it may block with interrupts masked until one raw flash transfer
/// completes. A sector erase can exceed an audio deadline.
#[inline(always)] // Keep the worker's return address in the native checkpoint's link register.
pub fn cpu1_checkpoint() {
    chip::cpu1_checkpoint();
}

impl Board {
    /// Acquire the idle SPI NOR port before moving board fields into tasks.
    /// Returns `None` while another handle owns it. Acquisition performs no I/O.
    #[must_use]
    pub fn take_storage(&self) -> Option<StoragePort> {
        (!CLAIMED.swap(true, Ordering::Acquire)).then(|| StoragePort(Token::new()))
    }
}

impl Drop for StoragePort {
    fn drop(&mut self) {
        CLAIMED.store(false, Ordering::Release);
    }
}

impl StoragePort {
    /// Identify the part and open this application's records. This only probes the chip.
    /// Waiting for a live CPU1 can mask CPU0 interrupts for up to 5 ms before
    /// the probe returns [`Error::Busy`] without issuing a flash command.
    ///
    /// # Safety
    /// The firmware integration must reserve the four sectors mapped by
    /// `fm1-storage.x` exclusively for this store and copy the BSP's RAM
    /// routines at startup. Initialize the BSP's shared RAM before releasing
    /// a managed CPU1. During this call and every operation on the
    /// returned store, independently configured DMA, debug agents and
    /// unmaskable handlers must neither fetch from XIP nor use SPI0 or its
    /// pins. No independent agent may start, reset or redirect CPU1 during
    /// a transfer. The normal BSP display and audio DMA buffers are in SRAM.
    /// The driver masks CPU0 interrupts and requires a held/inactive peer or
    /// an acknowledgment from CPU1 inside the BSP's SRAM parking loop. A peer
    /// booted by the BSP requires this acknowledgment until its verified hold.
    /// With a custom bootstrap, the hardware must report RUN set and HOLD
    /// clear whenever the live peer could fetch from XIP during the operation.
    /// No condition extends past a completed call if the store is never
    /// used again. Dropping or forgetting the handle performs no I/O.
    ///
    /// # Errors
    /// Returns a controller or chip error if the bus cannot be used.
    pub unsafe fn open(&mut self, owner: [u8; 4]) -> Result<Store<'_>, Error> {
        // SAFETY: the caller supplies the system-wide exclusion and layout
        // contract for this probe and all subsequent uses of the borrowed store.
        unsafe { self.probe()? };
        Ok(Store { flash: self, owner })
    }
}

trait Nor: core::fmt::Debug {
    fn read(&mut self, offset: usize, out: &mut [u8]) -> Result<(), Error>;
    fn program(&mut self, offset: usize, bytes: &[u8]) -> Result<(), Error>;
    fn erase(&mut self, offset: usize) -> Result<(), Error>;
}

/// A borrowed application store. Calls finish synchronously and retain no data buffers.
#[derive(Debug)]
pub struct Store<'a> {
    flash: &'a mut dyn Nor,
    owner: [u8; 4],
}

#[derive(Clone, Copy, Debug)]
struct Record {
    generation: u32,
    len: usize,
    payload_crc: u32,
}

#[derive(Clone, Copy, Debug)]
enum Bank {
    Blank,
    Occupied,
    Incomplete,
    Valid(Record),
}

fn newest(banks: [Bank; 2]) -> Option<(usize, Record)> {
    match banks {
        [Bank::Valid(a), Bank::Valid(b)] => {
            let distance = b.generation.wrapping_sub(a.generation);
            Some(if (1..0x8000_0000).contains(&distance) {
                (1, b)
            } else {
                (0, a)
            })
        }
        [Bank::Valid(a), _] => Some((0, a)),
        [_, Bank::Valid(b)] => Some((1, b)),
        _ => None,
    }
}

// Reflected CRC-32/ISO-HDLC, streamed across metadata and payload.
fn crc32(mut state: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        state ^= u32::from(byte);
        for _ in 0..8 {
            state = (state >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(state & 1));
        }
    }
    state
}

#[allow(
    clippy::arithmetic_side_effects,
    reason = "Validated slot < 2, bank < 2 and record length <= 14 pages bound all partition offsets."
)]
impl Store<'_> {
    fn pair(&self, slot: u8) -> Result<([usize; 2], [u8; 12]), Error> {
        if slot >= SLOT_COUNT {
            return Err(Error::OutOfRange);
        }
        let start = usize::from(slot) * 2 * SECTOR;
        let mut tag = [0; 12];
        tag[..4].copy_from_slice(b"F1S1");
        tag[4..8].copy_from_slice(&self.owner);
        tag[8..12].copy_from_slice(&u32::from(slot).to_le_bytes());
        Ok(([start, start + SECTOR], tag))
    }

    fn blank(&mut self, base: usize) -> Result<bool, Error> {
        let mut page = [0; PAGE];
        for offset in (0..SECTOR).step_by(PAGE) {
            self.flash.read(base + offset, &mut page)?;
            if page != [0xff; PAGE] {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn inspect(&mut self, base: usize, tag: &[u8; 12]) -> Result<Bank, Error> {
        let mut found = [0; 12];
        self.flash.read(base, &mut found)?;
        if found != *tag {
            return Ok(if found == [0xff; 12] && self.blank(base)? {
                Bank::Blank
            } else {
                Bank::Occupied
            });
        }

        let mut header = [[0; 4]; 4];
        self.flash.read(base + COMMIT, header.as_flattened_mut())?;
        let [magic, generation, len, expected] = header;
        let Ok(len) = usize::try_from(u32::from_le_bytes(len)) else {
            return Ok(Bank::Incomplete);
        };
        if &magic != b"DONE" || len > MAX_LEN {
            return Ok(Bank::Incomplete);
        }
        let mut payload_crc = u32::MAX;
        let mut page = [0; PAGE];
        for offset in (0..len).step_by(PAGE) {
            let part = &mut page[..PAGE.min(len - offset)];
            self.flash.read(base + PAGE + offset, part)?;
            payload_crc = crc32(payload_crc, part);
        }
        let checksum = !crc32(crc32(payload_crc, tag), header[..3].as_flattened());
        Ok(if checksum == u32::from_le_bytes(expected) {
            Bank::Valid(Record {
                generation: u32::from_le_bytes(generation),
                len,
                payload_crc,
            })
        } else {
            Bank::Incomplete
        })
    }

    fn banks(&mut self, bases: [usize; 2], tag: &[u8; 12]) -> Result<[Bank; 2], Error> {
        Ok([self.inspect(bases[0], tag)?, self.inspect(bases[1], tag)?])
    }

    /// Restore the newest intact record. On a blank slot returns `Ok(None)`.
    /// A saved zero-length record returns `Ok(Some(0))`.
    /// Each raw read can mask CPU0 interrupts for up to 5 ms waiting for CPU1.
    /// A single load can issue several reads.
    ///
    /// # Errors
    /// Reports an occupied or corrupt slot when neither copy can be loaded.
    /// A short output buffer is untouched. On a device read error discard the output.
    pub fn load(&mut self, slot: u8, out: &mut [u8]) -> Result<Option<usize>, Error> {
        let (bases, tag) = self.pair(slot)?;
        let banks = self.banks(bases, &tag)?;
        let Some((index, record)) = newest(banks) else {
            return match banks {
                [Bank::Blank, Bank::Blank] => Ok(None),
                [Bank::Occupied, _] | [_, Bank::Occupied] => Err(Error::Occupied),
                _ => Err(Error::Corrupt),
            };
        };
        if out.len() < record.len {
            return Err(Error::BufferTooSmall(record.len));
        }
        for (page, part) in out[..record.len].chunks_mut(PAGE).enumerate() {
            self.flash.read(bases[index] + (1 + page) * PAGE, part)?;
        }
        if crc32(u32::MAX, &out[..record.len]) != record.payload_crc {
            return Err(Error::Io);
        }
        Ok(Some(record.len))
    }

    fn matches(&mut self, offset: usize, bytes: &[u8]) -> Result<bool, Error> {
        let mut readback = [0; PAGE];
        for (page, part) in bytes.chunks(PAGE).enumerate() {
            let actual = &mut readback[..part.len()];
            self.flash.read(offset + page * PAGE, actual)?;
            if actual != part {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn write_verified(&mut self, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        self.flash.program(offset, bytes)?;
        if self.matches(offset, bytes)? {
            Ok(())
        } else {
            Err(Error::Io)
        }
    }

    fn erase_verified(&mut self, base: usize) -> Result<(), Error> {
        self.flash.erase(base)?;
        if self.blank(base)? {
            Ok(())
        } else {
            Err(Error::Io)
        }
    }

    /// Replace one record. An identical save performs no erase or program.
    /// The previous committed copy survives interruption while its peer is written.
    ///
    /// # Errors
    /// Rejects an invalid slot, oversized record, or an unrecognized destination.
    /// On an I/O error during the final commit, load the slot to resolve the outcome.
    pub fn save(&mut self, slot: u8, bytes: &[u8]) -> Result<(), Error> {
        if bytes.len() > MAX_LEN {
            return Err(Error::OutOfRange);
        }
        let len = u32::try_from(bytes.len()).map_err(|_| Error::OutOfRange)?;
        let (bases, tag) = self.pair(slot)?;
        let banks = self.banks(bases, &tag)?;
        let (next, generation) = if let Some((current, record)) = newest(banks) {
            if bytes.len() == record.len && self.matches(bases[current] + PAGE, bytes)? {
                return Ok(());
            }
            (1 - current, record.generation.wrapping_add(1))
        } else {
            (0, 0)
        };
        if banks.iter().any(|bank| matches!(bank, Bank::Occupied)) {
            return Err(Error::Occupied);
        }
        let base = bases[next];
        if !matches!(banks[next], Bank::Blank) {
            self.erase_verified(base)?;
        }
        self.write_verified(base, &tag)?;
        for (page, part) in bytes.chunks(PAGE).enumerate() {
            self.write_verified(base + (1 + page) * PAGE, part)?;
        }
        let mut header = [
            *b"DONE",
            generation.to_le_bytes(),
            len.to_le_bytes(),
            [0; 4],
        ];
        let payload_crc = crc32(u32::MAX, bytes);
        let checksum = !crc32(crc32(payload_crc, &tag), header[..3].as_flattened());
        header[3] = checksum.to_le_bytes();
        self.write_verified(base + COMMIT, header.as_flattened())
    }

    /// Explicitly erase unrecognized sectors in this slot. This destroys any
    /// foreign contents there. It preserves every valid record for this owner.
    /// Use after an intentional firmware change or a torn ownership tag/erase.
    ///
    /// # Errors
    /// Reports invalid slot indices, bus errors, or an erase that failed verification.
    pub fn reclaim(&mut self, slot: u8) -> Result<(), Error> {
        let (bases, tag) = self.pair(slot)?;
        let banks = self.banks(bases, &tag)?;
        for (base, bank) in bases.into_iter().zip(banks) {
            if matches!(bank, Bank::Occupied) {
                self.erase_verified(base)?;
            }
        }
        Ok(())
    }
}
