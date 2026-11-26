//! Exercise the physical MIDI input using an external keyboard or sequencer.
#![allow(missing_docs, reason = "RTICX generates task traits and initializers.")]
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
#[fm1_rticx::app(dispatchers = [SOFT0])]
mod app {
    use fm1_graphics::{TextGrid, palette};
    use fm1_rticx::{
        bsp::{
            Board, Cpu0,
            display::{Display, SIZE, SpiClock},
            enter_download_mode,
            panel::Play,
            trs_midi::{
                MidlyRx,
                midly::{
                    MidiMessage,
                    live::{LiveEvent, SystemRealtime},
                },
            },
        },
        time::{Clock, ExtU32, Timebase, Timer5},
    };

    #[derive(Clone, Copy, Debug, Default)]
    pub struct Stats {
        bytes: u32,
        on: u32,
        off: u32,
        cc: u32,
        clock: u32,
        gaps: u32,
        note: Option<(u8, u8)>,
    }
    impl Stats {
        fn event(&mut self, event: LiveEvent<'_>) {
            match event {
                LiveEvent::Midi { channel, message } => match message {
                    MidiMessage::NoteOn { key, vel } => {
                        self.note = Some((channel.as_int(), key.as_int()));
                        if vel.as_int() == 0 {
                            self.off = self.off.saturating_add(1);
                        } else {
                            self.on = self.on.saturating_add(1);
                        }
                    }
                    MidiMessage::NoteOff { key, .. } => {
                        self.note = Some((channel.as_int(), key.as_int()));
                        self.off = self.off.saturating_add(1);
                    }
                    MidiMessage::Controller { .. } => self.cc = self.cc.saturating_add(1),
                    _ => {}
                },
                LiveEvent::Realtime(SystemRealtime::TimingClock) => {
                    self.clock = self.clock.saturating_add(1)
                }
                _ => {}
            }
        }
    }
    #[shared]
    struct Shared {
        stats: Stats,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let board = Board::take().expect("CPU0 board");
        let rx = board.take_trs_midi().expect("UART1").into_midly();
        let display = board.display.init_with_clock(SpiClock::Div1).expect("LCD");
        (
            Shared {
                stats: Stats::default(),
            },
            TaskInits {
                input: Input {
                    timer: Clock::start(board.timers),
                    rx: rx.into(),
                    counts: Stats::default(),
                },
                draw: Draw {
                    display: display.into(),
                    panel: board.panel.into(),
                },
            },
        )
    }
    #[post_init]
    fn start() {
        Draw::spawn(()).expect("one display task");
    }

    #[task(binds = TIMER5, priority = 2, shared = [stats])]
    struct Input {
        timer: Timer5,
        rx: Cpu0<MidlyRx>,
        counts: Stats,
    }
    impl RticTask for Input {
        fn exec(&mut self) {
            self.timer.on_interrupt();
            for _ in 0..4 {
                match self.rx.read_events(|e| self.counts.event(e)) {
                    Ok(0) => break,
                    Ok(n) => self.counts.bytes = self.counts.bytes.saturating_add(n as u32),
                    Err(_) => {
                        self.counts.gaps = self.counts.gaps.saturating_add(1);
                        self.counts.note = None;
                        break;
                    }
                }
            }
            let counts = self.counts;
            self.shared().stats.lock(|s| *s = counts);
        }
    }

    #[async_task(priority = 1, shared = [stats])]
    struct Draw {
        display: Cpu0<Display>,
        panel: Cpu0<Play>,
    }
    impl RticAsyncTask for Draw {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {
            self.display.fill_rect(0, 0, SIZE, SIZE, 0).expect("clear");
            let mut grid = TextGrid::new();
            grid.text(0, 0, b"TRS MIDI IN", palette::CYAN);
            for (y, label) in [
                (2, &b"RX BYTES"[..]),
                (3, b"NOTE ON"),
                (4, b"NOTE OFF"),
                (5, b"CC"),
                (6, b"CLOCK"),
                (7, b"OVERRUNS"),
                (9, b"CHANNEL"),
                (10, b"LAST NOTE"),
                (12, b"PLAY AN EXTERNAL KEY"),
                (14, b"PLAY FOR DOWNLOAD"),
            ] {
                grid.text(0, y, label, palette::WHITE);
            }
            loop {
                if self.panel.button().is_pressed() {
                    enter_download_mode();
                }
                let s = self.shared().stats.lock(|s| *s);
                for (y, value) in [
                    (2, s.bytes),
                    (3, s.on),
                    (4, s.off),
                    (5, s.cc),
                    (6, s.clock),
                    (7, s.gaps),
                ] {
                    let color = if y == 7 && value != 0 {
                        palette::RED
                    } else {
                        palette::GREEN
                    };
                    grid.number(10, y, 10, value, color);
                }
                if let Some((ch, note)) = s.note {
                    grid.number(16, 9, 4, u32::from(ch) + 1, palette::GREEN);
                    grid.number(16, 10, 4, u32::from(note), palette::GREEN);
                } else {
                    grid.text(16, 9, b"  --", palette::MUTED);
                    grid.text(16, 10, b"  --", palette::MUTED);
                }
                grid.paint(&mut self.display).expect("draw");
                Clock::delay(100.millis()).await;
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
