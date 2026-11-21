//! Hardware clocks for async delays, periodic deadlines and timeouts.
mod deadline;
pub use deadline::{DeadlineClock, DeadlineDuration, DeadlineInstant};
use fm1::time::Timers;
pub use rtic_time::{Timebase, fugit::ExtU32};
use rtic_time::{
    monotonic::TimerQueueBasedMonotonic,
    timer_queue::{TimerQueue, TimerQueueBackend},
};

const HZ: u64 = Timers::TICK_HZ as u64;

/// A wrapping timestamp in units of 10 ms.
pub type Instant = rtic_time::fugit::WrappingInstant<u32, 1, HZ>;
/// A duration in units of 10 ms.
pub type Duration = rtic_time::fugit::Duration<u32, 1, HZ>;

static QUEUE: TimerQueue<Clock> = TimerQueue::new();

/// Wrapping TIMER5 timebase. Import [`Timebase`] to call `now`, `delay`,
/// `delay_until` and the timeout methods. Schedule within half the tick range.
#[derive(Clone, Copy, Debug)]
pub struct Clock(());

impl Clock {
    /// Start the delay queue during application initialisation. Give the
    /// returned handle to the task bound to TIMER5.
    #[must_use]
    pub fn start(timers: Timers) -> Timer5 {
        QUEUE.initialize(Self(()));
        Timer5(timers)
    }
}

impl TimerQueueBasedMonotonic for Clock {
    type Backend = Self;
    type Instant = Instant;
    type Duration = Duration;
}

rtic_time::impl_embedded_hal_async_delay_fugit!(Clock);

impl TimerQueueBackend for Clock {
    type Ticks = u32;

    fn now() -> u32 {
        fm1::time::ticks()
    }

    // TIMER5 runs periodically. The next tick inspects the queue even if a
    // new deadline was inserted immediately after the previous tick.
    fn set_compare(_: u32) {}
    fn clear_compare_flag() {}
    fn pend_interrupt() {}
    fn timer_queue() -> &'static TimerQueue<Self> {
        &QUEUE
    }
}

/// The timer interrupt task's exclusive handle to TIMER5.
#[derive(Debug)]
pub struct Timer5(Timers);
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
        // SAFETY: the queue is initialized by start. The unique handle serializes
        // servicing it, and the application binds this call to TIMER5.
        unsafe { QUEUE.on_monotonic_interrupt() };
    }
}
