//! Front-panel diagnostic. Button lamps latch until the next press. Hold both octave buttons for USB.
#![allow(missing_docs, reason = "RTICX generates task traits and initialisers.")]
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
#[fm1_rticx::app(dispatchers = [SOFT0])]
mod app {
    use fm1_graphics::{TextGrid, palette};
    use fm1_rticx::{
        bsp::{
            Board,
            display::{Display, SIZE, SpiClock},
            enter_download_mode,
            panel::{
                Button, Controls, Encoder, Event, Leds, MasterLevel, PianoKey, PlayLedState, Scan,
                Switch,
            },
        },
        time::{Clock, ExtU32, Timebase, Timer5},
    };
    const LABELS: [&[u8]; 14] = [
        b"OCT-", b"OCT+", b"FX", b"SEL", b"ENV", b"LFO", b"EDIT", b"GLO", b"HOME", b"SAVE", b"ARP",
        b"SEQ", b"PLAY", b"REC",
    ];

    #[derive(Clone, Copy, Debug, Default)]
    pub struct View {
        leds: Leds,
        held: [bool; 14],
        presses: [u16; 14],
        positions: [i32; 7],
        master: Option<MasterLevel>,
        last_key: Option<PianoKey>,
    }
    impl View {
        fn update(&mut self, scan: &Scan) -> bool {
            let mut changed = false;
            for event in scan.events() {
                changed = true;
                match event {
                    Event::Pressed(Switch::Button(button)) => {
                        let i = button as usize;
                        self.held[i] = true;
                        self.presses[i] = self.presses[i].saturating_add(1);
                        if button == Button::Play {
                            self.leds.set_play(match self.leds.play() {
                                PlayLedState::Off => PlayLedState::Green,
                                PlayLedState::Green => PlayLedState::Red,
                                PlayLedState::Red => PlayLedState::Both,
                                PlayLedState::Both => PlayLedState::Off,
                            });
                        } else {
                            self.leds.toggle(button);
                        }
                    }
                    Event::Released(Switch::Button(button)) => self.held[button as usize] = false,
                    Event::Pressed(Switch::Piano(key)) => {
                        self.last_key = Some(key);
                        self.leds.set(key, true);
                    }
                    Event::Released(Switch::Piano(key)) => self.leds.set(key, false),
                    Event::Turned { encoder, detents } => {
                        let count = &mut self.positions[encoder as usize];
                        *count = count.saturating_add(i32::from(detents));
                    }
                    Event::MasterChanged(level) => self.master = level,
                }
            }
            changed
        }
        fn layout(screen: &mut TextGrid) {
            screen.text(0, 0, b"FM-1 CONTROL TEST", palette::WHITE);
            screen.text(0, 1, b"MASTER", palette::WHITE);
            screen.text(13, 1, b"/1023", palette::MUTED);
            for (row, label) in [(2, &b"SELECT"[..]), (3, b"PRESETS"), (4, b"ALGORITHM")] {
                screen.text(0, row, label, palette::WHITE);
            }
            for (row, label) in [(2, b"K1"), (3, b"K2"), (4, b"K3"), (5, b"K4")] {
                screen.text(14, row, label, palette::WHITE);
            }
            screen.text(0, 5, b"PLAY LED", palette::WHITE);
            screen.text(0, 6, b"HELD", palette::GREEN);
            screen.text(6, 6, b"SEEN", palette::CYAN);
            screen.text(12, 6, b"PRESSES", palette::MUTED);
            screen.text(0, 14, b"OCT-/+ 2S USB", palette::MUTED);
            screen.text(14, 14, b"KEY", palette::MUTED);
        }

        #[allow(
            clippy::arithmetic_side_effects,
            reason = "The fixed layout fits the 20 by 15 character screen."
        )]
        fn show(&self, screen: &mut TextGrid) {
            if let Some(value) = self.master.map(MasterLevel::raw) {
                screen.number(9, 1, 4, u32::from(value), palette::CYAN);
            } else {
                screen.text(9, 1, b" ERR", palette::AMBER);
            }
            for (i, enc) in Encoder::ALL.into_iter().enumerate() {
                let (x, y) = if i < 3 { (9, 2 + i) } else { (16, i - 1) };
                let count = self.positions[enc as usize];
                screen.signed_number(
                    x,
                    y,
                    4,
                    count,
                    if count < 0 {
                        palette::AMBER
                    } else {
                        palette::CYAN
                    },
                );
            }
            let (name, color) = match self.leds.play() {
                PlayLedState::Green => (&b"GREEN"[..], palette::GREEN),
                PlayLedState::Red => (&b"RED  "[..], palette::RED),
                PlayLedState::Both => (&b"BOTH "[..], palette::AMBER),
                PlayLedState::Off => (&b"OFF  "[..], palette::MUTED),
            };
            screen.text(9, 5, name, color);
            for (i, name) in LABELS.into_iter().enumerate() {
                let (x, y) = (10 * (i % 2), 7 + i / 2);
                let ink = if self.held[i] {
                    palette::GREEN
                } else if self.presses[i] != 0 {
                    palette::CYAN
                } else {
                    palette::MUTED
                };
                screen.text(x, y, name, ink);
                let button = Button::ALL[i];
                let lamp = if button == Button::Play {
                    self.leds.play() != PlayLedState::Off
                } else {
                    self.leds.is_on(button)
                };
                screen.text(x + 4, y, if lamp { b"*" } else { b" " }, palette::AMBER);
                screen.number(x + 6, y, 3, u32::from(self.presses[i]), ink);
            }
            if let Some(key) = self.last_key {
                screen.number(18, 14, 2, u32::from(key.index()) + 1, palette::CYAN);
            } else {
                screen.text(18, 14, b"--", palette::MUTED);
            }
        }
    }

    #[shared]
    struct Shared {
        view: View,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let board = Board::take().expect("CPU0 board");
        let display = board.display.init_with_clock(SpiClock::Div1).expect("LCD");
        let view = View::default();
        let timer = Clock::start(board.timers.with_panel_scan_rate());
        (
            Shared { view },
            TaskInits {
                panel: Panel {
                    timer,
                    controls: board.panel.into_controls(),
                    view,
                    octave_since: None,
                },
                draw: Draw { display },
            },
        )
    }

    #[post_init]
    fn start() {
        Draw::spawn(()).expect("one screen task");
    }

    #[task(binds = TIMER5, priority = 2, shared = [view])]
    struct Panel {
        timer: Timer5,
        controls: Controls,
        view: View,
        octave_since: Option<u32>,
    }
    impl RticTask for Panel {
        fn exec(&mut self) {
            self.timer.on_interrupt();
            let Some(scan) = self.controls.scan(&self.view.leds) else {
                return;
            };
            if self.view.update(&scan) {
                let next = self.view;
                self.shared().view.lock(|view| *view = next);
            }
            if scan.is_down(Button::OctaveDown) && scan.is_down(Button::OctaveUp) {
                let now = fm1_rticx::bsp::time::ticks();
                if now.wrapping_sub(*self.octave_since.get_or_insert(now)) > 200 {
                    enter_download_mode();
                }
            } else {
                self.octave_since = None;
            }
        }
    }

    #[async_task(priority = 1, shared = [view])]
    struct Draw {
        display: Display,
    }
    impl RticAsyncTask for Draw {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {
            let mut screen = TextGrid::new();
            if self.display.fill_rect(0, 0, SIZE, SIZE, 0).is_err() {
                return;
            }
            View::layout(&mut screen);
            loop {
                let view = self.shared().view.lock(|view| *view);
                view.show(&mut screen);
                if screen.paint(&mut self.display).is_err() {
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
