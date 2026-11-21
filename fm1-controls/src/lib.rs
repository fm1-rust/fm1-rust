#![no_std]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

pub use fm1::panel::*;
use rtic_sync::channel::{Channel, Receiver, Sender};
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
    channel: Channel<QueuedScan, N>,
}

impl<const N: usize> ScanQueue<N> {
    /// Reserve space for `N` scans. The capacity is checked at compile time.
    #[must_use]
    pub const fn new() -> Self {
        const { assert!(N > 0 && N <= 255, "scan capacity must be 1..=255") };
        Self {
            channel: Channel::new(),
        }
    }

    /// Start an empty queue. The producer never waits and the receiver can await a scan.
    pub fn split(&mut self) -> (ScanSender<'_, N>, ScanReceiver<'_, N>) {
        // rtic-sync 1.6.1 retains the previous receiver's waker across splits.
        self.channel = Channel::new();
        let (sender, receiver) = self.channel.split();
        (ScanSender { sender, missed: 0 }, receiver)
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
    sender: Sender<'a, QueuedScan, N>,
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
        let result = self.sender.try_send(QueuedScan {
            scan,
            captured_at,
            missed_before: self.missed,
        });
        match result {
            Ok(()) => self.missed = 0,
            Err(TrySendError::Full(_)) => self.missed = self.missed.saturating_add(1),
            Err(TrySendError::NoReceiver(_)) => {}
        }
        result
    }
}

/// Task-side FIFO endpoint. Use `recv().await` to wait or `try_recv()` to poll.
pub type ScanReceiver<'a, const N: usize> = Receiver<'a, QueuedScan, N>;

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
        struct CountWakes(AtomicUsize);

        impl Wake for CountWakes {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }

        let mut queue = ScanQueue::<1>::new();
        let previous = Arc::new(CountWakes(AtomicUsize::new(0)));
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
}
