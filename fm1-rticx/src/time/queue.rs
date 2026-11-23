//! Each pending delay reserves a slot until completion or cancellation.
#![forbid(unsafe_code)]

use core::{
    cell::RefCell,
    future::{Future, poll_fn},
    mem,
    pin::pin,
    task::{Context, Poll, Waker},
};
use critical_section::Mutex;
use rtic_time::TimeoutError;

/// Storage for one outstanding delay or timeout on one clock.
/// See [`super::Clock::start_with_slots`] and [`super::DeadlineClock::start_with_slots`].
#[derive(Debug)]
pub struct TimerSlot(Entry);

#[derive(Debug)]
enum Entry {
    Free,
    Armed { at: u32, waker: Waker },
    Fired,
}

impl TimerSlot {
    /// An unused slot. Construct an array with `[const { TimerSlot::EMPTY }; N]`.
    pub const EMPTY: Self = Self(Entry::Free);
}

// Hardware access only. Queue updates program the timer with interrupts masked.
pub(super) trait Hardware {
    fn now() -> u32;
    fn program(at: Option<u32>);
}

pub(super) struct Queue<H> {
    slots: Mutex<RefCell<Option<&'static mut [TimerSlot]>>>,
    hardware: core::marker::PhantomData<fn() -> H>,
}

impl<H: Hardware> Queue<H> {
    pub(super) const fn new() -> Self {
        Self {
            slots: Mutex::new(RefCell::new(None)),
            hardware: core::marker::PhantomData,
        }
    }

    pub(super) fn initialize(&self, slots: &'static mut [TimerSlot]) {
        assert!(!slots.is_empty(), "empty timer storage");
        critical_section::with(|cs| {
            let mut storage = self.slots.borrow_ref_mut(cs);
            assert!(storage.is_none(), "clock already started");
            *storage = Some(slots);
        });
    }

    fn with_slots<R>(&self, f: impl FnOnce(&mut [TimerSlot]) -> R) -> R {
        critical_section::with(|cs| {
            f(self
                .slots
                .borrow_ref_mut(cs)
                .as_deref_mut()
                .expect("clock not started"))
        })
    }

    pub(super) async fn delay_until(&self, at: u32) {
        let mut registration = Registration {
            queue: self,
            slot: None,
        };
        poll_fn(|cx| registration.poll(at, cx)).await;
    }

    fn next(slots: &[TimerSlot], now: u32) -> Option<(usize, u32)> {
        slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| match s.0 {
                Entry::Armed { at, .. } => Some((i, at)),
                _ => None,
            })
            .max_by_key(|(_, at)| now.wrapping_sub(*at).cast_signed())
    }

    // Select and program the earliest deadline under the same interrupt mask.
    fn schedule(slots: &[TimerSlot], now: u32) {
        H::program(Self::next(slots, now).map(|(_, at)| at));
    }

    pub(super) fn service(&self) {
        loop {
            let wake = self.with_slots(|slots| {
                let now = H::now();
                match Self::next(slots, now) {
                    Some((i, at)) if due(now, at) => {
                        let Entry::Armed { waker, .. } =
                            mem::replace(&mut slots[i].0, Entry::Fired)
                        else {
                            unreachable!("next selected an armed slot")
                        };
                        Self::schedule(slots, now);
                        Some(waker)
                    }
                    next => {
                        H::program(next.map(|(_, at)| at));
                        None
                    }
                }
            });
            let Some(waker) = wake else { return };
            waker.wake();
        }
    }
}

fn due(now: u32, at: u32) -> bool {
    now.wrapping_sub(at).cast_signed() >= 0
}

struct Registration<'a, H: Hardware> {
    queue: &'a Queue<H>,
    slot: Option<usize>,
}

impl<H: Hardware> Registration<'_, H> {
    fn poll(&mut self, at: u32, cx: &Context<'_>) -> Poll<()> {
        // Waker::clone can itself service the timer or cancel another delay.
        // Recheck the time and slot after cloning. Retire the old waker outside the mask.
        if let Some(poll) = self.queue.with_slots(|slots| {
            if due(H::now(), at) {
                return Some(Poll::Ready(()));
            }
            match &slots[self.slot?].0 {
                Entry::Fired => Some(Poll::Ready(())),
                Entry::Armed { waker, .. } if waker.will_wake(cx.waker()) => Some(Poll::Pending),
                _ => None,
            }
        }) {
            return poll;
        }
        let mut replacement = Entry::Armed {
            at,
            waker: cx.waker().clone(),
        };
        self.queue.with_slots(|slots| {
            let now = H::now();
            if due(now, at)
                || self
                    .slot
                    .is_some_and(|i| matches!(slots[i].0, Entry::Fired))
            {
                return Poll::Ready(());
            }
            let index = *self.slot.get_or_insert_with(|| {
                slots
                    .iter()
                    .position(|s| matches!(s.0, Entry::Free))
                    .expect("timer slots exhausted")
            });
            mem::swap(&mut slots[index].0, &mut replacement);
            if matches!(replacement, Entry::Free) {
                Queue::<H>::schedule(slots, now);
            }
            Poll::Pending
        })
    }
}

impl<H: Hardware> Drop for Registration<'_, H> {
    fn drop(&mut self) {
        if let Some(i) = self.slot {
            let old = self.queue.with_slots(|slots| {
                let old = mem::replace(&mut slots[i].0, Entry::Free);
                if matches!(old, Entry::Armed { .. }) {
                    Queue::<H>::schedule(slots, H::now());
                }
                old
            });
            drop(old);
        }
    }
}

// As in rtic-time, a relative wait includes one tick of uncertainty, except for zero.
pub(super) fn after(now: u32, ticks: u32) -> u32 {
    now.wrapping_add(ticks).wrapping_add(u32::from(ticks != 0))
}

pub(super) async fn timeout<F: Future>(
    future: F,
    delay: impl Future<Output = ()>,
) -> Result<F::Output, TimeoutError> {
    let mut future = pin!(future);
    let mut delay = pin!(delay);
    poll_fn(|cx| {
        if let Poll::Ready(value) = future.as_mut().poll(cx) {
            return Poll::Ready(Ok(value));
        }
        delay.as_mut().poll(cx).map(|()| Err(TimeoutError))
    })
    .await
}

macro_rules! impl_timebase {
    ($clock:ty, $instant:ty, $duration:ty, $queue:ident) => {
        impl rtic_time::Timebase for $clock {
            type Instant = $instant;
            type Duration = $duration;

            fn now() -> Self::Instant {
                Self::Instant::from_ticks(<Self as $crate::time::queue::Hardware>::now())
            }

            async fn delay_until(at: Self::Instant) {
                $queue.delay_until(at.as_ticks()).await;
            }

            async fn delay(duration: Self::Duration) {
                let at = $crate::time::queue::after(
                    <Self as $crate::time::queue::Hardware>::now(),
                    duration.as_ticks(),
                );
                $queue.delay_until(at).await;
            }

            async fn timeout_at<F: core::future::Future>(
                at: Self::Instant,
                future: F,
            ) -> Result<F::Output, rtic_time::TimeoutError> {
                $crate::time::queue::timeout(future, $queue.delay_until(at.as_ticks())).await
            }

            async fn timeout_after<F: core::future::Future>(
                duration: Self::Duration,
                future: F,
            ) -> Result<F::Output, rtic_time::TimeoutError> {
                let at = $crate::time::queue::after(
                    <Self as $crate::time::queue::Hardware>::now(),
                    duration.as_ticks(),
                );
                $crate::time::queue::timeout(future, $queue.delay_until(at)).await
            }
        }
    };
}
pub(super) use impl_timebase;
