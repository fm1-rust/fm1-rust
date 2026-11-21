use super::*;
use display::{DisplayPort, Error, SpiClock, graphics::Framebuffer};
use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use embedded_hal::delay::DelayNs;
use mipidsi::interface::Interface;

struct Delay;
impl DelayNs for Delay {
    fn delay_ns(&mut self, _: u32) {}
}

fn transfers() -> Vec<(bool, Vec<u8>)> {
    CHIP.with_borrow_mut(|chip| core::mem::take(&mut chip.transfers))
}

#[test]
fn framebuffer_positions_dirty_rows_and_preserves_them_on_invalid_writes() {
    let _display = DISPLAY.lock().expect("exclusive display");
    let mut lcd = DisplayPort(Token::new()).init().expect("working SPI");
    let _init = transfers();
    let mut canvas = Framebuffer::<2, 4>::new();
    Pixel(Point::new(1, 1), Rgb565::RED)
        .draw(&mut canvas)
        .expect("infallible");
    Pixel(Point::new(0, 3), Rgb565::GREEN)
        .draw(&mut canvas)
        .expect("infallible");
    assert_eq!(
        lcd.write_framebuffer_buffered(238, 237, &mut canvas),
        Err(Error::InvalidRegion)
    );
    assert!(transfers().is_empty());
    lcd.write_framebuffer_buffered(238, 236, &mut canvas)
        .expect("fits at bottom right");
    lcd.write_framebuffer_buffered(238, 236, &mut canvas)
        .expect("clean");
    CHIP.with_borrow(|chip| assert!(chip.dma.is_some(), "clean writes leave DMA running"));
    let Ok(()) = canvas.clear(Rgb565::BLUE);
    lcd.flush().expect("finish staged rows");
    let sent = transfers();
    assert_eq!(sent.len(), 7);
    assert_eq!(sent[1].1, [0, 238, 0, 239]);
    assert_eq!(sent[3].1, [0, 237, 0, 239]);
    assert_eq!(sent[5].1, [0, 0, 0xf8, 0, 0, 0, 0, 0, 0x07, 0xe0, 0, 0]);
    assert_eq!(sent[6].1, [0x29]);

    // A timeout while staging the next update leaves it available for repaint.
    CHIP.with_borrow_mut(|chip| chip.dma_stuck = true);
    assert_eq!(
        lcd.write_framebuffer_buffered(0, 0, &mut canvas),
        Err(Error::Timeout)
    );
    assert_eq!(canvas.take_dirty().expect("retry").rows, 4);
}

#[test]
fn mipidsi_streams_rgb565_in_order_across_dma_boundaries() {
    let _display = DISPLAY.lock().expect("exclusive display");
    let mut lcd = DisplayPort(Token::new())
        .init_mipidsi(SpiClock::Div1, &mut Delay)
        .expect("working ST7789");
    assert_eq!(lcd.size(), Size::new(240, 240));
    assert_eq!(read(0x11d04), SpiClock::Div1 as u32);
    let init = transfers();
    assert!(
        init.windows(2)
            .any(|w| w[0].1 == [0x3a] && w[1].1 == [0x55])
    );
    assert!(init.windows(2).any(|w| w[0].1 == [0x36] && w[1].1 == [0]));
    assert!(init.iter().any(|(data, v)| !data && v == &[0x21]));

    let area = Rectangle::new(Point::new(40, 230), Size::new(193, 10));
    let palette = [Rgb565::RED, Rgb565::GREEN, Rgb565::BLUE];
    lcd.fill_contiguous(&area, palette.into_iter().cycle())
        .expect("stream pixels");
    let sent = transfers();
    assert_eq!(sent[1].1, [0, 40, 0, 232]);
    assert_eq!(sent[3].1, [0, 230, 0, 239]);
    assert_eq!(sent[5].1.len(), 3840);
    assert_eq!(sent[6].1.len(), 20);
    let bytes = sent[5..].iter().flat_map(|(_, v)| v.iter().copied());
    assert!(bytes.eq([0xf8, 0, 7, 0xe0, 0, 0x1f].into_iter().cycle().take(3860)));

    lcd.fill_solid(&area, Rgb565::BLUE).expect("repeat color");
    let sent = transfers();
    assert_eq!(sent[5].1.len(), 3840);
    assert_eq!(sent[6].1.len(), 20);
    assert!(
        sent[5..]
            .iter()
            .flat_map(|(_, v)| v.as_chunks::<2>().0)
            .all(|p| p == &[0, 0x1f])
    );

    let (mut spi, _, _) = lcd.release();
    spi.send_pixels(core::iter::empty::<[u8; 2]>())
        .expect("empty stream");
    spi.send_repeated_pixel([], 1).expect("empty word");
    spi.send_repeated_pixel([0; 2], 0).expect("zero pixels");
    assert!(transfers().is_empty());
    CHIP.with_borrow_mut(|chip| chip.dma_stuck = true);
    assert_eq!(spi.send_pixels([[1, 2]]), Err(Error::Timeout));
    assert_eq!(spi.send_command(0x29, &[]), Err(Error::Timeout));
    assert!(transfers().is_empty());
}
