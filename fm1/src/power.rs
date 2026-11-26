// SPDX-License-Identifier: MPL-2.0
//! Read the AC791N's 5 V detector and on-chip charger through its P33 bridge.
//! The register protocol follows Felucca's `fm1_sys.h`, and the status bits
//! follow the AC79 SDK's `p33.h` and fm1-base's `plat_battery_charging`.

use crate::{Board, Token, registers};
use core::sync::atomic::{AtomicBool, Ordering};

const CON: usize = 0x13e08;
const DAT: usize = 0x13e0c;
const SELECT: u32 = 1;
const BUSY: u32 = 1 << 1;
const START: u32 = 1 << 4;
const RTC: u32 = 1 << 8;
static CLAIMED: AtomicBool = AtomicBool::new(false);

/// CPU0's exclusive access to the P33 serial bridge.
#[derive(Debug)]
pub struct Power(Token);

impl Board {
    /// Claim the power monitor before moving other board fields into tasks.
    /// Dropping the handle releases the bridge.
    #[must_use]
    pub fn take_power(&self) -> Option<Power> {
        (!CLAIMED.swap(true, Ordering::Acquire)).then(|| Power(Token::new()))
    }
}

impl Drop for Power {
    fn drop(&mut self) {
        CLAIMED.store(false, Ordering::Release);
    }
}

/// Three independent hardware indications, sampled in sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SupplyStatus {
    /// P33 detects 5 V on the supply input, independent of USB enumeration.
    pub external_power: bool,
    /// The AC791N's internal charger is enabled.
    pub charger_enabled: bool,
    /// The AC791N's internal charge-full flag is set.
    pub charge_full: bool,
}

impl SupplyStatus {
    /// Whether the internal charger indicates charging. A separate charger on
    /// the board may operate independently of these flags.
    #[must_use]
    pub const fn is_charging(self) -> bool {
        self.external_power && self.charger_enabled && !self.charge_full
    }
}

/// The bridge could not complete a read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// The bridge was already selected or a previous transfer was still busy.
    Busy,
    /// A byte did not complete within the polling budget.
    Timeout,
}

impl Power {
    /// Sample the supply detector and charger flags. Call from a low-priority
    /// task. Panel and audio interrupts may preempt the exclusive bridge owner.
    /// No power-management or charger setting is changed.
    ///
    /// # Errors
    /// Returns [`Error::Busy`] or [`Error::Timeout`] without publishing a partial sample.
    pub fn supply_status(&mut self) -> Result<SupplyStatus, Error> {
        Ok(SupplyStatus {
            external_power: read_p33(0xa1)? & (1 << 5) != 0,
            charger_enabled: read_p33(0x1a)? & 1 != 0,
            charge_full: read_p33(0x19)? & 1 != 0,
        })
    }
}

fn read_p33(address: u8) -> Result<u8, Error> {
    let idle = registers::read(CON) & !START;
    if idle & (SELECT | BUSY) != 0 {
        return Err(Error::Busy);
    }
    let selected = (idle & !RTC) | SELECT;
    registers::write(CON, selected);
    // A P33 read shifts a read opcode, an 8-bit address and a dummy byte.
    for byte in [0x80, address, 0x5e] {
        registers::write(DAT, u32::from(byte));
        registers::write(CON, selected | START);
        // Read back the posted start before testing BUSY (Felucca's p33_xfer).
        let _ = registers::read(CON);
        if !(0..1024).any(|_| registers::read(CON) & BUSY == 0) {
            registers::write(CON, idle);
            return Err(Error::Timeout);
        }
    }
    let value = registers::read(DAT).to_le_bytes()[0];
    registers::write(CON, idle);
    Ok(value)
}
