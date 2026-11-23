use super::{
    MidiDevice,
    controller::{self, CON0},
};
use crate::{
    registers::{clear, read, set},
    time::{Timers, cycles},
};
use core::{
    cell::Cell,
    future::poll_fn,
    pin::pin,
    task::{Poll, Waker},
};
use critical_section::Mutex;
use embedded_hal_async::delay::DelayNs;

// USB_CON0, AC79 SDK (wl82/asm/usb.h). The indexed SIE status registers
// clear on read. Only the task reads them, including when a wait is cancelled.
const SIEIE: u32 = 1 << 11;
const SIE_PND: u32 = 1 << 14;
const GUARD_MS: u32 = 100;
static WAITER: Mutex<Cell<Option<Waker>>> = Mutex::new(Cell::new(None));
static SIGNALED: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));

/// Wake the USB owner. Bind this to the CPU0 USB0 controller interrupt (IRQ 9).
pub fn on_usb0_interrupt() {
    #[cfg(any(fm1, test))]
    if let Some(waker) = critical_section::with(|cs| {
        if read(CON0) & SIEIE == 0 {
            return None;
        }
        clear(CON0, SIEIE);
        SIGNALED.borrow(cs).set(true);
        WAITER.borrow(cs).take()
    }) {
        waker.wake();
    }
}

impl MidiDevice {
    /// Await USB0 activity and service one batch of bus and endpoint interrupts.
    /// Inspect [`Self::session`], receive pending packets and retry queued output
    /// after each wake. A local producer can race this future with its own input.
    ///
    /// While attached, the delay also wakes the guard checker every 100 ms
    /// so a silent DMA write can be detected without an endpoint interrupt.
    /// An active DMA probe also wakes for a deferred acknowledgement, the end of
    /// its capture window, and the drain before freezing its snapshot.
    /// Suspend or disconnect can signal loss of the data host. For a cable pull
    /// that raises neither interrupt, callers can use [`Self::poll`] to sample
    /// frame progress while a missed disconnect matters (e.g. a sounding note).
    ///
    /// Cancelling this wait preserves endpoint data and the attachment and probe deadlines.
    ///
    /// # Panics
    /// Detecting a changed receive guard or an oversized normal endpoint count
    /// detaches USB0 and panics, as in [`Self::poll`].
    pub async fn wait_for_activity(&mut self, delay: &mut impl DelayNs) {
        if let Some((since, ticks)) = self.wait {
            let remaining = ticks.saturating_sub(cycles().wrapping_sub(since));
            if remaining != 0 {
                delay
                    .delay_us(remaining.div_ceil(Timers::CRYSTAL_HZ / 1_000_000))
                    .await;
            }
        } else {
            controller::check_rx_guards();
            let mut irq = pin!(wait_for_irq());
            loop {
                #[cfg(feature = "usb-dma-probe")]
                let (timer, service_on_timeout) = {
                    let ticks = self
                        .probe
                        .as_ref()
                        .and_then(|probe| probe.ticks_until_service(cycles(), self.rx_pending));
                    let (us, due) = probe_wait_us(ticks);
                    if us == 0 {
                        break;
                    }
                    (delay.delay_us(us), due)
                };
                #[cfg(feature = "usb-dma-probe")]
                let mut timer = pin!(timer);
                #[cfg(not(feature = "usb-dma-probe"))]
                let mut timer = pin!(delay.delay_ms(GUARD_MS));
                let signaled = poll_fn(|cx| {
                    if irq.as_mut().poll(cx).is_ready() {
                        Poll::Ready(true)
                    } else {
                        timer.as_mut().poll(cx).map(|()| false)
                    }
                })
                .await;
                if signaled {
                    break;
                }
                #[cfg(feature = "usb-dma-probe")]
                if service_on_timeout {
                    break;
                }
                controller::check_rx_guards();
            }
        }
        if let Err(error) = self.service(false) {
            self.retry(error);
        }
    }
}

/// Microseconds until the next guard check or probe deadline, and whether service is due.
#[cfg(feature = "usb-dma-probe")]
fn probe_wait_us(probe_ticks: Option<u32>) -> (u32, bool) {
    let guard_ticks = GUARD_MS * (Timers::CRYSTAL_HZ / 1000);
    match probe_ticks {
        Some(ticks) if ticks <= guard_ticks => {
            (ticks.div_ceil(Timers::CRYSTAL_HZ / 1_000_000), true)
        }
        _ => (GUARD_MS * 1000, false),
    }
}

async fn wait_for_irq() {
    let _registration = Registration;
    poll_fn(|cx| {
        let waker = cx.waker().clone();
        let (poll, previous) = critical_section::with(|cs| {
            if SIGNALED.borrow(cs).replace(false) {
                // Drop the unused clone after leaving the critical section.
                return (Poll::Ready(()), Some(waker));
            }
            let previous = WAITER.borrow(cs).replace(Some(waker));
            set(CON0, SIEIE);
            // A cause may already be latched when we open the interrupt gate.
            if read(CON0) & SIE_PND != 0 {
                clear(CON0, SIEIE);
                (Poll::Ready(()), previous)
            } else {
                (Poll::Pending, previous)
            }
        });
        drop(previous);
        poll
    })
    .await;
}

struct Registration;

impl Drop for Registration {
    fn drop(&mut self) {
        drop(critical_section::with(|cs| {
            clear(CON0, SIEIE);
            WAITER.borrow(cs).take()
        }));
    }
}

#[cfg(test)]
mod tests {
    extern crate alloc;

    use super::*;
    use crate::registers::write;
    use alloc::{sync::Arc, task::Wake};
    use core::{
        pin::pin,
        sync::atomic::{AtomicUsize, Ordering},
        task::Context,
    };

    #[derive(Default)]
    struct Counter(AtomicUsize);

    impl Wake for Counter {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[cfg(feature = "usb-dma-probe")]
    #[test]
    fn timer_services_a_probe_deadline_and_keeps_the_guard_interval() {
        let guard_ticks = Timers::CRYSTAL_HZ / 10;
        assert_eq!(probe_wait_us(None), (100_000, false));
        assert_eq!(probe_wait_us(Some(guard_ticks + 1)), (100_000, false));
        assert_eq!(probe_wait_us(Some(guard_ticks)), (100_000, true));
        assert_eq!(probe_wait_us(Some(1)), (1, true));
        assert_eq!(probe_wait_us(Some(0)), (0, true));
    }

    #[test]
    fn irq_wait_preserves_pending_work_across_registration_and_cancellation() {
        let counter = Arc::new(Counter::default());
        let waker = Waker::from(counter.clone());
        let mut cx = Context::from_waker(&waker);

        // A packet landed before the first poll.
        write(CON0, SIE_PND);
        assert!(pin!(wait_for_irq()).as_mut().poll(&mut cx).is_ready());
        assert_eq!(read(CON0) & SIEIE, 0);

        write(CON0, 0);
        {
            let mut wait = pin!(wait_for_irq());
            assert!(wait.as_mut().poll(&mut cx).is_pending());
            assert_ne!(read(CON0) & SIEIE, 0);
        }
        // A competing application wake cancelled the future.
        assert_eq!(read(CON0) & SIEIE, 0);
        on_usb0_interrupt();
        assert_eq!(counter.0.load(Ordering::Relaxed), 0);

        {
            let mut wait = pin!(wait_for_irq());
            assert!(wait.as_mut().poll(&mut cx).is_pending());
            set(CON0, SIE_PND);
            on_usb0_interrupt();
            on_usb0_interrupt();
            assert_eq!(counter.0.load(Ordering::Relaxed), 1);
            assert_eq!(read(CON0) & (SIEIE | SIE_PND), SIE_PND);
        }
        // The IRQ woke a future that lost its race. Its successor still runs.
        assert!(pin!(wait_for_irq()).as_mut().poll(&mut cx).is_ready());
        clear(CON0, SIE_PND);
        assert!(pin!(wait_for_irq()).as_mut().poll(&mut cx).is_pending());
    }
}
