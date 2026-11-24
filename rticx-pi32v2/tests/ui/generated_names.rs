//! The app macro sees cfg-disabled declarations before rustc removes them.
#![no_std]
#![cfg(target_os = "none")]
#![forbid(unsafe_code)]
#![allow(missing_docs, reason = "RTICX generates public task types.")]

// With an unhardened wake callback, a by-value receiver wins method
// resolution and lets application code poll on the waking context.
#[cfg(duplicate_wake)]
trait TakeWake {
    fn set_pending(self);
}
#[cfg(duplicate_wake)]
impl<F: Future<Output = ()> + 'static> TakeWake for rticx_pi32v2::executor::Installed<F> {
    fn set_pending(self) {
        rticx_pi32v2::executor::Installed::set_pending(&self);
        self.poll(|| {});
    }
}

macro_rules! probe {
    ($($extra:item)*) => {
        #[rticx_pi32v2::app(device = crate, dispatchers = [SOFT0])]
        mod app {
            $($extra)*

            #[cfg(any())]
            fn initial() -> u32 { 0 }
            fn initial() -> u32 { 42 }

            #[shared]
            struct Shared { word: u32 }
            #[init]
            fn init() -> (Shared, TaskInits) {
                (Shared { word: initial() }, TaskInits {})
            }
            #[async_task(priority = 1, init = generated)]
            struct Work;
            impl RticAsyncTask for Work {
                type SpawnInput = ();
                async fn exec(&mut self, _: ()) {}
            }
        }
    };
}

#[cfg(not(any(
    duplicate_entry,
    duplicate_wake,
    duplicate_storage,
    duplicate_installer
)))]
probe!();
#[cfg(duplicate_entry)]
probe! {
    #[cfg(any())]
    #[__rticx_backend::__entry]
    fn main() {}
}
#[cfg(duplicate_wake)]
probe! {
    #[cfg(any())]
    fn __rticx_internal__Work__wake() {}

    use crate::TakeWake as _;
}
#[cfg(duplicate_storage)]
probe! {
    #[cfg(any())]
    static mut r#SHARED: core::mem::MaybeUninit<Shared> = core::mem::MaybeUninit::uninit();
}
#[cfg(duplicate_installer)]
probe! {
    #[cfg(any())]
    fn __rticx_internal__Core0Prio1Tasks__try_install() {}
}
