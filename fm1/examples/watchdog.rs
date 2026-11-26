//! Test normal reboot and watchdog recovery from a stalled CPU1 on the FM-1.
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
                primitives::Rectangle,
                text::{Baseline, Text},
            },
        },
        embedded_hal::delay::DelayNs,
        panel::{Button, Leds},
        power::{Power, WatchdogTimeout},
        time::{Timers, cycles},
    };

    #[link(name = "fm1_cpu1_probe", kind = "static")]
    unsafe extern "C" {
        fn fm1_cpu1_probe_run(persistent: u32) -> u32;
        fn fm1_cpu1_audio_ready(reply: u32);
        fn fm1_cpu1_probe_stop();
    }

    static JOB: AtomicU32 = AtomicU32::new(0);
    static DONE: AtomicU32 = AtomicU32::new(0);
    const STALL: u32 = u32::MAX;

    // SAFETY: the probe calls this fixed SRAM entry once, on CPU1's private stack.
    #[unsafe(no_mangle)]
    #[unsafe(link_section = ".cpu1.text.watchdog")]
    extern "C" fn fm1_cpu1_probe_word(challenge: u32) -> ! {
        // SAFETY: CPU1 alone writes the probe's boot acknowledgement.
        unsafe { fm1_cpu1_audio_ready(challenge.rotate_left(7) ^ 0x000a_c791) };
        loop {
            let job = JOB.load(Ordering::Acquire);
            if job != DONE.load(Ordering::Relaxed) {
                DONE.store(job, Ordering::Release);
                if job == STALL {
                    loop {
                        core::hint::spin_loop();
                    }
                }
            }
        }
    }

    fn run_job(job: u32) -> Result<(), &'static str> {
        JOB.store(job, Ordering::Release);
        let start = cycles();
        while DONE.load(Ordering::Acquire) != job {
            if cycles().wrapping_sub(start) >= Timers::CRYSTAL_HZ / 10 {
                return Err("CPU1 did not reply");
            }
        }
        Ok(())
    }

    fn exercise(
        power: &mut Power,
        timers: &mut Timers,
        line: &mut impl FnMut(i32, &str),
    ) -> Result<(), &'static str> {
        line(128, "Feeding after CPU1 replies");
        power
            .arm_watchdog(WatchdogTimeout::Sec1)
            .map_err(|_| "Arm failed")?;
        // Keep both cores working for longer than the watchdog period.
        for job in 1..=8 {
            timers.delay_ms(200);
            run_job(job)?;
            power.feed_watchdog().map_err(|_| "Feed failed")?;
        }
        run_job(STALL)?;
        JOB.store(9, Ordering::Release); // This job will never complete.
        line(128, "CPU1 stalled");
        timers.delay_ms(250);
        line(148, "CPU0 alive, awaiting reset");
        timers.delay_ms(2000);
        Err("Watchdog did not reset")
    }

    fn hex(digits: &mut [u8], mut value: u32) {
        for digit in digits.iter_mut().rev() {
            *digit = b"0123456789ABCDEF"[(value & 15) as usize];
            value >>= 4;
        }
    }

    // SAFETY: crt0 calls the only C main on CPU0 after setting up RAM and stacks.
    #[unsafe(export_name = "main")]
    extern "C" fn start() -> ! {
        // SAFETY: the probe reset entry held CPU1 before RAM setup. IRQs remain
        // masked. The peer owns its stack and only shares the two atomic words.
        let boot = unsafe { fm1_cpu1_probe_run(1) };
        // SAFETY: CPU0 still exclusively owns the board peripherals and startup.
        let mut board = unsafe { Board::from_reset() };
        let mut power = board.take_power().expect("P33");
        let cause = power.reset_cause();
        let mut lcd = board
            .display
            .init_mipidsi(SpiClock::Div1, &mut board.timers)
            .expect("LCD");
        lcd.clear(Rgb565::BLACK).expect("clear LCD");
        let mut line = |y, msg: &str| {
            let origin = Point::new(0, y);
            lcd.fill_solid(&Rectangle::new(origin, Size::new(240, 15)), Rgb565::BLACK)
                .expect("row");
            Text::with_baseline(
                msg,
                origin,
                MonoTextStyle::new(&FONT_8X13, Rgb565::WHITE),
                Baseline::Top,
            )
            .draw(&mut lcd)
            .expect("text");
        };
        line(4, "FM-1 WATCHDOG");
        line(
            26,
            match (cause.watchdog(), cause.software()) {
                (Some(true), Some(true)) => "WATCHDOG + SOFTWARE FLAGS",
                (Some(true), _) => "WATCHDOG RESET",
                (_, Some(true)) => "SOFTWARE RESET",
                (None, _) => "P33 RESET FLAGS UNAVAILABLE",
                _ if cause.power_on() == Some(true) => "POWER-ON RESET",
                _ => "BOOT RESET FLAGS",
            },
        );
        let mut raw = *b"P33 --   SYS 00000000";
        if let Some(flags) = cause.p33 {
            hex(&mut raw[4..6], u32::from(flags));
        }
        hex(&mut raw[13..], cause.system);
        line(44, core::str::from_utf8(&raw).expect("ASCII"));
        line(72, "SAVE   Normal reboot");
        line(
            88,
            if boot == 1 {
                "REC    Stall CPU1"
            } else {
                "CPU1 startup failed"
            },
        );
        line(104, "PLAY   Download mode");
        let mut controls = board.panel.into_controls();
        loop {
            board.timers.delay_us(100);
            let Some(scan) = controls.scan(&Leds::default()) else {
                continue;
            };
            if scan.released(Button::Play) {
                // SAFETY: CPU0 owns the terminal shutdown of this probe.
                unsafe { fm1_cpu1_probe_stop() };
                fm1::enter_download_mode();
            }
            if scan.released(Button::Save) {
                power.reboot();
            }
            if scan.released(Button::Rec)
                && boot == 1
                && let Err(reason) = exercise(&mut power, &mut board.timers, &mut line)
            {
                line(128, reason);
                if power.disarm_watchdog().is_err() {
                    line(148, "Disarm failed");
                }
            }
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
