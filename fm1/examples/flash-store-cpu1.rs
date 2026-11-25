//! Exercise refusal, saving and worker resumption on an FM-1 with a live CPU1.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(not(target_os = "none"))]
fn main() {}

#[cfg(target_os = "none")]
mod target {
    use core::sync::atomic::{AtomicU32, Ordering};
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
        panel::{Button, Leds},
        storage::{self, Error, Store},
        time::{Timers, cycles},
    };

    // CPU0 first presents the reported bad read of zero to the XIP worker.
    // Its reply proves it saw that value and is still withholding checkpoints.
    static JOB: AtomicU32 = AtomicU32::new(0);
    static COMPLETED: AtomicU32 = AtomicU32::new(0);
    const STRAY_BIT: u32 = 0x0020_0000;
    const WITHHOLDING: u32 = 0x5741_4954;
    const ENABLE: u32 = 0x4152_4d31;
    // Private dry-run results from the same native gate used by all NOR commands.
    const PARKED: u32 = 17;
    const NO_ACK: u32 = 19;
    const PATCH: [[u8; 4]; 2] = [[1, 0, 8, 70], [1, 1, 24, 50]];

    #[link(name = "fm1_cpu1_probe", kind = "static")]
    unsafe extern "C" {
        fn fm1_cpu1_probe_run(persistent: u32) -> u32;
        fn fm1_cpu1_audio_ready(reply: u32);
        fn fm1_cpu1_probe_stop();
        safe fn fm1_core_id() -> u32;
    }

    unsafe extern "C" {
        // CPU0 checks the gate and restores interrupts. This operation never
        // changes SFC or the flash pins, including if the peer is misclassified.
        safe fn fm1_storage_check_peer() -> u32;
    }

    // SAFETY: the probe assembly calls this single fixed entry on the checked
    // CPU1 stack. The worker only uses its own stack and static atomic words.
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".cpu1.text.flash_demo")]
    extern "C" fn fm1_cpu1_probe_word(challenge: u32) -> ! {
        // SAFETY: CPU1 alone publishes this boot response in the probe mailbox.
        unsafe { fm1_cpu1_audio_ready(challenge.rotate_left(7) ^ 0x000a_c791) };
        worker()
    }

    #[inline(never)]
    fn worker() -> ! {
        while JOB.load(Ordering::Acquire) != STRAY_BIT {
            core::hint::spin_loop();
        }
        COMPLETED.store(WITHHOLDING, Ordering::Release);
        while JOB.load(Ordering::Acquire) != ENABLE {
            core::hint::spin_loop();
        }
        loop {
            storage::cpu1_checkpoint();
            let job = JOB.load(Ordering::Acquire);
            COMPLETED.store(job, Ordering::Release);
        }
    }

    fn wait_for(job: u32) -> bool {
        let start = cycles();
        for _ in 0..100_000 {
            if COMPLETED.load(Ordering::Acquire) == job {
                return true;
            }
            if cycles().wrapping_sub(start) >= Timers::CRYSTAL_HZ / 10 {
                break;
            }
        }
        false
    }

    fn send_job(job: u32) -> bool {
        JOB.store(job, Ordering::Release);
        wait_for(job)
    }

    fn gate_error(code: u32) -> &'static str {
        match code {
            1 => "Gate called on wrong core",
            2 => "Gate code/stack outside SRAM",
            4 => "Invalid storage reservation",
            16 => "Live peer classified inactive",
            PARKED => "Peer acknowledged too early",
            18 => "Previous ack did not clear",
            NO_ACK => "CPU1 did not acknowledge",
            _ => "Unknown gate result",
        }
    }

    fn hex(digits: &mut [u8], mut value: u32) {
        for digit in digits.iter_mut().rev() {
            *digit = b"0123456789ABCDEF"[(value & 15) as usize];
            value >>= 4;
        }
    }

    fn details(
        line: &mut impl FnMut(i32, &str),
        boot: u32,
        negative: Option<u32>,
        positive: Option<u32>,
    ) {
        let mut codes = *b"B=-- N=-- P=-- C1=--------";
        hex(&mut codes[2..4], boot);
        if let Some(rc) = negative {
            hex(&mut codes[7..9], rc);
        }
        if let Some(rc) = positive {
            hex(&mut codes[12..14], rc);
        }
        // SAFETY: fixed, word-aligned AC791N control registers with side-effect-free
        // reads. Only CPU0 runs the UI, after the native transfer has returned.
        let (control, sfc, base) = unsafe {
            (
                core::ptr::without_provenance::<u32>(0x01ee_e004).read_volatile(),
                core::ptr::without_provenance::<u32>(0x0004_0200).read_volatile(),
                core::ptr::without_provenance::<u32>(0x0004_020c).read_volatile(),
            )
        };
        hex(&mut codes[18..], control);
        line(151, core::str::from_utf8(&codes).expect("ASCII"));
        let mut words = *b"JOB=00000000 REPLY=00000000";
        hex(&mut words[4..12], JOB.load(Ordering::Relaxed));
        hex(&mut words[19..], COMPLETED.load(Ordering::Acquire));
        line(167, core::str::from_utf8(&words).expect("ASCII"));
        let mut bus = *b"SFC=00000000 MAP=00000000";
        hex(&mut bus[4..12], sfc);
        hex(&mut bus[17..], base);
        line(183, core::str::from_utf8(&bus).expect("ASCII"));
    }

    // SAFETY: the probe reset prefix held CPU1 before CRT initialized RAM.
    // The only C main runs once on CPU0 with interrupts masked and owns the board.
    #[unsafe(export_name = "main")]
    extern "C" fn start() -> ! {
        // SAFETY: startup as above, before Board starts TIMER4. The persistent
        // worker never touches peripherals, CPU0's stacks or its IRQ vectors.
        let boot = unsafe { fm1_cpu1_probe_run(1) };
        // SAFETY: CPU0 has the ordinary post-CRT context and exclusive peripherals.
        let mut board = unsafe { Board::from_reset() };
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
        line(8, "CPU1 flash checkpoint v2");
        line(208, "PLAY enters download mode");
        let mut negative = None;
        let mut positive = None;
        JOB.store(STRAY_BIT, Ordering::Release);
        let mut store = if boot != 1 {
            Err("CPU1 boot failed")
        } else if !wait_for(WITHHOLDING) {
            Err("CPU1 XIP worker not ready")
        } else {
            let rc = fm1_storage_check_peer();
            negative = Some(rc);
            if rc != NO_ACK {
                Err(gate_error(rc))
            } else {
                line(32, "Busy check passed");
                if send_job(ENABLE) {
                    let rc = fm1_storage_check_peer();
                    positive = Some(rc);
                    if rc == PARKED {
                        line(48, "SRAM park verified");
                        // SAFETY: fm1-storage.x reserves the sectors. CPU1 has fixed
                        // SRAM stacks and an SRAM exception vector. The only DMA
                        // client is the SRAM-backed LCD. CPU0 owns the peripherals.
                        unsafe { port.open(*b"DEMO") }.map_err(flash_error)
                    } else {
                        Err(gate_error(rc))
                    }
                } else {
                    Err("CPU1 did not take job")
                }
            }
        };
        let mut current = match &mut store {
            Ok(s) => variation(s),
            Err(msg) => Err(*msg),
        };
        line(
            68,
            match current {
                Ok(None) => "Patch slot blank",
                Ok(Some(0)) => "Patch A restored",
                Ok(Some(_)) => "Patch B restored",
                Err(e) => e,
            },
        );
        details(&mut line, boot, negative, positive);
        if current.is_ok() {
            line(127, "SAVE toggles demo patch");
        }
        let mut controls = board.panel.into_controls();
        loop {
            board.timers.delay_us(100);
            let Some(scan) = controls.scan(&Leds::default()) else {
                continue;
            };
            if scan.pressed(Button::Play) {
                download();
            }
            if !scan.pressed(Button::Save) {
                continue;
            }
            if let (Ok(s), Ok(old)) = (&mut store, current) {
                let next = old.map_or(0, |i| i ^ 1);
                let result = s.save(0, &PATCH[next]).map_err(flash_error).and_then(|()| {
                    if variation(s)? != Some(next) {
                        Err("Patch readback mismatch")
                    } else if !send_job(JOB.load(Ordering::Relaxed).wrapping_add(1)) {
                        Err("Saved - CPU1 stalled")
                    } else {
                        Ok(())
                    }
                });
                match result {
                    Ok(()) => {
                        line(
                            68,
                            if next == 0 {
                                "Patch A verified"
                            } else {
                                "Patch B verified"
                            },
                        );
                        line(102, "Saved - CPU1 resumed");
                        current = Ok(Some(next));
                    }
                    Err(e) => {
                        line(102, e);
                        details(&mut line, boot, negative, positive);
                        current = Err(e);
                    }
                }
            }
        }
    }

    fn flash_error(error: Error) -> &'static str {
        match error {
            Error::Busy => "CPU1 or controller busy",
            Error::Unsupported => "Unsupported flash or XIP map",
            Error::Occupied => "Foreign data preserved",
            Error::Io => "Flash I/O failed",
            _ => "Unreadable patch preserved",
        }
    }

    fn variation(store: &mut Store<'_>) -> Result<Option<usize>, &'static str> {
        let mut bytes = [0; 4];
        match store.load(0, &mut bytes) {
            Ok(None) => Ok(None),
            Ok(Some(4)) => PATCH
                .iter()
                .position(|v| v == &bytes)
                .map(Some)
                .ok_or("Unknown patch preserved"),
            Err(e) => Err(flash_error(e)),
            _ => Err("Unreadable patch preserved"),
        }
    }

    fn download() -> ! {
        if fm1_core_id() == 0 {
            // SAFETY: CPU0 is outside a transfer and owns the probe. Hold CPU1
            // and restore its exception vector before invoking the loader.
            unsafe { fm1_cpu1_probe_stop() };
            fm1::enter_download_mode();
        }
        loop {
            core::hint::spin_loop();
        }
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
        download()
    }
}
