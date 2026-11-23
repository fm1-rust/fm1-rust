//! Target fixture for task dispatch and shared-field alignment.
#![no_std]
#![allow(
    missing_docs,
    reason = "RTICX generates public task traits and initializers."
)]

#[cfg(target_os = "none")]
#[rticx_pi32v2::app(device = crate, dispatchers = [])]
mod app {
    #[shared]
    #[cfg_attr(packed_shared, repr(C, packed))]
    struct Shared {
        first: u32,
        pad: u8,
        second: u32,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        (
            Shared {
                first: 1,
                pad: 0,
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
            self.shared().first.lock(|word| *word = 3);
            self.shared().second.lock(|word| *word = 4);
        }
    }
}
