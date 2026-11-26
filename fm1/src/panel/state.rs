// SPDX-License-Identifier: MPL-2.0
// Piano-key wiring follows the Felucca FM-1 board map.
//! Values exchanged between the panel scanner and application code.

use super::{Button, Encoder, PlayColor};

/// A physical piano key, in chromatic order from the leftmost F to the rightmost G.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PianoKey(u8);

impl PianoKey {
    /// Number of keys on the FM-1.
    pub const COUNT: u8 = 27;

    pub(super) const PRESSURE_KEYS: usize = 20;

    /// A zero-based physical key index, independent of transpose and octave settings.
    #[must_use]
    pub const fn new(index: u8) -> Option<Self> {
        if index < Self::COUNT {
            Some(Self(index))
        } else {
            None
        }
    }

    /// Zero-based index from the left.
    #[must_use]
    pub const fn index(self) -> u8 {
        self.0
    }

    /// Iterate from left to right.
    pub fn all() -> impl ExactSizeIterator<Item = Self> + DoubleEndedIterator {
        (0..Self::COUNT).map(Self)
    }

    const PINS: [(usize, u8); Self::COUNT as usize] = [
        (3, 4),
        (2, 4),
        (5, 4),
        (4, 4),
        (7, 4),
        (6, 4),
        (8, 4),
        (9, 4),
        (10, 4),
        (0, 3),
        (1, 3),
        (2, 3),
        (3, 3),
        (4, 3),
        (5, 3),
        (6, 3),
        (7, 3),
        (8, 3),
        (9, 3),
        (10, 3),
        (0, 2),
        (1, 2),
        (2, 2),
        (3, 2),
        (4, 2),
        (6, 2),
        (5, 2),
    ];

    pub(super) fn position(self) -> (usize, u8) {
        Self::PINS[usize::from(self.0)]
    }

    // Invert the same board map for the ADC scan, which visits one row and column at a time.
    pub(super) const BY_ROW: [[Option<Self>; 11]; 3] = {
        let mut rows = [[None; 11]; 3];
        let mut i = 0;
        while i < Self::COUNT {
            let (column, row) = Self::PINS[i as usize];
            rows[(row - 2) as usize][column] = Some(Self(i));
            i += 1;
        }
        rows
    };
    pub(super) const ROW_MASKS: [u8; 11] = {
        let mut masks = [0; 11];
        let mut i = 0;
        while i < Self::PINS.len() {
            let (column, row) = Self::PINS[i];
            masks[column] |= 1 << row;
            i += 1;
        }
        masks
    };

    /// Whether this key has an ADC row for polyphonic aftertouch (F3 through C5).
    #[must_use]
    pub const fn supports_pressure(self) -> bool {
        (self.0 as usize) < Self::PRESSURE_KEYS
    }
}

/// A panel switch and the position of its associated indicator.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Switch {
    /// A named front-panel button.
    Button(Button),
    /// A piano key.
    Piano(PianoKey),
}

impl From<Button> for Switch {
    fn from(button: Button) -> Self {
        Self::Button(button)
    }
}
impl From<PianoKey> for Switch {
    fn from(key: PianoKey) -> Self {
        Self::Piano(key)
    }
}
impl Switch {
    fn position(self) -> (usize, u8) {
        match self {
            Self::Button(b) => b.position(),
            Self::Piano(k) => k.position(),
        }
    }

    fn active(self, columns: &[u8; 11]) -> bool {
        let (col, row) = self.position();
        columns[col] & (1 << row) != 0
    }
}

/// Absolute 10-bit position of the MASTER potentiometer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MasterLevel(u16);
impl MasterLevel {
    /// Lowest representable position.
    pub const MIN: Self = Self(0);
    /// Highest representable position.
    pub const MAX: Self = Self(1023);
    pub(super) fn from_adc(raw: u32) -> Self {
        Self((raw & 0x3ff) as u16)
    }
    /// Unfiltered ADC reading in `0..=1023`.
    #[must_use]
    pub const fn raw(self) -> u16 {
        self.0
    }
}

/// The two separately wired LEDs under PLAY/STOP.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlayLedState {
    /// Both channels off.
    #[default]
    Off,
    /// Green PLAY channel.
    Green,
    /// Warm STOP channel, called [`PlayColor::Amber`] on the fixed-column handle.
    Red,
    /// Both channels lit on their respective scan columns.
    Both,
}
impl From<PlayColor> for PlayLedState {
    fn from(color: PlayColor) -> Self {
        match color {
            PlayColor::Amber => Self::Red,
            PlayColor::Green => Self::Green,
        }
    }
}

/// Desired indicator states. Changes take effect as the hardware scans the next sweep.
/// An application can construct or copy this value independently of the hardware.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Leds {
    pub(super) columns: [u8; 11],
}
impl Leds {
    /// Set the lamp at a button or piano key. For PLAY this sets only the warm STOP channel.
    pub fn set(&mut self, switch: impl Into<Switch>, on: bool) {
        let (column, row) = switch.into().position();
        self.set_line(column, row, on);
    }
    /// Toggle the primary lamp at a button or piano key.
    pub fn toggle(&mut self, switch: impl Into<Switch>) {
        let (column, row) = switch.into().position();
        self.columns[column] ^= 1 << row;
    }

    /// Test the requested state of the button's or key's primary lamp.
    #[must_use]
    pub fn is_on(&self, switch: impl Into<Switch>) -> bool {
        switch.into().active(&self.columns)
    }

    /// Set both PLAY/STOP channels as a single state.
    pub fn set_play(&mut self, state: PlayLedState) {
        self.set(
            Button::Play,
            matches!(state, PlayLedState::Red | PlayLedState::Both),
        );
        self.set_line(
            usize::from(PlayColor::Green.column().0),
            1,
            matches!(state, PlayLedState::Green | PlayLedState::Both),
        );
    }
    /// Requested PLAY/STOP state.
    #[must_use]
    pub fn play(&self) -> PlayLedState {
        let green = self.columns[usize::from(PlayColor::Green.column().0)] & 2 != 0;
        match (self.is_on(Button::Play), green) {
            (false, false) => PlayLedState::Off,
            (false, true) => PlayLedState::Green,
            (true, false) => PlayLedState::Red,
            (true, true) => PlayLedState::Both,
        }
    }
    fn set_line(&mut self, column: usize, row: u8, on: bool) {
        let mask = 1 << row;
        if on {
            self.columns[column] |= mask;
        } else {
            self.columns[column] &= !mask;
        }
    }
}

/// One input change within a complete matrix sweep.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    /// A switch debounced to closed.
    Pressed(Switch),
    /// A switch debounced to open.
    Released(Switch),
    /// Signed whole detents. Clockwise is positive.
    Turned {
        /// Encoder that moved.
        encoder: Encoder,
        /// Relative motion during this sweep.
        detents: i8,
    },
    /// A new absolute MASTER reading, or `None` when the ADC times out.
    MasterChanged(Option<MasterLevel>),
}

/// An owned snapshot of one complete sweep. Button levels persist across sweeps.
/// Edges and signed encoder deltas belong to this sweep alone.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Scan {
    pub(super) held: [u8; 11],
    pub(super) changed: [u8; 11],
    pub(super) detents: [i8; 7],
    pub(super) master: Option<MasterLevel>,
    pub(super) battery_raw: Option<u16>,
    pub(super) battery_low: Option<bool>,
    pub(super) master_changed: bool,
}
impl Scan {
    /// Latest 10-bit PB1 battery-divider sample. Returns `None` before sampling
    /// is enabled, before the first result, or if the latest conversion missed
    /// its slot. Zero is a valid reading. Values are uncalibrated ADC counts.
    #[must_use]
    pub const fn battery_raw(&self) -> Option<u16> {
        self.battery_raw
    }

    /// Low-battery warning from the PB1 divider. Three consecutive samples
    /// below 561 counts indicate low at startup. Once established, the warning
    /// turns on below 557 and clears at 565 or above, each for three samples.
    /// External power and charger flags have no part in this decision.
    ///
    /// Returns `None` until a state is confirmed or if sampling a healthy
    /// battery times out. A confirmed warning survives missing samples.
    #[must_use]
    pub const fn battery_low(&self) -> Option<bool> {
        self.battery_low
    }

    /// Current debounced switch level. Several switches can be held together.
    #[must_use]
    pub fn is_down(&self, switch: impl Into<Switch>) -> bool {
        switch.into().active(&self.held)
    }
    /// True on the sweep that accepted the press.
    #[must_use]
    pub fn pressed(&self, switch: impl Into<Switch>) -> bool {
        let key = switch.into();
        key.active(&self.changed) && key.active(&self.held)
    }
    /// True on the sweep that accepted the release.
    #[must_use]
    pub fn released(&self, switch: impl Into<Switch>) -> bool {
        let key = switch.into();
        key.active(&self.changed) && !key.active(&self.held)
    }
    /// Whole detents this sweep. Add these to an application parameter or position.
    #[must_use]
    pub fn encoder_delta(&self, encoder: Encoder) -> i8 {
        self.detents[encoder as usize]
    }
    /// Latest absolute MASTER reading, or `None` until ready or after a timeout.
    #[must_use]
    pub fn master(&self) -> Option<MasterLevel> {
        self.master
    }
    /// Iterate over changes without allocation. Simultaneous changes are reported in button,
    /// piano key, encoder, then MASTER order. Reading a snapshot never consumes its events.
    pub fn events(&self) -> impl Iterator<Item = Event> + '_ {
        let switches = Button::ALL
            .into_iter()
            .map(Switch::Button)
            .chain(PianoKey::all().map(Switch::Piano));
        let edges = switches.filter_map(|key| {
            if !key.active(&self.changed) {
                None
            } else if key.active(&self.held) {
                Some(Event::Pressed(key))
            } else {
                Some(Event::Released(key))
            }
        });
        let turns = Encoder::ALL.into_iter().filter_map(|encoder| {
            let detents = self.encoder_delta(encoder);
            (detents != 0).then_some(Event::Turned { encoder, detents })
        });
        edges.chain(turns).chain(
            self.master_changed
                .then_some(Event::MasterChanged(self.master)),
        )
    }
}
