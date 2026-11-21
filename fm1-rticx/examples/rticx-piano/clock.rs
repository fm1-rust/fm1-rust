//! USB MIDI transport and 24 timing clocks per quarter note at 120 BPM.
use fm1_rticx::{
    bsp::{time::Timers, usb_midi::EventPacket},
    time::{DeadlineDuration, DeadlineInstant},
};

const PULSE: u32 = Timers::CRYSTAL_HZ / (2 * 24);
const POLL: DeadlineDuration = DeadlineDuration::from_millis(1);

#[derive(Default)]
pub(super) struct MidiClock {
    next: Option<DeadlineInstant>,
    pending: bool,
    pub(super) worst_us: u32,
    pub(super) missed: u32,
}

#[allow(
    clippy::arithmetic_side_effects,
    reason = "Wrapping instants advance by at most one pulse. The remainder is smaller than PULSE."
)]
impl MidiClock {
    // Service USB between timing messages. An overdue edge runs immediately.
    pub(super) fn wake_by(&self, now: DeadlineInstant, playing: bool) -> DeadlineInstant {
        let poll = now + POLL;
        if playing
            && let Some(edge) = self.next
            && poll.checked_duration_since(edge).is_some()
        {
            return edge;
        }
        poll
    }

    pub(super) fn service(
        &mut self,
        now: DeadlineInstant,
        playing: bool,
        mut send: impl FnMut(EventPacket) -> bool,
    ) {
        if playing != self.next.is_some() {
            if send([0x0f, if playing { 0xfa } else { 0xfc }, 0, 0]) {
                self.next = playing.then(|| now + DeadlineDuration::from_ticks(PULSE));
                self.pending = false;
                if playing {
                    self.worst_us = 0;
                    self.missed = 0;
                }
            }
            return;
        }

        if let Some(late) = self.next.and_then(|edge| now.checked_duration_since(edge)) {
            let ticks = late.as_ticks();
            self.worst_us = self.worst_us.max(late.as_micros());
            self.missed = self
                .missed
                .saturating_add(ticks / PULSE)
                .saturating_add(u32::from(self.pending));
            // Preserve phase and skip whole missed periods. Never send a burst of stale clocks.
            self.next = Some(now + DeadlineDuration::from_ticks(PULSE - ticks % PULSE));
            self.pending = true;
        }
        if self.pending && send([0x0f, 0xf8, 0, 0]) {
            self.pending = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absolute_pulses_survive_rollover_backpressure_and_stop() {
        let start = DeadlineInstant::from_ticks(u32::MAX - 7);
        let pulse = DeadlineDuration::from_ticks(PULSE);
        let mut clock = MidiClock::default();
        let mut events = Vec::new();
        let mut send = |event: EventPacket| {
            events.push(event[1]);
            true
        };
        clock.service(start, true, &mut send);
        let first = start + pulse;
        assert_eq!(clock.wake_by(first - POLL, true), first);
        clock.service(first, true, |_| false);
        clock.service(first + POLL, true, &mut send);
        clock.service(first + pulse + pulse, true, &mut send);
        assert_eq!(clock.missed, 1);
        assert_eq!(clock.worst_us, 20_833);
        assert_eq!(clock.next, Some(first + pulse + pulse + pulse));
        clock.service(first + pulse + pulse, false, |_| false);
        clock.service(first + pulse + pulse + POLL, false, &mut send);
        assert_eq!(events, [0xfa, 0xf8, 0xf8, 0xfc]);
        assert_eq!(clock.next, None);
    }
}
