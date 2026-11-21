//! A playable oscillator and an oscilloscope of its DAC samples.
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
#[path = "common/timing.rs"]
mod timing;
#[cfg(any(target_os = "none", test))]
#[path = "rticx-waveform/voice.rs"]
mod voice;

#[cfg(target_os = "none")]
#[fm1_rticx::app(dispatchers = [SOFT0, SOFT1])]
mod app {
    use crate::{
        timing::{PanelReport, PanelTiming, TimingFooter},
        voice::{Capture, HEIGHT, ScopeBlock, Trace, Voice, WIDTH},
    };
    use fm1_audio::{MasterLevel, Output, Refill, Stats};
    use fm1_graphics::{TextGrid, Waveform, palette};
    use fm1_rticx::{
        bsp::{
            Board,
            display::{DisplayPort, SIZE, SpiClock, on_spi1_interrupt},
            enter_download_mode,
            panel::{Button, Controls, Encoder, Event, Leds, PlayLedState, Switch},
            time::{Timers, cycles, ticks},
        },
        rticx::asynchronous::{
            channel::{Receiver, Sender},
            make_channel,
        },
        time::{Clock, Timer5},
    };

    #[shared]
    struct Shared {
        voice: Voice,
        frame: Frame,
        audio_report: AudioReport,
    }

    #[derive(Clone, Copy, Debug, Default)]
    pub struct AudioReport {
        stats: Stats,
        scope_drops: u32,
    }

    impl AudioReport {
        fn record(
            &mut self,
            started: u32,
            refill_ended: u32,
            result: Refill,
            dropped: bool,
        ) -> bool {
            if dropped {
                self.scope_drops = self.scope_drops.saturating_add(1);
            }
            let overrun = self.stats.record_with_refill(started, refill_ended, result);
            // Refresh the counters even if the observer cannot finish a trace.
            let show = |count: u32| count == 1 || count.is_multiple_of(32);
            (overrun && show(self.stats.overruns)) || (dropped && show(self.scope_drops))
        }
    }

    #[derive(Debug)]
    pub struct Frame {
        voice: Voice,
        trace: Trace,
        panel: PanelReport,
        ready: Sender<'static, (), 1>,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let board = Board::take().expect("CPU0 board");
        let voice = Voice::default();
        let (ready, updates) = make_channel!((), 1);
        let (tap, blocks) = make_channel!(ScopeBlock, 2);
        let timers = Clock::start(board.timers.with_panel_scan_rate());
        let delay = timers.clock();
        let mut leds = Leds::default();
        leds.set_play(PlayLedState::Green);
        (
            Shared {
                voice,
                audio_report: AudioReport::default(),
                frame: Frame {
                    voice,
                    trace: [64; WIDTH],
                    panel: PanelReport::default(),
                    ready: ready.clone(),
                },
            },
            TaskInits {
                audio: Audio {
                    output: Output::start(board.audio).expect("audio clock"),
                    phase: 0,
                    tap,
                    gap: false,
                    redraw: ready,
                },
                observe: Observe {
                    blocks,
                    capture: Capture::new(),
                },
                panel: Panel {
                    timers,
                    controls: board.panel.into_controls(),
                    timing: PanelTiming::default(),
                    leds,
                    voice,
                    octave_since: None,
                },
                draw: Draw {
                    port: Some(board.display),
                    updates,
                    delay,
                },
            },
        )
    }

    #[post_init]
    fn start() {
        Observe::spawn(()).expect("one audio observer");
        Draw::spawn(()).expect("one scope renderer");
    }

    #[task(binds = TIMER5, priority = 3, shared = [voice, frame])]
    struct Panel {
        timers: Timer5,
        controls: Controls,
        timing: PanelTiming,
        leds: Leds,
        voice: Voice,
        octave_since: Option<u32>,
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
            if self.voice != before {
                let next = self.voice;
                self.shared().voice.lock(|v| *v = next);
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

    #[task(binds = ALNK0, priority = 4, shared = [voice, audio_report])]
    struct Audio {
        output: Output,
        phase: u32,
        tap: Sender<'static, ScopeBlock, 2>,
        gap: bool,
        redraw: Sender<'static, (), 1>,
    }
    impl RticTask for Audio {
        fn exec(&mut self) {
            let started = cycles();
            let voice = self.shared().voice.lock(|v| *v);
            let result = self.output.render(MasterLevel::MAX, |block| {
                self.phase = voice.render(self.phase, block);
            });
            let refill_ended = cycles();
            let dropped = match result {
                Refill::WouldBlock => return,
                Refill::Late => false,
                // Avoid even the sample copy when the observer is behind.
                Refill::Written if self.tap.is_full() => true,
                Refill::Written => {
                    let block = ScopeBlock::new(self.output.block(), voice, self.gap);
                    self.tap.try_send(block).is_err()
                }
            };
            self.gap = dropped || result == Refill::Late;
            let notify = self
                .shared()
                .audio_report
                .lock(|report| report.record(started, refill_ended, result, dropped));
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
                    self.shared().frame.lock(|frame| {
                        frame.voice = block.voice;
                        frame.trace = trace;
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
        port: Option<DisplayPort>,
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
                .init_with_clock(SpiClock::Div1)
                .expect("LCD");
            let mut text = TextGrid::new();
            let mut footer = TimingFooter::default();
            let mut scope = Waveform::<WIDTH, HEIGHT>::new(8, 48).expect("scope fits the LCD");
            lcd.fill_rect(0, 0, SIZE, SIZE, 0).expect("LCD clear");
            text.text(0, 2, b"NOTE", palette::MUTED);
            text.text(10, 2, b"VEL", palette::MUTED);
            loop {
                self.updates.recv().await.expect("frame owns sender");
                let (voice, trace, panel) =
                    self.shared().frame.lock(|f| (f.voice, f.trace, f.panel));
                let report = self.shared().audio_report.lock(|report| *report);
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
                text.text(5, 2, &crate::pitch::note_name(voice.note()), palette::WHITE);
                text.number(14, 2, 3, u32::from(voice.velocity), palette::WHITE);
                if footer.draw(&mut text, panel, &report.stats) {
                    text.text(0, 1, b"SCOPE DROP", palette::MUTED);
                    text.number(10, 1, 10, report.scope_drops, palette::WHITE);
                } else {
                    text.text(0, 1, [b' '; TextGrid::COLUMNS], palette::MUTED);
                    text.text(0, 11, b"PRESETS  WAVEFORM", palette::MUTED);
                    text.text(0, 12, b"MASTER   VELOCITY", palette::MUTED);
                    text.text(0, 13, b"SELECT   TRANS", palette::MUTED);
                    text.signed_number(16, 13, 3, i32::from(voice.transpose), palette::WHITE);
                    text.text(0, 14, b"OCT-/+ 2S USB   HOME", palette::MUTED);
                }
                if report.stats.overruns != 0 {
                    text.text(9, 0, b"LATE", palette::RED);
                }
                text.paint_async(&mut lcd, &mut self.delay)
                    .await
                    .expect("LCD labels");
                scope
                    .paint_async(&mut lcd, &trace, &mut self.delay)
                    .await
                    .expect("LCD trace");
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
