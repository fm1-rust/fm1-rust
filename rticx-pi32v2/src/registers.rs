//! CPU0 interrupt controller register and RAM vector addresses.
use core::ptr::without_provenance_mut;

// Eight four-bit configurations per word, 128 sources in total. Bit zero
// enables the source and the upper three bits select its hardware priority.
pub(crate) const CONFIG: *mut u32 = without_provenance_mut(0x01ee_f100);

// The host mask tests can also supply an allocated register bank.
pub(crate) fn config_word(base: *mut u32, word: usize) -> *mut u32 {
    base.wrapping_add(word)
}

pub(crate) fn vector_slot(irq: usize) -> *mut unsafe extern "C" fn() {
    const VECTORS: *mut unsafe extern "C" fn() = without_provenance_mut(0x01c7_fe00);
    VECTORS.wrapping_add(irq)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn address_calculation_needs_no_backing_allocation() {
        const SECOND_VECTOR: usize = 0x01c7_fe00 + size_of::<unsafe extern "C" fn()>();
        // Exercise the same nonzero offsets as startup without touching hardware.
        assert_eq!(config_word(CONFIG, 1).addr(), 0x01ee_f104);
        assert_eq!(vector_slot(1).addr(), SECOND_VECTOR);
    }
}
