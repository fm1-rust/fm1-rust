//! A 100 Hz clock for async delays, periodic deadlines and timeouts.
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
    /// Acknowledge one TIMER5 tick and wake the tasks whose deadlines have passed.
    pub fn on_interrupt(&mut self) {
        self.0.on_interrupt();
        // SAFETY: the queue is initialized by start. The unique handle serializes
        // servicing it, and the application binds this call to TIMER5.
        unsafe { QUEUE.on_monotonic_interrupt() };
    }
}
