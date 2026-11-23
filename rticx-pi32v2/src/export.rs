//! Implementation details used by the application macro.
use crate::Interrupt;
use crate::mask::WORDS;
use crate::registers::{CONFIG, config_word, vector_slot};
pub use crate::{mask::SourceMask, queue::Queue};
/// The thread-handoff bound used by the application macro.
pub use core::marker::Send as HandoffSend;
/// Storage types used by the application entry for task values and executor slots.
pub use core::mem::{ManuallyDrop, MaybeUninit as TaskStorage};
use core::{
    cell::Cell,
    ptr::{read_volatile, write_volatile},
    sync::atomic::{AtomicBool, Ordering},
};
pub use critical_section;

const LATCH_SET: *mut u32 = 0x01ee_f1a0 as _;
const LATCH_CLEAR: *mut u32 = 0x01ee_f1a4 as _;
static INITIALIZED: AtomicBool = AtomicBool::new(false);

unsafe extern "C" {
    fn __rticx_enable();
    fn __rticx_irq_save() -> u32;
    fn __rticx_irq_restore(state: u32);
    /// Wait for an interrupt on CPU0.
    pub fn __rticx_idle();
}

/// Disable the CPU0 interrupt sources. Leave unused RAM vectors intact:
/// vector 126 also holds the CPU1 entry address.
///
/// # Safety
/// Called once from CPU0 startup with interrupts disabled.
pub unsafe fn reset() {
    for word in 0..WORDS {
        // SAFETY: word indexes the CPU0 configuration bank. Startup owns it with IRQs masked.
        unsafe { write_volatile(config_word(CONFIG, word), 0) };
    }
    // SAFETY: LATCH_CLEAR is the word-aligned, write-one-to-clear CPU0 software IRQ register.
    unsafe { write_volatile(LATCH_CLEAR, 0xff) };
}

/// Install a native interrupt entry while initialising the application.
///
/// # Safety
/// CPU0 IRQs must be disabled. The vector must save interrupted state and
/// return with `rti`. Priority must be in the RTICX range 1..=8.
pub unsafe fn install(irq: Interrupt, priority: u16, vector: unsafe extern "C" fn()) {
    // SAFETY: every Interrupt discriminant is a CPU0 source below 128. The caller
    // supplies the native vector and priority while interrupts are masked.
    unsafe { install_at(irq as usize, priority, vector) };
}

/// Install the TIMER2 entry for the board monotonic and its nesting trampoline.
///
/// # Safety
/// Same as [`install`]. The board driver must exclusively own TIMER2.
pub unsafe fn install_timer2(priority: u16) {
    unsafe extern "C" {
        fn __rticx_irq_TIMER2();
    }
    const TIMER2_IRQ: usize = 6; // AC79 SDK, asm/hwi.h: IRQ_TIMER2_IDX
    // SAFETY: interrupt.S supplies the native TIMER2 trampoline for slot 6.
    // The caller owns TIMER2 and supplies the priority with IRQs masked.
    unsafe { install_at(TIMER2_IRQ, priority, __rticx_irq_TIMER2) };
}

/// Install a CPU0 RAM vector by its hardware index.
///
/// # Safety
/// The index must be below 128. The startup and vector requirements of [`install`] apply.
unsafe fn install_at(n: usize, priority: u16, vector: unsafe extern "C" fn()) {
    let shift = (n % 8) * 4;
    let config = ((u32::from(priority) - 1) << 1) | 1;
    // SAFETY: n < 128 selects a reserved RAM vector and one of 16 aligned MMIO
    // words. The caller supplies a valid handler and excludes interrupt dispatch.
    unsafe {
        write_volatile(vector_slot(n), vector);
        let reg = config_word(CONFIG, n / 8);
        write_volatile(
            reg,
            (read_volatile(reg) & !(15 << shift)) | (config << shift),
        );
    }
}

/// Enable dispatch after task, executor and vector initialisation.
///
/// # Safety
/// All task state and every enabled vector must be initialised.
pub unsafe fn enable() {
    INITIALIZED.store(true, Ordering::Relaxed);
    // SAFETY: the caller has initialized the state and handlers the shim enables.
    unsafe { __rticx_enable() };
}

/// Compute the latch bit for a dispatcher. Invalid bindings fail at compile time.
#[must_use]
pub const fn soft_bit(irq: Interrupt) -> u32 {
    assert!(
        irq as u8 >= Interrupt::SOFT0 as u8,
        "a dispatcher must be SOFT0..SOFT7"
    );
    1 << ((irq as u8) - (Interrupt::SOFT0 as u8))
}

/// Pend a software interrupt on CPU0.
pub fn pend(irq: Interrupt) {
    // SAFETY: LATCH_SET takes an aligned word write. soft_bit selects a software IRQ.
    unsafe { write_volatile(LATCH_SET, soft_bit(irq)) };
}

/// Clear a software interrupt before running its dispatcher, so a new wake
/// during dispatch stays pending.
pub fn acknowledge(irq: Interrupt) {
    // SAFETY: LATCH_CLEAR takes an aligned word write. Only the selected IRQ is cleared.
    unsafe { write_volatile(LATCH_CLEAR, soft_bit(irq)) };
}

/// A shared field may have multiple RTICX proxies, including two obtained
/// by the same task. Track the active borrow for the field itself.
#[derive(Debug)]
pub struct BorrowFlag(critical_section::Mutex<Cell<bool>>);

impl BorrowFlag {
    /// Flag for one shared field.
    pub const FREE: Self = Self(critical_section::Mutex::new(Cell::new(false)));
}

struct Restore<'a> {
    enabled: SourceMask,
    flag: &'a BorrowFlag,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        critical_section::with(|cs| {
            self.flag.0.borrow(cs).set(false);
            // SAFETY: this guard restores the same bank after nested guards have
            // dropped. The CPU0 critical section excludes interrupt access.
            unsafe { self.enabled.restore(CONFIG) };
        });
    }
}

/// Run a critical section at the resource ceiling. Global masking only
/// protects the register updates. Higher-priority tasks can preempt
/// the closure. Nested locks restore only bits they actually disabled.
///
/// # Safety
/// The mask must cover all tasks that access the protected resource. CPU0
/// must be the only core using the controller and resource.
pub unsafe fn with_ceiling<R>(mask: &SourceMask, flag: &BorrowFlag, f: impl FnOnce() -> R) -> R {
    let restore = critical_section::with(|cs| {
        assert!(
            INITIALIZED.load(Ordering::Relaxed),
            "resource accessed before startup"
        );
        assert!(
            !flag.0.borrow(cs).replace(true),
            "resource already borrowed"
        );
        Restore {
            // SAFETY: CONFIG is the 16-word bank owned by CPU0 and IRQs are masked here.
            enabled: unsafe { mask.disable(CONFIG) },
            flag,
        }
    });
    let result = f();
    drop(restore);
    result
}

struct Cpu0;
critical_section::set_impl!(Cpu0);
// SAFETY: only CPU0 executes code using this backend or its protected state.
// The CPU1 diagnostics link a fixed SRAM worker with no path to either.
// The shims mask CPU0 IRQs and restore the full prior ICFG on nested release.
unsafe impl critical_section::Impl for Cpu0 {
    unsafe fn acquire() -> u32 {
        // SAFETY: this shim saves the current ICFG before masking IRQs on the active CPU.
        unsafe { __rticx_irq_save() }
    }
    unsafe fn release(state: u32) {
        // SAFETY: release receives the saved state from the matching acquire on CPU0.
        unsafe { __rticx_irq_restore(state) };
    }
}
