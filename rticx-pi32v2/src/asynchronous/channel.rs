//! Bounded FIFO with nonblocking producers and one async receiver.
#![forbid(unsafe_code)]

use core::{
    cell::RefCell,
    fmt,
    future::poll_fn,
    task::{Poll, Waker},
};
use critical_section::Mutex;
use heapless::Deque;
pub use rtic_sync::channel::{ReceiveError, TrySendError};

/// Storage for up to `N` messages, shared by cloned senders and one receiver.
pub struct Channel<T, const N: usize> {
    state: Mutex<RefCell<State<T, N>>>,
}

struct State<T, const N: usize> {
    messages: Deque<T, N>,
    senders: usize,
    has_receiver: bool,
    waiter: Option<Waker>,
}

impl<T, const N: usize> State<T, N> {
    fn try_recv(&mut self) -> Result<T, ReceiveError> {
        self.messages.pop_front().ok_or(if self.senders == 0 {
            ReceiveError::NoSender
        } else {
            ReceiveError::Empty
        })
    }
}

impl<T, const N: usize> Channel<T, N> {
    /// Create storage for 1–255 messages.
    #[must_use]
    pub const fn new() -> Self {
        const { assert!(N > 0 && N <= 255, "channel capacity must be 1..=255") };
        Self {
            state: Mutex::new(RefCell::new(State {
                messages: Deque::new(),
                senders: 0,
                has_receiver: false,
                waiter: None,
            })),
        }
    }

    /// Start empty, including when endpoints from the previous split were forgotten.
    pub fn split(&mut self) -> (Sender<'_, T, N>, Receiver<'_, T, N>) {
        let state = self.state.get_mut().get_mut();
        state.messages.clear();
        state.waiter = None;
        state.senders = 1;
        state.has_receiver = true;
        (Sender(self), Receiver(self))
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut State<T, N>) -> R) -> R {
        critical_section::with(|cs| f(&mut self.state.borrow_ref_mut(cs)))
    }
}

impl<T, const N: usize> Default for Channel<T, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const N: usize> fmt::Debug for Channel<T, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Channel")
            .field("capacity", &N)
            .finish_non_exhaustive()
    }
}

/// Nonblocking producer suitable for use in an interrupt.
/// Use [`try_send`](Self::try_send) and handle a full queue at the producer.
///
/// ```compile_fail,E0599
/// use rticx_pi32v2::asynchronous::channel::Channel;
/// let mut channel = Channel::<u8, 1>::new();
/// let (mut tx, _rx) = channel.split();
/// let _pending = tx.send(1);
/// ```
#[derive(Debug)]
pub struct Sender<'a, T, const N: usize>(&'a Channel<T, N>);

impl<T, const N: usize> Sender<'_, T, N> {
    /// Submit one message and wake a waiting receiver.
    ///
    /// # Errors
    /// Returns the message in [`TrySendError::Full`] or [`TrySendError::NoReceiver`].
    pub fn try_send(&mut self, value: T) -> Result<(), TrySendError<T>> {
        let waiter = self.0.with_state(|state| {
            if !state.has_receiver {
                return Err(TrySendError::NoReceiver(value));
            }
            state
                .messages
                .push_back(value)
                .map_err(TrySendError::Full)?;
            Ok(state.waiter.take())
        })?;
        if let Some(waker) = waiter {
            waker.wake();
        }
        Ok(())
    }

    /// Has the receiver dropped?
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.0.with_state(|s| !s.has_receiver)
    }

    /// Are all message slots occupied?
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.0.with_state(|s| s.messages.is_full())
    }

    /// Are there no queued messages?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.with_state(|s| s.messages.is_empty())
    }
}

impl<T, const N: usize> Clone for Sender<'_, T, N> {
    fn clone(&self) -> Self {
        self.0.with_state(|s| {
            s.senders = s.senders.checked_add(1).expect("too many channel senders");
        });
        Self(self.0)
    }
}

impl<T, const N: usize> Drop for Sender<'_, T, N> {
    fn drop(&mut self) {
        let waiter = self.0.with_state(|s| {
            s.senders = s.senders.checked_sub(1).expect("dropping a live sender");
            if s.senders == 0 {
                s.waiter.take()
            } else {
                None
            }
        });
        if let Some(waker) = waiter {
            waker.wake();
        }
    }
}

/// The channel's sole receiver.
#[derive(Debug)]
pub struct Receiver<'a, T, const N: usize>(&'a Channel<T, N>);

impl<T, const N: usize> Receiver<'_, T, N> {
    /// Take the oldest queued message immediately.
    ///
    /// # Errors
    /// Returns [`ReceiveError::Empty`] while a sender is live, or
    /// [`ReceiveError::NoSender`] after all senders drop and the queue drains.
    pub fn try_recv(&mut self) -> Result<T, ReceiveError> {
        self.0.with_state(State::try_recv)
    }

    /// Wait for a message. Cancelling this future unregisters its waker.
    ///
    /// # Errors
    /// Returns [`ReceiveError::NoSender`] after all senders drop and the queue drains.
    pub async fn recv(&mut self) -> Result<T, ReceiveError> {
        match self.try_recv() {
            Err(ReceiveError::Empty) => {}
            ready => return ready,
        }

        let registration = Registration(self.0);
        poll_fn(|cx| {
            // Cloning can itself send or close the channel. Recheck before registering.
            let waker = cx.waker().clone();
            let (poll, unused) = registration.0.with_state(|s| match s.try_recv() {
                Err(ReceiveError::Empty) => (Poll::Pending, s.waiter.replace(waker)),
                ready => (Poll::Ready(ready), Some(waker)),
            });
            drop(unused);
            poll
        })
        .await
    }

    /// Have all senders dropped? Queued messages can still be received.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.0.with_state(|s| s.senders == 0)
    }

    /// Are all message slots occupied?
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.0.with_state(|s| s.messages.is_full())
    }

    /// Are there no queued messages?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.with_state(|s| s.messages.is_empty())
    }
}

impl<T, const N: usize> Drop for Receiver<'_, T, N> {
    fn drop(&mut self) {
        let waiter = self.0.with_state(|s| {
            s.has_receiver = false;
            s.waiter.take()
        });
        drop(waiter);
        // A queued value can drop a sender or run other application code.
        while let Some(value) = self.0.with_state(|s| s.messages.pop_front()) {
            drop(value);
        }
    }
}

// Exclusive access to the receiver permits one live registration at a time.
struct Registration<'a, T, const N: usize>(&'a Channel<T, N>);

impl<T, const N: usize> Drop for Registration<'_, T, N> {
    fn drop(&mut self) {
        drop(self.0.with_state(|s| s.waiter.take()));
    }
}
