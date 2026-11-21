#![no_std]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

use core::{
    cell::RefCell,
    future::poll_fn,
    task::{Poll, Waker},
};
use critical_section::Mutex;
pub use fm1::panel::*;
use heapless::Deque;
pub use rtic_sync::channel::{ReceiveError, TrySendError};

/// One completed panel sweep and its position relative to any lost input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct QueuedScan {
    /// Debounced levels, edges and encoder deltas from this sweep.
    pub scan: Scan,
    /// Wrapping TIMER4 count sampled when the scan completed.
    pub captured_at: u32,
    /// Sweeps dropped on a full queue since the previous successful send (saturating).
    pub missed_before: u32,
}

/// Bounded FIFO for one panel interrupt and one input task. Capacity is 1–255 scans.
pub struct ScanQueue<const N: usize> {
    state: Mutex<RefCell<State<N>>>,
}

struct State<const N: usize> {
    scans: Deque<QueuedScan, N>,
    waiter: Option<Waker>,
    // While either endpoint is usable, this means its peer has dropped.
    closed: bool,
}

impl<const N: usize> State<N> {
    fn try_recv(&mut self) -> Result<QueuedScan, ReceiveError> {
        self.scans.pop_front().ok_or(if self.closed {
            ReceiveError::NoSender
        } else {
            ReceiveError::Empty
        })
    }
}

impl<const N: usize> ScanQueue<N> {
    /// Reserve space for `N` scans. The capacity is checked at compile time.
    #[must_use]
    pub const fn new() -> Self {
        const { assert!(N > 0 && N <= 255, "scan capacity must be 1..=255") };
        Self {
            state: Mutex::new(RefCell::new(State {
                scans: Deque::new(),
                waiter: None,
                closed: false,
            })),
        }
    }

    /// Start an empty queue. The producer never waits and the receiver can await a scan.
    pub fn split(&mut self) -> (ScanSender<'_, N>, ScanReceiver<'_, N>) {
        let state = self.state.get_mut().get_mut();
        state.scans.clear();
        state.waiter = None;
        state.closed = false;
        (
            ScanSender {
                queue: self,
                missed: 0,
            },
            ScanReceiver { queue: self },
        )
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut State<N>) -> R) -> R {
        critical_section::with(|cs| f(&mut self.state.borrow_ref_mut(cs)))
    }

    fn close(&self) -> Option<Waker> {
        self.with_state(|state| {
            state.closed = true;
            state.waiter.take()
        })
    }
}

impl<const N: usize> Default for ScanQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> core::fmt::Debug for ScanQueue<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ScanQueue")
            .field("capacity", &N)
            .finish_non_exhaustive()
    }
}

/// Interrupt-side producer. A full queue keeps the older scans in arrival order.
#[derive(Debug)]
pub struct ScanSender<'a, const N: usize> {
    queue: &'a ScanQueue<N>,
    missed: u32,
}

impl<const N: usize> ScanSender<'_, N> {
    /// Submit each sweep once, with its capture time from [`fm1::time::cycles`].
    ///
    /// # Errors
    /// Returns [`TrySendError::Full`] if this sweep was dropped. The next accepted
    /// sweep reports the gap. [`TrySendError::NoReceiver`] means the consumer is gone.
    pub fn try_send(
        &mut self,
        scan: Scan,
        captured_at: u32,
    ) -> Result<(), TrySendError<QueuedScan>> {
        let frame = QueuedScan {
            scan,
            captured_at,
            missed_before: self.missed,
        };
        let waiter = self.queue.with_state(|state| {
            if state.closed {
                return Err(TrySendError::NoReceiver(frame));
            }
            if let Err(frame) = state.scans.push_back(frame) {
                self.missed = self.missed.saturating_add(1);
                return Err(TrySendError::Full(frame));
            }
            Ok(state.waiter.take())
        })?;
        self.missed = 0;
        if let Some(waker) = waiter {
            waker.wake();
        }
        Ok(())
    }
}

impl<const N: usize> Drop for ScanSender<'_, N> {
    fn drop(&mut self) {
        if let Some(waker) = self.queue.close() {
            waker.wake();
        }
    }
}

/// Task-side FIFO endpoint. Use `recv().await` to wait or `try_recv()` to poll.
#[derive(Debug)]
pub struct ScanReceiver<'a, const N: usize> {
    queue: &'a ScanQueue<N>,
}

impl<const N: usize> ScanReceiver<'_, N> {
    /// Take the oldest available scan.
    ///
    /// # Errors
    /// An empty queue returns [`ReceiveError::Empty`] while the sender is live
    /// and [`ReceiveError::NoSender`] after the sender drops.
    pub fn try_recv(&mut self) -> Result<QueuedScan, ReceiveError> {
        self.queue.with_state(State::try_recv)
    }

    /// Wait for the oldest available scan. Cancelling releases the registered waker.
    ///
    /// # Errors
    /// Returns [`ReceiveError::NoSender`] after the sender drops and the queue drains.
    pub async fn recv(&mut self) -> Result<QueuedScan, ReceiveError> {
        match self.try_recv() {
            Err(ReceiveError::Empty) => {}
            ready => return ready,
        }

        let registration = Registration(self.queue);
        poll_fn(|cx| registration.poll(cx.waker().clone())).await
    }

    /// Has the sender dropped? Buffered scans can still be received.
    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.queue.with_state(|state| state.closed)
    }

    /// Are all slots occupied?
    #[must_use]
    pub fn is_full(&self) -> bool {
        self.queue.with_state(|state| state.scans.is_full())
    }

    /// Is the queue empty?
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.with_state(|state| state.scans.is_empty())
    }
}

impl<const N: usize> Drop for ScanReceiver<'_, N> {
    fn drop(&mut self) {
        drop(self.queue.close());
    }
}

// Mutable access to the receiver permits only one live receive at a time.
struct Registration<'a, const N: usize>(&'a ScanQueue<N>);

impl<const N: usize> Registration<'_, N> {
    // Cloning a task waker may itself deliver a scan. Accept an owned clone
    // and check again under the same mask used to install the waiter.
    fn poll(&self, waker: Waker) -> Poll<Result<QueuedScan, ReceiveError>> {
        let (poll, unused) = self.0.with_state(|state| match state.try_recv() {
            Err(ReceiveError::Empty) => (Poll::Pending, state.waiter.replace(waker)),
            ready => (Poll::Ready(ready), Some(waker)),
        });
        drop(unused);
        poll
    }
}

impl<const N: usize> Drop for Registration<'_, N> {
    fn drop(&mut self) {
        drop(self.0.with_state(|state| state.waiter.take()));
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use alloc::{sync::Arc, task::Wake};
    use core::{
        future::Future,
        pin::pin,
        sync::atomic::{AtomicUsize, Ordering},
        task::{Context, Poll, Waker},
    };

    #[derive(Default)]
    struct CountWakes(AtomicUsize);

    impl Wake for CountWakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn full_queue_preserves_order_and_marks_only_the_first_scan_after_a_gap() {
        let mut queue = ScanQueue::<2>::new();
        let (mut panel, mut input) = queue.split();
        let scan = Scan::default();
        for t in [u32::MAX - 1, u32::MAX] {
            panel.try_send(scan, t).expect("two free slots");
        }
        for t in [0, 1] {
            assert!(matches!(
                panel.try_send(scan, t),
                Err(TrySendError::Full(_))
            ));
        }
        let first = input.try_recv().expect("oldest scan");
        assert_eq!((first.captured_at, first.missed_before), (u32::MAX - 1, 0));
        panel.try_send(scan, 2).expect("one free slot");
        let second = input.try_recv().expect("second scan");
        assert_eq!((second.captured_at, second.missed_before), (u32::MAX, 0));
        let after_gap = input.try_recv().expect("first scan after gap");
        assert_eq!((after_gap.captured_at, after_gap.missed_before), (2, 2));
        panel.try_send(scan, 3).expect("queue drained");
        assert_eq!(input.try_recv().expect("consecutive scan").missed_before, 0);
        drop(input);
        assert!(matches!(
            panel.try_send(scan, 4),
            Err(TrySendError::NoReceiver(_))
        ));
    }

    #[test]
    fn async_receiver_waits_for_a_scan_and_can_be_reopened() {
        let mut queue = ScanQueue::<1>::new();
        for t in [17, 38] {
            let (mut panel, mut input) = queue.split();
            let mut next = pin!(input.recv());
            let mut cx = Context::from_waker(Waker::noop());
            assert_eq!(next.as_mut().poll(&mut cx), Poll::Pending);
            panel.try_send(Scan::default(), t).expect("free slot");
            let expected = QueuedScan {
                scan: Scan::default(),
                captured_at: t,
                missed_before: 0,
            };
            assert_eq!(next.as_mut().poll(&mut cx), Poll::Ready(Ok(expected)));
        }
    }

    #[test]
    fn reopening_does_not_wake_the_previous_receiver() {
        let mut queue = ScanQueue::<1>::new();
        let previous = Arc::new(CountWakes::default());
        {
            let (mut panel, mut input) = queue.split();
            panel.try_send(Scan::default(), 1).expect("free slot");
            drop(panel);

            let waker = Waker::from(Arc::clone(&previous));
            let mut next = pin!(input.recv());
            assert!(matches!(
                next.as_mut().poll(&mut Context::from_waker(&waker)),
                Poll::Ready(Ok(_))
            ));
        }

        let (mut panel, _input) = queue.split();
        panel.try_send(Scan::default(), 2).expect("reopened slot");
        assert_eq!(previous.0.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cancellation_releases_the_task_and_preserves_the_next_scan() {
        let mut queue = ScanQueue::<1>::new();
        let (mut panel, mut input) = queue.split();
        let task = Arc::new(CountWakes::default());
        {
            let waker = Waker::from(task.clone());
            let mut next = pin!(input.recv());
            assert!(
                next.as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );
        }
        assert_eq!(Arc::strong_count(&task), 1);
        panel.try_send(Scan::default(), 7).expect("free slot");
        assert_eq!(task.0.load(Ordering::Relaxed), 0);
        assert_eq!(input.try_recv().expect("queued scan").captured_at, 7);
    }

    #[test]
    fn registration_observes_a_scan_delivered_after_the_empty_check() {
        let mut queue = ScanQueue::<1>::new();
        let (mut panel, mut input) = queue.split();
        assert_eq!(input.try_recv(), Err(ReceiveError::Empty));
        let task = Arc::new(CountWakes::default());
        let waker = Waker::from(task.clone());
        panel.try_send(Scan::default(), 42).expect("free slot");
        drop(panel);

        let registration = Registration(input.queue);
        assert!(
            matches!(registration.poll(waker), Poll::Ready(Ok(scan)) if scan.captured_at == 42)
        );
        assert_eq!(Arc::strong_count(&task), 1);
        assert_eq!(input.try_recv(), Err(ReceiveError::NoSender));
    }

    #[test]
    fn closing_wakes_only_the_latest_waiter() {
        let mut queue = ScanQueue::<1>::new();
        let (panel, mut input) = queue.split();
        let old = Arc::new(CountWakes::default());
        let new = Arc::new(CountWakes::default());
        let mut next = pin!(input.recv());
        for task in [&old, &new] {
            let waker = Waker::from(task.clone());
            assert!(
                next.as_mut()
                    .poll(&mut Context::from_waker(&waker))
                    .is_pending()
            );
        }
        assert_eq!(Arc::strong_count(&old), 1);
        drop(panel);
        assert_eq!(old.0.load(Ordering::Relaxed), 0);
        assert_eq!(new.0.load(Ordering::Relaxed), 1);
        assert_eq!(
            next.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Err(ReceiveError::NoSender))
        );
    }
}
