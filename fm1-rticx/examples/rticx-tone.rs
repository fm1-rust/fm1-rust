//! Tap PLAY/STOP to toggle the tone. Hold for two seconds to return to download mode.
#![allow(
    missing_docs,
    reason = "RTICX generates public task traits and initialisers."
)]
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
#[fm1_rticx::app(dispatchers = [SOFT0])]
mod app {
    use fm1_audio::{MasterLevel, Output, Stats};
    use fm1_rticx::bsp::Cpu0;
    use fm1_rticx::{
        bsp::{
            Board,
            display::{Display, SIZE, SpiClock},
            embedded_hal::digital::OutputPin,
            panel::{Play, PlayColor},
            time::{Timers, cycles},
        },
        rticx::asynchronous::{
            channel::{Receiver, Sender},
            make_channel,
        },
    };

    #[shared]
    struct Shared {
        transport: Transport,
    }

    #[derive(Debug)]
    pub struct Transport {
        playing: bool,
        stats: Stats,
        redraw: Sender<'static, (), 1>,
    }
    impl Transport {
        fn notify_screen(&mut self) {
            // A full channel already has a redraw pending. The screen reads the latest state.
            let _ = self.redraw.try_send(());
        }
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let board = Board::take().expect("CPU0 board");
        let display = board.display.init_with_clock(SpiClock::Div1).expect("LCD");
        let mut panel = board.panel;
        panel.led().set_color(PlayColor::Green);
        let Ok(()) = panel.led().set_high();
        let (redraw, updates) = make_channel!((), 1);
        let output = Output::start(board.audio).expect("audio clock");
        (
            Shared {
                transport: Transport {
                    playing: true,
                    stats: Stats::default(),
                    redraw,
                },
            },
            TaskInits {
                audio: Audio { output, phase: 0 },
                keys: Keys {
                    timers: board.timers.into(),
                    panel: panel.into(),
                    previous_down: false,
                    held_ticks: 0,
                },
                screen: Screen {
                    display: display.into(),
                    updates,
                },
            },
        )
    }

    #[post_init]
    fn start() {
        Screen::spawn(()).expect("screen starts once");
    }

    #[task(binds = TIMER5, priority = 2, shared = [transport])]
    struct Keys {
        timers: Cpu0<Timers>,
        panel: Cpu0<Play>,
        previous_down: bool,
        held_ticks: u32,
    }
    impl RticTask for Keys {
        fn exec(&mut self) {
            self.timers.on_interrupt();
            let down = self.panel.button().is_pressed();
            // Require two matching 10 ms samples on both edges. Toggle only on release.
            if core::mem::replace(&mut self.previous_down, down) != down {
                return;
            }
            if down {
                self.held_ticks = self.held_ticks.saturating_add(1);
                if self.held_ticks == 2 * Timers::TICK_HZ {
                    fm1_rticx::bsp::enter_download_mode();
                }
            } else if self.held_ticks != 0 {
                self.held_ticks = 0;
                let playing = self.shared().transport.lock(|t| {
                    t.playing = !t.playing;
                    t.notify_screen();
                    t.playing
                });
                self.panel.led().set_color(if playing {
                    PlayColor::Green
                } else {
                    PlayColor::Amber
                });
            }
        }
    }

    #[task(binds = ALNK0, priority = 3, shared = [transport])]
    struct Audio {
        output: Output,
        phase: u32,
    }
    impl RticTask for Audio {
        fn exec(&mut self) {
            let started = cycles();
            let playing = self.shared().transport.lock(|t| t.playing);
            let result = self.output.render(MasterLevel::MAX, |block| {
                if playing {
                    for frame in block {
                        *frame = [if self.phase < 50 { 32_768 } else { -32_768 }; 2];
                        self.phase = (self.phase + 1) % 100;
                    }
                } else {
                    self.phase = 0;
                }
            });
            self.shared().transport.lock(|t| {
                if t.stats.record(started, result) && t.stats.overruns == 1 {
                    t.notify_screen();
                }
            });
        }
    }

    #[async_task(priority = 1, shared = [transport])]
    struct Screen {
        display: Cpu0<Display>,
        updates: Receiver<'static, (), 1>,
    }
    impl RticAsyncTask for Screen {
        type SpawnInput = ();

        async fn exec(&mut self, (): ()) {
            const MARKER: u16 = 8;
            let mut shown = None;
            loop {
                let (playing, late) = self
                    .shared()
                    .transport
                    .lock(|t| (t.playing, t.stats.overruns != 0));
                if shown.map(|(old, _)| old) != Some(playing) {
                    let height = if late { SIZE - MARKER } else { SIZE };
                    let color = if playing { 0x07e0 } else { 0xf800 };
                    self.display
                        .fill_rect(0, 0, SIZE, height, color)
                        .expect("LCD fill");
                }
                if late && !shown.is_some_and(|(_, marked)| marked) {
                    self.display
                        .fill_rect(0, SIZE - MARKER, SIZE, MARKER, 0xf81f)
                        .expect("late marker");
                }
                shown = Some((playing, late));
                self.updates
                    .recv()
                    .await
                    .expect("transport owns the sender");
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
