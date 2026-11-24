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
    pub struct Inner {
        second: u32,
    }

    #[shared]
    #[cfg_attr(packed_shared, repr(C, packed))]
    struct Shared {
        first: Inner,
        pad: u8,
        #[cfg(not(deref_shared))]
        second: u32,
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
        (
            Shared {
                first: Inner { second: 1 },
                pad: 0,
                #[cfg(not(deref_shared))]
                second: 2,
            },
            TaskInits {},
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
    impl RticIdleTask for Idle {
        fn exec(&mut self) -> ! {
            loop {
                core::hint::spin_loop()
            }
        }
    }

    #[task(binds = TIMER5, priority = 1, shared = [first, second], init = generated)]
    struct Tick;

    // An inherent method must never capture the runtime's static task borrow.
    impl Tick {
        #[allow(dead_code)]
        fn exec(&'static mut self, _: ()) {}
    }
    impl RticTask for Tick {
        fn exec(&mut self) {
            self.shared().first.lock(|inner| {
                self.shared().second.lock(|word| {
                    *word = 4;
                    inner.second = 3;
                });
            });
        }
    }
}
