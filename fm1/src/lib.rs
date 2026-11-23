#![no_std]
#![doc = include_str!("../README.md")]

pub use embedded_hal;
#[cfg(any(feature = "async-display", feature = "async-usb"))]
pub use embedded_hal_async;

#[cfg(feature = "audio")]
pub mod audio;
mod cpu0;
pub use cpu0::Cpu0;
pub mod display;
pub mod panel;
mod registers;
#[cfg(test)]
mod test_board;
pub mod time;
pub mod usb_midi;
use core::marker::PhantomData;
#[cfg(fm1)]
use core::sync::atomic::{AtomicBool, Ordering};
use embedded_hal::digital::{ErrorType, InputPin};

#[cfg(fm1)]
static TAKEN: AtomicBool = AtomicBool::new(false);

/// A private token keeps peripheral access on the core which acquired it.
#[derive(Debug)]
struct Token(PhantomData<*mut ()>);
impl Token {
    fn new() -> Self {
        Self(PhantomData)
    }
}

/// Resources handed to the application once, after RAM and timers are ready.
#[derive(Debug)]
pub struct Board {
    /// ALNK0 stereo output. Acquiring the board leaves the codec idle.
    #[cfg(feature = "audio")]
    pub audio: audio::AudioPort,
    /// Shared 74HC595 column selector, PLAY LED and button.
    pub panel: panel::Play,
    /// Free-running crystal timer and 100 Hz interrupt clock.
    pub timers: time::Timers,
    /// SPI1 and ST7789 panel. Call `init` when the screen is needed.
    pub display: display::DisplayPort,
    /// Raw headphone insertion switch (PB5).
    pub headphones: HeadphoneJack,
}

impl Board {
    /// Acquire the CPU0 peripherals once under an external interrupt runtime.
    ///
    /// Starts the two timers and leaves the interrupt controller to the runtime.
    /// Bind TIMER5 and call [`time::Timers::on_interrupt`] to maintain the 100 Hz
    /// tick count. Returns `None` on other cores, on a host, or after acquisition.
    #[must_use]
    pub fn take() -> Option<Self> {
        #[cfg(fm1)]
        {
            if cpu0::on_cpu0() && !TAKEN.swap(true, Ordering::Relaxed) {
                return Some(Self::init());
            }
        }
        None
    }

    /// Called once by the runtime at the bootloader entry point.
    ///
    /// # Safety
    ///
    /// Requires an FM-1 running on CPU0 with interrupts disabled, .data
    /// initialized, .bss zeroed and exclusive ownership of these peripherals.
    #[doc(hidden)]
    #[must_use]
    pub unsafe fn from_reset() -> Self {
        #[cfg(fm1)]
        TAKEN.store(true, Ordering::Relaxed);
        let board = Self::init();
        time::install_tick_interrupt();
        board
    }

    fn init() -> Self {
        let panel = panel::Play::init();
        let timers = time::Timers::start();
        Self {
            #[cfg(feature = "audio")]
            audio: audio::AudioPort(Token::new()),
            panel,
            timers,
            display: display::DisplayPort(Token::new()),
            headphones: HeadphoneJack(Token::new()),
        }
    }
}

/// Define the firmware entry. The supplied Rust function must be `fn(Board) -> !`.
#[cfg(fm1)]
#[macro_export]
macro_rules! entry {
    ($entry:path) => {
        #[unsafe(export_name = "main")]
        unsafe extern "C" fn __fm1_main() -> ! {
            let run: fn($crate::Board) -> ! = $entry;
            // SAFETY: crt0 invokes this once after setting up both stacks and RAM.
            run(unsafe { $crate::Board::from_reset() })
        }
    };
}

/// Type-check a firmware entry on the host. Running it requires the target build.
#[cfg(not(fm1))]
#[macro_export]
macro_rules! entry {
    ($entry:path) => {
        fn main() {
            let _: fn($crate::Board) -> ! = $entry;
            panic!("build this firmware for the FM-1 with fm1/scripts/build-example.sh");
        }
    };
}

/// Headphone insertion switch. The bootloader configures PB5 as an input.
/// Low means inserted. Mechanical bounce is visible to the caller.
#[derive(Debug)]
pub struct HeadphoneJack(Token);
impl HeadphoneJack {
    /// Whether headphones are plugged in.
    #[must_use]
    pub fn is_inserted(&self) -> bool {
        registers::read(registers::PB + registers::IN) & (1 << 5) == 0
    }
}
impl ErrorType for HeadphoneJack {
    type Error = core::convert::Infallible;
}
impl InputPin for HeadphoneJack {
    fn is_high(&mut self) -> Result<bool, Self::Error> {
        Ok(!self.is_inserted())
    }
    fn is_low(&mut self) -> Result<bool, Self::Error> {
        Ok(self.is_inserted())
    }
}

/// Ask m-bootloader to enter USB download mode, then reset the chip.
///
/// # Panics
/// On a host this function panics because the bootloader is unavailable.
#[cfg_attr(
    not(fm1),
    allow(clippy::panic, reason = "A host has no FM-1 reset controller.")
)]
pub fn enter_download_mode() -> ! {
    #[cfg(not(fm1))]
    panic!("download mode requires an FM-1");
    #[cfg(fm1)]
    {
        use registers::{csync, set, write};
        unsafe extern "C" {
            fn fm1_cli();
        }
        // SAFETY: this path never returns to the caller. Mask CPU0 interrupts
        // before updating the bootloader mailbox and requesting a reset.
        unsafe { fm1_cli() };
        write(time::T5_CON, 0x4000);
        #[allow(
            clippy::arithmetic_side_effects,
            reason = "Four words fit in the bootloader mailbox."
        )]
        for (i, word) in b"usb_update_mode\0".as_chunks::<4>().0.iter().enumerate() {
            write(0x01c7_fd80 + 4 * i, u32::from_le_bytes(*word));
        }
        csync();
        set(0x10000, 0x10);
        loop {
            core::hint::spin_loop();
        }
    }
}
