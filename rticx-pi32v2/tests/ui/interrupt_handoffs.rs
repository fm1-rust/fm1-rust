//! The target CI job checks each rejected handoff independently.
#![no_std]
#![cfg(target_os = "none")]
#![forbid(unsafe_code)]
#![allow(
    missing_docs,
    reason = "RTICX generates public task and initializer types."
)]

// The core facade accepts every handoff. The runtime facade accepts none.
// The positive fixture and each negative fixture must use the genuine bounds.
extern crate rticx_pi32v2 as actual_runtime;
extern crate self as rticx_pi32v2;
pub use actual_runtime::*;
pub mod export {
    pub use crate::actual_runtime::export::*;
    pub trait HandoffSend {}
    #[cfg(forge_handoff)]
    impl<T: ?Sized> HandoffSend for T {}
}

extern crate core as real_core;
extern crate self as core;
pub use real_core::*;
pub mod marker {
    pub use crate::real_core::marker::*;

    /// This lookalike deliberately accepts every type.
    pub trait Send {}
    impl<T: ?Sized> Send for T {}
}

// A facade that replaces the wrapper cannot authorize its own backend safely.
#[cfg(forge_handoff)]
#[macro_export]
macro_rules! __expand_app {
    ([$($args:tt)*] $($app:tt)*) => {
        $crate::actual_runtime::__generate_app! {
            $crate [__rticx_origin::assume_genuine()] [$($args)*] $($app)*
        }
    };
}

#[rticx_pi32v2::app(device = crate, dispatchers = [SOFT0])]
mod app {
    use core::cell::Cell;
    #[cfg(bad_shared)]
    type SharedValue = &'static Cell<u32>;
    #[cfg(not(bad_shared))]
    type SharedValue = Cell<u32>;
    #[cfg(bad_local)]
    type LocalValue = &'static Cell<u32>;
    #[cfg(not(bad_local))]
    type LocalValue = u32;
    #[cfg(bad_input)]
    type Input = &'static Cell<u32>;
    #[cfg(not(bad_input))]
    type Input = u32;

    #[shared]
    struct Shared {
        mailbox: Option<SharedValue>,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        (Shared { mailbox: None }, TaskInits { tick: Tick(None) })
    }

    #[task(binds = TIMER5, priority = 2, shared = [mailbox])]
    struct Tick(Option<LocalValue>);
    impl RticTask for Tick {
        fn exec(&mut self) {
            let _ = self.0;
            self.shared().mailbox.lock(|mail| {
                mail.take();
            });
        }
    }

    #[async_task(priority = 1, shared = [mailbox], init = generated)]
    struct Work;
    impl RticAsyncTask for Work {
        type SpawnInput = Input;
        async fn exec(&mut self, _: Input) {
            // Spawn inputs cross priorities. This borrow stays on one dispatcher.
            let local = Cell::new(0);
            let borrowed = &local;
            rticx_pi32v2::asynchronous::yield_now::yield_now().await;
            borrowed.set(1);
            core::hint::black_box(borrowed.get());
        }
    }

    #[async_task(init = generated)]
    struct Background;
    impl RticAsyncTask for Background {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {}
    }
}
