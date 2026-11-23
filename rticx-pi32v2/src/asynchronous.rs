//! Async channels and task execution for the pi32v2 backend.
pub mod channel;
pub use portable_atomic;
pub mod yield_now;

#[doc(hidden)]
pub use static_cell::ConstStaticCell;

/// Create and split a static [`channel::Channel`].
///
/// # Panics
/// Each call site can be evaluated only once.
#[macro_export]
macro_rules! make_channel {
    ($ty:ty, $size:expr) => {{
        static CHANNEL: $crate::asynchronous::ConstStaticCell<
            $crate::asynchronous::channel::Channel<$ty, $size>,
        > = $crate::asynchronous::ConstStaticCell::new(
            $crate::asynchronous::channel::Channel::new(),
        );
        CHANNEL.take().split()
    }};
}
pub use crate::make_channel;

#[cfg(test)]
mod tests;
