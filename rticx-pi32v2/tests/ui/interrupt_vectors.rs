//! A task binding must select a runtime interrupt variant.
#![no_std]
#![cfg(target_os = "none")]
#![forbid(unsafe_code)]
#![allow(missing_docs, reason = "RTICX generates public task types.")]

// A USB0 conversion emitted in the app would find this module.
mod vector_index {
    #[allow(dead_code)]
    pub fn from(_: u8) {}
}

#[cfg(forge_vector)]
trait Alias {
    const TIMER2: Self;
}
#[cfg(forge_vector)]
impl Alias for rticx_pi32v2::Interrupt {
    const TIMER2: Self = Self::ALNK0;
}

macro_rules! bind {
    ($source:ident) => {
        #[rticx_pi32v2::app(device = crate, dispatchers = [])]
        mod app {
            #[cfg(forge_vector)]
            use crate::Alias as _;
            #[allow(unused_imports, reason = "A typed vector needs no caller conversion.")]
            use crate::vector_index as usize;

            #[shared]
            struct Shared {}
            #[init]
            fn init() -> (Shared, TaskInits) {
                (Shared {}, TaskInits {})
            }
            #[task(binds = $source, priority = 1, init = generated)]
            struct Handler;
            impl RticTask for Handler {
                fn exec(&mut self) {}
            }
        }
    };
}

#[cfg(not(forge_vector))]
bind!(USB0);
#[cfg(forge_vector)]
bind!(TIMER2);
