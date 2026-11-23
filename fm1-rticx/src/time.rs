//! Hardware clocks for async delays, periodic deadlines and timeouts.
mod deadline;
mod queue;
pub use deadline::{DeadlineClock, DeadlineDuration, DeadlineInstant};
use fm1::time::Timers;
pub use queue::TimerSlot;
use queue::{Hardware, Queue};
pub use rtic_time::{Timebase, fugit::ExtU32};
use static_cell::ConstStaticCell;
const HZ: u64 = Timers::TICK_HZ as u64;

/// A wrapping timestamp in units of 10 ms.
pub type Instant = rtic_time::fugit::WrappingInstant<u32, 1, HZ>;
/// A duration in units of 10 ms.
pub type Duration = rtic_time::fugit::Duration<u32, 1, HZ>;

static QUEUE: Queue<Clock> = Queue::new();

/// Wrapping TIMER5 timebase. Import [`Timebase`] to call `now`, `delay`,
/// `delay_until` and the timeout methods. Schedule within half the tick range.
///
/// A new pending wait panics on poll if all slots are occupied. A slot becomes
/// available when its wait completes or is dropped. See [`Self::start_with_slots`].
#[derive(Clone, Copy, Debug)]
pub struct Clock(());

impl Clock {
    /// Start the delay queue during application initialisation. Give the
    /// returned handle to the task bound to TIMER5. Provides 16 timer slots.
    ///
    /// # Panics
    /// Panics if this clock has already started.
    #[must_use]
    pub fn start(timers: Timers) -> Timer5 {
        static SLOTS: ConstStaticCell<[TimerSlot; 16]> =
            ConstStaticCell::new([const { TimerSlot::EMPTY }; 16]);
        Self::start_with_slots(timers, SLOTS.take())
    }

    /// Start with one caller-supplied slot per outstanding delay or timeout.
    ///
    /// # Panics
    /// Panics if the storage is empty or this clock has already started.
    #[must_use]
    pub fn start_with_slots(timers: Timers, slots: &'static mut [TimerSlot]) -> Timer5 {
        QUEUE.initialize(slots);
        Timer5(timers.into())
    }
}

queue::impl_timebase!(Clock, Instant, Duration, QUEUE);

rtic_time::impl_embedded_hal_async_delay_fugit!(Clock);

impl Hardware for Clock {
    fn now() -> u32 {
        fm1::time::ticks()
    }
    // TIMER5 runs periodically even when there are no delays.
    fn program(_: Option<u32>) {}
}

/// The timer interrupt task's exclusive handle to TIMER5.
#[derive(Debug)]
pub struct Timer5(fm1::Cpu0<Timers>);
impl Timer5 {
    /// Obtain a delay provider for peripherals using `embedded-hal-async`.
    #[must_use]
    pub fn clock(&self) -> Clock {
        Clock(())
    }

    /// Acknowledge TIMER5 and wake tasks on each 10 ms clock tick. A faster panel
    /// interrupt can therefore share TIMER5 with the normal async delay queue.
    pub fn on_interrupt(&mut self) {
        let previous = self.0.ticks();
        self.0.on_interrupt();
        if self.0.ticks() == previous {
            return;
        }
        QUEUE.service();
    }
}

#[cfg(test)]
mod tests;
