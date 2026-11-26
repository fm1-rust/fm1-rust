// SPDX-License-Identifier: MPL-2.0
//! Supply status, reset cause and watchdog control on the AC791N.
//! The register protocol follows Felucca's `fm1_sys.h`, and the status bits
//! follow the AC79 SDK's `p33.h` and fm1-base's `plat_battery_charging`.

use crate::{Board, Token, registers};
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const CON: usize = 0x13e08;
const DAT: usize = 0x13e0c;
const SELECT: u32 = 1;
const BUSY: u32 = 1 << 1;
const START: u32 = 1 << 4;
const RTC: u32 = 1 << 8;
static CLAIMED: AtomicBool = AtomicBool::new(false);
const READ: u8 = 0x80;
const WRITE: u8 = 0;
const OR: u8 = 1 << 5;
const AND: u8 = 2 << 5;
const WDT_CON: u8 = 0x80;
const WDT_ENABLE: u8 = 1 << 4;
const WDT_CLEAR: u8 = 1 << 6;
#[cfg(any(fm1, test))]
const PR_PWR: u8 = 0xa0;
#[cfg(any(fm1, test))]
const CHIP_RESET: u8 = 1 << 4;

// CPU0 publishes these once during Board initialization, before handing out Power.
static SYSTEM_RESET: AtomicU32 = AtomicU32::new(0);
static P33_RESET: AtomicU32 = AtomicU32::new(u32::MAX);

#[cfg(any(fm1, test))]
pub(crate) fn snapshot_reset_cause() {
    SYSTEM_RESET.store(registers::read(0x100c0), Ordering::Relaxed);
    let flags = read_p33(0x12).map_or(u32::MAX, u32::from);
    P33_RESET.store(flags, Ordering::Relaxed);
}

/// Reset-source bits as the bootloader left them at board startup.
/// More than one flag can be set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResetCause {
    /// Raw P3_RST_SRC, or `None` if the P33 bridge was busy or timed out.
    pub p33: Option<u8>,
    /// Raw system RST_SRC.
    pub system: u32,
}

impl ResetCause {
    /// Whether P33 reported power-on.
    #[must_use]
    pub fn power_on(self) -> Option<bool> {
        self.p33.map(|flags| flags & 1 != 0)
    }

    /// Whether P33 reported watchdog expiry.
    #[must_use]
    pub fn watchdog(self) -> Option<bool> {
        self.p33.map(|flags| flags & (1 << 2) != 0)
    }

    /// Whether either reset source reported a software reset.
    #[must_use]
    pub fn software(self) -> Option<bool> {
        if self.system & (1 << 5) != 0 {
            Some(true)
        } else {
            self.p33.map(|flags| flags & (1 << 6) != 0)
        }
    }
}

/// Nominal watchdog periods in seconds, selected by the AC791N's P33 clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum WatchdogTimeout {
    /// One second.
    Sec1 = 0xa,
    /// Two seconds.
    Sec2,
    /// Four seconds.
    Sec4,
    /// Eight seconds.
    Sec8,
    /// Sixteen seconds.
    Sec16,
    /// Thirty-two seconds.
    Sec32,
}

/// CPU0's exclusive access to the P33 serial bridge.
#[derive(Debug)]
pub struct Power(Token);

impl Board {
    /// Claim the power controller before moving other board fields into tasks.
    /// Dropping the handle releases the bridge. An armed watchdog keeps running.
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

/// The bridge could not complete a transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    /// The bridge was already selected or a previous transfer was still busy.
    Busy,
    /// A byte did not complete within the polling budget.
    Timeout,
}

impl Power {
    /// Return the boot snapshot, including after dropping and reclaiming the handle.
    #[must_use]
    pub fn reset_cause(&self) -> ResetCause {
        ResetCause {
            p33: u8::try_from(P33_RESET.load(Ordering::Relaxed)).ok(),
            system: SYSTEM_RESET.load(Ordering::Relaxed),
        }
    }

    /// Arm the watchdog in chip-reset mode, with a fresh countdown.
    /// The application must feed it while the work it supervises makes progress.
    ///
    /// # Errors
    /// Returns a bridge error if setup fails. Earlier writes may have reached
    /// P33. Retry the complete operation or explicitly disarm the watchdog.
    pub fn arm_watchdog(&mut self, timeout: WatchdogTimeout) -> Result<(), Error> {
        self.disarm_watchdog()?;
        transfer(AND, 0x17, !(1 << 6))?; // P3_VLD_KEEP.WDT_EXPT_EN
        transfer(WRITE, WDT_CON, timeout as u8 | WDT_CLEAR)?;
        transfer(OR, WDT_CON, WDT_ENABLE)?;
        Ok(())
    }

    /// Restart the current countdown. Feeding a disabled watchdog leaves it disabled.
    ///
    /// # Errors
    /// A bridge error means the feed could not be confirmed.
    pub fn feed_watchdog(&mut self) -> Result<(), Error> {
        transfer(OR, WDT_CON, WDT_CLEAR)?;
        Ok(())
    }

    /// Disable the watchdog.
    ///
    /// # Errors
    /// A bridge error means the watchdog could still be running.
    pub fn disarm_watchdog(&mut self) -> Result<(), Error> {
        transfer(WRITE, WDT_CON, 0)?;
        Ok(())
    }

    /// Restart the installed firmware through the normal bootloader path.
    /// Request a P33 chip reset. If the bridge is unavailable, use the
    /// system reset controller. Both paths leave the download mailbox alone.
    ///
    /// # Panics
    /// A host build has no reset controller.
    #[cfg_attr(
        not(fm1),
        allow(clippy::panic, reason = "A host has no reset controller.")
    )]
    pub fn reboot(&mut self) -> ! {
        #[cfg(fm1)]
        {
            unsafe extern "C" {
                fn fm1_cpu1_try_hold() -> core::ffi::c_int;
            }
            crate::prepare_reset();
            // SAFETY: CPU0 owns the terminal reset path with its IRQs masked.
            // Still request a whole-chip reset if CPU1 fails to acknowledge hold.
            unsafe { fm1_cpu1_try_hold() };
            if request_chip_reset().is_err() {
                crate::reset_cores();
            }
            loop {
                core::hint::spin_loop();
            }
        }
        #[cfg(not(fm1))]
        panic!("reboot requires an FM-1")
    }

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
    transfer(READ, address, 0x5e)
}

#[cfg(any(fm1, test))]
pub(crate) fn request_chip_reset() -> Result<u8, Error> {
    transfer(OR, PR_PWR, CHIP_RESET)
}

fn transfer(command: u8, address: u8, data: u8) -> Result<u8, Error> {
    let idle = registers::read(CON) & !START;
    if idle & (SELECT | BUSY) != 0 {
        return Err(Error::Busy);
    }
    let selected = (idle & !RTC) | SELECT;
    registers::write(CON, selected);
    for byte in [command, address, data] {
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
