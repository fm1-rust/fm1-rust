// SPDX-FileCopyrightText: 2026 Julius Vitkauskas
// SPDX-License-Identifier: MIT-0
//! FM-1 keys and LEDs share an active-low column selector (two 74HC595s).
//! The default mode keeps PLAY selected. Consume it with [`Play::into_matrix`]
//! to scan other columns. Only one column can be lit or sampled at a time.

use crate::{Token, registers::*, time::wait_cycles};
use core::convert::Infallible;
use embedded_hal::digital::{ErrorType, InputPin, OutputPin, StatefulOutputPin};

const LATCH: u32 = 1 << 1;
const BACKLIGHT: u32 = 1 << 2;
const CLOCK: u32 = 1 << 3;
const DATA: u32 = 1 << 4;
const PLAY_INPUT: u32 = 1 << 6;
const PLAY_LED: u32 = 1 << 10;
const LED_A: u32 = (1 << 9) | PLAY_LED;
const LED_H: u32 = (1 << 6) | (1 << 9);

/// A physical shift-register column.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Column(u8);
impl Column {
    /// Number of populated columns.
    pub const COUNT: u8 = 11;
    /// Column shared by the PLAY button and LED.
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
        !(1u16 << self.0)
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

/// Fixed PLAY column with mutually exclusive LED and button access.
#[derive(Debug)]
pub struct Play(Token);
impl Play {
    pub(crate) fn init() -> Self {
        // PA2 low is required for key reads, including in LED-only firmware.
        outputs(PA, LATCH | CLOCK | DATA | BACKLIGHT);
        drive_leds(PA, PLAY_LED);
        pull_up_inputs(PA, PLAY_INPUT);
        select(Column::PLAY);
        Self(Token::new())
    }
    /// Borrow the active-high PLAY LED.
    pub fn led(&mut self) -> PlayLed<'_> {
        PlayLed { _panel: self }
    }
    /// Borrow the active-low PLAY button.
    pub fn button(&mut self) -> PlayButton<'_> {
        PlayButton { _panel: self }
    }
    /// Enable all four LED lines and six input rows. Initially selects PLAY with all LEDs off.
    #[must_use]
    pub fn into_matrix(self) -> Matrix {
        drive_leds(PA, LED_A);
        drive_leds(PH, LED_H);
        pull_up_inputs(PA, 1 | (0x0f << 5));
        pull_up_inputs(PB, 1 << 7);
        Matrix(self.0)
    }
}

/// Active-high `embedded-hal` output. The LED remains in its last state when dropped.
#[derive(Debug)]
pub struct PlayLed<'a> {
    _panel: &'a mut Play,
}
impl ErrorType for PlayLed<'_> {
    type Error = Infallible;
}
impl OutputPin for PlayLed<'_> {
    fn set_low(&mut self) -> Result<(), Infallible> {
        clear(PA + OUT, PLAY_LED);
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Infallible> {
        set(PA + OUT, PLAY_LED);
        Ok(())
    }
}
impl StatefulOutputPin for PlayLed<'_> {
    fn is_set_high(&mut self) -> Result<bool, Infallible> {
        Ok(read(PA + OUT) & PLAY_LED != 0)
    }
    fn is_set_low(&mut self) -> Result<bool, Infallible> {
        Ok(!self.is_set_high()?)
    }
}

/// Active-low `embedded-hal` input. Sampling briefly blanks the LED and restores it.
#[derive(Debug)]
pub struct PlayButton<'a> {
    _panel: &'a mut Play,
}
impl PlayButton<'_> {
    /// Read the raw contact after the line settles. Debounce in the application.
    pub fn is_pressed(&mut self) -> bool {
        let led = read(PA + OUT) & PLAY_LED;
        clear(PA + OUT, PLAY_LED);
        wait_cycles(240); // 10 us @ 24 MHz
        let down = read(PA + IN) & PLAY_INPUT == 0;
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
            LedRow::Pa6 => (PA, PLAY_LED),
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
        clear(PA + OUT, LED_A);
        clear(PH + OUT, LED_H);
        wait_cycles(240);
        let rows = closed_rows(read(PA + IN), read(PB + IN));
        set(PA + OUT, a);
        set(PH + OUT, h);
        rows
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
        for c in 0..Column::COUNT {
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
