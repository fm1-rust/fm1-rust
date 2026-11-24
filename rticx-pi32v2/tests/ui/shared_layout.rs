//! Target fixture for task dispatch and shared-field alignment.
#![no_std]
#![cfg(target_os = "none")]
#![forbid(unsafe_code)]
#![cfg_attr(deref_shared, allow(dangerous_implicit_autorefs))]
#![allow(
    missing_docs,
    reason = "RTICX generates public task traits and initializers."
)]

#[rticx_pi32v2::app(device = crate, dispatchers = [])]
mod app {
    // Application names take precedence over RTICX's glob re-export.
    pub trait RticTask {
        fn exec(&'static mut self);
    }
    #[cfg(not(capture_task))]
    pub trait RticIdleTask {
        fn exec(&'static mut self) -> !;
    }

    pub struct Inner {
        second: u32,
    }

    #[shared]
    #[cfg(not(coerced_field))]
    #[cfg_attr(packed_shared, repr(C, packed))]
    struct Shared {
        first: Inner,
        pad: u8,
        #[cfg(not(deref_shared))]
        second: u32,
    }

    // The macro saw a u32. The compiled field has the same name and can
    // deref to u32, but its borrow guard cannot cross an interrupt boundary.
    #[cfg(coerced_field)]
    struct Shared {
        first: Inner,
        pad: u8,
        second: core::cell::RefMut<'static, u32>,
    }

    // With the direct field removed, ordinary field access finds first.second.
    #[cfg(deref_shared)]
    mod forwarded_field {
        use super::{Inner, Shared};

        impl core::ops::Deref for Shared {
            type Target = Inner;
            fn deref(&self) -> &Inner {
                &self.first
            }
        }

        impl core::ops::DerefMut for Shared {
            fn deref_mut(&mut self) -> &mut Inner {
                &mut self.first
            }
        }
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        #[cfg(coerced_field)]
        static CELL: rticx_pi32v2::asynchronous::ConstStaticCell<core::cell::RefCell<u32>> =
            rticx_pi32v2::asynchronous::ConstStaticCell::new(core::cell::RefCell::new(2));
        (
            Shared {
                first: Inner { second: 1 },
                pad: 0,
                #[cfg(not(any(deref_shared, coerced_field)))]
                second: 2,
                #[cfg(coerced_field)]
                second: CELL.take().borrow_mut(),
            },
            TaskInits { tick: Tick(0) },
        )
    }

    #[idle(init = generated)]
    struct Idle;

    // The extra argument makes method-call syntax fail to compile on either task.
    impl Idle {
        #[allow(dead_code)]
        fn exec(&'static mut self, _: ()) -> ! {
            loop {
                core::hint::spin_loop()
            }
        }
    }
    impl rticx_traits::RticIdleTask for Idle {
        fn exec(&mut self) -> ! {
            loop {
                core::hint::spin_loop()
            }
        }
    }

    #[task(binds = TIMER5, priority = 1, shared = [first, second])]
    struct Tick(u32);

    // An inherent method must never capture the runtime's static task borrow.
    impl Tick {
        #[allow(dead_code)]
        fn exec(&'static mut self, _: ()) {}
    }
    #[cfg(not(capture_task))]
    impl rticx_traits::RticTask for Tick {
        fn exec(&mut self) {
            self.0 = self.0.wrapping_add(1);
            self.shared().first.lock(|inner| {
                self.shared().second.lock(|word| {
                    *word = 4;
                    inner.second = 3;
                });
            });
        }
    }

    // Placing this impl in a module lets rustc select it after macro expansion.
    #[cfg(capture_task)]
    mod capture {
        use super::{RticTask, Tick};
        use core::cell::Cell;
        use critical_section::Mutex;

        impl RticTask for Tick {
            fn exec(&'static mut self) {
                static SAVED: Mutex<Cell<Option<&'static mut Tick>>> = Mutex::new(Cell::new(None));
                critical_section::with(|cs| {
                    if let Some(previous) = SAVED.borrow(cs).replace(Some(self)) {
                        previous.0 = previous.0.wrapping_add(1);
                    }
                });
            }
        }
    }
}
