use crate::pitch::{FIRST_NOTE, midi_note};
use fm1_rticx::bsp::{
    panel::PianoKey,
    usb_midi::{EventPacket, MAX_EVENTS},
};

pub(super) type Velocities = [u8; PianoKey::COUNT as usize];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct Notes {
    pub(super) held: u32,
    pub(super) velocities: Velocities,
    pub(super) pressures: Velocities,
}

impl Notes {
    /// Pressure increases the gain of an already sounding note.
    #[allow(
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        reason = "Velocity and pressure are seven-bit MIDI data. The scaled gain is at most 127."
    )]
    fn level(&self, i: usize) -> u8 {
        let velocity = u16::from(self.velocities[i]);
        (velocity + ((127 - velocity) * u16::from(self.pressures[i]) / 127)) as u8
    }

    pub(super) fn levels(&self, remote: &Self) -> Velocities {
        core::array::from_fn(|i| {
            if self.held & (1 << i) != 0 {
                self.level(i)
            } else {
                remote.level(i)
            }
        })
    }

    /// Update host-owned notes. Return a positive note-on for the LCD history.
    pub(super) fn apply(&mut self, event: EventPacket) -> Option<PianoKey> {
        let [cin, status, number, value] = event;
        if (cin, status) == (0x0b, 0xb0) && matches!(number, 120 | 123) && value < 128 {
            *self = Self::default();
            return None;
        }
        if !matches!((cin, status), (8, 0x80) | (9, 0x90) | (10, 0xa0)) || value >= 128 {
            return None;
        }
        let key = PianoKey::new(number.checked_sub(FIRST_NOTE)?)?;
        let bit = 1 << key.index();
        let i = usize::from(key.index());
        if status == 0xa0 {
            if self.held & bit != 0 {
                self.pressures[i] = value;
            }
            return None;
        }
        self.pressures[i] = 0;
        let slot = &mut self.velocities[i];
        if status == 0x80 || value == 0 {
            self.held &= !bit;
            *slot = 0;
            None
        } else {
            self.held |= bit;
            *slot = value;
            Some(key)
        }
    }
}

const CAPACITY: usize = 64;

/// Queue physical key edges in order while a host is slow. If it stops reading
/// for long enough to fill the queue, reconcile remaining keys as space returns.
pub(super) struct Output {
    events: [EventPacket; CAPACITY],
    head: usize,
    len: usize,
    planned: u32,
    pressure_sent: Velocities,
    next_pressure: usize,
}

impl Output {
    pub(super) fn new() -> Self {
        let mut events = [[0; 4]; CAPACITY];
        events[0] = [0x0b, 0xb0, 123, 0]; // Release any notes the previous session left at the host.
        Self {
            events,
            head: 0,
            len: 1,
            planned: 0,
            pressure_sent: [0; PianoKey::COUNT as usize],
            next_pressure: 0,
        }
    }

    #[allow(
        clippy::arithmetic_side_effects,
        reason = "Queue indices and lengths are bounded by CAPACITY."
    )]
    pub(super) fn observe(&mut self, notes: &Notes) {
        let held = notes.held;
        if held == self.planned {
            return;
        }
        for (down, mut keys) in [(false, self.planned & !held), (true, held & !self.planned)] {
            while keys != 0 && self.len < CAPACITY {
                let index = keys.trailing_zeros().to_le_bytes()[0];
                let Some(key) = PianoKey::new(index) else {
                    return;
                };
                let bit = 1 << index;
                let note = midi_note(key);
                let packet = if down {
                    [9, 0x90, note, notes.velocities[usize::from(index)]]
                } else {
                    [8, 0x80, note, 0]
                };
                self.events[(self.head + self.len) % CAPACITY] = packet;
                self.len += 1;
                self.planned ^= bit;
                self.pressure_sent[usize::from(index)] = 0;
                keys &= !bit;
            }
        }
    }

    // Pressure is continuous. Send only the newest value, after queued note edges, and
    // commit only the accepted prefix so a busy endpoint never loses the final value.
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "The batch is bounded by MAX_EVENTS and contains physical MIDI note numbers."
    )]
    pub(super) fn send_pressure(
        &mut self,
        notes: &Notes,
        send: impl FnOnce(&[EventPacket]) -> usize,
    ) {
        if self.len != 0 || notes.held == 0 || notes.held != self.planned {
            return;
        }
        let mut batch = [[0; 4]; MAX_EVENTS];
        let mut count = 0;
        // Rotate after the last accepted packet. A moving low note must not starve a
        // higher one when the host accepts only one update at a time.
        let keys = PianoKey::all()
            .skip(self.next_pressure)
            .chain(PianoKey::all().take(self.next_pressure));
        for key in keys {
            let i = usize::from(key.index());
            let p = notes.pressures[i];
            if notes.held & (1 << i) != 0 && p != self.pressure_sent[i] {
                batch[count] = [10, 0xa0, midi_note(key), p];
                count += 1;
                if count == MAX_EVENTS {
                    break;
                }
            }
        }
        if count != 0 {
            for &[_, _, note, p] in &batch[..send(&batch[..count])] {
                let i = usize::from(note - FIRST_NOTE);
                self.pressure_sent[i] = p;
                self.next_pressure = i + 1;
            }
        }
    }

    #[allow(
        clippy::arithmetic_side_effects,
        reason = "Both indices are at most CAPACITY."
    )]
    pub(super) fn pending(&self) -> &[EventPacket] {
        &self.events[self.head..(self.head + self.len.min(MAX_EVENTS)).min(CAPACITY)]
    }

    #[allow(
        clippy::arithmetic_side_effects,
        reason = "The device accepts at most the contiguous prefix offered."
    )]
    pub(super) fn sent(&mut self, n: usize) {
        debug_assert!(n <= self.pending().len());
        self.head = (self.head + n) % CAPACITY;
        self.len -= n;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn channel_one_note_off_and_panic() {
        let mut r = Notes::default();
        assert_eq!(r.apply([9, 0x90, 60, 25]), PianoKey::new(7));
        assert_eq!(r.velocities[7], 25);
        for packet in [
            [9, 0x91, 60, 0],
            [0x19, 0x90, 60, 0],
            [9, 0x90, 80, 127],
            [9, 0x90, 52, 1],
        ] {
            assert_eq!(r.apply(packet), None);
            assert_eq!(r.held, 1 << 7);
        }
        for packet in [
            [9, 0x90, 60, 0],
            [8, 0x80, 60, 64],
            [11, 0xb0, 123, 0],
            [11, 0xb0, 120, 0],
        ] {
            r.apply([9, 0x90, 60, 100]);
            assert_eq!(r.apply(packet), None);
            assert_eq!(r, Notes::default());
        }
    }

    #[test]
    fn preserves_taps_and_reconciles_after_congestion() {
        let mut o = Output::new();
        o.sent(1);
        let mut notes = Notes {
            held: 1,
            velocities: [73; PianoKey::COUNT as usize],
            ..Notes::default()
        };
        o.observe(&notes);
        notes.held = 0;
        o.observe(&notes);
        assert_eq!(o.pending(), &[[9, 0x90, 53, 73], [8, 0x80, 53, 0]]);
        let all = (1 << PianoKey::COUNT) - 1;
        for _ in 0..3 {
            notes.held = all;
            o.observe(&notes);
            notes.held = 0;
            o.observe(&notes);
        }
        let mut host = Notes::default();
        while !o.pending().is_empty() {
            let packet = o.pending()[0];
            host.apply(packet);
            o.sent(1);
            o.observe(&notes);
        }
        assert_eq!(host.held, 0);
    }

    #[test]
    fn pressure_follows_note_on_coalesces_under_backpressure_and_resets_on_release() {
        let mut notes = Notes::default();
        let mut out = Output::new();
        let mut host = Notes::default();
        out.sent(1);
        for (note, velocity) in [(59, 38), (60, 90)] {
            notes.apply([9, 0x90, note, velocity]);
        }
        notes.apply([10, 0xa0, 59, 10]);
        out.observe(&notes);
        out.send_pressure(&notes, |_| unreachable!("notes first"));
        for &pkt in out.pending() {
            host.apply(pkt);
        }
        out.sent(2);
        notes.apply([10, 0xa0, 59, 70]);
        notes.apply([10, 0xa0, 60, 30]);
        out.send_pressure(&notes, |_| 0);
        out.send_pressure(&notes, |packets| {
            assert_eq!(packets, [[10, 0xa0, 59, 70], [10, 0xa0, 60, 30]]);
            host.apply(packets[0]);
            1
        });
        notes.apply([10, 0xa0, 59, 71]);
        notes.apply([10, 0xa0, 60, 80]);
        out.send_pressure(&notes, |packets| {
            assert_eq!(packets, [[10, 0xa0, 60, 80], [10, 0xa0, 59, 71]]);
            host.apply(packets[0]);
            1
        });
        out.send_pressure(&notes, |packets| {
            assert_eq!(packets, [[10, 0xa0, 59, 71]]);
            host.apply(packets[0]);
            1
        });
        assert_eq!(notes, host);
        assert!(notes.levels(&Notes::default())[7] > 90);
        notes.apply([8, 0x80, 60, 0]);
        notes.apply([10, 0xa0, 60, 120]); // pressure arriving after note-off
        notes.apply([10, 0xa0, 59, 0]); // ease back to rest while still held
        out.observe(&notes);
        assert_eq!(out.pending(), [[8, 0x80, 60, 0]]);
        out.sent(1);
        out.send_pressure(&notes, |p| {
            assert_eq!(p, [[10, 0xa0, 59, 0]]);
            1
        });
        out.send_pressure(&notes, |_| unreachable!("no repeated pressure"));
        assert_eq!(notes.levels(&Notes::default())[7], 0);
    }
}
