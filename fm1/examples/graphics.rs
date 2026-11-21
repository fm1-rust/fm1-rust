//! RGB test card. Hold PLAY at startup to draw through a framebuffer.
//! Press PLAY after the card appears to return to download mode.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use fm1::display::graphics::embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_6X10},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::Rectangle,
    text::{Baseline, Text},
};
use fm1::{
    Board,
    display::{SIZE, SpiClock, graphics::Framebuffer},
    enter_download_mode, entry,
};

entry!(test_card);

fn test_card(mut board: Board) -> ! {
    const X: u16 = 72;
    const Y: u16 = 110;
    if board.panel.button().is_pressed() {
        let mut lcd = board
            .display
            .init_with_clock(SpiClock::Div1)
            .expect("initialize LCD");
        lcd.fill_rect(0, 0, SIZE, SIZE, 0).expect("clear LCD");
        let mut canvas = Framebuffer::<96, 20>::new();
        let Ok(()) = draw_card(&mut canvas, "FRAMEBUFFER");
        lcd.write_framebuffer_buffered(X, Y, &mut canvas)
            .expect("send changed rows");
        lcd.flush().expect("finish pixel DMA");
    } else {
        let mut lcd = board
            .display
            .init_mipidsi(SpiClock::Div1, &mut board.timers)
            .expect("initialize ST7789");
        lcd.clear(Rgb565::BLACK).expect("clear LCD");
        let area = Rectangle::new(Point::new(i32::from(X), i32::from(Y)), Size::new(96, 20));
        draw_card(&mut lcd.cropped(&area), "ST7789").expect("draw directly to LCD");
    }
    while board.panel.button().is_pressed() {}
    while !board.panel.button().is_pressed() {}
    enter_download_mode()
}

fn draw_card<D: DrawTarget<Color = Rgb565>>(target: &mut D, label: &str) -> Result<(), D::Error> {
    for (x, color) in [(0, Rgb565::RED), (32, Rgb565::GREEN), (64, Rgb565::BLUE)] {
        target.fill_solid(&Rectangle::new(Point::new(x, 0), Size::new(32, 8)), color)?;
    }
    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    Text::with_baseline(label, Point::new(0, 10), style, Baseline::Top).draw(target)?;
    Ok(())
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    enter_download_mode()
}
