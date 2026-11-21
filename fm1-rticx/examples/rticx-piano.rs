//! Play the FM-1 keyboard locally or over USB MIDI. Hold both octave buttons for two seconds for USB download.
#![allow(missing_docs, reason = "RTICX generates task traits and initialisers.")]
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(any(target_os = "none", test))]
#[path = "rticx-piano/midi.rs"]
mod midi;
#[cfg(any(target_os = "none", test))]
#[path = "common/pitch.rs"]
mod pitch;
#[cfg(target_os = "none")]
#[path = "common/screen.rs"]
mod screen;
#[cfg(any(target_os = "none", test))]
#[path = "rticx-piano/synth.rs"]
mod synth;

#[cfg(target_os = "none")]
#[fm1_rticx::app(dispatchers = [SOFT0, SOFT1])]
mod app {
    use crate::{
        midi::{Output, RemoteNotes},
        pitch::{midi_note, note_name},
        screen::{Ink, Screen},
        synth::Synth,
    };
    use fm1_rticx::{
        bsp::{
            Board,
            audio::{BLOCK_FRAMES, Block, Refill, Stream},
            display::{DisplayPort, SpiClock},
            enter_download_mode,
            panel::{Button, Controls, Event, Leds, MasterLevel, PianoKey, Scan, Switch},
            time::{Timers, ticks},
            usb_midi::{Identity, MidiDevice, PacketBuffer},
        },
        rticx::asynchronous::{
            channel::{Receiver, Sender},
            make_channel,
        },
        time::{Clock, ExtU32, Timebase, Timer5},
    };

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub struct Keyboard {
        physical: u32,
        remote: RemoteNotes,
        online: bool,
        seen: u32,
        last: Option<PianoKey>,
        master: Option<MasterLevel>,
    }

    impl Keyboard {
        fn held(&self) -> u32 {
            self.physical | self.remote.held
        }

        fn update(&mut self, scan: &Scan) {
            for event in scan.events() {
                match event {
                    Event::Pressed(Switch::Piano(key)) => {
                        self.physical |= 1 << key.index();
                        self.seen |= 1 << key.index();
                        self.last = Some(key);
                    }
                    Event::Released(Switch::Piano(key)) => {
                        self.physical &= !(1 << key.index());
                    }
                    Event::MasterChanged(Some(level)) => self.master = Some(level),
                    _ => {}
                }
            }
        }

        fn draw(&self, screen: &mut Screen) {
            screen.text(
                11,
                0,
                b"USB",
                if self.online { Ink::Green } else { Ink::Muted },
            );
            if let Some(key) = self.last {
                screen.text(5, 2, &note_name(midi_note(key)), Ink::White);
                screen.number(16, 2, 2, u32::from(midi_note(key)), None, Ink::White);
            }
            screen.number(5, 3, 2, self.held().count_ones(), None, Ink::Green);
            screen.number(15, 3, 2, self.seen.count_ones(), None, Ink::Cyan);
            for key in PianoKey::all() {
                let bit = 1 << key.index();
                let ink = if self.held() & bit != 0 {
                    Ink::Green
                } else if self.seen & bit != 0 {
                    Ink::Cyan
                } else {
                    Ink::Muted
                };
                let i = usize::from(key.index());
                screen.text((i % 5) * 4, 5 + i / 5, &note_name(midi_note(key)), ink);
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
        let controls = board.panel.into_controls();
        let stream = board.audio.into_stream().expect("audio clock");
        let timer = Clock::start(board.timers.with_panel_scan_rate());
        let keyboard = Keyboard::default();
        let (wake, updates) = make_channel!((), 1);
        (
            Shared {
                keyboard,
                late: false,
            },
            TaskInits {
                panel: Panel {
                    controls,
                    timer,
                    wake,
                    lit: 0,
                    leds: Leds::default(),
                    octave_since: None,
                },
                midi: Midi {
                    device,
                    updates,
                    session: None,
                    remote: RemoteNotes::default(),
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
        controls: Controls,
        timer: Timer5,
        wake: Sender<'static, (), 1>,
        lit: u32,
        leds: Leds,
        octave_since: Option<u32>,
    }
    impl RticTask for Panel {
        fn exec(&mut self) {
            self.timer.on_interrupt();
            let Some(scan) = self.controls.scan(&self.leds) else {
                return;
            };
            let held = self.shared().keyboard.lock(|keyboard| {
                keyboard.update(&scan);
                keyboard.held()
            });
            if held != self.lit {
                for key in PianoKey::all() {
                    self.leds.set(key, held & (1 << key.index()) != 0);
                }
                self.lit = held;
            }
            // A completed 11-column sweep wakes USB every 1.1 ms. A pending
            // wake already covers this sweep; no USB access occurs in this ISR.
            let _ = self.wake.try_send(());
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
                .lock(|k| (k.physical, k.remote.velocities, k.master));
            let mut synth = self.synth;
            synth.render(
                physical,
                &remote,
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
        updates: Receiver<'static, (), 1>,
        session: Option<u32>,
        remote: RemoteNotes,
        output: Output,
    }

    impl RticAsyncTask for Midi {
        type SpawnInput = ();

        async fn exec(&mut self, (): ()) {
            let mut buffer = PacketBuffer::default();
            loop {
                self.updates.recv().await.expect("panel owns the sender");
                self.device.poll();
                let session = self.device.session();
                let new_session = session != self.session;
                let previous = self.remote;
                if new_session {
                    self.remote = RemoteNotes::default();
                    self.output = Output::new();
                    self.session = session;
                }
                let mut last = None;
                let mut seen = 0;
                for &event in self.device.receive(&mut buffer) {
                    if let Some(key) = self.remote.apply(event) {
                        last = Some(key);
                        seen |= 1 << key.index();
                    }
                }
                let remote = self.remote;
                let physical = self.shared().keyboard.lock(|k| {
                    if new_session || remote != previous || last.is_some() {
                        k.online = session.is_some();
                        k.remote = remote;
                        k.seen |= seen;
                        if last.is_some() {
                            k.last = last;
                        }
                    }
                    k.physical
                });
                if session.is_some() {
                    // Host input never enters this queue, even when a physical
                    // key and an incoming note have the same pitch.
                    self.output.observe(physical);
                    let n = self.device.send(self.output.pending());
                    self.output.sent(n);
                }
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
