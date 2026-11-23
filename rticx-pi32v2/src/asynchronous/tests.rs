extern crate alloc;
extern crate std;

use super::channel::{Channel, ReceiveError};
use alloc::{boxed::Box, sync::Arc, task::Wake};
use core::{
    cell::RefCell,
    future::Future,
    pin::pin,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

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

std::thread_local! {
    static ON_CLONE: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    static ON_DROP: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
}

fn clone_raw(_: *const ()) -> RawWaker {
    if let Some(callback) = ON_CLONE.take() {
        callback();
    }
    RawWaker::new(core::ptr::null(), &VTABLE)
}

fn drop_raw(_: *const ()) {
    if let Some(callback) = ON_DROP.take() {
        callback();
    }
}

static VTABLE: RawWakerVTable = RawWakerVTable::new(clone_raw, |_| {}, |_| {}, drop_raw);

fn reentrant_waker() -> Waker {
    // SAFETY: the vtable never dereferences the pointer or owns an allocation.
    // All callbacks operate on thread-local state, including when cloned or dropped.
    unsafe { Waker::new(core::ptr::null(), &VTABLE) }
}

#[derive(Default)]
struct Count(AtomicUsize);
impl Wake for Count {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn clone_can_deliver_during_recv_registration() {
    let (mut tx, mut rx) = crate::make_channel!(u8, 1);
    let raw = reentrant_waker();
    ON_CLONE.set(Some(Box::new(move || {
        tx.try_send(7).expect("receiver has space")
    })));
    assert_eq!(
        pin!(rx.recv())
            .as_mut()
            .poll(&mut Context::from_waker(&raw)),
        Poll::Ready(Ok(7))
    );
    assert!(rx.is_closed());
}

#[test]
fn retiring_the_previous_waker_can_send_and_wake_the_new_one() {
    let (mut tx, mut rx) = crate::make_channel!(u8, 1);
    let first = reentrant_waker();
    let count = Arc::new(Count::default());
    let latest = Waker::from(count.clone());
    let mut receive = pin!(rx.recv());
    assert!(
        receive
            .as_mut()
            .poll(&mut Context::from_waker(&first))
            .is_pending()
    );
    ON_DROP.set(Some(Box::new(move || {
        tx.try_send(8).expect("receiver has space")
    })));
    assert!(
        receive
            .as_mut()
            .poll(&mut Context::from_waker(&latest))
            .is_pending()
    );
    assert_eq!(count.0.load(Ordering::Relaxed), 1);
    assert_eq!(
        receive.as_mut().poll(&mut Context::from_waker(&latest)),
        Poll::Ready(Ok(8))
    );
}

#[test]
fn cancellation_unregisters_and_last_sender_wakes_receiver() {
    let mut channel = Channel::<u8, 2>::new();
    let (tx, mut rx) = channel.split();
    let mut other = tx.clone();
    let count = Arc::new(Count::default());
    let waker = Waker::from(count.clone());
    let mut cx = Context::from_waker(&waker);
    {
        let mut cancelled = pin!(rx.recv());
        assert!(cancelled.as_mut().poll(&mut cx).is_pending());
    }
    other.try_send(1).expect("receiver has space");
    assert_eq!(count.0.load(Ordering::Relaxed), 0);
    assert_eq!(rx.try_recv(), Ok(1));
    let mut receive = pin!(rx.recv());
    assert!(receive.as_mut().poll(&mut cx).is_pending());
    drop(tx);
    assert_eq!(count.0.load(Ordering::Relaxed), 0);
    drop(other);
    assert_eq!(count.0.load(Ordering::Relaxed), 1);
    assert_eq!(
        receive.as_mut().poll(&mut cx),
        Poll::Ready(Err(ReceiveError::NoSender))
    );
}

#[test]
fn dropping_a_queued_payload_can_reenter_the_closed_channel() {
    #[derive(Debug)]
    struct Payload(fn());
    impl Drop for Payload {
        fn drop(&mut self) {
            (self.0)();
        }
    }
    let (mut tx, rx) = crate::make_channel!(Payload, 1);
    let observer = tx.clone();
    ON_DROP.set(Some(Box::new(move || assert!(observer.is_closed()))));
    tx.try_send(Payload(|| drop_raw(core::ptr::null())))
        .expect("receiver has space");
    drop(rx);
    assert!(ON_DROP.take().is_none());
    assert!(tx.is_empty());
    assert!(matches!(
        tx.try_send(Payload(|| {})),
        Err(super::channel::TrySendError::NoReceiver(_))
    ));
}

#[test]
fn forgotten_endpoints_release_queued_ownership_on_resplit() {
    let count = Arc::new(());
    let mut channel = Channel::<Arc<()>, 1>::new();
    let (mut tx, rx) = channel.split();
    tx.try_send(count.clone()).expect("receiver has space");
    assert!(matches!(
        tx.try_send(count.clone()),
        Err(super::channel::TrySendError::Full(_))
    ));
    core::mem::forget(tx);
    core::mem::forget(rx);
    assert_eq!(Arc::strong_count(&count), 2);
    let (tx, mut rx) = channel.split();
    assert_eq!(Arc::strong_count(&count), 1);
    assert!(rx.is_empty());
    drop(tx);
    assert_eq!(rx.try_recv(), Err(ReceiveError::NoSender));
}
