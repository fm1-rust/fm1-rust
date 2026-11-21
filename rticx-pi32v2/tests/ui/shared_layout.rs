//! Compiled twice on pi32v2 to check field references in the generated lock.
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

    #[task(binds = TIMER5, priority = 1, shared = [first, second], init = generated)]
    struct Tick;

    impl RticTask for Tick {
        fn exec(&mut self) {
            self.shared().first.lock(|word| *word = 3);
            self.shared().second.lock(|word| *word = 4);
        }
    }
}
