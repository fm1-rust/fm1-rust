// SPDX-FileCopyrightText: 2026 Julius Vitkauskas
// SPDX-License-Identifier: MIT-0
//! FM-1 keys and LEDs share an active-low column selector (two 74HC595s).
//! [`Play`] handles both transport LED colors and the PLAY/STOP switch.
//! Use [`Play::into_controls`] for debounced buttons, piano keys, encoder
//! detents, the MASTER pot and LEDs, or [`Play::into_matrix`] for raw GPIO rows.

mod controls;
mod state;
mod touch;
pub use controls::{Button, Controls, Encoder};
pub use state::{Event, Leds, MasterLevel, PianoKey, PlayLedState, Scan, Switch};
pub use touch::{PianoTouch, TouchControls};

use crate::{Token, registers::*, time::wait_cycles};
use core::convert::Infallible;
use embedded_hal::digital::{ErrorType, InputPin, OutputPin, StatefulOutputPin};

const LATCH: u32 = 1 << 1;
const BACKLIGHT: u32 = 1 << 2;
const CLOCK: u32 = 1 << 3;
const DATA: u32 = 1 << 4;
const PLAY_INPUT: u32 = 1 << 6;
const PLAY_AMBER: u32 = 1 << 10;
const PLAY_GREEN: u32 = 1 << 9;
const LED_A: u32 = PLAY_GREEN | PLAY_AMBER;
const LED_H: u32 = (1 << 6) | (1 << 9);

/// A physical shift-register column.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Column(u8);
impl Column {
    /// Number of populated columns.
    pub const COUNT: u8 = 11;
    /// Column shared by the PLAY/STOP switch and its amber LED.
    pub const PLAY: Self = Self(10);
    /// Accept an index in `0..11`.
    #[must_use]
    pub const fn new(index: u8) -> Option<Self> {
        if index < Self::COUNT {
            Some(Self(index))
        } else {
            None
        }
    }
    const fn word(self) -> u16 {
        let octave = match self.0 {
            0 => 1 << 11,
            1 => 1 << 12,
            _ => 0,
        };
        !(1u16 << self.0 | octave)
    }
}

fn select(column: Column) {
    for bit in (0..16).rev() {
        // The RMW on every edge gives the 595s enough pulse width.
        level(PA + OUT, DATA, column.word() & (1 << bit) != 0);
        set(PA + OUT, CLOCK);
        clear(PA + OUT, CLOCK);
    }
    set(PA + OUT, LATCH);
    clear(PA + OUT, LATCH);
}

#[allow(
    clippy::arithmetic_side_effects,
    reason = "The caller supplies PA or PH and the offsets lie within that GPIO bank."
)]
fn drive_leds(port: usize, mask: u32) {
    outputs(port, mask);
    set(port + HD0, mask);
    set(port + HD, mask);
}

/// The two LED positions under the PLAY/STOP button.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlayColor {
    /// STOP indicator on PA10, column 10. This is the default.
    Amber,
    /// PLAY indicator on PA9, column 8.
    Green,
}

impl PlayColor {
    const fn column(self) -> Column {
        match self {
            Self::Amber => Column::PLAY,
            Self::Green => Column(8),
        }
    }

    const fn line(self) -> u32 {
        match self {
            Self::Amber => PLAY_AMBER,
            Self::Green => PLAY_GREEN,
        }
    }
}

/// Mutually exclusive access to the PLAY/STOP switch and its indicator.
#[derive(Debug)]
pub struct Play {
    token: Token,
    color: PlayColor,
}
impl Play {
    pub(crate) fn init() -> Self {
        // PA2 low is required for key reads, including in LED-only firmware.
        outputs(PA, LATCH | CLOCK | DATA | BACKLIGHT);
        drive_leds(PA, LED_A);
        pull_up_inputs(PA, PLAY_INPUT);
        select(Column::PLAY);
        Self {
            token: Token::new(),
            color: PlayColor::Amber,
        }
    }
    /// Borrow the active-high PLAY LED.
    pub fn led(&mut self) -> PlayLed<'_> {
        PlayLed { panel: self }
    }
    /// Borrow the active-low PLAY button.
    pub fn button(&mut self) -> PlayButton<'_> {
        PlayButton { panel: self }
    }
    /// Enable all four LED lines and six input rows. Initially selects PLAY with all LEDs off.
    #[must_use]
    pub fn into_matrix(self) -> Matrix {
        drive_leds(PA, LED_A);
        drive_leds(PH, LED_H);
        pull_up_inputs(PA, 1 | (0x0f << 5));
        pull_up_inputs(PB, 1 << 7);
        if self.color == PlayColor::Green {
            select(Column::PLAY);
        }
        Matrix(self.token)
    }
    /// Acquire the multiplexed inputs, both transport LED channels and the MASTER ADC.
    #[must_use]
    pub fn into_controls(self) -> Controls {
        Controls::new(self.into_matrix())
    }

    /// Scan the panel with strike velocity on every piano key and pressure on the lower 20.
    #[must_use]
    pub fn into_touch_controls(self) -> TouchControls {
        TouchControls::new(self.into_controls())
    }
}

/// Active-high `embedded-hal` output. The LED remains in its last state when dropped.
#[derive(Debug)]
pub struct PlayLed<'a> {
    panel: &'a mut Play,
}
impl PlayLed<'_> {
    /// Select a color while preserving the on/off state.
    pub fn set_color(&mut self, color: PlayColor) {
        if self.panel.color != color {
            let old_line = self.panel.color.line();
            let on = read(PA + OUT) & old_line != 0;
            clear(PA + OUT, old_line);
            select(color.column());
            level(PA + OUT, color.line(), on);
            self.panel.color = color;
        }
    }
}
impl ErrorType for PlayLed<'_> {
    type Error = Infallible;
}
impl OutputPin for PlayLed<'_> {
    fn set_low(&mut self) -> Result<(), Infallible> {
        clear(PA + OUT, self.panel.color.line());
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Infallible> {
        set(PA + OUT, self.panel.color.line());
        Ok(())
    }
}
impl StatefulOutputPin for PlayLed<'_> {
    fn is_set_high(&mut self) -> Result<bool, Infallible> {
        Ok(read(PA + OUT) & self.panel.color.line() != 0)
    }
    fn is_set_low(&mut self) -> Result<bool, Infallible> {
        Ok(!self.is_set_high()?)
    }
}

/// Active-low `embedded-hal` input. Sampling briefly blanks the LED and restores it.
#[derive(Debug)]
pub struct PlayButton<'a> {
    panel: &'a mut Play,
}
impl PlayButton<'_> {
    /// Read the raw contact after the line settles. Debounce in the application.
    pub fn is_pressed(&mut self) -> bool {
        let column = self.panel.color.column();
        let line = self.panel.color.line();
        let led = read(PA + OUT) & line;
        clear(PA + OUT, line);
        if column != Column::PLAY {
            select(Column::PLAY);
        }
        wait_cycles(240); // 10 us @ 24 MHz
        let down = read(PA + IN) & PLAY_INPUT == 0;
        if column != Column::PLAY {
            select(column);
        }
        set(PA + OUT, led);
        down
    }
}
impl ErrorType for PlayButton<'_> {
    type Error = Infallible;
}
impl InputPin for PlayButton<'_> {
    fn is_high(&mut self) -> Result<bool, Infallible> {
        Ok(!self.is_pressed())
    }
    fn is_low(&mut self) -> Result<bool, Infallible> {
        Ok(self.is_pressed())
    }
}

/// The four rows that also drive LEDs, named by their corresponding switch input.
#[derive(Copy, Clone, Debug)]
pub enum LedRow {
    /// Switches on PA5, LEDs on PA9.
    Pa5,
    /// Switches on PA6, LEDs on PA10 (PLAY in column 10).
    Pa6,
    /// Switches on PA7, LEDs on PH6.
    Pa7,
    /// Switches on PA8, LEDs on PH9.
    Pa8,
}

/// Full panel matrix. Methods operate on the currently selected column.
#[derive(Debug)]
pub struct Matrix(Token);
impl Matrix {
    /// Blank the previous column and latch the new one.
    pub fn select(&mut self, column: Column) {
        clear(PA + OUT, LED_A);
        clear(PH + OUT, LED_H);
        select(column);
    }
    /// Set an LED in the selected column until the next select or write.
    pub fn set_led(&mut self, row: LedRow, on: bool) {
        let (port, mask) = match row {
            LedRow::Pa5 => (PA, 1 << 9),
            LedRow::Pa6 => (PA, PLAY_AMBER),
            LedRow::Pa7 => (PH, 1 << 6),
            LedRow::Pa8 => (PH, 1 << 9),
        };
        level(port + OUT, mask, on);
    }
    /// Return six raw closed-contact bits, in PA0, PA5, PA6, PA7, PA8, PB7 order.
    /// Blanks the LEDs for the 10 us settling time, then restores their levels.
    pub fn sample(&mut self) -> u8 {
        let a = read(PA + OUT) & LED_A;
        let h = read(PH + OUT) & LED_H;
        let rows = self.sample_and_blank();
        set(PA + OUT, a);
        set(PH + OUT, h);
        rows
    }
    fn sample_and_blank(&mut self) -> u8 {
        clear(PA + OUT, LED_A);
        clear(PH + OUT, LED_H);
        wait_cycles(240);
        closed_rows(read(PA + IN), read(PB + IN))
    }
}
fn closed_rows(a: u32, b: u32) -> u8 {
    (!((a & 1) | ((a >> 4) & 0x1e) | ((b >> 2) & 0x20))).to_le_bytes()[0] & 0x3f
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn panel_wiring() {
        assert_eq!(Column::PLAY.word(), 0xfbff);
        assert_eq!(Column(0).word(), 0xf7fe);
        assert_eq!(Column(1).word(), 0xeffd);
        for c in 2..Column::COUNT {
            assert_eq!(!Column::new(c).expect("populated column").word(), 1 << c);
        }
        assert!(Column::new(11).is_none());
        assert_eq!(closed_rows(!0, !0), 0);
        for (i, pin) in [0, 5, 6, 7, 8].into_iter().enumerate() {
            assert_eq!(closed_rows(!(1 << pin), !0), 1 << i);
        }
        assert_eq!(closed_rows(!0, !(1 << 7)), 1 << 5);
    }
}
