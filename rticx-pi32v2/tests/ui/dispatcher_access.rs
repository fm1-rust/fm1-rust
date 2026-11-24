//! Safe application code cannot manufacture or reinstall an executor.
#![no_std]
#![cfg(target_os = "none")]
#![forbid(unsafe_code)]
#![allow(
    missing_docs,
    reason = "RTICX generates public task and initializer types."
)]

// A runtime facade may re-export the backend while defining its own token type.
#[cfg(forge_facade)]
extern crate rticx_pi32v2 as real_runtime;
#[cfg(forge_facade)]
extern crate self as rticx_pi32v2;
#[cfg(forge_facade)]
pub use real_runtime::*;
#[cfg(forge_facade)]
pub mod executor {
    pub use crate::real_runtime::executor::*;

    #[derive(Clone, Copy, Debug, Default)]
    pub struct DispatchToken;
    impl DispatchToken {
        pub const fn new() -> Self {
            Self
        }
    }
}

// Each by-value receiver wins over an implicit borrow of an Installed handle.
// Wrong arities make any unqualified call fail in the target build.
#[allow(dead_code)]
trait ExecutorMethods: Sized {
    fn set_pending(self, _: ()) {}
    fn try_allocate(self, _: ()) {}
    fn spawn(self) {}
    fn poll(self) {}
}
impl<F: core::future::Future<Output = ()> + 'static> ExecutorMethods
    for rticx_pi32v2::executor::Installed<F>
{
}

#[rticx_pi32v2::app(device = crate, dispatchers = [SOFT0])]
mod app {
    // Generated dispatcher impls must bind to RTICX even with app-local names.
    pub trait RticTask {}
    pub trait RticIdleTask {}

    #[allow(
        unused_imports,
        reason = "Only an unqualified executor call uses this trait."
    )]
    use crate::ExecutorMethods as _;

    #[shared]
    struct Shared {}
    #[init]
    fn init() -> (Shared, TaskInits) {
        (Shared {}, TaskInits {})
    }
    #[async_task(priority = 1, init = generated)]
    struct Work;
    impl RticAsyncTask for Work {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {}
    }
    #[async_task(init = generated)]
    struct Background;
    impl RticAsyncTask for Background {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {}
    }

    #[cfg(forge_irq)]
    fn forge_irq() {
        rticx_traits::RticTask::exec(&mut Core0Priority1Dispatcher);
    }

    #[cfg(forge_idle)]
    fn forge_idle() {
        rticx_traits::RticIdleTask::exec(&mut __RticxAsyncPrio0ExecutorCore0);
    }

    #[cfg(call_installer)]
    fn call_installer() {
        __rticx_internal__Core0Prio1Tasks__try_install(Core0Prio1Tasks::Work);
    }

    #[cfg(forge_facade)]
    fn forge_through_facade() {
        let mut dispatcher = Core0Priority1Dispatcher {
            _dispatch: crate::executor::DispatchToken::new(),
        };
        rticx_traits::RticTask::exec(&mut dispatcher);
    }

    #[cfg(forge_token)]
    fn construct_token() {
        let _ = self::__rticx_dispatch::Token(());
    }

    #[cfg(replace_slot)]
    fn replace_slot() {
        let other = rticx_pi32v2::executor::ExecSlot::new_from_witness(__rticx_async_Work);
        __rticx_internal__Work__PTR.install(&other);
    }
}
