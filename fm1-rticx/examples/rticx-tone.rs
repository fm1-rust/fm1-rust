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
    use fm1_rticx::{
        bsp::{
            Board,
            audio::{BLOCK_FRAMES, Block, Refill, Stream},
            display::{Display, SIZE, SpiClock},
            embedded_hal::digital::OutputPin,
            panel::{Play, PlayColor},
            time::Timers,
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
        late: bool,
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
        let stream = board.audio.into_stream().expect("audio clock");
        (
            Shared {
                transport: Transport {
                    playing: true,
                    late: false,
                    redraw,
                },
            },
            TaskInits {
                audio: Audio {
                    stream,
                    block: [[0; 2]; BLOCK_FRAMES],
                    phase: 0,
                },
                keys: Keys {
                    timers: board.timers,
                    panel,
                    previous_down: false,
                    held_ticks: 0,
                },
                screen: Screen { display, updates },
            },
        )
    }

    #[post_init]
    fn start() {
        Screen::spawn(()).expect("screen starts once");
    }

    #[task(binds = TIMER5, priority = 2, shared = [transport])]
    struct Keys {
        timers: Timers,
        panel: Play,
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
        stream: Stream<'static>,
        block: Block,
        phase: u32,
    }
    impl RticTask for Audio {
        fn exec(&mut self) {
            let mut phase = self.phase;
            if self.shared().transport.lock(|t| t.playing) {
                for frame in &mut self.block {
                    *frame = [if phase < 50 { 32_768 } else { -32_768 }; 2];
                    phase = (phase + 1) % 100;
                }
            } else {
                self.block.fill([0; 2]);
                phase = 0;
            }
            match self.stream.try_write(&self.block) {
                Refill::WouldBlock => return,
                Refill::Late => self.shared().transport.lock(|t| {
                    if !t.late {
                        t.late = true;
                        t.notify_screen();
                    }
                }),
                Refill::Written => {}
            }
            self.phase = phase;
        }
    }

    #[async_task(priority = 1, shared = [transport])]
    struct Screen {
        display: Display,
        updates: Receiver<'static, (), 1>,
    }
    impl RticAsyncTask for Screen {
        type SpawnInput = ();

        async fn exec(&mut self, (): ()) {
            const MARKER: u16 = 8;
            let mut shown = None;
            loop {
                let (playing, late) = self.shared().transport.lock(|t| (t.playing, t.late));
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
