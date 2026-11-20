// SPDX-FileCopyrightText: 2026 Julius Vitkauskas
// SPDX-License-Identifier: MIT-0
//! Adapted from Julius Vitkauskas’s [original FM-1 Rust blink example](https://github.com/jvitkauskas/pi32v2-isa/tree/701a76faf103d1889d9bdf67987b872899c27257/examples/fm1-blink-rust).
//!
//! PLAY LED at 1 Hz. Hold PLAY for two seconds to return to M-Flasher.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use fm1::{Board, embedded_hal::digital::StatefulOutputPin, enter_download_mode, entry};

entry!(blink);

fn blink(mut board: Board) -> ! {
    let mut last_sample = board.timers.ticks();
    let mut last_edge = last_sample;
    let mut pressed_since = None;

    loop {
        let now = board.timers.ticks();
        if now == last_sample {
            continue;
        }
        last_sample = now;
        if now.wrapping_sub(last_edge) >= 50 {
            board.panel.led().toggle().unwrap();
            last_edge = now;
        }
        if board.panel.button().is_pressed() {
            if now.wrapping_sub(*pressed_since.get_or_insert(now)) >= 200 {
                enter_download_mode();
            }
        } else {
            pressed_since = None;
        }
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    enter_download_mode()
}
