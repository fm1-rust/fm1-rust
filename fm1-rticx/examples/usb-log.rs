//! Read the last seven PLAY button events over USB-MIDI. Mirror each line to RTT.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use core::fmt::Write;
use fm1_rticx::bsp::{
    Board, enter_download_mode, entry,
    usb_midi::{Identity, PacketBuffer, UsbLog},
};
use fm1_rticx::rtt::{self, UpChannel};

entry!(run);

fn record(log: &mut UsbLog, rtt: &mut UpChannel, line: &str) {
    log.record(line).expect("short ASCII event");
    let _ = writeln!(rtt, "{line}");
}

fn run(mut board: Board) -> ! {
    let mut log = UsbLog::new();
    let mut rtt = rtt::init();
    record(&mut log, &mut rtt, "ready");
    let mut usb = board.take_usb().expect("one USB port").into_midi(Identity {
        vendor_id: 0x1209,
        product_id: 0x0001, // pid.codes shared test identity
        manufacturer: "fm1-rust",
        product: "FM-1 USB Log",
    });
    let mut rx = PacketBuffer::default();
    let mut pressed_since = None;
    let mut last_sample = board.timers.ticks();
    loop {
        usb.poll();
        let received = usb.receive(&mut rx);
        log.service(&mut usb, received);

        let now = board.timers.ticks();
        if now != last_sample {
            last_sample = now;
            match (board.panel.button().is_pressed(), pressed_since) {
                (true, None) => {
                    record(&mut log, &mut rtt, "play+");
                    pressed_since = Some(now);
                }
                (false, Some(_)) => {
                    record(&mut log, &mut rtt, "play-");
                    pressed_since = None;
                }
                (true, Some(start)) if now.wrapping_sub(start) >= 200 => enter_download_mode(),
                _ => {}
            }
        }
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    enter_download_mode()
}
