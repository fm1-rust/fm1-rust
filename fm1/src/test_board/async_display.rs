use super::*;
extern crate alloc;
use alloc::{sync::Arc, task::Wake};
use core::{
    cell::Cell,
    future::poll_fn,
    pin::pin,
    sync::atomic::{AtomicU32, Ordering::Relaxed},
    task::{Context, Poll, Waker},
};
use display::{DisplayPort, Error, on_spi1_interrupt};

#[derive(Default)]
struct Wakes(AtomicU32);
impl Wake for Wakes {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Relaxed);
    }
}

#[derive(Default)]
struct Alarm {
    fired: Cell<bool>,
    waker: Cell<Option<Waker>>,
}
impl embedded_hal_async::delay::DelayNs for &Alarm {
    async fn delay_ns(&mut self, _: u32) {
        poll_fn(|cx| {
            if self.fired.get() {
                Poll::Ready(())
            } else {
                self.waker.set(Some(cx.waker().clone()));
                Poll::Pending
            }
        })
        .await;
    }
}

fn complete_dma() {
    CHIP.with_borrow_mut(|chip| chip.dma_stuck = false);
    on_spi1_interrupt();
}

#[test]
fn interrupt_releases_each_strip_and_early_completion_is_latched() {
    let _display = DISPLAY.lock().expect("exclusive display");
    let mut lcd = DisplayPort(Token::new()).init().expect("working SPI");
    let alarm = Alarm::default();
    let mut delay = &alarm;
    let mut writer = lcd.as_async(&mut delay).expect("idle LCD");
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    CHIP.with_borrow_mut(|chip| chip.hold_irq_dma = true);
    let pixels = [0x34; 240 * 9 * 2];
    {
        let mut write = pin!(writer.write_rect_buffered(0, 0, 240, 9, &pixels));
        assert!(write.as_mut().poll(&mut cx).is_pending());
        on_spi1_interrupt(); // spurious IRQ while the first strip is busy
        assert_eq!(wakes.0.load(Relaxed), 0);
        complete_dma();
        on_spi1_interrupt();
        assert_eq!(wakes.0.load(Relaxed), 1);
        assert_eq!(write.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }
    complete_dma(); // last strip is ready before flush registers a waker
    assert_eq!(pin!(writer.flush()).poll(&mut cx), Poll::Ready(Ok(())));
    CHIP.with_borrow(|chip| {
        let data: Vec<_> = chip
            .transfers
            .iter()
            .filter(|(data, v)| *data && v.len() > 4)
            .collect();
        assert_eq!(data[0].1, pixels[..3840]);
        assert_eq!(data[1].1, pixels[3840..]);
    });
}

#[test]
fn cancelled_wait_can_be_finished_synchronously_and_a_stall_times_out() {
    let _display = DISPLAY.lock().expect("exclusive display");
    let mut lcd = DisplayPort(Token::new()).init().expect("working SPI");
    let alarm = Alarm::default();
    let mut delay = &alarm;
    let wakes = Arc::new(Wakes::default());
    let waker = Waker::from(wakes.clone());
    let mut cx = Context::from_waker(&waker);
    CHIP.with_borrow_mut(|chip| chip.hold_irq_dma = true);
    for cancel in [true, false] {
        let mut writer = lcd.as_async(&mut delay).expect("idle LCD");
        assert_eq!(
            pin!(writer.write_rect_buffered(1, 2, 1, 1, &[1, 2])).poll(&mut cx),
            Poll::Ready(Ok(()))
        );
        {
            let mut wait = pin!(writer.flush());
            assert!(wait.as_mut().poll(&mut cx).is_pending());
            if !cancel {
                CHIP.with_borrow_mut(|chip| chip.ticks = chip.ticks.wrapping_add(50 * 24_000));
                alarm.fired.set(true);
                alarm
                    .waker
                    .take()
                    .expect("independent timeout armed")
                    .wake();
                assert_eq!(
                    wait.as_mut().poll(&mut cx),
                    Poll::Ready(Err(Error::Timeout))
                );
            }
        }
        drop(writer);
        assert_eq!(read(0x11d00) & (1 << 13), 0);
        if cancel {
            CHIP.with_borrow(|chip| assert!(chip.dma.is_some()));
            complete_dma();
            assert_eq!(wakes.0.load(Relaxed), 0);
            lcd.flush().expect("finish the cancelled wait");
        }
    }
    assert_eq!(lcd.flush(), Err(Error::Timeout));
    assert_eq!(read(0x11d00) & 1, 0);
    assert_ne!(read(PC + OUT) & (1 << 7), 0);
}
