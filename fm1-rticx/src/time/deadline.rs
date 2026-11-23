//! The free-running TIMER4 counter supplies timestamps. TIMER2 wakes the queue.
use core::cell::RefCell;
use critical_section::Mutex;
use fm1::time::{Alarm, Timers, cycles};
use rtic_time::{
    fugit::{Duration, WrappingInstant},
    monotonic::TimerQueueBasedMonotonic,
    timer_queue::{TimerQueue, TimerQueueBackend},
};

const HZ: u64 = Timers::CRYSTAL_HZ as u64;

/// A wrapping TIMER4 timestamp in 24 MHz crystal cycles.
pub type DeadlineInstant = WrappingInstant<u32, 1, HZ>;
/// A duration in 24 MHz crystal cycles.
pub type DeadlineDuration = Duration<u32, 1, HZ>;

static QUEUE: TimerQueue<DeadlineClock> = TimerQueue::new();
static STATE: Mutex<RefCell<State>> = Mutex::new(RefCell::new(State {
    alarm: None,
    requested: false,
}));

struct State {
    alarm: Option<Alarm>,
    // A higher-priority task may insert a new head while the ISR examines the
    // queue. Its immediate alarm must survive the ISR committing the old head.
    requested: bool,
}

/// An interrupt-driven clock independent of TIMER5 and the panel scan.
///
/// Import [`super::Timebase`] for `now`, `delay_until`, `delay` and timeouts.
/// Keep deadlines less than 2³¹ cycles (about 89 seconds) ahead. Absolute
/// periodic deadlines can continue across any number of counter rollovers.
#[derive(Clone, Copy, Debug)]
pub struct DeadlineClock(());

impl DeadlineClock {
    /// Start the queue during RTICX initialization. `PRIORITY` is the TIMER2
    /// interrupt priority (1–8). Waiting tasks resume at their own priorities.
    /// The driver installs its vector and stops TIMER2 when the queue is empty.
    pub fn start<const PRIORITY: u8>(alarm: Alarm) {
        const {
            assert!(
                PRIORITY >= 1 && PRIORITY <= 8,
                "RTICX priority must be 1..=8"
            )
        };
        critical_section::with(|cs| {
            STATE.borrow_ref_mut(cs).alarm = Some(alarm);
            QUEUE.initialize(Self(()));
            #[cfg(target_os = "none")]
            // SAFETY: Board startup hands out one alarm. Interrupts are masked
            // here and the handler and queue are ready before the vector is enabled.
            unsafe {
                rticx_pi32v2::export::install_timer2(u16::from(PRIORITY))
            };
        });
    }
}

impl TimerQueueBasedMonotonic for DeadlineClock {
    type Backend = Self;
    type Instant = DeadlineInstant;
    type Duration = DeadlineDuration;
}

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

// Commit the head observed by the ISR unless another task has since kicked it.
fn program_next(deadline: Option<u32>) {
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        if !state.requested
            && let Some(alarm) = &mut state.alarm
        {
            match deadline {
                Some(at) => alarm.arm_after(period(cycles(), at)),
                None => alarm.cancel(),
            }
        }
    });
}

impl TimerQueueBackend for DeadlineClock {
    type Ticks = u32;

    fn now() -> u32 {
        cycles()
    }

    fn pend_interrupt() {
        critical_section::with(|cs| {
            let mut state = STATE.borrow_ref_mut(cs);
            state.requested = true;
            if let Some(alarm) = &mut state.alarm {
                alarm.arm_after(1);
            }
        });
    }

    fn clear_compare_flag() {
        critical_section::with(|cs| {
            let mut state = STATE.borrow_ref_mut(cs);
            state.requested = false;
            if let Some(alarm) = &mut state.alarm {
                alarm.cancel();
            }
        });
    }

    fn set_compare(deadline: u32) {
        program_next(Some(deadline));
    }

    fn disable_timer() {
        program_next(None);
    }

    fn timer_queue() -> &'static TimerQueue<Self> {
        &QUEUE
    }
}

/// # Safety
/// Call from the TIMER2 trampoline after queue initialization, with same-priority
/// reentry excluded.
#[cfg(target_os = "none")]
#[unsafe(export_name = "__fm1_rticx_deadline_irq")]
unsafe extern "C" fn interrupt() {
    // SAFETY: only the TIMER2 trampoline calls this handler. It cannot
    // re-enter at its own priority.
    unsafe { QUEUE.on_monotonic_interrupt() };
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
