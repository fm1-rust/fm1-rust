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
#[path = "rticx-waveform/voice.rs"]
mod voice;

#[cfg(target_os = "none")]
#[fm1_rticx::app(dispatchers = [SOFT0])]
mod app {
    use crate::voice::{Capture, HEIGHT, Trace, Voice, WIDTH};
    use fm1_graphics::{TextGrid, Waveform, palette};
    use fm1_rticx::{
        bsp::{
            Board,
            audio::{BLOCK_FRAMES, Block, Refill, Stream},
            display::{DisplayPort, SIZE, SpiClock},
            enter_download_mode,
            panel::{Button, Controls, Encoder, Event, Leds, PlayLedState, Switch},
            time::{Timers, ticks},
        },
        rticx::asynchronous::{
            channel::{Receiver, Sender},
            make_channel,
        },
    };

    #[shared]
    struct Shared {
        voice: Voice,
        frame: Frame,
    }

    #[derive(Debug)]
    pub struct Frame {
        voice: Voice,
        trace: Trace,
        late: bool,
        ready: Sender<'static, (), 1>,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let board = Board::take().expect("CPU0 board");
        let voice = Voice::default();
        let (ready, updates) = make_channel!((), 1);
        let mut leds = Leds::default();
        leds.set_play(PlayLedState::Green);
        (
            Shared {
                voice,
                frame: Frame {
                    voice,
                    trace: [64; WIDTH],
                    late: false,
                    ready,
                },
            },
            TaskInits {
                audio: Audio {
                    stream: board.audio.into_stream().expect("audio clock"),
                    phase: 0,
                    block: [[0; 2]; BLOCK_FRAMES],
                    capture: Capture::new(),
                },
                panel: Panel {
                    timers: board.timers.with_panel_scan_rate(),
                    controls: board.panel.into_controls(),
                    leds,
                    voice,
                    octave_since: None,
                },
                draw: Draw {
                    port: Some(board.display),
                    updates,
                },
            },
        )
    }

    #[post_init]
    fn start() {
        Draw::spawn(()).expect("one scope renderer");
    }

    #[task(binds = TIMER5, priority = 2, shared = [voice])]
    struct Panel {
        timers: Timers,
        controls: Controls,
        leds: Leds,
        voice: Voice,
        octave_since: Option<u32>,
    }
    impl RticTask for Panel {
        fn exec(&mut self) {
            self.timers.on_interrupt();
            let Some(scan) = self.controls.scan(&self.leds) else {
                return;
            };
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
        }
    }

    #[task(binds = ALNK0, priority = 3, shared = [voice, frame])]
    struct Audio {
        stream: Stream<'static>,
        block: Block,
        phase: u32,
        capture: Capture,
    }
    impl RticTask for Audio {
        fn exec(&mut self) {
            let voice = self.shared().voice.lock(|v| *v);
            let next = voice.render(self.phase, &mut self.block);
            match self.stream.try_write(&self.block) {
                Refill::WouldBlock => return,
                Refill::Late => {
                    self.capture.discontinuity();
                    self.shared().frame.lock(|f| f.late = true);
                }
                Refill::Written => {
                    if self.capture.push(&self.block, voice) {
                        let trace = self.capture.trace;
                        self.shared().frame.lock(|frame| {
                            frame.voice = voice;
                            frame.trace = trace;
                            // A full channel already has one redraw pending. Always keep the latest PCM.
                            let _ = frame.ready.try_send(());
                        });
                    }
                }
            }
            self.phase = next;
        }
    }

    #[async_task(priority = 1, shared = [frame])]
    struct Draw {
        port: Option<DisplayPort>,
        updates: Receiver<'static, (), 1>,
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
            let mut scope = Waveform::<WIDTH, HEIGHT>::new(8, 48).expect("scope fits the LCD");
            lcd.fill_rect(0, 0, SIZE, SIZE, 0).expect("LCD clear");
            text.text(0, 2, b"NOTE", palette::MUTED);
            text.text(10, 2, b"VEL", palette::MUTED);
            text.text(0, 11, b"PRESETS  WAVEFORM", palette::MUTED);
            text.text(0, 12, b"MASTER   VELOCITY", palette::MUTED);
            text.text(0, 13, b"SELECT   TRANS", palette::MUTED);
            text.text(0, 14, b"OCT-/+ 2S USB", palette::MUTED);
            loop {
                self.updates.recv().await.expect("audio owns sender");
                let (voice, trace, late) = self.shared().frame.lock(|f| (f.voice, f.trace, f.late));
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
                let shift = voice.transpose;
                text.signed_number(16, 13, 3, i32::from(shift), palette::WHITE);
                if late {
                    text.text(9, 0, b"LATE", palette::RED);
                }
                text.paint(&mut lcd).expect("LCD labels");
                scope.paint(&mut lcd, &trace).expect("LCD trace");
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
