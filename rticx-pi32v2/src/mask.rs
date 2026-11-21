//! CPU0 interrupt enable bits for a resource ceiling.
use crate::Interrupt;
use core::ptr::{read_volatile, write_volatile};

pub(crate) const WORDS: usize = 16;

/// One enable bit per four-bit source configuration, eight sources per word.
#[derive(Clone, Copy, Debug)]
pub struct SourceMask([u32; WORDS]);

impl SourceMask {
    /// Collect enabled-source bit positions up to the given logical priority.
    #[must_use]
    pub const fn through(irqs: &[(Interrupt, u16)], ceiling: u16) -> Self {
        let mut mask = [0; WORDS];
        let mut remaining = irqs;
        while let [(irq, priority), rest @ ..] = remaining {
            if *priority <= ceiling {
                let n = *irq as usize;
                mask[n / 8] |= 1 << ((n % 8) << 2);
            }
            remaining = rest;
        }
        Self(mask)
    }

    // CONFIG must point to the 16-word register bank and IRQs must be masked
    // during each of these operations. Save only bits this lock actually clears.
    pub(crate) unsafe fn disable(&self, config: *mut u32) -> Self {
        let mut saved = Self([0; WORDS]);
        for (word, &bits) in self.0.iter().enumerate() {
            if bits != 0 {
                unsafe {
                    let reg = config.wrapping_add(word);
                    let old = read_volatile(reg);
                    saved.0[word] = old & bits;
                    write_volatile(reg, old & !bits);
                }
            }
        }
        saved
    }

    pub(crate) unsafe fn restore(self, config: *mut u32) {
        for (word, bits) in self.0.into_iter().enumerate() {
            if bits != 0 {
                unsafe {
                    let reg = config.wrapping_add(word);
                    write_volatile(reg, read_volatile(reg) | bits);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Interrupt::{SOFT0, SOFT1, SOFT2, SOFT3, TIMER5};

    #[test]
    fn nested_ceilings_preserve_priorities_and_disabled_sources() {
        let irqs = [(SOFT0, 1), (SOFT1, 2), (SOFT2, 3), (SOFT3, 2), (TIMER5, 2)];
        let mut bank = [0; WORDS];
        bank[15] = 0x2531; // SOFT3 already disabled
        bank[7] = 0x3000_0000; // TIMER5 in a different word
        unsafe {
            let outer = SourceMask::through(&irqs, 1).disable(bank.as_mut_ptr());
            assert_eq!(bank[15], 0x2530);
            let inner = SourceMask::through(&irqs, 2).disable(bank.as_mut_ptr());
            assert_eq!(bank[7], 0x2000_0000);
            assert_eq!(bank[15], 0x2520);
            bank[15] |= 0x1_0000; // unrelated source changed during the closure
            inner.restore(bank.as_mut_ptr());
            assert_eq!(bank[15], 0x1_2530);
            outer.restore(bank.as_mut_ptr());
        }
        assert_eq!(bank[7], 0x3000_0000);
        assert_eq!(bank[15], 0x1_2531);
    }
}
