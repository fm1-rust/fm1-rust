//! Private AC791N register access. All addresses are aligned 32-bit MMIO.
//! Hardware handles are constructed only by the CPU0 startup path.

/// The AC791N has a 32-bit address bus. Host fixtures model the low register word.
#[cfg_attr(target_pointer_width = "64", allow(clippy::cast_possible_truncation))]
pub(crate) fn bus_address<T>(ptr: *const T) -> u32 {
    ptr.addr() as u32
}

pub(crate) const PA: usize = 0x50000;
pub(crate) const PB: usize = 0x50040;
pub(crate) const PC: usize = 0x50080;
pub(crate) const PH: usize = 0x501c0;
pub(crate) const OUT: usize = 0;
pub(crate) const IN: usize = 4;
pub(crate) const DIR: usize = 8;
pub(crate) const DIE: usize = 12;
pub(crate) const PU: usize = 16;
pub(crate) const PD: usize = 20;
pub(crate) const HD0: usize = 24;
pub(crate) const HD: usize = 28;

#[cfg(not(test))]
pub(crate) fn read(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}
#[cfg(not(test))]
pub(crate) fn write(addr: usize, value: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, value) }
}
pub(crate) fn set(addr: usize, mask: u32) {
    write(addr, read(addr) | mask);
}
pub(crate) fn clear(addr: usize, mask: u32) {
    write(addr, read(addr) & !mask);
}
pub(crate) fn level(addr: usize, mask: u32, high: bool) {
    if high {
        set(addr, mask)
    } else {
        clear(addr, mask)
    }
}
#[allow(
    clippy::arithmetic_side_effects,
    reason = "All callers pass fixed AC791N GPIO base addresses and register offsets."
)]
pub(crate) fn outputs(port: usize, mask: u32) {
    set(port + DIE, mask);
    clear(port + OUT, mask);
    clear(port + DIR, mask);
}
#[allow(
    clippy::arithmetic_side_effects,
    reason = "All callers pass fixed AC791N GPIO base addresses and register offsets."
)]
pub(crate) fn pull_up_inputs(port: usize, mask: u32) {
    set(port + DIE, mask);
    set(port + DIR, mask);
    clear(port + PD, mask);
    set(port + PU, mask);
}

#[cfg(test)]
pub(crate) use crate::test_board::{read, write};
