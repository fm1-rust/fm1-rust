//! The free-running TIMER4 counter supplies timestamps. TIMER2 wakes the queue.
use super::{
    TimerSlot,
    queue::{self, Hardware, Queue},
};
use core::cell::RefCell;
use critical_section::Mutex;
use fm1::time::{Alarm, Timers, cycles};
use rtic_time::fugit::{Duration, WrappingInstant};
use static_cell::ConstStaticCell;

const HZ: u64 = Timers::CRYSTAL_HZ as u64;

/// A wrapping TIMER4 timestamp in 24 MHz crystal cycles.
pub type DeadlineInstant = WrappingInstant<u32, 1, HZ>;
/// A duration in 24 MHz crystal cycles.
pub type DeadlineDuration = Duration<u32, 1, HZ>;

static QUEUE: Queue<DeadlineClock> = Queue::new();
struct Compare {
    alarm: Alarm,
    at: Option<u32>,
}

static COMPARE: Mutex<RefCell<Option<Compare>>> = Mutex::new(RefCell::new(None));

/// An interrupt-driven clock independent of TIMER5 and the panel scan.
///
/// Import [`super::Timebase`] for `now`, `delay_until`, `delay` and timeouts.
/// Keep deadlines less than 2³¹ cycles (about 89 seconds) ahead. Absolute
/// periodic deadlines can continue across any number of counter rollovers.
///
/// A new pending wait panics on poll if all slots are occupied. A slot becomes
/// available when its wait completes or is dropped. See [`Self::start_with_slots`].
#[derive(Clone, Copy, Debug)]
pub struct DeadlineClock(());

impl DeadlineClock {
    /// Start the queue during RTICX initialization. `PRIORITY` is the TIMER2
    /// interrupt priority (1–8). Waiting tasks resume at their own priorities.
    /// The driver installs its vector and stops TIMER2 when the queue is empty.
    /// Provides 16 timer slots.
    ///
    /// # Panics
    /// Panics if this clock has already started.
    pub fn start<const PRIORITY: u8>(alarm: Alarm) {
        static SLOTS: ConstStaticCell<[TimerSlot; 16]> =
            ConstStaticCell::new([const { TimerSlot::EMPTY }; 16]);
        Self::start_with_slots::<PRIORITY>(alarm, SLOTS.take());
    }

    /// Start TIMER2 with one caller-supplied slot per outstanding delay or timeout.
    ///
    /// # Panics
    /// Panics if the storage is empty or this clock has already started.
    pub fn start_with_slots<const PRIORITY: u8>(alarm: Alarm, slots: &'static mut [TimerSlot]) {
        const {
            assert!(
                PRIORITY >= 1 && PRIORITY <= 8,
                "RTICX priority must be 1..=8"
            )
        };
        critical_section::with(|cs| {
            QUEUE.initialize(slots);
            *COMPARE.borrow_ref_mut(cs) = Some(Compare { alarm, at: None });
            #[cfg(target_os = "none")]
            // SAFETY: Board startup hands out one alarm. Interrupts are masked
            // here and the handler and queue are ready before the vector is enabled.
            unsafe {
                rticx_pi32v2::export::install_timer2(u16::from(PRIORITY))
            };
        });
    }
}

queue::impl_timebase!(DeadlineClock, DeadlineInstant, DeadlineDuration, QUEUE);

// Positive differences in the wrapping half range are future deadlines.
// An already due deadline needs only the minimum nonzero hardware period.
fn period(now: u32, deadline: u32) -> u32 {
    let delta = deadline.wrapping_sub(now);
    if delta > i32::MAX as u32 {
        1
    } else {
        delta.max(1)
    }
}

impl Hardware for DeadlineClock {
    fn now() -> u32 {
        cycles()
    }

    fn program(deadline: Option<u32>) {
        critical_section::with(|cs| {
            if let Some(compare) = COMPARE.borrow_ref_mut(cs).as_mut()
                && compare.at != deadline
            {
                // A later registration must not restart an already armed compare.
                compare.at = deadline;
                match deadline {
                    Some(at) => compare.alarm.arm_after(period(cycles(), at)),
                    None => compare.alarm.cancel(),
                }
            }
        });
    }
}

/// # Safety
/// Call from the TIMER2 trampoline after queue initialization, with same-priority
/// reentry excluded.
#[cfg(target_os = "none")]
#[unsafe(export_name = "__fm1_rticx_deadline_irq")]
unsafe extern "C" fn interrupt() {
    // Acknowledge the interrupt before processing expired timers.
    DeadlineClock::program(None);
    QUEUE.service();
}

#[cfg(test)]
mod tests {
    use super::period;

    #[test]
    fn alarm_period_crosses_wrap_and_handles_an_elapsed_deadline() {
        assert_eq!(period(u32::MAX - 23, 24), 48);
        assert_eq!(period(24, u32::MAX - 23), 1);
        assert_eq!(period(42, 42), 1);
        assert_eq!(period(42, 43), 1);
        assert_eq!(period(0, i32::MAX as u32), i32::MAX as u32);
    }
}
