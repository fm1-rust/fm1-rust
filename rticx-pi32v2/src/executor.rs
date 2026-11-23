//! Private ABI between the CPU0 application macro and its dispatcher.
use core::{
    cell::{Cell, OnceCell, RefCell},
    future::Future,
    pin::Pin,
    ptr,
    sync::atomic::{AtomicBool, AtomicPtr, Ordering},
    task::{Context, RawWaker, RawWakerVTable, Waker},
};

/// Storage allocated in the non-returning CPU0 entry frame.
#[derive(Debug)]
pub struct ExecSlot<F: Future<Output = ()> + 'static> {
    future: RefCell<Option<F>>,
    occupied: Cell<bool>,
    pending: AtomicBool,
    wake: OnceCell<fn()>,
}

impl<F: Future<Output = ()> + 'static> ExecSlot<F> {
    pub fn new_from_witness<T, I>(_: fn(T, I) -> F) -> Self {
        Self {
            future: RefCell::new(None),
            occupied: Cell::new(false),
            pending: AtomicBool::new(false),
            wake: OnceCell::new(),
        }
    }
}

/// A single assignment, type-erased slot address in the generated application.
#[derive(Debug, Default)]
pub struct ExecSlotPtr(AtomicPtr<()>);
impl ExecSlotPtr {
    #[must_use]
    pub const fn new() -> Self {
        Self(AtomicPtr::new(ptr::null_mut()))
    }

    /// # Safety
    /// Keep the slot live at this address for the rest of the firmware's lifetime.
    /// After installation, access it only through the generated dispatcher and
    /// its wake callback. Every recovery must use the same concrete future type.
    pub unsafe fn install<F: Future<Output = ()> + 'static>(&self, slot: &ExecSlot<F>) {
        let address = ptr::from_ref(slot).cast_mut().cast();
        assert!(
            self.0
                .compare_exchange(
                    ptr::null_mut(),
                    address,
                    Ordering::Release,
                    Ordering::Relaxed
                )
                .is_ok(),
            "executor slot already installed"
        );
    }
}

/// Only a dispatcher that owns the task may reserve, initialize or poll it.
/// A waker may only signal the pending flag.
#[derive(Debug)]
pub struct Installed<F: Future<Output = ()> + 'static>(&'static ExecSlot<F>);

/// # Safety
/// Use the witness for this slot. Only its designated, non-reentrant CPU0
/// dispatcher may drive the future. Other contexts may only call `set_pending`.
pub unsafe fn recover_slot<F: Future<Output = ()> + 'static, T, I>(
    _: fn(T, I) -> F,
    ptr: &'static ExecSlotPtr,
) -> Installed<F> {
    let address = ptr.0.load(Ordering::Acquire).cast::<ExecSlot<F>>();
    assert!(!address.is_null(), "executor accessed before startup");
    // SAFETY: install published permanent storage. Its concrete type is
    // established by the generated witness, including for stale wakeups.
    Installed(unsafe { &*address })
}

impl<F: Future<Output = ()> + 'static> Installed<F> {
    pub fn set_pending(&self) {
        self.0.pending.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn try_allocate(&self) -> bool {
        !self.0.occupied.replace(true)
    }

    /// # Safety
    /// The generated installer reserved the slot before borrowing its task.
    pub unsafe fn spawn(&self, future: F) {
        let mut storage = self.0.future.borrow_mut();
        assert!(
            self.0.occupied.get() && storage.is_none(),
            "slot is not reserved"
        );
        *storage = Some(future);
        self.set_pending();
    }

    pub fn poll(&self, wake: fn()) -> bool {
        let slot = self.0;
        if !slot.occupied.get() {
            return false;
        }
        // The dispatcher visits every task at its priority. A sleeping task
        // needs no storage borrow or atomic read-modify-write. A racing wake
        // leaves pending set and schedules another dispatcher pass.
        if !slot.pending.load(Ordering::Relaxed) {
            return true;
        }
        let Ok(mut storage) = slot.future.try_borrow_mut() else {
            // A wake during poll stays pending for the next dispatcher pass.
            return true;
        };
        if slot.pending.swap(false, Ordering::AcqRel) {
            // SAFETY: install keeps the slot at this address forever. Spawn
            // writes only into None. Pin::set below drops a completed future
            // in place while the RefCell borrow still excludes re-entry.
            let mut pinned = unsafe { Pin::new_unchecked(&mut *storage) };
            if let Some(future) = pinned.as_mut().as_pin_mut() {
                let callback: &'static fn() = slot.wake.get_or_init(|| wake);
                // SAFETY: WAKER reads an immutable function pointer at a
                // permanent address. Clones share it and require no destructor.
                let waker = unsafe { Waker::new(ptr::from_ref(callback).cast(), &WAKER) };
                if future.poll(&mut Context::from_waker(&waker)).is_ready() {
                    pinned.set(None);
                    slot.occupied.set(false);
                }
            }
        }
        slot.occupied.get()
    }
}

const WAKER: RawWakerVTable = RawWakerVTable::new(clone_waker, wake, wake, |_| {});
fn clone_waker(data: *const ()) -> RawWaker {
    RawWaker::new(data, &WAKER)
}
unsafe fn wake(data: *const ()) {
    // SAFETY: only poll creates these wakers, with a permanent, initialized
    // fn() as data. The generated callback only signals the slot and its IRQ.
    let callback = unsafe { *data.cast::<fn()>() };
    callback();
}

#[cfg(test)]
mod tests {
    extern crate alloc;
    extern crate std;
    use super::*;
    use alloc::boxed::Box;
    use core::{marker::PhantomPinned, sync::atomic::AtomicUsize, task::Poll};
    use std::sync::Mutex;

    static PTR: ExecSlotPtr = ExecSlotPtr::new();
    static POLLS: AtomicUsize = AtomicUsize::new(0);
    static DROPS: AtomicUsize = AtomicUsize::new(0);
    static SAVED: Mutex<Option<Waker>> = Mutex::new(None);

    #[derive(Debug)]
    struct Check {
        address: Cell<*const Check>,
        _pin: PhantomPinned,
    }

    fn make((): (), (): ()) -> Check {
        Check {
            address: Cell::new(ptr::null()),
            _pin: PhantomPinned,
        }
    }

    fn signal() {
        // SAFETY: PTR permanently holds the slot for make; waking only sets the flag.
        unsafe { recover_slot(make, &PTR) }.set_pending();
    }

    fn owner() -> Installed<Check> {
        // SAFETY: the test runs all dispatcher operations on this test thread.
        unsafe { recover_slot(make, &PTR) }
    }

    impl Future for Check {
        type Output = ();
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            let address = ptr::from_ref(&*self);
            if self.address.get().is_null() {
                self.address.set(address);
                *SAVED.lock().expect("waker lock") = Some(cx.waker().clone());
                // Wake from another context while the dispatcher still holds the
                // pinned future and its RefCell borrow.
                let waker = cx.waker().clone();
                std::thread::spawn(move || waker.wake())
                    .join()
                    .expect("wake during poll");
                assert!(owner().poll(signal)); // nested poll cannot acquire the future
                POLLS.fetch_add(1, Ordering::Relaxed);
                Poll::Pending
            } else {
                assert_eq!(self.address.get(), address);
                POLLS.fetch_add(1, Ordering::Relaxed);
                Poll::Ready(())
            }
        }
    }

    impl Drop for Check {
        fn drop(&mut self) {
            assert_eq!(self.address.get(), ptr::from_ref(self));
            assert!(!owner().try_allocate());
            DROPS.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn stale_wake_reservation_reentry_and_pinned_drop() {
        let slot = Box::leak(Box::new(ExecSlot::new_from_witness(make)));
        // SAFETY: PTR roots this allocation for the program lifetime.
        unsafe { PTR.install(slot) };
        let exec = owner();
        assert!(exec.try_allocate());
        exec.set_pending();
        assert!(exec.poll(signal)); // the original safe reproducer
        assert_eq!(POLLS.load(Ordering::Relaxed), 0);
        for n in 1..=2 {
            // SAFETY: this dispatcher owns the preceding reservation.
            unsafe { exec.spawn(make((), ())) };
            assert!(exec.poll(signal));
            assert!(!exec.poll(signal));
            assert_eq!(POLLS.load(Ordering::Relaxed), 2 * n);
            assert_eq!(DROPS.load(Ordering::Relaxed), n);
            let waker = SAVED
                .lock()
                .expect("waker lock")
                .take()
                .expect("poll saved a waker");
            std::thread::spawn(move || waker.wake())
                .join()
                .expect("wake returned");
            assert!(!exec.poll(signal)); // stale wake on empty slot
            if n == 1 {
                assert!(exec.try_allocate());
            }
        }
    }
}
