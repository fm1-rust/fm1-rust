//! Initialize a synth patch and a four-note MIDI phrase, save edits and restore them on boot.
//! Installing this example authorizes it to erase foreign data in its 16 KiB reservation.
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
    embedded_hal::delay::DelayNs,
    enter_download_mode, entry,
    panel::{Button, Leds},
    storage::{Error, Store},
};

entry!(run);

// Application format v1: waveform, attack, release; MIDI note-on messages.
const PATCHES: [&[u8]; 2] = [&[1, 0, 8, 70], &[1, 1, 24, 50]];
const PHRASES: [&[u8]; 2] = [
    &[0x90, 60, 96, 0x90, 64, 80, 0x90, 67, 90, 0x90, 72, 70],
    &[0x90, 62, 96, 0x90, 65, 80, 0x90, 69, 90, 0x90, 74, 70],
];

// This example owns the reserved partition. Provision each slot independently
// so a reset during the first boot can resume without losing the other record.
fn restore_or_initialize(
    store: &mut Store<'_>,
    slot: u8,
    choices: [&[u8]; 2],
) -> Result<usize, Error> {
    store.reclaim(slot)?;
    let mut bytes = [0; 12];
    let n = match store.load(slot, &mut bytes) {
        Ok(Some(n)) => n,
        Ok(None) | Err(Error::Corrupt) => {
            store.save(slot, choices[0])?;
            return Ok(0);
        }
        Err(e) => return Err(e),
    };
    choices
        .iter()
        .position(|&expected| expected == &bytes[..n])
        .ok_or(Error::Corrupt)
}

fn label(value: Result<usize, Error>) -> &'static str {
    match value {
        Ok(0) => "Variation A",
        Ok(_) => "Variation B",
        Err(e) => message(e),
    }
}

fn message(error: Error) -> &'static str {
    match error {
        Error::Busy => "CPU1 or controller busy",
        Error::Unsupported => "Unsupported flash or XIP map",
        Error::Occupied => "Storage still occupied",
        Error::Corrupt | Error::BufferTooSmall(_) => "Unrecognized demo record",
        Error::Io => "Flash I/O failed",
        Error::OutOfRange => "Invalid slot or length",
    }
}

fn run(mut board: Board) -> ! {
    let mut port = board.take_storage().expect("one SPI NOR port");
    let mut lcd = board
        .display
        .init_mipidsi(SpiClock::Div1, &mut board.timers)
        .expect("LCD");
    lcd.clear(Rgb565::BLACK).expect("clear LCD");
    let style = MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE);
    let mut line = |y, s: &str| {
        let area =
            embedded_graphics::primitives::Rectangle::new(Point::new(0, y), Size::new(240, 15));
        lcd.fill_solid(&area, Rgb565::BLACK).expect("clear row");
        Text::with_baseline(s, Point::new(4, y), style, Baseline::Top)
            .draw(&mut lcd)
            .expect("text");
    };
    line(8, "FM-1 flash storage");

    line(42, "Patch");
    line(84, "MIDI phrase");
    line(200, "PLAY enters download mode");
    let mut controls = board.panel.into_controls();
    let mut wait_for_save = || loop {
        board.timers.delay_us(100);
        let Some(scan) = controls.scan(&Leds::default()) else {
            continue;
        };
        if scan.pressed(Button::Play) {
            enter_download_mode();
        }
        if scan.pressed(Button::Save) {
            return;
        }
    };
    loop {
        line(132, "Preparing flash...");
        line(160, "");
        // SAFETY: this single-core image reserves fm1-storage.x and uses the
        // BSP startup, TIMER5 and SRAM-backed LCD DMA. No other handler or DMA
        // client can access SPI0 or XIP during a storage operation.
        let mut store = unsafe { port.open(*b"DEMO") };
        let (patch, phrase) = match &mut store {
            Ok(s) => (
                restore_or_initialize(s, 0, PATCHES),
                restore_or_initialize(s, 1, PHRASES),
            ),
            Err(e) => (Err(*e), Err(*e)),
        };
        line(59, label(patch));
        line(101, label(phrase));
        if let (Ok(s), Ok(current), Ok(_)) = (&mut store, patch, phrase) {
            line(132, "Ready");
            line(160, "SAVE stores next variation");
            let mut next = current ^ 1;
            loop {
                wait_for_save();
                if let Err(e) = s.save(0, PATCHES[next]) {
                    line(132, message(e));
                    break;
                }
                line(59, label(Ok(next)));
                if let Err(e) = s.save(1, PHRASES[next]) {
                    line(132, message(e));
                    break;
                }
                line(101, label(Ok(next)));
                line(132, "Saved and verified");
                next ^= 1;
            }
        } else {
            line(132, "Flash setup failed");
        }
        line(160, "SAVE retries flash setup");
        wait_for_save();
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    enter_download_mode()
}
