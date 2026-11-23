//! FM-1 physical timing diagnostic: audio, panel, input dispatch, LCD and CPU load.
//! Read the frozen, paged report using `fm1 timing` over USB-MIDI.
#![allow(
    missing_docs,
    reason = "RTICX generates public task traits and initialisers."
)]
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(any(target_os = "none", test))]
#[path = "rticx-timing/measure.rs"]
mod measure;
#[cfg(all(target_os = "none", feature = "usb-dma-probe"))]
#[path = "rticx-timing/probe_screen.rs"]
mod probe_screen;

#[cfg(target_os = "none")]
#[fm1_rticx::app(dispatchers = [SOFT0, SOFT1])]
mod app {
    use crate::measure::{
        AudioMetrics, InputMetrics, PAGE_COUNT, PHASE_TICKS, PanelMetrics, Report, WakeMetrics,
        WorkMetrics, micros,
    };
    use core::sync::atomic::{AtomicBool, AtomicU8, Ordering::Relaxed};
    use fm1_audio::{MasterLevel, Output};
    use fm1_controls::{ScanQueue, ScanReceiver, ScanSender};
    use fm1_graphics::{TextGrid, palette};
    use fm1_rticx::{
        bsp::{
            Board,
            display::{DisplayPort, SIZE, SpiClock},
            enter_download_mode,
            panel::{Button, Controls, Leds},
            time::{Timers, cycles, ticks},
            usb_midi::{Identity, MidiDevice, PacketBuffer, UsbLog, on_usb0_interrupt},
        },
        time::{Clock, DeadlineClock, DeadlineDuration, ExtU32, Timebase, Timer5},
    };
    use static_cell::ConstStaticCell;

    // 0: setup/between phases; 1: idle LCD; 2: LCD redraw; 3: low-priority CPU load.
    // ISR-local counters are published once, only on a phase transition.
    static ACTIVE: AtomicU8 = AtomicU8::new(0);
    static FINISHED: AtomicBool = AtomicBool::new(false);
    const SCANS: usize = 8;
    #[cfg(feature = "usb-dma-probe")]
    static DASHBOARD: critical_section::Mutex<
        core::cell::Cell<Option<fm1_rticx::bsp::usb_midi::ProbeDisplay>>,
    > = critical_section::Mutex::new(core::cell::Cell::new(None));
    #[cfg(feature = "usb-dma-probe")]
    static LISTEN: AtomicBool = AtomicBool::new(false);

    #[shared]
    struct Shared {
        audio: [AudioMetrics; 3],
        panel: [PanelMetrics; 3],
        input: [InputMetrics; 3],
        wake: [WakeMetrics; 3],
        lcd: WorkMetrics,
        cpu: WorkMetrics,
        usb: WorkMetrics,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        static QUEUE: ConstStaticCell<ScanQueue<SCANS>> = ConstStaticCell::new(ScanQueue::new());
        let (send, recv) = QUEUE.take().split();
        let mut board = Board::take().expect("CPU0 board");
        let port = board.take_usb().expect("one USB device");
        let id = Identity {
            vendor_id: 0x1209,
            product_id: 0x0001, // pid.codes shared development identity
            manufacturer: "fm1-rust",
            product: "FM-1 Timing",
        };
        // SAFETY: run on the controlled bench, including on reconnect. The OS
        // enumerates EP0 and packetizes normal MIDI. Only the selected test
        // runner sends diagnostic EP1 traffic after ARM. RXS1's schedule sends
        // at most 4096 payload bytes per acquisition into an 8 KiB arena.
        // The board must confine each RX store and respect CPU handoffs,
        // including the three-millisecond drain on diagnostics. ARM records intent.
        // the operator controls the traffic that actually reaches the cable.
        let usb = unsafe {
            #[cfg(not(feature = "usb-dma-probe"))]
            {
                port.into_midi(id)
            }
            #[cfg(feature = "usb-dma-probe")]
            {
                use fm1_rticx::bsp::usb_midi::ProbeProfile;
                match option_env!("FM1_DMA_PROFILE") {
                    None => port.into_midi(id),
                    Some("midi") => port.into_dma_probe(id, ProbeProfile::Midi),
                    Some("bulk") => port.into_dma_probe(
                        Identity {
                            product: "FM-1 DMA Bulk",
                            ..id
                        },
                        ProbeProfile::Bulk,
                    ),
                    #[cfg(feature = "usb-dma-stress")]
                    Some("stress") => port.into_dma_probe(
                        Identity {
                            product: "FM-1 DMA Stress",
                            ..id
                        },
                        ProbeProfile::Stress,
                    ),
                    Some("iso") => port.into_dma_probe(
                        Identity {
                            product: "FM-1 DMA Iso",
                            ..id
                        },
                        ProbeProfile::Iso,
                    ),
                    Some(_) => unreachable!("unknown DMA probe profile"),
                }
            }
        };
        DeadlineClock::start::<3>(board.timers.take_alarm().expect("TIMER2 alarm"));
        let timer = Clock::start(board.timers.with_panel_scan_rate());
        let delay = timer.clock();
        (
            Shared {
                audio: [AudioMetrics::default(); 3],
                panel: [PanelMetrics::default(); 3],
                input: [InputMetrics::default(); 3],
                wake: [WakeMetrics::default(); 3],
                lcd: WorkMetrics::default(),
                cpu: WorkMetrics::default(),
                usb: WorkMetrics::default(),
            },
            TaskInits {
                audio_task: AudioTask {
                    output: Output::start(board.audio).expect("audio clock"),
                    phase: 0,
                    metrics: AudioMetrics::default(),
                },
                panel: Panel {
                    timer,
                    controls: board.panel.into_controls(),
                    leds: Leds::default(),
                    sender: send,
                    phase: 0,
                    metrics: PanelMetrics::default(),
                    octave_since: None,
                },
                input_task: InputTask {
                    receiver: recv,
                    phase: 0,
                    metrics: InputMetrics::default(),
                },
                wake_probe: WakeProbe {
                    phase: 0,
                    metrics: WakeMetrics::default(),
                },
                usb_task: UsbTask {
                    device: usb,
                    delay,
                    log: UsbLog::new(),
                    page: 0,
                    phase: 0,
                    metrics: WorkMetrics::default(),
                },
                screen: Screen {
                    port: Some(board.display),
                },
            },
        )
    }

    #[post_init]
    fn start() {
        InputTask::spawn(()).expect("one scan consumer");
        WakeProbe::spawn(()).expect("one deadline probe");
        UsbTask::spawn(()).expect("one USB owner");
        Screen::spawn(()).expect("one benchmark runner");
    }

    #[task(binds = ALNK0, priority = 5, shared = [audio])]
    struct AudioTask {
        output: Output,
        phase: u8,
        metrics: AudioMetrics,
    }
    impl RticTask for AudioTask {
        fn exec(&mut self) {
            let stage = ACTIVE.load(Relaxed);
            if self.phase != stage {
                if (1..=3).contains(&self.phase) {
                    let result = self.metrics;
                    self.shared()
                        .audio
                        .lock(|snapshots| snapshots[usize::from(self.phase - 1)] = result);
                }
                self.metrics = AudioMetrics::default();
                self.phase = stage;
            }
            let started = cycles();
            // Fixed workload: refill + stereo Q23 copy, not a WaveGraph DSP benchmark.
            let result = self.output.render(MasterLevel::MIN, |_| {});
            let copied = cycles();
            if stage != 0 {
                self.metrics.record(started, copied, cycles(), result);
            }
        }
    }

    #[task(binds = TIMER5, priority = 4, shared = [panel])]
    struct Panel {
        timer: Timer5,
        controls: Controls,
        leds: Leds,
        sender: ScanSender<'static, SCANS>,
        phase: u8,
        metrics: PanelMetrics,
        octave_since: Option<u32>,
    }
    impl RticTask for Panel {
        fn exec(&mut self) {
            let stage = ACTIVE.load(Relaxed);
            if self.phase != stage {
                if (1..=3).contains(&self.phase) {
                    let result = self.metrics;
                    self.shared()
                        .panel
                        .lock(|snapshots| snapshots[usize::from(self.phase - 1)] = result);
                }
                self.metrics = PanelMetrics::default();
                self.phase = stage;
            }
            let started = cycles();
            self.timer.on_interrupt();
            if let Some(scan) = self.controls.scan(&self.leds) {
                let captured = cycles();
                #[cfg(feature = "usb-dma-probe")]
                if option_env!("FM1_DMA_PROFILE") == Some("midi") && scan.pressed(Button::Play) {
                    LISTEN.store(true, Relaxed);
                }
                if scan.is_down(Button::OctaveDown) && scan.is_down(Button::OctaveUp) {
                    let now = ticks();
                    if now.wrapping_sub(*self.octave_since.get_or_insert(now))
                        >= 2 * Timers::TICK_HZ
                    {
                        enter_download_mode();
                    }
                } else {
                    self.octave_since = None;
                }
                if self.sender.try_send(scan, captured).is_err() && stage != 0 {
                    self.metrics.dropped = self.metrics.dropped.saturating_add(1);
                }
            }
            if stage != 0 {
                self.metrics.record(started, cycles());
            }
        }
    }

    #[async_task(priority = 2, shared = [input])]
    struct InputTask {
        receiver: ScanReceiver<'static, SCANS>,
        phase: u8,
        metrics: InputMetrics,
    }
    impl RticAsyncTask for InputTask {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {
            while let Ok(scan) = self.receiver.recv().await {
                let received_at = cycles();
                let stage = ACTIVE.load(Relaxed);
                if self.phase != stage {
                    if (1..=3).contains(&self.phase) {
                        let result = self.metrics;
                        self.shared()
                            .input
                            .lock(|snapshots| snapshots[usize::from(self.phase - 1)] = result);
                    }
                    self.metrics = InputMetrics::default();
                    self.phase = stage;
                }
                if stage != 0 {
                    self.metrics.record(scan.captured_at, received_at);
                }
            }
        }
    }

    // A separate priority-2 task measures TIMER2 wakeup-to-dispatch lateness.
    // It is intentionally not measured in the high-priority TIMER2 ISR.
    #[async_task(priority = 2, shared = [wake])]
    struct WakeProbe {
        phase: u8,
        metrics: WakeMetrics,
    }
    impl RticAsyncTask for WakeProbe {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {
            let mut next = DeadlineClock::now();
            loop {
                next += DeadlineDuration::from_millis(5);
                DeadlineClock::delay_until(next).await;
                let woke = cycles();
                let stage = ACTIVE.load(Relaxed);
                if self.phase != stage {
                    if (1..=3).contains(&self.phase) {
                        let result = self.metrics;
                        self.shared()
                            .wake
                            .lock(|snapshots| snapshots[usize::from(self.phase - 1)] = result);
                    }
                    self.metrics = WakeMetrics::default();
                    self.phase = stage;
                }
                if stage != 0 {
                    self.metrics.record(next.as_ticks(), woke);
                }
            }
        }
    }

    #[task(binds = USB0, priority = 3, init = generated)]
    struct UsbInterrupt;
    impl RticTask for UsbInterrupt {
        fn exec(&mut self) {
            on_usb0_interrupt();
        }
    }

    #[async_task(priority = 2, shared = [audio, panel, input, wake, lcd, cpu, usb])]
    struct UsbTask {
        device: MidiDevice,
        delay: Clock,
        log: UsbLog,
        page: u8,
        phase: u8,
        metrics: WorkMetrics,
    }
    impl RticAsyncTask for UsbTask {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {
            let mut rx = PacketBuffer::default();
            loop {
                // The USB0 interrupt is the fast path. A 10 ms timer fallback
                // also samples frame progress and makes idle USB cost measurable.
                {
                    let activity = core::pin::pin!(self.device.wait_for_activity(&mut self.delay));
                    let until = DeadlineClock::now() + DeadlineDuration::from_millis(10);
                    let _ = DeadlineClock::timeout_at(until, activity).await;
                }
                let stage = ACTIVE.load(Relaxed);
                if self.phase != stage {
                    if (1..=3).contains(&self.phase) {
                        let result = self.metrics;
                        self.shared().usb.lock(|snapshots| *snapshots = result);
                    }
                    // Keep an aggregate USB service cost across all three phases.
                    self.phase = stage;
                }
                let started = cycles();
                self.device.poll();
                #[cfg(feature = "usb-dma-probe")]
                if option_env!("FM1_DMA_PROFILE").is_some() {
                    if LISTEN.swap(false, Relaxed) {
                        self.device.probe_listen();
                    }
                    let display = self.device.probe_display();
                    critical_section::with(|cs| DASHBOARD.borrow(cs).set(display));
                }
                let packets = self.device.receive(&mut rx);
                for packet in packets {
                    // MIDI channel-1 CC 112 selects the 0..24 diagnostic page.
                    if packet[0] == 0x0b
                        && packet[1] == 0xb0
                        && packet[2] == 112
                        && packet[3] < PAGE_COUNT
                    {
                        self.page = packet[3];
                    }
                }
                if packets.contains(&[0x07, 0xf0, 0x7d, 0xf7]) {
                    self.log = UsbLog::new();
                    if cfg!(feature = "usb-dma-probe") && option_env!("FM1_DMA_PROFILE").is_some() {
                        self.log
                            .record("RXD3 MIDI READY")
                            .expect("short ASCII marker");
                        #[cfg(feature = "usb-dma-probe")]
                        if let Some(d) = self.device.probe_display()
                            && !d.active
                            && let Some(r) = d.reading
                        {
                            use core::fmt::Write;
                            let mut line = crate::measure::Line::new();
                            if let Some(g) = r.first_guard {
                                let _ = write!(
                                    line,
                                    "#{} HI{} G{}",
                                    r.token,
                                    r.highest.unwrap_or(g),
                                    g
                                );
                            } else if let Some(h) = r.highest {
                                let _ = write!(line, "#{} HI{} G-", r.token, h);
                            } else {
                                let _ = write!(line, "#{} HI- G-", r.token);
                            }
                            self.log
                                .record(line.as_str())
                                .expect("short offset summary");
                        }
                    } else {
                        let report = Report {
                            audio: self.shared().audio.lock(|v| *v),
                            panel: self.shared().panel.lock(|v| *v),
                            input: self.shared().input.lock(|v| *v),
                            wake: self.shared().wake.lock(|v| *v),
                            lcd: self.shared().lcd.lock(|v| *v),
                            cpu: self.shared().cpu.lock(|v| *v),
                            usb: self.shared().usb.lock(|v| *v),
                        };
                        let line = report.page(self.page, FINISHED.load(Relaxed));
                        self.log.record(line.as_str()).expect("short ASCII page");
                    }
                }
                self.log.service(&mut self.device, packets);
                if stage != 0 {
                    self.metrics.record(cycles().wrapping_sub(started));
                }
            }
        }
    }

    fn heading(text: &mut TextGrid, name: &[u8]) {
        text.text(0, 0, b"FM1 TIMING TEST", palette::CYAN);
        text.text(0, 1, b"24MHZ 128F 10KHZ", palette::MUTED);
        text.text(0, 2, b"PHASE", palette::WHITE);
        text.text(6, 2, b"              ", palette::WHITE);
        text.text(6, 2, name, palette::GREEN);
        text.text(0, 3, b"5 SEC PER STAGE", palette::MUTED);
        text.text(0, 4, b"AUDIO PEAK US", palette::WHITE);
        text.text(0, 5, b"AUDIO GAP US", palette::WHITE);
        text.text(0, 6, b"AUDIO OVERRUN", palette::WHITE);
        text.text(0, 7, b"PANEL PEAK US", palette::WHITE);
        text.text(0, 8, b"PANEL JIT US", palette::WHITE);
        text.text(0, 9, b"INPUT LAT US", palette::WHITE);
        text.text(0, 10, b"SCAN DROPS", palette::WHITE);
        text.text(0, 11, b"LCD MAX US", palette::WHITE);
        text.text(0, 12, b"USB FOR FULL REPORT", palette::AMBER);
        text.text(0, 13, b"RUN READ TIMING PY", palette::AMBER);
        text.text(0, 14, b"OCT-/+ 2S TO FLASH", palette::MUTED);
    }

    fn summary(
        text: &mut TextGrid,
        a: AudioMetrics,
        p: PanelMetrics,
        i: InputMetrics,
        lcd: WorkMetrics,
    ) {
        text.number(15, 4, 5, micros(a.max_work), palette::CYAN);
        text.number(15, 5, 5, micros(a.max_gap), palette::CYAN);
        text.number(
            15,
            6,
            5,
            a.overruns,
            if a.overruns == 0 {
                palette::GREEN
            } else {
                palette::RED
            },
        );
        text.number(15, 7, 5, micros(p.max_work), palette::CYAN);
        text.number(15, 8, 5, micros(p.max_jitter), palette::CYAN);
        text.number(15, 9, 5, micros(i.max_dispatch), palette::CYAN);
        text.number(
            15,
            10,
            5,
            p.dropped,
            if p.dropped == 0 {
                palette::GREEN
            } else {
                palette::RED
            },
        );
        text.number(15, 11, 5, micros(lcd.max_ticks), palette::CYAN);
    }

    #[async_task(priority = 1, shared = [audio, panel, input, lcd, cpu])]
    struct Screen {
        port: Option<DisplayPort>,
    }
    impl RticAsyncTask for Screen {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {
            let Some(port) = self.port.take() else { return };
            let Ok(mut lcd) = port.init_with_clock(SpiClock::Div1) else {
                return;
            };
            let mut text = TextGrid::new();
            if lcd.fill_rect(0, 0, SIZE, SIZE, 0).is_err() {
                return;
            }
            #[cfg(feature = "usb-dma-probe")]
            if let Some(profile) = option_env!("FM1_DMA_PROFILE") {
                let mut previous = None;
                loop {
                    let state =
                        critical_section::with(|cs| DASHBOARD.borrow(cs).get()).unwrap_or_default();
                    if previous != Some(state) {
                        crate::probe_screen::draw(&mut text, profile, state);
                        if text.paint(&mut lcd).is_err() {
                            return;
                        }
                        previous = Some(state);
                    }
                    Clock::delay(100.millis()).await;
                }
            }
            heading(&mut text, b"WARMUP");
            if text.paint(&mut lcd).is_err() {
                return;
            }
            Clock::delay(1.secs()).await; // allow audio and USB attachment to settle

            let mut draw = WorkMetrics::default();
            let mut cpu = WorkMetrics::default();
            for stage in 1..=3u8 {
                let name: &[u8] = match stage {
                    1 => b"IDLE LCD",
                    2 => b"LCD REDRAW",
                    _ => b"CPU LOAD",
                };
                heading(&mut text, name);
                if text.paint(&mut lcd).is_err() {
                    return;
                }
                Clock::delay(50.millis()).await; // exclude page-transition paint from timings
                ACTIVE.store(stage, Relaxed);
                let begun = cycles();
                match stage {
                    1 => Clock::delay(5.secs()).await,
                    2 => {
                        let mut next = Clock::now();
                        let mut toggle = false;
                        while cycles().wrapping_sub(begun) < PHASE_TICKS {
                            toggle = !toggle;
                            text.text(
                                0,
                                3,
                                if toggle {
                                    b"LCD STRESS A   "
                                } else {
                                    b"LCD STRESS B   "
                                },
                                palette::AMBER,
                            );
                            text.invalidate(); // 300 cells, full-screen SPI1 DMA stress
                            let start = cycles();
                            if text.paint(&mut lcd).is_err() {
                                return;
                            }
                            draw.record(cycles().wrapping_sub(start));
                            next += 100.millis();
                            Clock::delay_until(next).await;
                        }
                    }
                    _ => {
                        let mut next = DeadlineClock::now();
                        while cycles().wrapping_sub(begun) < PHASE_TICKS {
                            next += DeadlineDuration::from_millis(10);
                            let start = cycles();
                            // 7 ms of preemptible work in each 10 ms slot. Elapsed
                            // time includes interrupt preemption; it is not CPU cycles.
                            while cycles().wrapping_sub(start) < 7 * 24_000 {
                                core::hint::spin_loop();
                            }
                            cpu.record(cycles().wrapping_sub(start));
                            DeadlineClock::delay_until(next).await;
                        }
                    }
                }
                ACTIVE.store(0, Relaxed);
                Clock::delay(50.millis()).await; // let the ISR-local reports publish
                let index = usize::from(stage - 1);
                let a = self.shared().audio.lock(|v| v[index]);
                let p = self.shared().panel.lock(|v| v[index]);
                let i = self.shared().input.lock(|v| v[index]);
                self.shared().lcd.lock(|v| *v = draw);
                self.shared().cpu.lock(|v| *v = cpu);
                heading(&mut text, name);
                summary(&mut text, a, p, i, draw);
                if text.paint(&mut lcd).is_err() {
                    return;
                }
            }
            heading(&mut text, b"DONE - USB REPORT");
            text.text(0, 3, b"RESULTS FROM CPU", palette::GREEN);
            if text.paint(&mut lcd).is_err() {
                return;
            }
            FINISHED.store(true, Relaxed);
        }
    }
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    fm1_rticx::bsp::enter_download_mode()
}

#[cfg(not(target_os = "none"))]
fn main() {}
