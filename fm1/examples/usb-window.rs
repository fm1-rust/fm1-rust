//! Standalone WND3 comparator probe. Hold PLAY for two seconds to exit.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use fm1::{
    Board,
    display::{
        SpiClock,
        graphics::embedded_graphics::{
            mono_font::{MonoTextStyle, ascii::FONT_8X13},
            pixelcolor::Rgb565,
            prelude::*,
            text::{Baseline, Text},
        },
    },
    enter_download_mode, entry,
    time::{Timers, cycles},
    usb_midi::Identity,
};

entry!(run);

fn run(mut board: Board) -> ! {
    let port = board.take_usb().expect("one USB0");
    // SAFETY: this standalone bench image reserves the diagnostic record and slot
    // and uses only CPU0. Board::from_reset enables only TIMER5, the LCD is
    // polled, and this program installs no other interrupts. Deployment
    // requires the isolated fixture and DMA
    // bounds/drain documented in docs/usb-dma/window.md, including EP0
    // enumeration across reboot. The measured window is internal to the arena.
    let mut usb = unsafe {
        port.into_window_probe(Identity {
            vendor_id: 0x1209,
            product_id: 1,
            manufacturer: "fm1-rust",
            product: "FM-1 USB Window",
        })
    };
    let mut lcd = board
        .display
        .init_mipidsi(SpiClock::Div1, &mut board.timers)
        .expect("LCD");
    lcd.clear(Rgb565::BLACK).expect("clear");
    let style = MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE);
    let mut line = |s: &str, x, y| {
        Text::with_baseline(s, Point::new(x, y), style, Baseline::Top)
            .draw(&mut lcd)
            .expect("LCD line");
    };
    line("FM-1 USB WINDOW / WND3 R4", 8, 4);
    let h = usb.window_header().expect("window probe");
    if &h[..4] == b"WND3" {
        line("Retained record (hex)", 8, 23);
        let word = |i: usize| u32::from_le_bytes(h.as_chunks::<4>().0[i]);
        let details = match usb.window_setup_check() {
            Some((address, expected, observed)) => [
                ("Reg address", address),
                ("Requested", expected),
                ("Observed", observed),
            ],
            None => [
                ("First past", word(7)),
                ("Byte at cut", word(8)),
                ("Last change", word(9)),
            ],
        };
        for ((label, value), y) in [
            ("Token", word(1)),
            ("Case", word(2)),
            ("Status", word(3)),
            ("Cut", word(5)),
        ]
        .into_iter()
        .chain(details)
        .chain([
            ("DBG_MSG", word(10)),
            ("Slot mask", word(11)),
            ("Master", word(12)),
            ("Exception", word(14)),
        ])
        .zip((40..).step_by(15))
        {
            let hex = value.to_be_bytes().map(|b| {
                [
                    b"0123456789ABCDEF"[usize::from(b >> 4)],
                    b"0123456789ABCDEF"[usize::from(b & 15)],
                ]
            });
            line(label, 8, y);
            line(
                core::str::from_utf8(hex.as_flattened()).expect("hex"),
                130,
                y,
            );
        }
    } else {
        line("Ready: fm1 dma-window", 8, 38);
        line("No retained result", 8, 55);
    }
    line("Hold PLAY 2s for flasher", 8, 218);
    let mut pressed = None;
    loop {
        usb.poll();
        if board.panel.button().is_pressed() {
            let now = cycles();
            if now.wrapping_sub(*pressed.get_or_insert(now)) >= 2 * Timers::CRYSTAL_HZ {
                enter_download_mode();
            }
        } else {
            pressed = None;
        }
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    enter_download_mode()
}
