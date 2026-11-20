//! A quiet stereo square wave for checking the DAC. Press PLAY to stop and exit.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use fm1::{
    Board,
    audio::{BLOCK_FRAMES, Block, Refill},
    enter_download_mode, entry,
};

entry!(tone);

fn tone(mut board: Board) -> ! {
    let mut stream = board.audio.start().expect("initialize stereo output");
    let mut block: Block = [[0; 2]; BLOCK_FRAMES];
    let mut phase = 0u32;
    loop {
        for frame in &mut block {
            let sample = if phase < 50 { 32_768 } else { -32_768 };
            *frame = [sample; 2];
            phase = phase.wrapping_add(1) % 100;
        }
        while stream.try_write(&block) == Refill::WouldBlock {
            if board.panel.button().is_pressed() {
                drop(stream);
                enter_download_mode();
            }
        }
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    enter_download_mode()
}
