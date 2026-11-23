extern crate alloc;
extern crate std;

use super::{
    Duration, Instant, TimerSlot,
    queue::{self, Hardware, Queue},
};
use alloc::{boxed::Box, sync::Arc, task::Wake};
use core::{
    cell::{Cell, RefCell},
    future::{Future, pending, ready},
    pin::pin,
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};
use static_cell::ConstStaticCell;

struct Counter;
std::thread_local! {
    static NOW: Cell<u32> = const { Cell::new(0) };
    static COMPARE: Cell<Option<u32>> = const { Cell::new(None) };
    static ON_CLONE: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    static ON_WAKE: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    static ON_DROP: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

impl Hardware for Counter {
    fn now() -> u32 {
        NOW.get()
    }
    fn program(at: Option<u32>) {
        COMPARE.set(at);
    }
}

macro_rules! queue {
    ($n:literal) => {{
        static Q: Queue<Counter> = Queue::new();
        static S: ConstStaticCell<[TimerSlot; $n]> =
            ConstStaticCell::new([const { TimerSlot::EMPTY }; $n]);
        NOW.set(0);
        COMPARE.set(None);
        Q.initialize(S.take());
        &Q
    }};
}

fn raw() -> RawWaker {
    static TABLE: RawWakerVTable = RawWakerVTable::new(
        |_| {
            if let Some(f) = ON_CLONE.take() {
                f();
            }
            raw()
        },
        |_| {
            if let Some(f) = ON_WAKE.take() {
                f();
            }
        },
        |_| {
            if let Some(f) = ON_WAKE.take() {
                f();
            }
        },
        |_| {
            if let Some(f) = ON_DROP.take() {
                f();
            }
        },
    );
    RawWaker::new(core::ptr::null(), &TABLE)
}

fn reentrant_waker() -> Waker {
    // SAFETY: no callback accesses the data pointer or owns an allocation.
    // All hooks are local to the calling thread, even if the waker changes threads.
    unsafe { Waker::from_raw(raw()) }
}

#[derive(Default)]
struct Wakes(AtomicUsize);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Relaxed);
    }
}

#[test]
fn expiry_transfers_waker_before_cancellation_and_slot_reuse() {
    let q = queue!(1);
    let waker = reentrant_waker();
    let mut original = Box::pin(q.delay_until(2));
    assert!(
        original
            .as_mut()
            .poll(&mut Context::from_waker(&waker))
            .is_pending()
    );
    ON_CLONE.set(Some(Box::new(|| {})));
    ON_WAKE.set(Some(Box::new(move || {
        drop(original);
        let mut next = pin!(q.delay_until(4));
        assert!(
            next.as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert_eq!(COMPARE.get(), Some(4));
    })));
    NOW.set(2);
    q.service();
    assert!(ON_WAKE.take().is_none());
    assert!(ON_CLONE.take().is_some(), "expiry cloned the waker");
    assert_eq!(COMPARE.get(), None);
}

#[test]
fn repoll_wakes_only_latest_and_old_waker_can_drop_reentrantly() {
    let q = queue!(1);
    let first = reentrant_waker();
    let mut delay = pin!(q.delay_until(2));
    assert!(
        delay
            .as_mut()
            .poll(&mut Context::from_waker(&first))
            .is_pending()
    );
    ON_WAKE.set(Some(Box::new(|| {})));
    ON_DROP.set(Some(Box::new(move || {
        NOW.set(2);
        q.service();
    })));
    let count = Arc::new(Wakes::default());
    let latest = Waker::from(count.clone());
    assert!(
        delay
            .as_mut()
            .poll(&mut Context::from_waker(&latest))
            .is_pending()
    );
    assert_eq!(count.0.load(Relaxed), 1);
    assert!(ON_WAKE.take().is_some(), "stale waker fired");
    assert!(
        delay
            .as_mut()
            .poll(&mut Context::from_waker(&latest))
            .is_ready()
    );
}

#[test]
fn clone_can_advance_time_before_registration() {
    let q = queue!(1);
    ON_CLONE.set(Some(Box::new(move || {
        NOW.set(9);
        q.service();
    })));
    let w = reentrant_waker();
    assert!(
        pin!(q.delay_until(9))
            .as_mut()
            .poll(&mut Context::from_waker(&w))
            .is_ready()
    );
    assert_eq!(COMPARE.get(), None);
}

#[test]
fn cancellation_and_expiry_rearm_across_wrap() {
    let q = queue!(3);
    NOW.set(u32::MAX - 8);
    let count = Arc::new(Wakes::default());
    let waker = Waker::from(count.clone());
    let mut cx = Context::from_waker(&waker);
    let mut last = Box::pin(q.delay_until(3));
    let mut first = Box::pin(q.delay_until(u32::MAX - 1));
    let mut middle = Box::pin(q.delay_until(1));
    for future in [&mut last, &mut first, &mut middle] {
        assert!(future.as_mut().poll(&mut cx).is_pending());
    }
    assert_eq!(COMPARE.get(), Some(u32::MAX - 1));
    drop(first);
    assert_eq!(COMPARE.get(), Some(1));
    NOW.set(1);
    q.service();
    assert_eq!(count.0.load(Relaxed), 1);
    assert_eq!(COMPARE.get(), Some(3));
    drop(last);
    assert_eq!(COMPARE.get(), None);
    // A fired delay stays ready even after the counter moves past its comparison horizon.
    NOW.set(0x8000_0010);
    assert!(middle.as_mut().poll(&mut cx).is_ready());
}

#[test]
fn timeout_prefers_ready_inner_and_cancels_its_delay() {
    let q = queue!(1);
    let mut cx = Context::from_waker(Waker::noop());
    let mut timeout = Box::pin(queue::timeout(pending::<()>(), q.delay_until(8)));
    assert!(timeout.as_mut().poll(&mut cx).is_pending());
    drop(timeout);
    assert_eq!(COMPARE.get(), None);

    let mut timeout = pin!(queue::timeout(pending::<()>(), q.delay_until(7)));
    assert!(timeout.as_mut().poll(&mut cx).is_pending());
    NOW.set(7);
    q.service();
    assert!(matches!(
        timeout.as_mut().poll(&mut cx),
        Poll::Ready(Err(_))
    ));
    assert!(matches!(
        pin!(queue::timeout(ready(42), q.delay_until(7)))
            .as_mut()
            .poll(&mut cx),
        Poll::Ready(Ok(42))
    ));
    assert_eq!(queue::after(u32::MAX, 0), u32::MAX);
    assert_eq!(queue::after(u32::MAX, 1), 1);
}

#[test]
#[should_panic(expected = "timer slots exhausted")]
fn exhausted_storage_is_reported_at_registration() {
    let q = queue!(1);
    let mut cx = Context::from_waker(Waker::noop());
    let mut first = pin!(q.delay_until(2));
    assert!(first.as_mut().poll(&mut cx).is_pending());
    let _ = pin!(q.delay_until(3)).as_mut().poll(&mut cx);
}

static TIMEBASE_QUEUE: Queue<Counter> = Queue::new();
queue::impl_timebase!(Counter, Instant, Duration, TIMEBASE_QUEUE);

#[test]
fn timebase_relative_waits_round_up_and_ready_future_wins_a_tie() {
    use super::Timebase;

    static SLOTS: ConstStaticCell<[TimerSlot; 1]> = ConstStaticCell::new([TimerSlot::EMPTY]);
    TIMEBASE_QUEUE.initialize(SLOTS.take());
    NOW.set(u32::MAX);
    let mut cx = Context::from_waker(Waker::noop());
    assert_eq!(<Counter as Timebase>::now().as_ticks(), u32::MAX);
    assert!(
        pin!(Counter::delay(Duration::from_ticks(0)))
            .as_mut()
            .poll(&mut cx)
            .is_ready()
    );
    let mut delay = pin!(Counter::delay(Duration::from_ticks(1)));
    assert!(delay.as_mut().poll(&mut cx).is_pending());
    assert_eq!(COMPARE.get(), Some(1));
    NOW.set(0);
    assert!(delay.as_mut().poll(&mut cx).is_pending());
    NOW.set(1);
    TIMEBASE_QUEUE.service();
    assert!(delay.as_mut().poll(&mut cx).is_ready());
    let mut timeout = pin!(Counter::timeout_after(
        Duration::from_ticks(2),
        pending::<()>()
    ));
    assert!(timeout.as_mut().poll(&mut cx).is_pending());
    assert_eq!(COMPARE.get(), Some(4));
    NOW.set(4);
    TIMEBASE_QUEUE.service();
    assert!(matches!(
        timeout.as_mut().poll(&mut cx),
        Poll::Ready(Err(_))
    ));
    let at = <Counter as Timebase>::now();
    assert!(matches!(
        pin!(Counter::timeout_at(at, ready(7)))
            .as_mut()
            .poll(&mut cx),
        Poll::Ready(Ok(7))
    ));
}

#[test]
fn relative_wait_with_a_half_range_rounded_deadline_stays_pending() {
    let q = queue!(2);
    let mut cx = Context::from_waker(Waker::noop());
    let at = queue::after(0, i32::MAX as u32);
    let mut far = pin!(q.delay_until(at));
    assert!(far.as_mut().poll(&mut cx).is_pending());
    let mut near = pin!(q.delay_until(2));
    assert!(near.as_mut().poll(&mut cx).is_pending());
    assert_eq!(COMPARE.get(), Some(2));
    NOW.set(2);
    q.service();
    assert!(near.as_mut().poll(&mut cx).is_ready());
    assert!(far.as_mut().poll(&mut cx).is_pending());
    assert_eq!(COMPARE.get(), Some(at));
    NOW.set(at);
    q.service();
    assert!(far.as_mut().poll(&mut cx).is_ready());
}
