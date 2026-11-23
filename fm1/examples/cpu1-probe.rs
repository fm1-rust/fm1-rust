//! Run one Rust computation on CPU1 at boot. CPU0 reports the result on PLAY.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

// SAFETY: this binary is the sole definition of the C ABI symbol named by
// cpu1_entry.S. The linker includes its code in the CRT's SRAM copy. The
// function takes and returns one u32, touches no globals and cannot unwind.
#[cfg_attr(target_os = "none", unsafe(no_mangle))]
#[cfg_attr(target_os = "none", unsafe(link_section = ".cpu1.text.rust"))]
#[inline(never)]
extern "C" fn fm1_cpu1_probe_word(challenge: u32) -> u32 {
    challenge.rotate_left(7) ^ 0x000a_c791
}

/// # Safety
/// Enter once from the probe reset prefix and CRT, on CPU0 with IRQs masked.
#[cfg(target_os = "none")]
// SAFETY: this is the only C ABI main in the image, called once by crt0.
#[unsafe(export_name = "main")]
unsafe extern "C" fn probe_main() -> ! {
    #[link(name = "fm1_cpu1_probe", kind = "static")]
    unsafe extern "C" {
        fn fm1_cpu1_probe_run() -> u32;
    }
    // SAFETY: referencing the probe pulls its reset prefix into .entry.
    // It held CPU1 before _start copied .data and cleared .bss. This is the
    // first CPU0 call after that initialization, with interrupts still masked.
    let status = unsafe { fm1_cpu1_probe_run() };
    // SAFETY: the probe has held CPU1 again and restored both RAM vectors.
    // CPU0 still owns the peripherals and has not enabled interrupts.
    let board = unsafe { fm1::Board::from_reset() };
    indicate(board, status)
}

#[cfg(target_os = "none")]
fn indicate(mut board: fm1::Board, status: u32) -> ! {
    use fm1::{
        embedded_hal::{delay::DelayNs, digital::OutputPin},
        panel::PlayColor,
    };
    let passed = status == 1;
    let pulses = status.max(1);
    board.panel.led().set_color(if passed {
        PlayColor::Green
    } else {
        PlayColor::Amber
    });
    loop {
        for step in 0..20 {
            let on = passed || (step % 2 == 0 && step / 2 < pulses);
            let Ok(()) = board.panel.led().set_state(on.into());
            if board.panel.button().is_pressed() {
                fm1::enter_download_mode();
            }
            board.timers.delay_ms(100);
        }
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    // Host-side arithmetic check. Executing the boot probe requires pi32v2.
    assert_eq!(fm1_cpu1_probe_word(0x0123_4567), 0x91a8_7411);
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    fm1::enter_download_mode()
}
