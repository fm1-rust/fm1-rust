//! Play the FM-1 keyboard locally or over USB MIDI. Hold both octave buttons for two seconds for USB download.
#![allow(missing_docs, reason = "RTICX generates task traits and initialisers.")]
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(any(target_os = "none", test))]
#[path = "rticx-piano/clock.rs"]
mod clock;
#[cfg(any(target_os = "none", test))]
#[path = "rticx-piano/midi.rs"]
mod midi;
#[cfg(any(target_os = "none", test))]
#[path = "common/pitch.rs"]
mod pitch;
#[cfg(any(target_os = "none", test))]
#[path = "rticx-piano/synth.rs"]
mod synth;

#[cfg(target_os = "none")]
#[fm1_rticx::app(dispatchers = [SOFT0, SOFT1])]
mod app {
    use crate::{
        clock::MidiClock,
        midi::{Notes, Output},
        pitch::{midi_note, note_name},
        synth::Synth,
    };
    use fm1_graphics::{TextGrid, palette};
    use fm1_rticx::{
        bsp::{
            Board,
            audio::{BLOCK_FRAMES, Block, Refill, Stream},
            display::{DisplayPort, SIZE, SpiClock},
            enter_download_mode,
            panel::{
                Button, Event, Leds, MasterLevel, PianoKey, PianoTouch, PlayLedState, Scan, Switch,
                TouchControls,
            },
            time::{Timers, ticks},
            usb_midi::{Identity, MidiDevice, PacketBuffer},
        },
        time::{Clock, DeadlineClock, ExtU32, Timebase, Timer5},
    };

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct Keyboard {
        physical: Notes,
        remote: Notes,
        online: bool,
        clock_playing: bool,
        clock_stats: (u32, u32),
        seen: u32,
        last: Option<PianoKey>,
        last_velocity: u8,
        master: Option<MasterLevel>,
    }

    impl Keyboard {
        fn held(&self) -> u32 {
            self.physical.held | self.remote.held
        }

        fn update(&mut self, scan: &Scan, touch: &PianoTouch) {
            for event in scan.events() {
                match event {
                    Event::Pressed(Switch::Piano(key)) => {
                        let velocity = touch.velocity(key).expect("press has a strike");
                        self.physical.held |= 1 << key.index();
                        self.physical.velocities[usize::from(key.index())] = velocity;
                        self.last_velocity = velocity;
                        self.seen |= 1 << key.index();
                        self.last = Some(key);
                    }
                    Event::Released(Switch::Piano(key)) => {
                        self.physical.held &= !(1 << key.index());
                        self.physical.velocities[usize::from(key.index())] = 0;
                        self.physical.pressures[usize::from(key.index())] = 0;
                    }
                    Event::Pressed(Switch::Button(Button::Play)) => {
                        self.clock_playing = !self.clock_playing
                    }
                    Event::MasterChanged(Some(level)) => self.master = Some(level),
                    _ => {}
                }
            }
            for (key, pressure) in touch.pressure_changes() {
                self.physical.pressures[usize::from(key.index())] = pressure;
            }
        }

        fn draw(&self, screen: &mut TextGrid) {
            screen.text(
                11,
                0,
                b"USB",
                if self.online {
                    palette::GREEN
                } else {
                    palette::MUTED
                },
            );
            if let Some(key) = self.last {
                screen.text(5, 2, &note_name(midi_note(key)), palette::WHITE);
                screen.number(16, 2, 2, u32::from(midi_note(key)), palette::WHITE);
                screen.number(4, 4, 3, u32::from(self.last_velocity), palette::AMBER);
                let i = usize::from(key.index());
                let bit = 1 << i;
                let pressure = if self.physical.held & bit != 0 {
                    key.supports_pressure()
                        .then_some(self.physical.pressures[i])
                } else {
                    (self.remote.held & bit != 0).then_some(self.remote.pressures[i])
                };
                if let Some(p) = pressure {
                    screen.number(15, 4, 3, u32::from(p), palette::CYAN);
                } else {
                    screen.text(15, 4, b" --", palette::MUTED);
                }
            }
            screen.number(5, 3, 2, self.held().count_ones(), palette::GREEN);
            screen.number(15, 3, 2, self.seen.count_ones(), palette::CYAN);
            for key in PianoKey::all() {
                let bit = 1 << key.index();
                let ink = if self.held() & bit != 0 {
                    palette::GREEN
                } else if self.seen & bit != 0 {
                    palette::CYAN
                } else {
                    palette::MUTED
                };
                let i = usize::from(key.index());
                screen.text((i % 5) * 4, 5 + i / 5, &note_name(midi_note(key)), ink);
            }
            let (worst, missed) = self.clock_stats;
            screen.text(
                0,
                11,
                b"CLK120",
                if self.clock_playing {
                    palette::GREEN
                } else {
                    palette::MUTED
                },
            );
            screen.number(10, 11, 8, worst, palette::WHITE);
            screen.number(
                7,
                13,
                10,
                missed,
                if missed == 0 {
                    palette::MUTED
                } else {
                    palette::RED
                },
            );
            if let Some(level) = self.master {
                screen.number(7, 12, 4, u32::from(level.raw()), palette::AMBER);
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
        let mut board = Board::take().expect("CPU0 board");
        // pid.codes shared test VID/PID, for the diagnostic only.
        let device = board
            .take_usb()
            .expect("one USB device")
            .into_midi(Identity {
                vendor_id: 0x1209,
                product_id: 0x0001,
                manufacturer: "fm1-rust",
                product: "FM-1 MIDI Piano",
            });
        DeadlineClock::start::<3>(board.timers.take_alarm().expect("one TIMER2 alarm"));
        let controls = board.panel.into_touch_controls();
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
                    lit: 0,
                    leds: Leds::default(),
                    octave_since: None,
                },
                midi: Midi {
                    device,
                    clock: MidiClock::default(),
                    session: None,
                    remote: Notes::default(),
                    output: Output::new(),
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
        Midi::spawn(()).expect("one USB task");
        Draw::spawn(()).expect("one screen task");
    }

    // Keep column timing steady even while the audio task mixes a large chord.
    #[task(binds = TIMER5, priority = 4, shared = [keyboard])]
    struct Panel {
        controls: TouchControls,
        timer: Timer5,
        lit: u32,
        leds: Leds,
        octave_since: Option<u32>,
    }
    impl RticTask for Panel {
        fn exec(&mut self) {
            self.timer.on_interrupt();
            let Some((scan, touch)) = self.controls.scan(&self.leds) else {
                return;
            };
            let (held, playing) = self.shared().keyboard.lock(|keyboard| {
                keyboard.update(&scan, &touch);
                (keyboard.held(), keyboard.clock_playing)
            });
            if held != self.lit {
                for key in PianoKey::all() {
                    self.leds.set(key, held & (1 << key.index()) != 0);
                }
                self.lit = held;
            }
            self.leds.set_play(if playing {
                PlayLedState::Green
            } else {
                PlayLedState::Off
            });
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
            let (physical, remote, master) = self
                .shared()
                .keyboard
                .lock(|k| (k.physical, k.remote, k.master));
            let mut synth = self.synth;
            synth.render(
                &physical.levels(&remote),
                master.unwrap_or(MasterLevel::MIN),
                &mut self.block,
            );
            match self.stream.try_write(&self.block) {
                Refill::Written => {}
                Refill::WouldBlock => return,
                Refill::Late => self.shared().late.lock(|late| *late = true),
            }
            self.synth = synth;
        }
    }

    #[async_task(priority = 2, shared = [keyboard])]
    struct Midi {
        device: MidiDevice,
        clock: MidiClock,
        session: Option<u32>,
        remote: Notes,
        output: Output,
    }

    impl RticAsyncTask for Midi {
        type SpawnInput = ();

        async fn exec(&mut self, (): ()) {
            let mut buffer = PacketBuffer::default();
            loop {
                let woke = DeadlineClock::now();
                self.device.poll();
                let session = self.device.session();
                let new_session = session != self.session;
                let previous = self.remote;
                if new_session {
                    self.remote = Notes::default();
                    self.output = Output::new();
                    self.clock = MidiClock::default();
                    self.session = session;
                }
                let mut last = None;
                let mut seen = 0;
                for &event in self.device.receive(&mut buffer) {
                    if let Some(key) = self.remote.apply(event) {
                        last = Some((key, event[3]));
                        seen |= 1 << key.index();
                    }
                }
                let remote = self.remote;
                let stats = (self.clock.worst_us, self.clock.missed);
                let (physical, playing) = self.shared().keyboard.lock(|k| {
                    k.clock_stats = stats;
                    if new_session || remote != previous || last.is_some() {
                        k.online = session.is_some();
                        k.remote = remote;
                        k.seen |= seen;
                        if let Some((key, velocity)) = last {
                            k.last = Some(key);
                            k.last_velocity = velocity;
                        }
                    }
                    (k.physical, k.clock_playing)
                });
                if session.is_some() {
                    // Host input never enters this queue, even when a physical
                    // key and an incoming note have the same pitch.
                    self.clock
                        .service(woke, playing, |event| self.device.send(&[event]) == 1);
                    self.output.observe(&physical);
                    let n = self.device.send(self.output.pending());
                    self.output.sent(n);
                    self.output
                        .send_pressure(&physical, |events| self.device.send(events));
                }
                DeadlineClock::delay_until(self.clock.wake_by(DeadlineClock::now(), playing)).await;
            }
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
            let Ok(mut display) = port.init_with_clock(SpiClock::Div1) else {
                return;
            };
            let mut screen = TextGrid::new();
            if display.fill_rect(0, 0, SIZE, SIZE, 0).is_err() {
                return;
            }
            screen.text(0, 0, b"FM-1 PIANO", palette::WHITE);
            screen.text(0, 2, b"LAST --", palette::MUTED);
            screen.text(10, 2, b"MIDI  --", palette::MUTED);
            screen.text(0, 3, b"HELD", palette::GREEN);
            screen.text(10, 3, b"SEEN", palette::CYAN);
            screen.text(0, 4, b"VEL", palette::MUTED);
            screen.text(9, 4, b"PRESS", palette::MUTED);
            screen.text(17, 3, b"/27", palette::MUTED);
            screen.text(7, 11, b"MAX", palette::MUTED);
            screen.text(18, 11, b"US", palette::MUTED);
            screen.text(0, 12, b"MASTER", palette::WHITE);
            screen.text(0, 13, b"MISSED", palette::MUTED);
            screen.text(11, 12, b"/1023", palette::MUTED);
            screen.text(0, 14, b"OCT-/+ 2S USB", palette::MUTED);
            loop {
                let keyboard = self.shared().keyboard.lock(|k| *k);
                keyboard.draw(&mut screen);
                if self.shared().late.lock(|late| *late) {
                    screen.text(16, 0, b"LATE", palette::RED);
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
