//! TIMER2 counts the crystal to PRD, raises IRQ 6, and restarts from zero.
use crate::registers::write;

const CON: usize = 0x10600;
const CNT: usize = CON + 4;
const PRD: usize = CON + 8;
const CLEAR_PENDING: u32 = 1 << 14;
const COUNT_CRYSTAL: u32 = (2 << 2) | 1;

/// Exclusive handle to TIMER2's counter and period registers.
///
/// Use [`super::Timers::take_alarm`] to acquire it. The interrupt controller
/// determines which core services the alarm. All programming requires unique access.
#[derive(Debug)]
pub struct Alarm {
    _private: (),
}

impl Alarm {
    pub(super) fn new() -> Self {
        let mut alarm = Self { _private: () };
        alarm.cancel();
        alarm
    }

    /// Restart TIMER2 to request an interrupt after the given number of 24 MHz
    /// cycles. Zero requests the earliest interrupt. The flag remains pending
    /// until acknowledged. Call [`Self::cancel`] in the handler for a one-shot.
    pub fn arm_after(&mut self, cycles: u32) {
        self.cancel();
        write(CNT, 0);
        write(PRD, cycles.max(1));
        write(CON, COUNT_CRYSTAL);
    }

    /// Stop TIMER2 and clear its pending interrupt.
    pub fn cancel(&mut self) {
        write(CON, CLEAR_PENDING);
    }
}

impl Drop for Alarm {
    fn drop(&mut self) {
        self.cancel();
    }
}
