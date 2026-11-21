use crate::pitch::{FIRST_NOTE, midi_note};
use fm1_rticx::bsp::{
    panel::PianoKey,
    usb_midi::{EventPacket, MAX_EVENTS},
};

pub(super) type Velocities = [u8; PianoKey::COUNT as usize];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct RemoteNotes {
    pub(super) held: u32,
    pub(super) velocities: Velocities,
}

impl RemoteNotes {
    /// Update host-owned notes. Return a positive note-on for the LCD history.
    pub(super) fn apply(&mut self, event: EventPacket) -> Option<PianoKey> {
        let [cin, status, number, velocity] = event;
        if (cin, status) == (0x0b, 0xb0) && matches!(number, 120 | 123) && velocity < 128 {
            *self = Self::default();
            return None;
        }
        if !matches!((cin, status), (8, 0x80) | (9, 0x90)) || velocity >= 128 {
            return None;
        }
        let key = PianoKey::new(number.checked_sub(FIRST_NOTE)?)?;
        let bit = 1 << key.index();
        let slot = &mut self.velocities[usize::from(key.index())];
        if status == 0x80 || velocity == 0 {
            self.held &= !bit;
            *slot = 0;
            None
        } else {
            self.held |= bit;
            *slot = velocity;
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
        }
    }

    #[allow(
        clippy::arithmetic_side_effects,
        reason = "Queue indices and lengths are bounded by CAPACITY."
    )]
    pub(super) fn observe(&mut self, held: u32) {
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
                    [9, 0x90, note, 127]
                } else {
                    [8, 0x80, note, 0]
                };
                self.events[(self.head + self.len) % CAPACITY] = packet;
                self.len += 1;
                self.planned ^= bit;
                keys &= !bit;
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
        let mut r = RemoteNotes::default();
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
            assert_eq!(r, RemoteNotes::default());
        }
    }

    #[test]
    fn preserves_taps_and_reconciles_after_congestion() {
        let mut o = Output::new();
        o.sent(1);
        o.observe(1);
        o.observe(0);
        assert_eq!(o.pending(), &[[9, 0x90, 53, 127], [8, 0x80, 53, 0]]);
        let all = (1 << PianoKey::COUNT) - 1;
        for _ in 0..3 {
            o.observe(all);
            o.observe(0);
        }
        let mut host = RemoteNotes::default();
        while !o.pending().is_empty() {
            let packet = o.pending()[0];
            host.apply(packet);
            o.sent(1);
            o.observe(0);
        }
        assert_eq!(host.held, 0);
    }
}
