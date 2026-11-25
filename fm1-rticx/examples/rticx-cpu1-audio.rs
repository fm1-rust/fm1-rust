//! CPU1 renders numbered PCM blocks. CPU0 mixes a local voice and feeds ALNK0.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]
#![allow(
    missing_docs,
    reason = "RTICX generates public task traits and initializers."
)]

#[cfg(any(target_os = "none", test))]
#[path = "rticx-cpu1-audio/protocol.rs"]
mod protocol;

#[cfg(target_os = "none")]
#[path = "rticx-cpu1-audio/dashboard.rs"]
mod dashboard;

#[cfg(target_os = "none")]
mod native {
    use fm1_audio::OutputBlock;

    // SAFETY: the archive implements this private ABI on pi32v2. Only CPU0
    // calls these functions. CPU1 can reach only its fixed SRAM loop and the
    // scalar sample function. Scalar mailbox operations use aligned atomic
    // words, fixed addresses and bounds independent of the peer's observations.
    #[link(name = "fm1_cpu1_probe", kind = "static")]
    unsafe extern "C" {
        fn fm1_cpu1_audio_clear();
        fn fm1_cpu1_probe_run(persistent: u32) -> u32;
        pub(super) safe fn fm1_cpu1_probe_healthy() -> u32;
        pub(super) safe fn fm1_cpu1_probe_stop();
        pub(super) safe fn fm1_cpu1_audio_submit(job: u32, gate: u32);
        pub(super) safe fn fm1_cpu1_audio_done() -> u32;
        pub(super) safe fn fm1_cpu1_audio_held() -> u32;
        pub(super) safe fn fm1_cpu1_audio_release(job: u32);
        pub(super) safe fn fm1_cpu1_audio_ticks() -> u32;
        fn fm1_cpu1_audio_copy(out: *mut [i32; 2]) -> u32;
    }

    /// # Safety
    /// Call once on CPU0 from RTICX init, before starting timers or enabling
    /// dispatch. The linked reset prefix must have held CPU1 before CRT init.
    pub(super) unsafe fn start() -> u32 {
        // SAFETY: the caller establishes exclusive startup access. Clear the
        // NOLOAD mailbox while held, before releasing the fixed peer entry.
        unsafe {
            fm1_cpu1_audio_clear();
            fm1_cpu1_probe_run(1)
        }
    }

    pub(super) fn copy(block: &mut OutputBlock) -> u32 {
        // SAFETY: the native loop writes exactly 128 consecutive pairs of i32
        // to this exclusively borrowed array and retains no pointer. The
        // compile-time frame-count check is in protocol.rs.
        unsafe { fm1_cpu1_audio_copy(block.as_mut_ptr()) }
    }

    pub(super) fn exit() -> ! {
        fm1_cpu1_probe_stop();
        fm1_rticx::bsp::enter_download_mode()
    }
}

#[cfg(target_os = "none")]
#[fm1_rticx::app(dispatchers = [SOFT0, SOFT1])]
mod app {
    use super::{
        dashboard::Dashboard,
        native,
        protocol::{Action, DELAYED, JOBS, Pipeline, Report, mix_and_check},
    };
    use fm1_audio::{OUTPUT_FRAMES, OutputBlock, Refill, Stats, Stream};
    use fm1_rticx::{
        bsp::{
            Board, Cpu0,
            display::{Display, SIZE, SpiClock},
            embedded_hal::digital::OutputPin,
            panel::{Play, PlayColor},
            time::{Timers, cycles},
            usb_midi::{Identity, MidiDevice, PacketBuffer, UsbLog, UsbStatus, on_usb0_interrupt},
        },
        time::{Clock, ExtU32, Timebase, Timer5},
    };

    #[shared]
    struct Shared {
        report: Report,
        painted: bool,
        usb_status: UsbStatus,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        // SAFETY: the explicitly linked probe reset prefix held CPU1 before
        // CRT initialized RAM. RTICX enters init once on CPU0 with IRQs off,
        // before Board::take starts TIMER4. Only this image owns the worker.
        let boot = unsafe { native::start() };
        let board = Board::take().expect("CPU0 board");
        let port = board.take_usb().expect("one USB port");
        // SAFETY: this bench image uses the normal MIDI RX layout on a
        // qualified FM-1 and host restricted to ordinary control / MIDI
        // traffic, including reconnect. The board confines stores to the
        // two 2 KiB areas and respects the documented ready/idle handoffs.
        let usb = unsafe { port.assume_midi_dma() }.into_midi(Identity {
            vendor_id: 0x1209,
            product_id: 0x0001,
            manufacturer: "fm1-rust",
            product: "FM-1 CPU1 Audio",
        });
        let timer = Clock::start(board.timers);
        let delay = timer.clock();
        let display = board
            .display
            .init_with_clock(SpiClock::Div1)
            .ok()
            .map(Cpu0::from);
        let mut report = Report::at_boot(boot);
        if display.is_none() && report.status == "RUN" {
            report.status = "LCD";
        }
        if report.status == "RUN" {
            native::fm1_cpu1_audio_submit(1, 0);
        }
        let output = board.audio.into_stream_with_frames().ok().map(Cpu0::from);
        if output.is_none() && report.status == "RUN" {
            report.status = "DAC";
        }
        (
            Shared {
                report,
                painted: false,
                usb_status: usb.status(),
            },
            TaskInits {
                tick: Tick(timer),
                audio: Audio {
                    output,
                    block: [[0; 2]; OUTPUT_FRAMES],
                    pipe: Pipeline::default(),
                    stats: Stats::default(),
                },
                usb: Usb {
                    device: usb.into(),
                    delay,
                },
                monitor: Monitor {
                    display,
                    panel: board.panel.into(),
                    dashboard: Dashboard::new(),
                },
            },
        )
    }

    #[post_init]
    fn start() {
        Usb::spawn(()).expect("one USB owner");
        Monitor::spawn(()).expect("one CPU0 monitor");
    }

    #[task(binds = TIMER5, priority = 3)]
    struct Tick(Timer5);
    impl RticTask for Tick {
        fn exec(&mut self) {
            self.0.on_interrupt();
        }
    }

    #[task(binds = ALNK0, priority = 4, shared = [report, painted])]
    struct Audio {
        output: Option<Cpu0<Stream<'static, OUTPUT_FRAMES>>>,
        block: OutputBlock,
        pipe: Pipeline,
        stats: Stats,
    }
    impl RticTask for Audio {
        fn exec(&mut self) {
            static SILENCE: OutputBlock = [[0; 2]; OUTPUT_FRAMES];
            let start = cycles();
            let mut report = self.shared().report.lock(|r| *r);
            let Some(output) = &mut self.output else {
                return;
            };
            if report.status != "RUN" {
                output.try_write(&SILENCE);
                return;
            }
            let job = self.pipe.job();
            let mut action = Ok(Action::Silence);
            let block = &mut self.block;
            let result = output.try_write_with(|| {
                action = if native::fm1_cpu1_probe_healthy() == 0 {
                    Err("CPU1")
                } else {
                    self.pipe
                        .try_take(native::fm1_cpu1_audio_done(), native::fm1_cpu1_audio_held())
                };
                if action == Ok(Action::Play) {
                    let checksum = native::copy(block);
                    if mix_and_check(job, block, checksum) {
                        return &*block;
                    }
                    action = Err("DATA");
                }
                &SILENCE
            });
            let committed = cycles();
            if result == Refill::WouldBlock {
                return;
            }
            match (result, action) {
                (_, Err(reason)) => report.status = reason,
                (Refill::Written, Ok(Action::Silence)) => {
                    self.pipe.silenced = true;
                    native::fm1_cpu1_audio_release(job);
                }
                (Refill::Written, Ok(action)) => {
                    if action == Action::Play {
                        self.pipe.accepted = self.pipe.accepted.saturating_add(1);
                        report.worker = report.worker.max(native::fm1_cpu1_audio_ticks());
                    } else {
                        self.pipe.discarded = true;
                    }
                    if job == JOBS {
                        report.status = if self.shared().painted.lock(|p| *p) {
                            "PASS"
                        } else {
                            "DRAW"
                        };
                    } else {
                        let next = self.pipe.job();
                        native::fm1_cpu1_audio_submit(next, if next == DELAYED { next } else { 0 });
                    }
                }
                _ => report.status = "DMA",
            }
            if self.stats.record_with_refill(start, committed, result)
                && matches!(report.status, "RUN" | "PASS")
            {
                report.status = "TIME";
            }
            report.accepted = self.pipe.accepted;
            report.silenced = self.pipe.silenced;
            report.discarded = self.pipe.discarded;
            report.commit = self.stats.peak_commit_ticks;
            report.gap = self.stats.peak_gap_ticks;
            self.shared().report.lock(|r| *r = report);
        }
    }

    #[task(binds = USB0, priority = 3, init = generated)]
    struct UsbInterrupt;
    impl RticTask for UsbInterrupt {
        fn exec(&mut self) {
            on_usb0_interrupt();
        }
    }

    #[async_task(priority = 2, shared = [report, usb_status])]
    struct Usb {
        device: Cpu0<MidiDevice>,
        delay: Clock,
    }
    impl RticAsyncTask for Usb {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {
            let mut log = UsbLog::new();
            let mut rx = PacketBuffer::default();
            loop {
                {
                    let activity = core::pin::pin!(self.device.wait_for_activity(&mut self.delay));
                    let _ = Clock::timeout_at(Clock::now() + 10.millis(), activity).await;
                }
                self.device.poll();
                let packets = self.device.receive(&mut rx);
                if packets.contains(&[7, 0xf0, 0x7d, 0xf7]) {
                    let report = self.shared().report.lock(|r| *r);
                    log.record(report.line().as_str())
                        .expect("one ASCII report");
                }
                log.service(&mut self.device, packets);
                let status = self.device.status();
                self.shared().usb_status.lock(|s| *s = status);
            }
        }
    }

    #[async_task(priority = 1, shared = [report, painted, usb_status])]
    struct Monitor {
        display: Option<Cpu0<Display>>,
        panel: Cpu0<Play>,
        dashboard: Dashboard,
    }
    impl RticAsyncTask for Monitor {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {
            let mut last = 0;
            let mut progress = cycles();
            let mut redraw = 0_u8;
            let mut stopped = false;
            let mut pressed = None;
            loop {
                let mut report = self.shared().report.lock(|r| *r);
                let now = cycles();
                if report.accepted != last {
                    last = report.accepted;
                    progress = now;
                } else if report.status == "RUN" && now.wrapping_sub(progress) >= Timers::CRYSTAL_HZ
                {
                    self.shared().report.lock(|r| {
                        if r.status == "RUN" && r.accepted == last {
                            r.status = "IRQ";
                        }
                        report = *r;
                    });
                }
                if report.status != "RUN" && !stopped {
                    native::fm1_cpu1_probe_stop();
                    stopped = true;
                }
                let mut led = self.panel.led();
                led.set_color(match report.status {
                    "RUN" => PlayColor::Amber,
                    "PASS" => PlayColor::Green,
                    _ => PlayColor::Amber,
                });
                let Ok(()) = led.set_state((report.status != "RUN" || redraw & 16 == 0).into());
                if self.panel.button().is_pressed() {
                    if now.wrapping_sub(*pressed.get_or_insert(now)) >= 2 * Timers::CRYSTAL_HZ {
                        native::exit();
                    }
                } else {
                    pressed = None;
                }
                let usb = self.shared().usb_status.lock(|s| *s);
                let before = self.shared().report.lock(|r| r.accepted);
                if let Some(lcd) = &mut self.display {
                    let result = if report.status != "RUN" {
                        self.dashboard.paint(lcd, report, usb)
                    } else if redraw.is_multiple_of(8) {
                        let color = if redraw & 8 == 0 { 0x0010 } else { 0x0210 };
                        let result = lcd.fill_rect(0, 0, SIZE, SIZE, color);
                        if result.is_ok() && self.shared().report.lock(|r| r.accepted) > before {
                            self.shared().painted.lock(|p| *p = true);
                        }
                        result
                    } else {
                        Ok(())
                    };
                    if result.is_err() {
                        self.shared().report.lock(|r| {
                            if matches!(r.status, "RUN" | "PASS") {
                                r.status = "LCD";
                            }
                        });
                        self.display = None;
                    }
                }
                redraw = redraw.wrapping_add(1);
                Clock::delay(10.millis()).await;
            }
        }
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    native::exit()
}

#[cfg(not(target_os = "none"))]
fn main() {}
