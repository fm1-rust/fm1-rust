//! A flash-resident RGB565 test row, repeated vertically. Hold PLAY to exit.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use fm1::{Board, display::SIZE, enter_download_mode, entry};

entry!(test_card);

#[allow(
    clippy::arithmetic_side_effects,
    reason = "Each of the 240 RGB565 pixels occupies two bytes."
)]
const fn bars() -> [u8; SIZE as usize * 2] {
    let colors = [0xf800u16, 0x07e0, 0x001f];
    let mut row = [0; SIZE as usize * 2];
    let mut x = 0;
    while x < SIZE as usize {
        let [hi, lo] = colors[x / 80].to_be_bytes();
        row[2 * x] = hi;
        row[2 * x + 1] = lo;
        x += 1;
    }
    row
}
static ROW: [u8; SIZE as usize * 2] = bars();

fn test_card(mut board: Board) -> ! {
    let mut display = board.display.init().expect("initialize the LCD");
    for y in 0..SIZE {
        display
            .write_rect(0, y, SIZE, 1, &ROW)
            .expect("write a test row");
    }
    while !board.panel.button().is_pressed() {}
    enter_download_mode()
}
#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    enter_download_mode()
}
