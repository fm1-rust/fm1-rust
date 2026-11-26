// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
// Copyright (C) 2026 Charles Vestal
// Panel wiring, detent recovery and SARADC setup follow the Felucca FM-1 BSP.
//! Electrical positions follow the Felucca FM-1 board map and the panel names
//! in fm1-base. Contacts are (column, input bit) in Matrix::sample order.

use super::{Column, LedRow, Leds, MasterLevel, Matrix, PianoKey, Scan, select};
use crate::{registers::*, time::cycles};

/// Non-keyboard buttons in the board firmware's button-ID order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Button {
    /// OCT−
    OctaveDown,
    /// OCT+
    OctaveUp,
    /// FX
    Fx,
    /// SEL
    Sel,
    /// ENV
    Env,
    /// LFO
    Lfo,
    /// EDIT
    Edit,
    /// GLO
    Glo,
    /// HOME
    Home,
    /// SAVE
    Save,
    /// ARP
    Arp,
    /// SEQ
    Seq,
    /// PLAY/STOP
    Play,
    /// REC
    Rec,
}

impl Button {
    /// All fourteen buttons. The corresponding LED shares its switch position.
    pub const ALL: [Self; 14] = [
        Self::OctaveDown,
        Self::OctaveUp,
        Self::Fx,
        Self::Sel,
        Self::Env,
        Self::Lfo,
        Self::Edit,
        Self::Glo,
        Self::Home,
        Self::Save,
        Self::Arp,
        Self::Seq,
        Self::Play,
        Self::Rec,
    ];

    pub(super) const fn position(self) -> (usize, u8) {
        match self {
            Self::OctaveDown => (0, 4),
            Self::OctaveUp => (1, 4),
            Self::Fx => (6, 1),
            Self::Sel => (4, 1),
            Self::Env => (2, 1),
            Self::Lfo => (0, 1),
            Self::Edit => (9, 2),
            Self::Glo => (8, 2),
            Self::Home => (7, 1),
            Self::Save => (5, 1),
            Self::Arp => (3, 1),
            Self::Seq => (1, 1),
            Self::Play => (10, 2),
            Self::Rec => (7, 2),
        }
    }
}

/// Rotary encoders. MASTER is an absolute potentiometer, read with [`Scan::master`].
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Encoder {
    /// SELECT
    Select,
    /// PRESETS
    Presets,
    /// ALGORITHM
    Algorithm,
    /// Numbered knob 1.
    Knob1,
    /// Numbered knob 2.
    Knob2,
    /// Numbered knob 3.
    Knob3,
    /// Numbered knob 4.
    Knob4,
}

impl Encoder {
    /// All seven rotary encoders in panel label order.
    pub const ALL: [Self; 7] = [
        Self::Select,
        Self::Presets,
        Self::Algorithm,
        Self::Knob1,
        Self::Knob2,
        Self::Knob3,
        Self::Knob4,
    ];

    // A and B occupy adjacent columns on the same input row.
    fn phase(self, columns: &[u8; 11]) -> u8 {
        let (a, b, row) = match self {
            Self::Select => (0, 1, 0),
            Self::Presets => (0, 1, 5),
            Self::Algorithm => (2, 3, 0),
            Self::Knob1 => (8, 9, 1),
            Self::Knob2 => (8, 9, 0),
            Self::Knob3 => (6, 7, 0),
            Self::Knob4 => (4, 5, 0),
        };
        ((columns[a] >> row & 1) << 1) | (columns[b] >> row & 1)
    }
}

#[derive(Debug, Default)]
struct Decoder {
    previous: Option<u8>,
    rest: u8,
    still: u16,
    since_click: u16,
    direction: i8,
    quarters: i8,
}

impl Decoder {
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "Signs are -1..=1 and quarter counts are widened from i8."
    )]
    fn sample(&mut self, phase: u8) -> i8 {
        self.since_click = self.since_click.saturating_add(1);
        let Some(last) = self.previous.replace(phase) else {
            self.rest = phase;
            return 0;
        };
        if last == phase {
            self.still = self.still.saturating_add(1);
            // If powered on midway through a click, relearn the parked phase after ~1 s.
            if self.still == 900 && phase != self.rest {
                self.rest = phase;
                self.quarters = 0;
            }
            return 0;
        }
        self.still = 0;
        let delta = match (last, phase) {
            (0, 1) | (1, 3) | (3, 2) | (2, 0) => 1,
            (1, 0) | (3, 1) | (2, 3) | (0, 2) => -1,
            // A fast turn can skip a phase. Infer it only during a turn or shortly after a click.
            _ if self.quarters != 0 => self.quarters.signum() * 2,
            _ if self.since_click <= 20 => self.direction * 2,
            _ => 0,
        };
        self.quarters = self.quarters.saturating_add(delta);
        if phase != self.rest {
            return 0;
        }
        let q = core::mem::take(&mut self.quarters);
        // Widen before rounding: even i8::MIN represents only 32 detents.
        let magnitude = (u16::from(q.unsigned_abs()) + 2) / 4;
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_possible_wrap,
            reason = "A full i8 of quarter steps yields at most 32 detents."
        )]
        let clicks = magnitude as i8 * q.signum();
        if clicks != 0 {
            self.direction = clicks.signum();
            self.since_click = 0;
        }
        clicks
    }
}

#[derive(Debug, Default)]
struct Inputs {
    held: [u8; 11],
    history: [[u8; 8]; 11],
    next: usize,
    encoders: [Decoder; 7],
}
impl Inputs {
    fn sample(&mut self, columns: &[u8; 11], piano: Option<&[u8; 11]>) -> Scan {
        let mut scan = Scan::default();
        let prev = self.next.wrapping_sub(1) & 7;
        for (col, &raw) in columns.iter().enumerate() {
            // Exclude encoder contacts. Rows 2..=4 are populated in all columns.
            let raw = raw & if col < 8 { 0b1_1110 } else { 0b1_1100 };
            let history = &mut self.history[col];
            let closed_twice = raw & history[prev];
            history[self.next] = raw;
            let recently_closed = history.iter().fold(0, |bits, sample| bits | sample);
            let mut held = (self.held[col] | closed_twice) & recently_closed;
            if let Some(keys) = piano {
                held = (held & !PianoKey::ROW_MASKS[col]) | keys[col];
            }
            scan.changed[col] = self.held[col] ^ held;
            self.held[col] = held;
        }
        self.next = self.next.wrapping_add(1) & 7;
        scan.held = self.held;
        for enc in Encoder::ALL {
            scan.detents[enc as usize] = self.encoders[enc as usize].sample(enc.phase(columns));
        }
        scan
    }
}

/// A single owner for matrix scanning, multiplexed LEDs and the MASTER ADC.
/// No allocation or interrupt handler is required.
#[derive(Debug)]
pub struct Controls {
    matrix: Matrix,
    pub(super) column: u8,
    raw: [u8; 11],
    inputs: Inputs,
    master: Option<MasterLevel>,
    conversion_started: u32,
    // None keeps PB1 and the original MASTER schedule untouched.
    battery_started: Option<u32>,
    pub(super) battery: Battery,
    battery_pending: bool,
}

impl Controls {
    pub(super) fn new(mut matrix: Matrix) -> Self {
        const PIN: u32 = 1 << 6;
        set(PB + DIR, PIN);
        clear(PB + DIE, PIN);
        clear(PB + PU, PIN);
        clear(PB + PD, PIN);
        matrix.select(Column(0));
        start_adc(4, (15 << 12) | 6);
        Self {
            matrix,
            column: 0,
            raw: [0; 11],
            inputs: Inputs::default(),
            master: None,
            conversion_started: cycles(),
            battery_started: None,
            battery: Battery::default(),
            battery_pending: false,
        }
    }

    /// Recommended column scan frequency. Eleven calls make one complete sweep.
    pub const SCAN_HZ: u32 = 10_000;

    /// Sample the battery divider about ten times per second for
    /// [`Scan::battery_raw`] and [`Scan::battery_low`]. PB1 stays a high-impedance
    /// analog input. Calling this again preserves the current sampling interval.
    pub fn enable_battery(&mut self) {
        if self.battery_started.is_none() {
            // Holding PB1 low can reset the board. Never drive it or enable a pull.
            const PIN: u32 = 1 << 1;
            set(PB + DIR, PIN);
            clear(PB + PD, PIN);
            clear(PB + PU, PIN);
            clear(PB + DIE, PIN);
            self.battery_started = Some(cycles().wrapping_sub(BATTERY_INTERVAL));
        }
    }

    pub(super) fn battery_due(&mut self) -> bool {
        let Some(started) = &mut self.battery_started else {
            return false;
        };
        let now = cycles();
        if now.wrapping_sub(*started) < BATTERY_INTERVAL {
            return false;
        }
        *started = now;
        true
    }

    /// Sample the current column and drive the next column from `leds`.
    /// Call once per 100 µs. Every eleventh call returns a coherent input snapshot.
    /// A press needs two closed sweeps and a release needs eight open sweeps.
    pub fn scan(&mut self, leds: &Leds) -> Option<Scan> {
        if self.battery_pending {
            self.battery.record(adc_result());
            stop_adc();
            start_adc(4, (15 << 12) | 6);
            self.conversion_started = cycles();
            self.battery_pending = false;
        }
        self.advance();
        self.light(leds.columns[usize::from(self.column)]);
        if self.column != 0 {
            return None;
        }
        let master = self.poll_master();
        Some(self.snapshot(master, None))
    }

    // Blank and read the current column, then select the next with its lamps still off.
    pub(super) fn advance(&mut self) -> u8 {
        let rows = self.matrix.sample_and_blank();
        self.raw[usize::from(self.column)] = rows;
        self.column = self.column.wrapping_add(1);
        if self.column == Column::COUNT {
            self.column = 0;
        }
        select(Column(self.column));
        rows
    }

    pub(super) fn light(&mut self, lamps: u8) {
        for (mask, row) in [
            (2, LedRow::Pa5),
            (4, LedRow::Pa6),
            (8, LedRow::Pa7),
            (16, LedRow::Pa8),
        ] {
            if lamps & mask != 0 {
                self.matrix.set_led(row, true);
            }
        }
    }

    pub(super) fn snapshot(
        &mut self,
        master: Option<MasterLevel>,
        piano: Option<&[u8; 11]>,
    ) -> Scan {
        let mut scan = self.inputs.sample(&self.raw, piano);
        scan.master = master;
        scan.battery_raw = self.battery.raw;
        scan.battery_low = self.battery.low;
        scan.master_changed = master != self.master;
        self.master = master;
        scan
    }

    fn poll_master(&mut self) -> Option<MasterLevel> {
        let now = cycles();
        let value = if let Some(raw) = adc_result() {
            Some(MasterLevel::from_adc(u32::from(raw)))
        } else if now.wrapping_sub(self.conversion_started) >= crate::time::Timers::CRYSTAL_HZ / 100
        {
            None
        } else {
            return self.master;
        };
        stop_adc();
        if self.battery_due() {
            // Borrow the first column interval; MASTER resumes on the next tick.
            start_adc(BATTERY_CHANNEL, COLUMN_TIMING);
            self.battery_pending = true;
        } else {
            start_adc(4, (15 << 12) | 6);
            self.conversion_started = now;
        }
        value
    }
}

impl Drop for Controls {
    fn drop(&mut self) {
        // A lit multiplexed row would otherwise remain driven continuously.
        self.matrix.select(Column::PLAY);
        stop_adc();
    }
}

// One SARADC serves the key rows, MASTER (PB6) and the battery divider (PB1).
pub(super) const BATTERY_CHANNEL: u32 = 3;
pub(super) const COLUMN_TIMING: u32 = (2 << 12) | 7;
const BATTERY_INTERVAL: u32 = crate::time::Timers::CRYSTAL_HZ / 10;

#[derive(Debug, Default)]
pub(super) struct Battery {
    raw: Option<u16>,
    previous: Option<u16>,
    low: Option<bool>,
}

impl Battery {
    // The one-to-two-bar boundary and hysteresis come from fm1-base.
    const TWO_BARS: u16 = 561;
    const HYSTERESIS: u16 = 4;

    // Called exactly once per attempted battery conversion, including timeouts.
    pub(super) fn record(&mut self, sample: Option<u16>) {
        if let (Some(a), Some(b), Some(c)) = (self.previous, self.raw, sample) {
            let threshold = match self.low {
                None => Self::TWO_BARS,
                Some(false) => Self::TWO_BARS - Self::HYSTERESIS,
                Some(true) => Self::TWO_BARS + Self::HYSTERESIS,
            };
            if a.max(b).max(c) < threshold {
                self.low = Some(true);
            } else if a.min(b).min(c) >= threshold {
                self.low = Some(false);
            }
        } else if sample.is_none() && self.low == Some(false) {
            // An unreadable ADC cannot confirm a healthy battery. A low
            // warning persists until three new readings confirm recovery.
            self.low = None;
        }
        self.previous = self.raw;
        self.raw = sample;
    }
}

const ADC_CON: usize = 0x13100;
pub(super) fn adc_result() -> Option<u16> {
    (read(ADC_CON) & (1 << 7) != 0).then(|| (read(ADC_CON + 4) & 0x3ff) as u16)
}
pub(super) fn stop_adc() {
    write(ADC_CON, 1 << 6);
    write(ADC_CON, 0);
}
pub(super) fn start_adc(channel: u32, timing: u32) {
    write(ADC_CON, 0);
    const WLA_CON0: usize = 0x11900;
    if read(WLA_CON0) & (1 << 14) != 0 {
        clear(WLA_CON0, 1 << 14);
    }
    write(ADC_CON, timing | (channel << 8) | (1 << 6) | (1 << 3));
    set(ADC_CON, 1 << 4);
    set(ADC_CON, 1 << 6);
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::panel::{Event, PianoKey, PlayLedState, Switch};

    #[test]
    fn detents_bounce_reverse_and_recover() {
        let mut d = Decoder::default();
        let mut movement =
            |phases: &[u8]| phases.iter().map(|&p| i32::from(d.sample(p))).sum::<i32>();
        assert_eq!(movement(&[3, 2, 3, 2, 0, 1, 3]), 1);
        assert_eq!(movement(&[1, 0, 2, 3]), -1);
        assert_eq!(movement(&[3; 21]), 0);
        assert_eq!(movement(&[0, 3, 0, 3]), 0);
        assert_eq!(movement(&[1, 0, 2, 3]), -1);
    }

    #[test]
    fn skipped_phase_and_power_on_between_detents() {
        let mut d = Decoder::default();
        for phase in [1, 3, 2, 0] {
            assert_eq!(d.sample(phase), 0);
        }
        for _ in 0..900 {
            assert_eq!(d.sample(0), 0);
        }
        // A subsequent click missed phase 3, then the next one missed phase 1.
        for (phase, expected) in [(1, 0), (2, 0), (0, 1), (3, 0), (2, 0), (0, 1)] {
            assert_eq!(d.sample(phase), expected);
        }
    }

    #[test]
    fn simultaneous_edges_repeatable_events_and_bounce() {
        let mut inputs = Inputs::default();
        let mut rows = [0; 11];
        let piano = PianoKey::new(0).expect("lowest key");
        rows[3] = 16;
        rows[7] = 6; // HOME + REC
        assert_eq!(inputs.sample(&rows, None).events().count(), 0);
        let scan = inputs.sample(&rows, None);
        let events = [
            Event::Pressed(Button::Home.into()),
            Event::Pressed(Button::Rec.into()),
            Event::Pressed(piano.into()),
        ];
        assert!(scan.pressed(Button::Home) && scan.is_down(piano));
        assert!(scan.events().eq(events));
        assert!(scan.events().eq(events));
        assert_eq!(inputs.sample(&rows, None).events().count(), 0);
        rows[7] = 2;
        inputs.sample(&rows, None);
        rows[7] = 6;
        inputs.sample(&rows, None);
        rows[7] = 2;
        for _ in 0..7 {
            assert!(inputs.sample(&rows, None).is_down(Button::Rec));
        }
        let scan = inputs.sample(&rows, None);
        assert!(scan.released(Button::Rec) && scan.is_down(Button::Home));
        assert!(
            scan.events()
                .eq([Event::Released(Switch::Button(Button::Rec))])
        );
    }

    #[test]
    fn each_key_has_its_own_light_and_play_has_two() {
        let mut lights = Leds::default();
        for key in Button::ALL
            .into_iter()
            .map(Switch::Button)
            .chain(PianoKey::all().map(Switch::Piano))
        {
            assert!(!lights.is_on(key));
            lights.set(key, true);
            assert!(lights.is_on(key));
        }
        assert!(PianoKey::new(27).is_none());
        let before = lights;
        for state in [
            PlayLedState::Green,
            PlayLedState::Both,
            PlayLedState::Off,
            PlayLedState::Red,
        ] {
            lights.set_play(state);
            assert_eq!(lights.play(), state);
            assert_eq!(
                lights.columns[8] & 2 != 0,
                matches!(state, PlayLedState::Green | PlayLedState::Both)
            );
            assert_eq!(
                lights.columns[10] & 4 != 0,
                matches!(state, PlayLedState::Red | PlayLedState::Both)
            );
        }
        assert_eq!(lights, before);
    }

    #[test]
    fn master_samples_times_out_and_recovers() {
        const POT: u32 = 1 << 6;
        const BATTERY: u32 = 1 << 1;
        for offset in [DIE, PU, PD] {
            write(PB + offset, POT | BATTERY);
        }
        let mut panel = super::super::Play::init().into_controls();
        let mut sweep = || {
            let leds = Leds::default();
            for _ in 1..Column::COUNT {
                assert!(panel.scan(&leds).is_none());
            }
            panel.scan(&leds).expect("complete sweep")
        };
        assert_eq!(sweep().master(), None);
        assert_ne!(read(PB + DIR) & POT, 0);
        for offset in [DIE, PU, PD] {
            assert_eq!(read(PB + offset) & (POT | BATTERY), BATTERY);
        }
        assert_eq!(read(ADC_CON) >> 8 & 15, 4);
        write(ADC_CON + 4, 0xffff_feab);
        set(ADC_CON, 1 << 7);
        assert_eq!(sweep().master().map(MasterLevel::raw), Some(0x2ab));
        // The register fixture advances its 24 MHz timer one millisecond per read.
        assert!(sweep().events().any(|e| e == Event::MasterChanged(None)));
        write(ADC_CON + 4, 1023);
        set(ADC_CON, 1 << 7);
        assert!(
            sweep()
                .events()
                .any(|e| e == Event::MasterChanged(Some(MasterLevel::MAX)))
        );
        set(ADC_CON, 1 << 7);
        assert!(
            !sweep()
                .events()
                .any(|e| matches!(e, Event::MasterChanged(_)))
        );
        drop(panel);
        assert_eq!(read(ADC_CON), 0);
    }

    #[test]
    fn battery_uses_one_column_and_keeps_master_events_separate() {
        const PIN: u32 = 1 << 1;
        let mut controls = super::super::Play::init().into_controls();
        for register in [PB + OUT, PB + DIE, PB + PD, PB + PU] {
            set(register, PIN);
        }
        controls.enable_battery();
        assert_ne!(read(PB + DIR) & PIN, 0);
        assert_ne!(read(PB + OUT) & PIN, 0);
        for register in [PB + DIE, PB + PD, PB + PU] {
            assert_eq!(read(register) & PIN, 0);
        }
        let attempts = [
            (Some(0), None),
            (None, None),
            (Some(629), None),
            (Some(629), None),
            (Some(629), Some(false)),
            (Some(556), Some(false)),
            (Some(556), Some(false)),
            (Some(556), Some(true)),
        ];
        let mut count = 0;
        let mut expected = (None, None);
        let mut master_changes = 0;
        for _ in 0..5000 {
            let adc = read(ADC_CON);
            let value = match (adc >> 8) & 15 {
                BATTERY_CHANNEL => {
                    assert_eq!(controls.column, 0);
                    assert_eq!(adc & 0xf007, COLUMN_TIMING);
                    expected = attempts[count];
                    count += 1;
                    expected.0
                }
                4 => Some(789),
                _ => unreachable!("digital scanner channel"),
            };
            if let Some(value) = value {
                write(ADC_CON + 4, u32::from(value));
                set(ADC_CON, 1 << 7);
            }
            if let Some(scan) = controls.scan(&Leds::default()) {
                assert_eq!((scan.battery_raw(), scan.battery_low()), expected);
                assert_eq!(scan.master().map(MasterLevel::raw), Some(789));
                master_changes += scan
                    .events()
                    .filter(|e| matches!(e, Event::MasterChanged(_)))
                    .count();
                if count == attempts.len() {
                    break;
                }
            }
        }
        assert_eq!(count, attempts.len());
        assert_eq!(master_changes, 1);
    }

    #[test]
    fn low_battery_thresholds_hold_through_noise_and_missed_conversions() {
        let mut battery = Battery::default();
        let mut check = |raw, times, expected| {
            for _ in 0..times {
                battery.record(raw);
            }
            assert_eq!(battery.low, expected, "after {times} readings of {raw:?}");
        };
        check(Some(560), 2, None);
        check(Some(560), 1, Some(true));
        check(Some(564), 3, Some(true));
        check(Some(565), 2, Some(true));
        check(None, 1, Some(true));
        check(Some(565), 2, Some(true));
        check(Some(565), 1, Some(false));
        check(Some(557), 3, Some(false));
        check(Some(0), 1, Some(false));
        check(Some(629), 1, Some(false));
        check(Some(556), 2, Some(false));
        check(None, 1, None);
        check(Some(561), 2, None);
        check(Some(561), 1, Some(false));
        check(Some(0), 3, Some(true));
    }
}
