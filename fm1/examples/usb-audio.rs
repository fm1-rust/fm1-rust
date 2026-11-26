//! A stereo channel check for the black FM-1. Press PLAY for download mode.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use fm1::{
    Board,
    audio::Block,
    enter_download_mode, entry,
    usb_midi::{CaptureBuffer, Identity, PacketBuffer},
};
use static_cell::ConstStaticCell;

entry!(tone);

fn tone(mut board: Board) -> ! {
    // SAFETY: CHIP_ID is a word-readable MMIO register on the FM-1.
    if unsafe { core::ptr::without_provenance::<u32>(0x10200).read_volatile() } != 0x6f01 {
        enter_download_mode();
    }
    let id = Identity {
        vendor_id: 0x1209,
        product_id: 1,
        manufacturer: "fm1-rust",
        product: "FM-1 Stereo Test",
    };
    let port = board.take_usb().expect("USB0 available");
    static FIFO: ConstStaticCell<CaptureBuffer> = ConstStaticCell::new(CaptureBuffer::new());
    // SAFETY: trusted black FM-1 loader and the 0x6f01 USB0/copy-engine
    // bounds and retirement required by the constructor. Only CPU0 runs,
    // with exclusive use of the idle copy engine. The loader must quiesce
    // both engines before reclaiming SRAM. A fault parks the firmware.
    let (mut usb, mut capture) = unsafe { port.into_midi_with_audio_capture(id, FIFO.take()) };
    let mut audio = board.audio.start_with_frames::<128>().expect("DAC clock");
    let mut block: Block<128> = [[0; 2]; 128];
    let mut phase = 0u8;
    let mut rx = PacketBuffer::default();
    loop {
        let _ = audio.try_write_with(|| {
            for frame in &mut block {
                // The 44,118 Hz DAC gives approximately 441 Hz left and 882 Hz right.
                let level = |high| if high { 262_144 } else { -262_144 };
                *frame = [level(phase < 50), level(phase % 50 < 25)];
                phase = phase.wrapping_add(1) % 100;
            }
            capture.write_q23(&block);
            &block
        });
        usb.poll();
        usb.receive(&mut rx);
        if board.panel.button().is_pressed() {
            drop(audio);
            drop(usb);
            enter_download_mode();
        }
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
