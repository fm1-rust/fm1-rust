// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
// Copyright (C) 2026 Charles Vestal
// FM-1 row drive, LED probe and strike curves follow felucca-bsp and fm1-base's touch.c.
use super::{
    Controls, Leds, MasterLevel, PianoKey, Scan,
    controls::{BATTERY_CHANNEL, COLUMN_TIMING, adc_result, start_adc, stop_adc},
};
use crate::registers::{DIR, HD, HD0, OUT, PA, PU, clear, level, set};

/// Expression for the piano keys held at the end of one panel sweep.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PianoTouch {
    velocities: [u8; PianoKey::COUNT as usize],
    pressures: [u8; PianoKey::PRESSURE_KEYS],
    changed: u32,
}

impl PianoTouch {
    /// Strike velocity (1–127) for the current press, or `None` after release.
    /// The value stays constant while the key is held. An upper key held before
    /// its first open probe reading gets a neutral velocity of 64.
    #[must_use]
    pub fn velocity(&self, key: PianoKey) -> Option<u8> {
        let value = self.velocities[usize::from(key.index())];
        (value != 0).then_some(value)
    }

    /// Polyphonic aftertouch (0–127) while a pressure-sensitive key is held.
    /// Returns `Some(0)` at rest and `None` for released keys and the upper seven keys.
    #[must_use]
    pub fn pressure(&self, key: PianoKey) -> Option<u8> {
        self.velocity(key)?;
        self.pressures.get(usize::from(key.index())).copied()
    }

    /// Iterate over held keys whose pressure changed during this sweep, including a
    /// return to zero. Attacks and releases are reported by the accompanying [`Scan`].
    pub fn pressure_changes(&self) -> impl Iterator<Item = (PianoKey, u8)> + '_ {
        let mut changed = self.changed;
        core::iter::from_fn(move || {
            let key = PianoKey::new(u8::try_from(changed.trailing_zeros()).ok()?)?;
            changed &= changed.wrapping_sub(1);
            Some((key, self.pressures[usize::from(key.index())]))
        })
    }
}

/// Panel scanner with opt-in velocity and polyphonic aftertouch. Owns the shared
/// ADC and temporarily borrows the piano rows and an LED line between scan ticks.
#[derive(Debug)]
pub struct TouchControls {
    panel: Controls,
    analog: [AnalogKey; PianoKey::PRESSURE_KEYS],
    probes: [Probe; PianoKey::COUNT as usize - PianoKey::PRESSURE_KEYS],
    piano: PianoTouch,
    held: [u8; 11],
    master: Option<MasterLevel>,
    phase: u8,
    slot: Slot,
}

impl TouchControls {
    /// Recommended column scan frequency, shared with the digital scanner.
    pub const SCAN_HZ: u32 = Controls::SCAN_HZ;

    /// Sample the battery divider during a lamp-only column interval. Key
    /// expression, MASTER conversions and the lamp pattern keep their cadence.
    /// Read the latest attempt through [`Scan::battery_raw`] and the warning
    /// through [`Scan::battery_low`].
    pub fn enable_battery(&mut self) {
        self.panel.enable_battery();
    }

    pub(super) fn new(panel: Controls) -> Self {
        stop_adc(); // Take over the MASTER conversion started by Controls.
        let mut this = Self {
            panel,
            analog: Default::default(),
            probes: Default::default(),
            piano: PianoTouch::default(),
            held: [0; 11],
            master: None,
            phase: 0,
            slot: Slot::Lamps,
        };
        this.start_column(&Leds::default());
        this
    }

    /// Advance one column at [`Self::SCAN_HZ`] (10 kHz). Every eleventh call returns
    /// a panel scan and its matching piano expression. A piano press edge carries its
    /// strike velocity in the same snapshot. Subsequent sweeps can change its pressure.
    ///
    /// Three sweeps sample PA7, PA8 and the PA10 probe in turn. A fourth lights every
    /// requested lamp, even when all keys in a column are held. A seated analog key
    /// needs a dark column, and the top-row probe takes over its LED line. MASTER has a
    /// slot of its own. An unfinished ADC conversion is discarded. An open digital
    /// contact can still release a note during an ADC outage.
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "The four scan phases and the two disjoint physical key ranges are bounded."
    )]
    pub fn scan(&mut self, leds: &Leds) -> Option<(Scan, PianoTouch)> {
        let sample = self.slot.finish();
        let rows = self.panel.advance();
        match self.slot {
            Slot::Master => self.master = sample.map(|v| MasterLevel::from_adc(u32::from(v))),
            Slot::Battery => self.panel.battery.record(sample),
            Slot::Key { key, driven } => {
                let (column, row) = key.position();
                let mask = 1 << row;
                let closed = rows & mask != 0;
                let i = usize::from(key.index());
                let velocity = &mut self.piano.velocities[i];
                if let Some(analog) = self.analog.get_mut(i) {
                    let previous = self.piano.pressures[i];
                    analog.sample(
                        sample,
                        driven,
                        closed,
                        velocity,
                        &mut self.piano.pressures[i],
                    );
                    if *velocity != 0 && self.piano.pressures[i] != previous {
                        self.piano.changed |= 1 << i;
                    }
                } else {
                    self.probes[i - PianoKey::PRESSURE_KEYS].sample(sample, closed, velocity);
                }
                if *velocity != 0 {
                    self.held[column] |= mask;
                } else {
                    self.held[column] &= !mask;
                }
            }
            Slot::Lamps => {}
        }

        let complete = self.panel.column == 0;
        if complete {
            self.phase = (self.phase + 1) & 3;
        }
        self.start_column(leds);
        if !complete {
            return None;
        }
        let touch = self.piano;
        self.piano.changed = 0;
        Some((self.panel.snapshot(self.master, Some(&self.held)), touch))
    }

    fn start_column(&mut self, leds: &Leds) {
        let column = usize::from(self.panel.column);
        self.slot = match self.phase {
            1 if column == 0 => Slot::Master, // PA8 here is OCT−.
            3 if column == 0 && self.panel.battery_due() => Slot::Battery,
            0..=2 => {
                let row = [1, 2, 0][usize::from(self.phase)]; // PA7, PA8, then PA6 via PA10
                PianoKey::BY_ROW[row][column].map_or(Slot::Lamps, |key| Slot::Key {
                    key,
                    driven: self
                        .analog
                        .get(usize::from(key.index()))
                        .is_some_and(|k| k.drive_next),
                })
            }
            _ => Slot::Lamps,
        };
        let requested = leds.columns[column];
        let lamps = match self.slot {
            Slot::Key { key, .. } if !key.supports_pressure() => requested & !(1 << 2), // PA10
            Slot::Key { key, .. }
                if !matches!(self.analog[usize::from(key.index())].contact, Contact::Open) =>
            {
                0
            }
            _ => requested,
        };
        self.panel.light(lamps);
        self.slot.start();
    }
}

impl Drop for TouchControls {
    fn drop(&mut self) {
        self.slot.finish(); // Restore shared GPIO before Controls blanks the matrix.
    }
}

#[derive(Clone, Copy, Debug)]
enum Slot {
    Lamps,
    Master,
    Battery,
    Key { key: PianoKey, driven: bool },
}

#[allow(
    clippy::arithmetic_side_effects,
    reason = "Piano switches use row bits 2 through 4 on PA6 through PA8."
)]
impl Slot {
    fn start(self) {
        let channel = match self {
            Self::Lamps => return,
            Self::Master => 4,
            Self::Battery => BATTERY_CHANNEL,
            Self::Key { key, driven } => {
                let (_, row) = key.position();
                let probe = !key.supports_pressure();
                if probe {
                    // PA10 is normally a low LED output. Sense it through its pull-up.
                    set(PA + PU, 1 << 10);
                    set(PA + DIR, 1 << 10);
                }
                if driven || probe {
                    let pin = 1 << (row + 4);
                    level(PA + HD0, pin, driven); // HD0 only for analog, weakest for the probe.
                    clear(PA + HD, pin);
                    set(PA + OUT, pin);
                    clear(PA + DIR, pin);
                }
                match row {
                    3 => 0,
                    4 => 1,
                    _ => 2,
                }
            }
        };
        start_adc(channel, COLUMN_TIMING);
    }

    fn finish(self) -> Option<u16> {
        if matches!(self, Self::Lamps) {
            return None;
        }
        let value = adc_result();
        stop_adc();
        if let Self::Key { key, driven } = self {
            let probe = !key.supports_pressure();
            if driven || probe {
                let pin = 1 << (key.position().1 + 4);
                set(PA + DIR, pin);
                clear(PA + OUT, pin);
                clear(PA + HD0, pin);
            }
            if probe {
                clear(PA + PU, 1 << 10);
                clear(PA + DIR, 1 << 10);
            }
        }
        value
    }
}

#[derive(Clone, Copy, Debug, Default)]
enum Contact {
    #[default]
    Open,
    Landing {
        deepest: Option<u16>,
    },
    Down {
        rest: u16,
        filtered: u16,
        settling: u8,
    },
}

#[derive(Clone, Copy, Debug, Default)]
struct AnalogKey {
    contact: Contact,
    drive_next: bool,
    lifting: bool,
}

#[allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "ADC readings are 10 bits. The filtered pressure is in 0..=127 scaled by 16."
)]
impl AnalogKey {
    fn sample(
        &mut self,
        sample: Option<u16>,
        driven: bool,
        closed: bool,
        velocity: &mut u8,
        pressure: &mut u8,
    ) {
        // During an outage, two open contacts release the note without reusing stale ADC data.
        let value = match sample {
            Some(value) => value,
            None if closed => return,
            None => 1023,
        };
        if driven && sample.is_some() {
            self.drive_next = false;
            match &mut self.contact {
                Contact::Landing { deepest } => {
                    *deepest = Some(deepest.unwrap_or(value).min(value))
                }
                Contact::Down {
                    rest,
                    filtered,
                    settling,
                } if closed && !self.lifting => {
                    // The pill continues to compress just after the strike. Track its resting
                    // contact for the first 250 ms so seating alone cannot sustain pressure.
                    if value > *rest {
                        *rest = value;
                    } else if *settling != 0 {
                        *rest -= (3 * (*rest - value) + 4) / 8;
                    }
                    *settling = settling.saturating_sub(1);
                    // Remove the diode drop before comparing resistances.
                    let resistance = rest.saturating_sub(185).max(1);
                    let drop = u32::from(*rest - value);
                    let target = (400 * drop / u32::from(resistance))
                        .saturating_sub(10)
                        .min(127) as u16;
                    *filtered = (3 * *filtered + 16 * target + 2) / 4;
                    *pressure = ((*filtered + 8) / 16) as u8;
                }
                _ => {}
            }
            return;
        }

        match self.contact {
            Contact::Open if value < 900 => self.contact = Contact::Landing { deepest: None },
            Contact::Landing { .. } if value >= 900 => self.contact = Contact::Open,
            Contact::Landing {
                deepest: Some(rest),
            } if value < 400 => {
                *velocity = strike_velocity(rest);
                self.contact = Contact::Down {
                    rest,
                    filtered: 0,
                    settling: 28, // one driven reading per 8.8 ms
                };
            }
            Contact::Down { .. } => {
                if value >= 600 && self.lifting {
                    self.contact = Contact::Open;
                    *velocity = 0;
                    *pressure = 0;
                }
                self.lifting = value >= 600 && *velocity != 0;
            }
            _ => {}
        }
        self.drive_next = !matches!(self.contact, Contact::Open);
    }
}

// fm1-base's default driven curve (floor 530, ceiling 896, diode 185, exponent 0.70).
// Interpolate at 16-count intervals to keep floating point out of the scan interrupt.
#[allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "The descending 7-bit curve is interpolated with a four-bit fraction in u16."
)]
fn strike_velocity(reading: u16) -> u8 {
    const CURVE: [u8; 24] = [
        127, 122, 117, 111, 106, 101, 96, 91, 86, 81, 76, 71, 67, 62, 57, 52, 47, 42, 37, 31, 26,
        19, 12, 1,
    ];
    if reading <= 530 {
        return 127;
    }
    if reading >= 896 {
        return 1;
    }
    let x = reading.saturating_sub(528);
    let i = usize::from(x / 16);
    let a = u16::from(CURVE[i]);
    let b = u16::from(CURVE[i + 1]);
    (a - ((a - b) * (x % 16) + 8) / 16) as u8
}

#[derive(Clone, Copy, Debug, Default)]
struct Probe {
    baseline: Option<u16>, // open reading in sixteenths of an ADC count
    peak: Option<u16>,
    lifting: bool,
}

#[allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "The weighted average of 10-bit ADC readings, scaled by 16, fits in u16."
)]
impl Probe {
    fn sample(&mut self, sample: Option<u16>, closed: bool, velocity: &mut u8) {
        if !closed {
            self.peak = None;
            if *velocity != 0 && !self.lifting {
                self.lifting = true;
                return;
            }
            *velocity = 0;
            if let Some(value) = sample {
                let base = self.baseline.get_or_insert(value * 16);
                *base = ((7 * u32::from(*base) + 16 * u32::from(value) + 4) / 8) as u16;
            }
        } else {
            self.lifting = false;
            if *velocity == 0
                && let Some(value) = sample
            {
                let rise = value.saturating_sub(self.baseline.unwrap_or(0) / 16);
                if let Some(first) = self.peak.replace(rise) {
                    // Two closed probes reject bounce and include the landing. A key already
                    // held at startup has no open reference until it is released once.
                    const CURVE: [u8; 44] = [
                        1, 1, 1, 1, 1, 2, 2, 3, 3, 4, 5, 6, 7, 8, 10, 11, 13, 15, 17, 19, 21, 24,
                        26, 29, 32, 35, 39, 42, 46, 50, 54, 58, 63, 68, 73, 78, 83, 89, 95, 101,
                        107, 113, 120, 127,
                    ];
                    *velocity = self
                        .baseline
                        .map_or(64, |_| CURVE[usize::from(first.max(rise).min(43))]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
