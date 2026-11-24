//! A four-waveform oscillator and a selectable oscilloscope on the FM-1.
#![allow(
    missing_docs,
    reason = "RTICX generates public task traits and initialisers."
)]
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(any(target_os = "none", test))]
#[path = "common/pitch.rs"]
mod pitch;
#[cfg(any(target_os = "none", test))]
#[path = "rticx-waveform/render.rs"]
mod render;
#[cfg(any(target_os = "none", test))]
#[path = "common/timing.rs"]
mod timing;
#[cfg(any(target_os = "none", test))]
#[path = "rticx-waveform/voice.rs"]
mod voice;

#[cfg(target_os = "none")]
#[fm1_rticx::app(dispatchers = [SOFT0, SOFT1])]
mod app {
    use crate::{
        render::{PreparedBlock, RenderRequest},
        timing::{PanelReport, PanelTiming, TimingFooter},
        voice::{Capture, HEIGHT, Samples, ScopeBlock, Selection, Trace, Voice, WIDTH},
    };
    use fm1_audio::{MasterLevel, OUTPUT_FRAMES, Output, Refill, Stats};
    use fm1_graphics::{TextGrid, Waveform, palette};
    use fm1_rticx::bsp::Cpu0;
    use fm1_rticx::{
        bsp::{
            Board,
            display::{DisplayPort, SIZE, SpiClock, on_spi1_interrupt},
            enter_download_mode,
            panel::{Button, Controls, Encoder, Event, Leds, PlayLedState, Switch},
            time::{Timers, cycles, ticks},
        },
        rticx::asynchronous::{
            channel::{Receiver, Sender, TrySendError},
            make_channel,
        },
        time::{Clock, Timer5},
    };
    use static_cell::{ConstStaticCell, StaticCell};

    type RenderJob = (RenderRequest, &'static mut Samples);

    #[shared]
    struct Shared {
        settings: (Voice, Selection),
        frame: Frame,
        audio_report: AudioReport,
        prepared: Option<PreparedBlock<'static>>,
    }

    #[derive(Clone, Copy, Debug, Default)]
    pub struct AudioReport {
        stats: Stats,
        scope_drops: u32,
        fallbacks: u32,
        peak_hit_ticks: u32,
        peak_edit_ticks: u32,
    }

    impl AudioReport {
        fn record(
            &mut self,
            started: u32,
            refill_ended: u32,
            result: Refill,
            dropped: bool,
            hit: bool,
        ) -> bool {
            if !hit {
                self.fallbacks = self.fallbacks.saturating_add(1);
            } else if result == Refill::Written {
                self.peak_hit_ticks = self.peak_hit_ticks.max(refill_ended.wrapping_sub(started));
            }
            if dropped {
                self.scope_drops = self.scope_drops.saturating_add(1);
            }
            let overrun = self.stats.record_with_refill(started, refill_ended, result);
            // Refresh the counters even if the observer cannot finish a trace.
            let show = |count: u32| count == 1 || count.is_multiple_of(32);
            (overrun && show(self.stats.overruns))
                || (dropped && show(self.scope_drops))
                || (!hit && show(self.fallbacks))
        }
    }

    #[derive(Debug)]
    pub struct Frame {
        voice: Voice,
        trace: Trace,
        trace_start: Option<u32>,
        selection: Selection,
        panel: PanelReport,
        ready: Sender<'static, (), 1>,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let board = Board::take().expect("CPU0 board");
        let voice = Voice::default();
        static OSCILLATOR: ConstStaticCell<Samples> = ConstStaticCell::new([0; OUTPUT_FRAMES]);
        static SPARE: ConstStaticCell<Samples> = ConstStaticCell::new([0; OUTPUT_FRAMES]);
        let (renders, jobs) = make_channel!(RenderJob, 1);
        let (ready, updates) = make_channel!((), 1);
        let (tap, blocks) = make_channel!(ScopeBlock, 2);
        let timers = Clock::start(board.timers.with_panel_scan_rate());
        let delay = timers.clock();
        let mut leds = Leds::default();
        leds.set_play(PlayLedState::Green);
        (
            Shared {
                settings: (voice, Selection::default()),
                audio_report: AudioReport::default(),
                prepared: None,
                frame: Frame {
                    voice,
                    trace: [64; WIDTH],
                    trace_start: None,
                    selection: Selection::default(),
                    panel: PanelReport::default(),
                    ready: ready.clone(),
                },
            },
            TaskInits {
                audio: Audio {
                    output: Output::start(board.audio).expect("audio clock"),
                    next: RenderRequest::new(voice),
                    oscillator: OSCILLATOR.take(),
                    spare: Some(SPARE.take()),
                    renders,
                    applied: Selection::default(),
                    tap,
                    gap: false,
                    redraw: ready,
                },
                prepare: Prepare { jobs },
                observe: Observe {
                    blocks,
                    capture: Capture::new(),
                },
                panel: Panel {
                    timers,
                    controls: board.panel.into_controls().into(),
                    timing: PanelTiming::default(),
                    leds,
                    voice,
                    octave_since: None,
                    selection: Selection::default(),
                },
                draw: Draw {
                    port: Some(board.display.into()),
                    updates,
                    delay,
                },
            },
        )
    }

    #[post_init]
    fn start() {
        Prepare::spawn(()).expect("one oscillator producer");
        Observe::spawn(()).expect("one audio observer");
        Draw::spawn(()).expect("one scope renderer");
    }

    #[task(binds = TIMER5, priority = 3, shared = [settings, frame])]
    struct Panel {
        timers: Timer5,
        controls: Cpu0<Controls>,
        timing: PanelTiming,
        leds: Leds,
        voice: Voice,
        octave_since: Option<u32>,
        selection: Selection,
    }
    impl RticTask for Panel {
        fn exec(&mut self) {
            let started = cycles();
            let home = self.scan();
            if let Some(report) = self.timing.record(started, cycles(), home) {
                self.shared().frame.lock(|f| {
                    f.panel = report;
                    let _ = f.ready.try_send(());
                });
            }
        }
    }

    impl Panel {
        fn scan(&mut self) -> Option<bool> {
            self.timers.on_interrupt();
            let scan = self.controls.scan(&self.leds)?;
            let before = self.voice;
            let previous_selection = self.selection;
            for event in scan.events() {
                match event {
                    Event::Turned {
                        encoder: Encoder::Presets,
                        detents,
                    } => self.voice.wave = self.voice.wave.turn(detents),
                    Event::Turned {
                        encoder: Encoder::Select,
                        detents,
                    } => self.voice.transpose_by(detents),
                    Event::MasterChanged(Some(level)) => {
                        self.voice.velocity = (level.raw() >> 3) as u8
                    }
                    Event::Pressed(Switch::Piano(key)) => self.voice.select_key(key),
                    Event::Pressed(Switch::Button(Button::Fx)) => {
                        self.selection.half_gain = !self.selection.half_gain;
                    }
                    Event::Pressed(Switch::Button(Button::Sel)) => {
                        self.selection.oscillator_tap = !self.selection.oscillator_tap;
                    }
                    Event::Pressed(Switch::Button(Button::Play)) => {
                        self.voice.playing = !self.voice.playing;
                        self.leds.set_play(if self.voice.playing {
                            PlayLedState::Green
                        } else {
                            PlayLedState::Red
                        });
                    }
                    _ => {}
                }
            }
            if self.selection != previous_selection {
                self.selection.changed_at = cycles();
            }
            if self.voice != before || self.selection != previous_selection {
                let settings = (self.voice, self.selection);
                self.shared().settings.lock(|s| *s = settings);
            }
            if scan.is_down(Button::OctaveDown) && scan.is_down(Button::OctaveUp) {
                let now = ticks();
                if now.wrapping_sub(*self.octave_since.get_or_insert(now)) >= 2 * Timers::TICK_HZ {
                    enter_download_mode();
                }
            } else {
                self.octave_since = None;
            }
            Some(scan.is_down(Button::Home))
        }
    }

    #[async_task(priority = 2, shared = [prepared])]
    struct Prepare {
        jobs: Receiver<'static, RenderJob, 1>,
    }
    impl RticAsyncTask for Prepare {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {
            while let Ok((request, samples)) = self.jobs.recv().await {
                let block = request.prepare(samples);
                self.shared().prepared.lock(|slot| *slot = Some(block));
            }
        }
    }

    #[task(binds = ALNK0, priority = 4, shared = [settings, audio_report, prepared])]
    struct Audio {
        output: Output,
        next: RenderRequest,
        oscillator: &'static mut Samples,
        spare: Option<&'static mut Samples>,
        renders: Sender<'static, RenderJob, 1>,
        applied: Selection,
        tap: Sender<'static, ScopeBlock, 2>,
        gap: bool,
        redraw: Sender<'static, (), 1>,
    }
    impl RticTask for Audio {
        fn exec(&mut self) {
            let started = cycles();
            let mut shared = self.shared();
            let (voice, selection) = shared.settings.lock(|s| *s);
            self.next.voice = voice;
            let mut hit = false;
            let result = self.output.render(MasterLevel::MAX, |block| {
                let mut ready = shared.prepared.lock(Option::take);
                hit = self
                    .next
                    .render_and_advance(&mut self.oscillator, ready.as_mut());
                if let Some(ready) = ready {
                    self.spare = Some(ready.samples);
                }
                selection.write_stereo(self.oscillator, block);
            });
            let refill_ended = cycles();
            let dropped = match result {
                Refill::WouldBlock => return,
                Refill::Late => false,
                // Avoid even the sample copy when the observer is behind.
                Refill::Written if self.tap.is_full() => true,
                Refill::Written => {
                    let block = ScopeBlock::new(
                        self.output.block(),
                        self.oscillator,
                        voice,
                        selection,
                        refill_ended,
                        self.gap,
                    );
                    self.tap.try_send(block).is_err()
                }
            };
            self.gap = dropped || result == Refill::Late;
            // A job owns the spare buffer until the ISR consumes or rejects its prediction.
            if let Some(samples) = self.spare.take()
                && let Err(
                    TrySendError::Full((_, samples)) | TrySendError::NoReceiver((_, samples)),
                ) = self.renders.try_send((self.next, samples))
            {
                self.spare = Some(samples);
            }
            let notify = shared.audio_report.lock(|report| {
                if result == Refill::Written && self.applied != selection {
                    self.applied = selection;
                    report.peak_edit_ticks = report
                        .peak_edit_ticks
                        .max(refill_ended.wrapping_sub(selection.changed_at));
                }
                report.record(started, refill_ended, result, dropped, hit)
            });
            if notify {
                let _ = self.redraw.try_send(());
            }
        }
    }

    #[async_task(priority = 2, shared = [frame])]
    struct Observe {
        blocks: Receiver<'static, ScopeBlock, 2>,
        capture: Capture,
    }
    impl RticAsyncTask for Observe {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {
            while let Ok(block) = self.blocks.recv().await {
                if self.capture.push(&block) {
                    let trace = self.capture.trace;
                    let started_at = self.capture.started_at;
                    self.shared().frame.lock(|frame| {
                        frame.voice = block.voice;
                        frame.trace = trace;
                        frame.trace_start = Some(started_at);
                        frame.selection = block.selection;
                        let _ = frame.ready.try_send(());
                    });
                }
            }
        }
    }

    #[task(binds = SPI1, priority = 1, init = generated)]
    struct SpiDone;
    impl RticTask for SpiDone {
        fn exec(&mut self) {
            on_spi1_interrupt();
        }
    }

    #[async_task(priority = 1, shared = [frame, audio_report])]
    struct Draw {
        port: Option<Cpu0<DisplayPort>>,
        updates: Receiver<'static, (), 1>,
        delay: Clock,
    }
    impl RticAsyncTask for Draw {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {
            // Audio and panel interrupts can run throughout LCD wake-up and painting.
            let mut lcd = self
                .port
                .take()
                .expect("one LCD")
                .into_inner()
                .init_with_clock(SpiClock::Div1)
                .expect("LCD");
            // This task lives across DMA waits. Its cell grid has dedicated storage.
            static TEXT: StaticCell<TextGrid> = StaticCell::new();
            let text = TEXT.init_with(TextGrid::new);
            let mut footer = TimingFooter::default();
            let mut scope = Waveform::<WIDTH, HEIGHT>::new(8, 48).expect("scope fits the LCD");
            lcd.as_async(&mut self.delay)
                .expect("LCD")
                .fill_rect(0, 0, SIZE, SIZE, 0)
                .await
                .expect("LCD clear");
            let mut timing_page = false;
            let mut painted_trace = None;
            let mut last_paint = None;
            let mut peak_age = 0;
            let mut peak_paint_gap = 0;
            loop {
                self.updates.recv().await.expect("frame owns sender");
                let (voice, trace, trace_start, selection, panel) = self
                    .shared()
                    .frame
                    .lock(|f| (f.voice, f.trace, f.trace_start, f.selection, f.panel));
                let report = self.shared().audio_report.lock(|report| *report);
                let home = footer.draw(text, panel, &report.stats);
                if timing_page != home {
                    for row in 0..3 {
                        text.text(0, row, [b' '; TextGrid::COLUMNS], palette::MUTED);
                    }
                    timing_page = home;
                }
                if home {
                    text.text(0, 0, b"EDIT", palette::MUTED);
                    text.number(
                        4,
                        0,
                        5,
                        report
                            .peak_edit_ticks
                            .div_ceil(Timers::CRYSTAL_HZ / 1_000_000),
                        palette::WHITE,
                    );
                    text.text(9, 0, b" HIT", palette::MUTED);
                    text.number(
                        13,
                        0,
                        4,
                        report
                            .peak_hit_ticks
                            .div_ceil(Timers::CRYSTAL_HZ / 1_000_000),
                        palette::WHITE,
                    );
                    text.text(18, 0, b"US", palette::MUTED);
                    text.text(0, 1, b"DROP", palette::MUTED);
                    text.number(4, 1, 5, report.scope_drops, palette::WHITE);
                    text.text(9, 1, b" MISS", palette::MUTED);
                    text.number(14, 1, 6, report.fallbacks, palette::WHITE);
                    let ms = |ticks: u32| ticks.div_ceil(Timers::CRYSTAL_HZ / 1000);
                    text.text(0, 2, b"AGE", palette::MUTED);
                    text.number(3, 2, 5, ms(peak_age), palette::WHITE);
                    text.text(9, 2, b"GAP", palette::MUTED);
                    text.number(12, 2, 5, ms(peak_paint_gap), palette::WHITE);
                    text.text(18, 2, b"MS", palette::MUTED);
                } else {
                    text.text(0, 0, voice.wave.label(), palette::CYAN);
                    text.text(
                        16,
                        0,
                        if voice.playing { b"PLAY" } else { b"STOP" },
                        if voice.playing {
                            palette::GREEN
                        } else {
                            palette::AMBER
                        },
                    );
                    text.text(
                        0,
                        1,
                        if selection.half_gain {
                            b"FX HALF"
                        } else {
                            b"FX DRY "
                        },
                        palette::CYAN,
                    );
                    text.text(
                        10,
                        1,
                        if selection.oscillator_tap {
                            b"SEL OSC"
                        } else {
                            b"SEL DAC"
                        },
                        palette::CYAN,
                    );
                    text.text(0, 2, b"NOTE", palette::MUTED);
                    text.text(10, 2, b"VEL", palette::MUTED);
                    text.text(5, 2, &crate::pitch::note_name(voice.note()), palette::WHITE);
                    text.number(14, 2, 3, u32::from(voice.velocity), palette::WHITE);
                    text.text(0, 11, b"PRESETS  WAVEFORM", palette::MUTED);
                    text.text(0, 12, b"MASTER   VELOCITY", palette::MUTED);
                    text.text(0, 13, b"SELECT   TRANS", palette::MUTED);
                    text.signed_number(16, 13, 3, i32::from(voice.transpose), palette::WHITE);
                    text.text(0, 14, b"OCT-/+ 2S USB   HOME", palette::MUTED);
                }
                if !home && report.stats.overruns != 0 {
                    text.text(9, 0, b"LATE", palette::RED);
                }
                text.paint_async(&mut lcd, &mut self.delay)
                    .await
                    .expect("LCD labels");
                scope
                    .paint_async(&mut lcd, &trace, &mut self.delay)
                    .await
                    .expect("LCD trace");
                // Count completed paints of distinct captures. A footer redraw is not a new trace.
                if let Some(start) = trace_start
                    && painted_trace != Some(start)
                {
                    let now = cycles();
                    peak_age = peak_age.max(now.wrapping_sub(start));
                    if let Some(previous) = last_paint.replace(now) {
                        peak_paint_gap = peak_paint_gap.max(now.wrapping_sub(previous));
                    }
                    painted_trace = Some(start);
                }
            }
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
