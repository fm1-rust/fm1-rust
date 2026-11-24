use super::{CHUNK, CON, DONE, Display, Error, IRQ_ENABLE, TIMEOUT_CYCLES, clear, cycles, read};
use core::{
    cell::Cell,
    future::poll_fn,
    pin::pin,
    task::{Poll, Waker},
};
use critical_section::Mutex;
use embedded_hal_async::delay::DelayNs;

// The board gives CPU0 one display, with one staging strip and one possible waiter.
static WAITER: Mutex<Cell<Option<Waker>>> = Mutex::new(Cell::new(None));

/// Service the LCD's DMA completion interrupt. Bind this function to SPI1 on CPU0.
pub fn on_spi1_interrupt() {
    #[cfg(any(fm1, test))]
    if let Some(waker) = critical_section::with(|cs| {
        if read(CON) & (IRQ_ENABLE | DONE) != (IRQ_ENABLE | DONE) {
            return None;
        }
        // Mask the source even if DMA finished before the first poll. Leave DONE
        // latched so a subsequent synchronous or asynchronous flush can consume it.
        clear(CON, IRQ_ENABLE);
        WAITER.borrow(cs).take()
    }) {
        waker.wake();
    }
}

/// Buffered LCD writes with interrupt-driven pixel DMA and a timer for stalled transfers.
#[derive(Debug)]
pub struct AsyncDisplay<'a, D> {
    lcd: &'a mut Display,
    delay: &'a mut D,
}

impl Display {
    /// Finish any preceding blocking writes and borrow the display for async drawing.
    /// Bind SPI1 to [`on_spi1_interrupt`] and supply an independent async timer.
    ///
    /// # Errors
    /// Returns [`Error::Timeout`] if a preceding transfer failed.
    pub fn as_async<'a, D: DelayNs>(
        &'a mut self,
        delay: &'a mut D,
    ) -> Result<AsyncDisplay<'a, D>, Error> {
        self.flush()?;
        Ok(AsyncDisplay { lcd: self, delay })
    }
}

impl<D: DelayNs> AsyncDisplay<'_, D> {
    /// Fill a rectangle with one RGB565 color, yielding while SPI1 sends each
    /// strip. Completion includes the final transfer. Cancelling may leave a
    /// partial fill on screen. The display retains ownership of the pending DMA.
    ///
    /// # Errors
    /// Returns the same errors as [`Display::fill_rect`].
    pub async fn fill_rect(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        rgb565: u16,
    ) -> Result<(), Error> {
        let bytes = self.lcd.region_bytes(x, y, width, height)?;
        if bytes == 0 {
            return Ok(());
        }
        self.flush().await?;
        self.lcd.window(x, y, width, height)?;
        for len in self
            .lcd
            .spi
            .solid_transfers(rgb565.to_be_bytes(), bytes / 2)?
        {
            self.wait_for_dma().await;
            self.lcd.spi.wait_dma()?;
            self.lcd.spi.start_dma(len, true);
        }
        self.flush().await
    }

    /// Copy a rectangle into the DMA strip, yielding before each reuse. The last
    /// transfer stays in flight so the caller can prepare the next rectangle.
    /// If cancelled, a prefix of the rectangle may already have been submitted.
    ///
    /// # Errors
    /// Returns the same errors as [`Display::write_rect_buffered`].
    pub async fn write_rect_buffered(
        &mut self,
        x: u16,
        y: u16,
        width: u16,
        height: u16,
        pixels: &[u8],
    ) -> Result<(), Error> {
        self.lcd.validate_pixels(x, y, width, height, pixels)?;
        if pixels.is_empty() {
            return Ok(());
        }
        self.flush().await?;
        self.lcd.window(x, y, width, height)?;
        for chunk in pixels.chunks(CHUNK) {
            self.wait_for_dma().await;
            self.lcd.spi.wait_dma()?;
            self.lcd.spi.stage_bytes(chunk, true);
        }
        Ok(())
    }

    /// Await the last buffered pixels and reveal the first completed rectangle.
    /// Cancelling a wait leaves the pending transfer owned by the display.
    ///
    /// # Errors
    /// Returns [`Error::Timeout`] if SPI1 stalls or a previous transfer failed.
    pub async fn flush(&mut self) -> Result<(), Error> {
        self.wait_for_dma().await;
        self.lcd.flush()
    }

    async fn wait_for_dma(&mut self) {
        let Some(start) = self.lcd.spi.pending_since else {
            return;
        };
        let remaining = TIMEOUT_CYCLES.saturating_sub(cycles().wrapping_sub(start));
        if read(CON) & DONE == 0 && remaining != 0 {
            let mut timeout = pin!(self.delay.delay_us(remaining.div_ceil(24)));
            let _registration = Registration;
            poll_fn(|cx| {
                let waker = cx.waker().clone();
                drop(critical_section::with(|cs| {
                    WAITER.borrow(cs).replace(Some(waker))
                }));
                // Completion may have raced registration. DONE stays set until flush.
                if read(CON) & DONE != 0 || timeout.as_mut().poll(cx).is_ready() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            })
            .await;
        }
        // A fast completion can be observed before its interrupt is dispatched.
        // Keep the following synchronous window commands out of the IRQ path.
        clear(CON, IRQ_ENABLE);
    }
}

// A cancelled flush can leave its writer alive for a subsequent wait.
struct Registration;
impl Drop for Registration {
    fn drop(&mut self) {
        drop(critical_section::with(|cs| WAITER.borrow(cs).take()));
    }
}

impl<D> Drop for AsyncDisplay<'_, D> {
    fn drop(&mut self) {
        // A blocking caller can now finish any staged transfer using DONE.
        clear(CON, IRQ_ENABLE);
    }
}
