//! Implementation details used by the application macro.
use crate::Interrupt;
use crate::mask::WORDS;
pub use crate::{mask::SourceMask, queue::Queue};
use core::{
    cell::Cell,
    ptr::{read_volatile, write_volatile},
    sync::atomic::{AtomicBool, Ordering},
};
pub use critical_section;

// Physical MMIO and vector addresses have no Rust backing allocation.
// Use wrapping offsets when computing a register or vector address.
// 128 vectors, eight four-bit source configurations per word. Bit zero
// enables the source and the upper three bits select its hardware priority.
const CONFIG: *mut u32 = 0x01ee_f100 as _;
const VECTORS: *mut unsafe extern "C" fn() = 0x01c7_fe00 as _;
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
        unsafe { write_volatile(CONFIG.wrapping_add(word), 0) };
    }
    unsafe { write_volatile(LATCH_CLEAR, 0xff) };
}

/// Install a native interrupt entry while initialising the application.
///
/// # Safety
/// CPU0 IRQs must be disabled. The vector must save interrupted state and
/// return with `rti`. Priority must be in the RTICX range 1..=8.
pub unsafe fn install(irq: Interrupt, priority: u16, vector: unsafe extern "C" fn()) {
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
    unsafe { install_at(TIMER2_IRQ, priority, __rticx_irq_TIMER2) };
}

unsafe fn install_at(n: usize, priority: u16, vector: unsafe extern "C" fn()) {
    let shift = (n % 8) * 4;
    let config = ((u32::from(priority) - 1) << 1) | 1;
    unsafe {
        write_volatile(VECTORS.wrapping_add(n), vector);
        let reg = CONFIG.wrapping_add(n / 8);
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
    unsafe { write_volatile(LATCH_SET, soft_bit(irq)) };
}

/// Clear a software interrupt before running its dispatcher, so a new wake
/// during dispatch stays pending.
pub fn acknowledge(irq: Interrupt) {
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
            enabled: unsafe { mask.disable(CONFIG) },
            flag,
        }
    });
    let result = f();
    drop(restore);
    result
}

// CPU1 is parked for this distribution. Saving the full ICFG value also
// preserves a caller's disabled state when critical sections nest.
struct Cpu0;
critical_section::set_impl!(Cpu0);
unsafe impl critical_section::Impl for Cpu0 {
    unsafe fn acquire() -> u32 {
        unsafe { __rticx_irq_save() }
    }
    unsafe fn release(state: u32) {
        unsafe { __rticx_irq_restore(state) };
    }
}
