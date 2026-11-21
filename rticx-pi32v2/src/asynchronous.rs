//! Async channels and task execution for the pi32v2 backend.
//!
//! Import endpoint and error types from [`channel`]. They come from the same
//! `rtic-sync` implementation as [`make_channel!`].
pub use rtic_sync::{channel, make_channel};
pub use rticx_async::{executor, portable_atomic, yield_now};

#[cfg(test)]
mod tests {
    use super::channel::{Channel, ReceiveError};

    #[test]
    fn channel_can_close_and_reopen() {
        let mut channel = Channel::<u8, 1>::new();
        for value in [1, 2] {
            let (mut tx, mut rx) = channel.split();
            assert_eq!(tx.try_send(value), Ok(()));
            drop(tx);
            assert_eq!(rx.try_recv(), Ok(value));
            assert_eq!(rx.try_recv(), Err(ReceiveError::NoSender));
        }
    }
}
