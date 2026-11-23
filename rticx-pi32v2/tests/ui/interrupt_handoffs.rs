//! The target CI job checks each rejected handoff independently.
#![no_std]
#![allow(
    missing_docs,
    reason = "RTICX generates public task and initializer types."
)]

#[cfg(target_os = "none")]
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
