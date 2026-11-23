//! Regression for overlapping executor storage in an optimized RTICX entry.
use core::{
    mem::ManuallyDrop,
    ptr,
    sync::atomic::{AtomicPtr, Ordering::Relaxed},
};
use rticx_pi32v2_macro::__entry;

mod __rticx_backend {
    pub mod export {
        /// # Safety
        /// This test double has no preconditions. Its signature matches the target backend.
        pub unsafe fn reset() {}
    }
}

struct SlotPtr(AtomicPtr<u32>);
impl SlotPtr {
    const fn new() -> Self {
        Self(AtomicPtr::new(ptr::null_mut()))
    }

    /// # Safety
    /// This test double matches the unsafe production installer. It only
    /// records the address for comparison, so it has no caller preconditions.
    unsafe fn install(&self, slot: &u32) {
        self.0.store(ptr::from_ref(slot).cast_mut(), Relaxed);
    }
}
static FIRST: SlotPtr = SlotPtr::new();
static SECOND: SlotPtr = SlotPtr::new();

// The async pass emits one such block per task. Only compare addresses while
// entry is running. The target entry lives for the lifetime of the application.
#[__entry]
fn distinct_slots() -> bool {
    {
        let __s = ManuallyDrop::new(0u32);
        FIRST.store(ptr::from_ref(&*__s).cast_mut(), Relaxed);
    }
    {
        let __s = ManuallyDrop::new(0u32);
        SECOND.store(ptr::from_ref(&*__s).cast_mut(), Relaxed);
    }
    FIRST.0.load(Relaxed) != SECOND.0.load(Relaxed)
}

#[test]
fn executor_slots_have_distinct_addresses() {
    // SAFETY: this test is the sole caller of the generated entry. Its backend
    // reset is inert and it only compares task-slot addresses while they are live.
    assert!(unsafe { distinct_slots() });
}
