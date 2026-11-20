//! Two periodic tasks share the PLAY controls. Hold PLAY for two seconds to exit.
#![allow(
    missing_docs,
    reason = "RTICX generates public task traits and initialisers."
)]
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
#[fm1_rticx::app(dispatchers = [SOFT0, SOFT1])]
mod app {
    use fm1_rticx::{
        bsp::{Board, embedded_hal::digital::StatefulOutputPin, panel::Play},
        time::{Clock, Duration, ExtU32, Timebase, Timer5},
    };

    #[shared]
    struct Shared {
        panel: Play,
    }

    #[init]
    fn init() -> (Shared, TaskInits) {
        let board = Board::take().expect("CPU0 board");
        (
            Shared { panel: board.panel },
            TaskInits {
                tick: Tick(Clock::start(board.timers)),
            },
        )
    }

    #[post_init]
    fn start() {
        Blink::spawn(()).expect("blink starts once");
        Keys::spawn(()).expect("keys starts once");
    }

    #[task(binds = TIMER5, priority = 3)]
    struct Tick(Timer5);
    impl RticTask for Tick {
        fn exec(&mut self) {
            self.0.on_interrupt();
        }
    }

    #[async_task(priority = 1, shared = [panel], init = generated)]
    struct Blink;
    impl RticAsyncTask for Blink {
        type SpawnInput = ();

        async fn exec(&mut self, (): ()) {
            let mut edge = Clock::now();
            loop {
                edge += 500.millis();
                Clock::delay_until(edge).await;
                self.shared().panel.lock(|p| {
                    let Ok(()) = p.led().toggle();
                });
            }
        }
    }

    #[async_task(priority = 2, shared = [panel], init = generated)]
    struct Keys;
    impl RticAsyncTask for Keys {
        type SpawnInput = ();

        async fn exec(&mut self, (): ()) {
            let mut sample = Clock::now();
            let mut pressed_since = None;
            loop {
                sample += 10.millis();
                Clock::delay_until(sample).await;
                if self.shared().panel.lock(|p| p.button().is_pressed()) {
                    let now = Clock::now();
                    if now - *pressed_since.get_or_insert(now) >= Duration::from_secs(2) {
                        fm1_rticx::bsp::enter_download_mode();
                    }
                } else {
                    pressed_since = None;
                }
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
