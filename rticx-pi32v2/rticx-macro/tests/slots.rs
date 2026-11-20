//! Regression for overlapping executor storage in an optimized RTICX entry.
use core::{
    mem::ManuallyDrop,
    ptr,
    sync::atomic::{AtomicPtr, Ordering::Relaxed},
};
use rticx_pi32v2_macro::__entry;

mod __rticx_backend {
    pub mod export {
        pub unsafe fn reset() {}
    }
}

static FIRST: AtomicPtr<u32> = AtomicPtr::new(ptr::null_mut());
static SECOND: AtomicPtr<u32> = AtomicPtr::new(ptr::null_mut());

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
    FIRST.load(Relaxed) != SECOND.load(Relaxed)
}

#[test]
fn executor_slots_have_distinct_addresses() {
    assert!(unsafe { distinct_slots() });
}
