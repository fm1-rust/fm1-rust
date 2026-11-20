// SPDX-FileCopyrightText: 2026 Julius Vitkauskas
// SPDX-License-Identifier: MIT-0
//! AC791N TIMER4 and TIMER5, both referenced to the 24 MHz crystal.

use crate::{
    Token,
    registers::{read, write},
};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};
use embedded_hal::delay::DelayNs;

const T4_CON: usize = 0x10800;
const T4_CNT: usize = T4_CON + 4;
pub(crate) const T5_CON: usize = 0x10900;
const ICFG_WORDS: usize = 0x01ee_f100;
const SOURCE: usize = 63;
static TICKS: AtomicU32 = AtomicU32::new(0);

unsafe extern "C" {
    fn fm1_timer5_isr();
    fn fm1_set_icfg(value: u32);
}

/// Called exclusively by the C interrupt trampoline (Rust has no pi32v2 interrupt ABI).
#[cfg(any(fm1, test))]
#[cfg_attr(fm1, unsafe(no_mangle))]
pub(crate) extern "C" fn rust_timer5_tick() {
    tick();
}

fn tick() {
    crate::registers::set(T5_CON, 0x4000);
    TICKS.store(TICKS.load(Relaxed).wrapping_add(1), Relaxed);
}

/// Clock frequencies are fixed for this boot configuration.
#[derive(Debug)]
pub struct Timers(Token);
impl Timers {
    /// TIMER4 crystal counts per second.
    pub const CRYSTAL_HZ: u32 = 24_000_000;
    /// TIMER5 interrupts per second.
    pub const TICK_HZ: u32 = 100;

    pub(crate) fn start() -> Self {
        write(T4_CON, 0);
        write(T4_CON + 8, u32::MAX);
        write(T4_CNT, 0);
        write(T4_CON, 0x4009);
        write(T5_CON, 0x4000);
        write(T5_CON + 4, 0);
        write(T5_CON + 8, Self::CRYSTAL_HZ / 4 / Self::TICK_HZ - 1);
        write(T5_CON, 0x4019);
        Self(Token::new())
    }

    /// Acknowledge TIMER5 and count one tick. Call once per TIMER5 interrupt
    /// when the application owns the vector.
    pub fn on_interrupt(&mut self) {
        tick();
    }

    /// Free-running 24 MHz count. Use `wrapping_sub` across rollover.
    #[must_use]
    pub fn cycles(&self) -> u32 {
        cycles()
    }
    /// Wrapping interrupt count, in 10 ms units.
    #[must_use]
    pub fn ticks(&self) -> u32 {
        ticks()
    }
}

/// Read the wrapping 100 Hz tick count from any task on CPU0.
#[must_use]
pub fn ticks() -> u32 {
    TICKS.load(Relaxed)
}

pub(crate) fn install_tick_interrupt() {
    // crt0 left global interrupts masked. Initialize the controller once,
    // before user code can install other handlers.
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "There are 32 aligned interrupt configuration words."
    )]
    for i in 0..32 {
        write(ICFG_WORDS + 4 * i, 0);
    }
    write(0x01ee_f1a4, 0xff);
    write(
        0x01c7_fe00 + 4 * SOURCE,
        crate::registers::bus_address(fm1_timer5_isr as *const ()),
    );
    write(ICFG_WORDS + 4 * (SOURCE / 8), 3 << (4 * (SOURCE % 8)));
    unsafe { fm1_set_icfg(0x300) };
}

pub(crate) fn cycles() -> u32 {
    read(T4_CNT)
}
pub(crate) fn wait_cycles(span: u32) {
    let start = cycles();
    while cycles().wrapping_sub(start) < span {}
}
#[allow(
    clippy::arithmetic_side_effects,
    reason = "The result is at most ceil(u32::MAX * 3 / 125)."
)]
fn ns_to_cycles(ns: u32) -> u32 {
    // 24 MHz = 3 cycles per 125 ns. Split before multiplying to fit a u32.
    (ns / 125) * 3 + ((ns % 125) * 3).div_ceil(125)
}
impl DelayNs for Timers {
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "The maximum converted duration leaves room for the extra tick."
    )]
    fn delay_ns(&mut self, ns: u32) {
        // An extra tick covers starting immediately before a counter transition.
        wait_cycles(ns_to_cycles(ns) + 1);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn delay_rounds_up_without_overflow() {
        for ns in [0, 1, 41, 42, 124, 125, 126, u32::MAX] {
            assert_eq!(
                u64::from(super::ns_to_cycles(ns)),
                (u64::from(ns) * 3).div_ceil(125)
            );
        }
    }
}
