//! Three solid RGB565 bars using one LCD window per color. Press PLAY to exit.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use fm1::{
    Board,
    display::{SIZE, SpiClock},
    enter_download_mode, entry,
};

entry!(test_card);

fn test_card(mut board: Board) -> ! {
    let mut display = board
        .display
        .init_with_clock(SpiClock::Div1)
        .expect("initialize the LCD");
    for (x, color) in [(0, 0xf800), (80, 0x07e0), (160, 0x001f)] {
        display
            .fill_rect(x, 0, SIZE / 3, SIZE, color)
            .expect("write a color bar");
    }
    while !board.panel.button().is_pressed() {}
    enter_download_mode()
}
#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    enter_download_mode()
}
