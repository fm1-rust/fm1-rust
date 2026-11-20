//! Play the FM-1 keyboard. Hold both octave buttons for two seconds for USB download.
#![allow(missing_docs, reason = "RTICX generates task traits and initialisers.")]
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
#[path = "common/screen.rs"]
mod screen;
#[cfg(any(target_os = "none", test))]
#[path = "rticx-piano/synth.rs"]
mod synth;

#[cfg(target_os = "none")]
#[fm1_rticx::app(dispatchers = [SOFT0])]
mod app {
    use crate::{
        screen::{Ink, Screen},
        synth::{Synth, midi_note, note_name},
    };
    use fm1_rticx::{
        bsp::{
            Board,
            audio::{BLOCK_FRAMES, Block, Refill, Stream},
            display::DisplayPort,
            enter_download_mode,
            panel::{Button, Controls, Event, Leds, MasterLevel, PianoKey, Scan, Switch},
            time::{Timers, ticks},
        },
        time::{Clock, ExtU32, Timebase, Timer5},
    };

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct Keyboard {
        held: u32,
        seen: u32,
        last: Option<PianoKey>,
        master: Option<MasterLevel>,
    }

    impl Keyboard {
        fn update(&mut self, scan: &Scan, leds: &mut Leds) {
            for event in scan.events() {
                match event {
                    Event::Pressed(Switch::Piano(key)) => {
                        self.held |= 1 << key.index();
                        self.seen |= 1 << key.index();
                        self.last = Some(key);
                        leds.set(key, true);
                    }
                    Event::Released(Switch::Piano(key)) => {
                        self.held &= !(1 << key.index());
                        leds.set(key, false);
                    }
                    Event::MasterChanged(Some(level)) => self.master = Some(level),
                    _ => {}
                }
            }
        }

        fn draw(&self, screen: &mut Screen) {
            if let Some(key) = self.last {
                screen.text(5, 2, &note_name(key), Ink::White);
                screen.number(16, 2, 2, u32::from(midi_note(key)), None, Ink::White);
            }
            screen.number(5, 3, 2, self.held.count_ones(), None, Ink::Green);
            screen.number(15, 3, 2, self.seen.count_ones(), None, Ink::Cyan);
            for key in PianoKey::all() {
                let bit = 1 << key.index();
                let ink = if self.held & bit != 0 {
                    Ink::Green
                } else if self.seen & bit != 0 {
                    Ink::Cyan
                } else {
                    Ink::Muted
                };
                let i = usize::from(key.index());
                screen.text((i % 5) * 4, 5 + i / 5, &note_name(key), ink);
            }
            if let Some(level) = self.master {
                screen.number(7, 12, 4, u32::from(level.raw()), None, Ink::Amber);
            }
        }
    }

    #[shared]
    struct Shared {
        keyboard: Keyboard,
        late: bool,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let board = Board::take().expect("CPU0 board");
        let controls = board.panel.into_controls();
        let stream = board.audio.into_stream().expect("audio clock");
        let timer = Clock::start(board.timers.with_panel_scan_rate());
        let keyboard = Keyboard::default();
        (
            Shared {
                keyboard,
                late: false,
            },
            TaskInits {
                panel: Panel {
                    controls,
                    timer,
                    keyboard,
                    leds: Leds::default(),
                    octave_since: None,
                },
                audio: Audio {
                    stream,
                    synth: Synth::default(),
                    block: [[0; 2]; BLOCK_FRAMES],
                },
                draw: Draw {
                    port: Some(board.display),
                },
            },
        )
    }

    #[post_init]
    fn start() {
        Draw::spawn(()).expect("one screen task");
    }

    // Keep column timing steady even while the audio task mixes a large chord.
    #[task(binds = TIMER5, priority = 4, shared = [keyboard])]
    struct Panel {
        controls: Controls,
        timer: Timer5,
        keyboard: Keyboard,
        leds: Leds,
        octave_since: Option<u32>,
    }
    impl RticTask for Panel {
        fn exec(&mut self) {
            self.timer.on_interrupt();
            let Some(scan) = self.controls.scan(&self.leds) else {
                return;
            };
            let previous = self.keyboard;
            self.keyboard.update(&scan, &mut self.leds);
            if self.keyboard != previous {
                let next = self.keyboard;
                self.shared().keyboard.lock(|keyboard| *keyboard = next);
            }
            if scan.is_down(Button::OctaveDown) && scan.is_down(Button::OctaveUp) {
                let now = ticks();
                if now.wrapping_sub(*self.octave_since.get_or_insert(now)) > 2 * Timers::TICK_HZ {
                    enter_download_mode();
                }
            } else {
                self.octave_since = None;
            }
        }
    }

    #[task(binds = ALNK0, priority = 3, shared = [keyboard, late])]
    struct Audio {
        stream: Stream<'static>,
        synth: Synth,
        block: Block,
    }
    impl RticTask for Audio {
        fn exec(&mut self) {
            let (held, master) = self.shared().keyboard.lock(|k| (k.held, k.master));
            let mut synth = self.synth;
            synth.render(held, master.unwrap_or(MasterLevel::MIN), &mut self.block);
            match self.stream.try_write(&self.block) {
                Refill::Written => {}
                Refill::WouldBlock => return,
                Refill::Late => self.shared().late.lock(|late| *late = true),
            }
            self.synth = synth;
        }
    }

    #[async_task(priority = 1, shared = [keyboard, late])]
    struct Draw {
        port: Option<DisplayPort>,
    }
    impl RticAsyncTask for Draw {
        type SpawnInput = ();

        async fn exec(&mut self, (): ()) {
            // The keyboard and audio can run while the LCD wakes up.
            let Some(port) = self.port.take() else { return };
            let Ok(mut display) = port.init() else { return };
            let mut screen = Screen::new();
            if Screen::clear(&mut display).is_err() {
                return;
            }
            screen.text(0, 0, b"FM-1 PIANO", Ink::White);
            screen.text(0, 2, b"LAST --", Ink::Muted);
            screen.text(10, 2, b"MIDI  --", Ink::Muted);
            screen.text(0, 3, b"HELD", Ink::Green);
            screen.text(10, 3, b"SEEN", Ink::Cyan);
            screen.text(17, 3, b"/27", Ink::Muted);
            screen.text(0, 12, b"MASTER", Ink::White);
            screen.text(11, 12, b"/1023", Ink::Muted);
            screen.text(0, 14, b"OCT-/+ 2S USB", Ink::Muted);
            loop {
                let keyboard = self.shared().keyboard.lock(|k| *k);
                keyboard.draw(&mut screen);
                if self.shared().late.lock(|late| *late) {
                    screen.text(16, 0, b"LATE", Ink::Red);
                }
                if screen.paint(&mut display).is_err() {
                    return;
                }
                Clock::delay(20.millis()).await;
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
