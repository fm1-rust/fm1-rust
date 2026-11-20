//! RTICX's dispatcher retains consumer handles while a higher-priority task
//! can split the same queue to spawn. Borrow the backing ring only during
//! each operation, inside a CPU0 critical section.
use core::cell::RefCell;
use critical_section::Mutex;
use heapless::spsc::Queue as Ring;

/// A dispatcher ring with room for N - 1 queued items.
#[derive(Debug)]
pub struct Queue<T, const N: usize>(Mutex<RefCell<Ring<T, N>>>);

impl<T, const N: usize> Queue<T, N> {
    /// Create an empty ring.
    pub const fn new() -> Self {
        Self(Mutex::new(RefCell::new(Ring::new())))
    }

    /// Give the dispatcher independent producer and consumer handles.
    pub fn split(&self) -> (Endpoint<'_, T, N>, Endpoint<'_, T, N>) {
        (Endpoint(self), Endpoint(self))
    }
}

#[derive(Debug)]
pub struct Endpoint<'a, T, const N: usize>(&'a Queue<T, N>);

impl<T, const N: usize> Endpoint<'_, T, N> {
    pub fn enqueue(&mut self, item: T) -> Result<(), T> {
        critical_section::with(|cs| self.0.0.borrow_ref_mut(cs).enqueue(item))
    }

    pub fn dequeue(&mut self) -> Option<T> {
        critical_section::with(|cs| self.0.0.borrow_ref_mut(cs).dequeue())
    }

    pub fn enqueue_unchecked(&mut self, item: T) {
        assert!(self.enqueue(item).is_ok(), "RTICX dispatcher queue full");
    }

    pub fn dequeue_unchecked(&mut self) -> T {
        self.dequeue().expect("RTICX dispatcher queue empty")
    }
}

#[cfg(test)]
mod tests {
    use super::Queue;

    #[test]
    fn spawn_while_consumer_is_live() {
        let q = Queue::<u8, 3>::new();
        let (mut low, mut dispatcher) = q.split();
        low.enqueue_unchecked(1);
        let mut high = q.split().0;
        assert_eq!(high.enqueue(2), Ok(()));
        assert_eq!(low.enqueue(3), Err(3));
        assert_eq!(dispatcher.dequeue_unchecked(), 1);
        high.enqueue_unchecked(3);
        assert_eq!(dispatcher.dequeue(), Some(2));
        assert_eq!(dispatcher.dequeue(), Some(3));
        assert_eq!(dispatcher.dequeue(), None);
    }
}
