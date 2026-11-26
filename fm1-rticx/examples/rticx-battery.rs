//! Show the low-battery warning, raw battery reading and on-chip power flags.
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
            panel::{Button, Leds, MasterLevel, PianoKey, TouchControls},
            power::{Error, Power},
        },
        time::{Clock, ExtU32, Timebase, Timer5},
    };

    #[derive(Clone, Copy, Debug, Default)]
    pub struct Readings {
        battery: Option<u16>,
        low: Option<bool>,
        preview: bool,
        master: Option<u16>,
        velocity: Option<u8>,
        pressure: Option<u8>,
    }

    #[shared]
    struct Shared {
        readings: Readings,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let board = Board::take().expect("CPU0 board");
        let power = board.take_power().expect("P33 bridge");
        let display = board.display.init_with_clock(SpiClock::Div1).expect("LCD");
        let mut controls = board.panel.into_touch_controls();
        controls.enable_battery();
        (
            Shared {
                readings: Readings::default(),
            },
            TaskInits {
                panel: Panel {
                    timer: Clock::start(board.timers.with_panel_scan_rate()),
                    controls: controls.into(),
                },
                draw: Draw {
                    display: display.into(),
                    power: power.into(),
                },
            },
        )
    }

    #[post_init]
    fn start() {
        Draw::spawn(()).expect("one display task");
    }

    #[task(binds = TIMER5, priority = 2, shared = [readings])]
    struct Panel {
        timer: Timer5,
        controls: Cpu0<TouchControls>,
    }
    impl RticTask for Panel {
        fn exec(&mut self) {
            self.timer.on_interrupt();
            if let Some((scan, touch)) = self.controls.scan(&Leds::default()) {
                if scan.pressed(Button::Play) {
                    enter_download_mode();
                }
                let key = PianoKey::new(0).expect("F3");
                self.shared().readings.lock(|r| {
                    r.battery = scan.battery_raw();
                    r.low = scan.battery_low();
                    r.preview = scan.is_down(Button::Home);
                    r.master = scan.master().map(MasterLevel::raw);
                    if let Some(velocity) = touch.velocity(key) {
                        r.velocity = Some(velocity);
                    }
                    r.pressure = touch.pressure(key);
                });
            }
        }
    }

    #[async_task(priority = 1, shared = [readings])]
    struct Draw {
        display: Cpu0<Display>,
        power: Cpu0<Power>,
    }
    impl RticAsyncTask for Draw {
        type SpawnInput = ();
        async fn exec(&mut self, (): ()) {
            self.display.fill_rect(0, 0, SIZE, SIZE, 0).expect("clear");
            let mut grid = TextGrid::new();
            for (y, label) in [
                (2, &b"BATTERY ADC"[..]),
                (3, b"P33 5V DETECT"),
                (4, b"CHARGER EN"),
                (5, b"FULL FLAG"),
                (6, b"CHIP CHARGING"),
                (7, b"P33 READ"),
                (8, b"MASTER ADC"),
                (9, b"F3 VELOCITY"),
                (10, b"F3 PRESSURE"),
                (12, b"HOLD HOME TO PREVIEW"),
                (14, b"PLAY FOR DOWNLOAD"),
            ] {
                grid.text(0, y, label, palette::WHITE);
            }
            loop {
                let r = self.shared().readings.lock(|r| *r);
                let (banner, ink) = match (r.low, r.preview) {
                    (Some(true), _) => (b"LOW BATTERY        ", palette::RED),
                    (_, true) => (b"LOW BATTERY PREVIEW", palette::RED),
                    (Some(false), false) => (b"BATTERY OK         ", palette::GREEN),
                    (None, false) => (b"WAITING FOR ADC    ", palette::MUTED),
                };
                grid.text(0, 0, banner, ink);
                for (y, value) in [
                    (2, r.battery),
                    (8, r.master),
                    (9, r.velocity.map(u16::from)),
                    (10, r.pressure.map(u16::from)),
                ] {
                    if let Some(v) = value {
                        grid.number(15, y, 5, u32::from(v), palette::GREEN);
                    } else {
                        grid.text(15, y, b"   --", palette::MUTED);
                    }
                }
                let supply = self.power.supply_status();
                for (y, flag) in [
                    (3, supply.map(|s| s.external_power)),
                    (4, supply.map(|s| s.charger_enabled)),
                    (5, supply.map(|s| s.charge_full)),
                    (6, supply.map(|s| s.is_charging())),
                ] {
                    let (word, ink) = match flag {
                        Ok(true) => (b"YES", palette::GREEN),
                        Ok(false) => (b"NO ", palette::WHITE),
                        Err(_) => (b"-- ", palette::MUTED),
                    };
                    grid.text(15, y, word, ink);
                }
                let (bus, ink) = match supply {
                    Ok(_) => (&b"OK     "[..], palette::GREEN),
                    Err(Error::Busy) => (&b"BUSY   "[..], palette::AMBER),
                    Err(Error::Timeout) => (&b"TIMEOUT"[..], palette::RED),
                };
                grid.text(13, 7, bus, ink);
                grid.paint(&mut self.display).expect("draw");
                Clock::delay(250.millis()).await;
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
