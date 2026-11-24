use super::RefillInterval;
use core::num::NonZeroU32;

#[derive(Clone, Copy, Debug)]
pub(super) struct Observation {
    before: u32,
    after: u32,
    active: u16,
}

impl Observation {
    pub(super) fn new(before: u32, after: u32, active: u16) -> Self {
        Self {
            before,
            after,
            active,
        }
    }

    #[allow(
        clippy::arithmetic_side_effects,
        reason = "A DMA half is at least 544 ticks. Dividing leaves headroom for the parity adjustment."
    )]
    #[inline]
    pub(super) fn interval_since(self, prev: Self, period: u32) -> RefillInterval {
        let shortest = self.before.wrapping_sub(prev.after);
        let longest = self.after.wrapping_sub(prev.before);
        if longest < shortest {
            return RefillInterval::Uncertain;
        }
        // The phase within either half is unknown. In d ticks the DAC can cross
        // floor(d / period) or ceil(d / period) boundaries. Allow a tick for
        // clock quantization, and a full sample window for interrupt preemption.
        let lower = (shortest.saturating_sub(1) / period).max(1);
        let upper = longest.saturating_add(1).div_ceil(period);
        // Each boundary toggles the active half. All candidates with matching
        // parity are two apart. Return a count only when exactly one fits.
        let parity = u32::from(self.active != prev.active);
        let first = lower + ((lower & 1) ^ parity);
        if first > upper || first + 2 <= upper {
            return RefillInterval::Uncertain;
        }
        RefillInterval::Elapsed(NonZeroU32::new(first).expect("lower bound is positive"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(before: u32, after: u32, active: bool) -> Observation {
        Observation::new(before, after, u16::from(active))
    }

    #[test]
    fn coalesced_releases_include_even_and_odd_counts_across_timer_wrap() {
        let p = 69_632;
        let base = u32::MAX - p;
        for halves in 1..=12 {
            // Vary the service phase between opposite ends of the DMA half.
            for (previous_phase, phase) in [(9, p - 10), (p - 10, 9), (340, 340)] {
                let a = base.wrapping_add(previous_phase);
                let b = base.wrapping_add(halves * p).wrapping_add(phase);
                let prev = observation(a.wrapping_sub(3), a.wrapping_add(3), false);
                let next = observation(b.wrapping_sub(3), b.wrapping_add(3), halves % 2 != 0);
                assert_eq!(
                    next.interval_since(prev, p),
                    RefillInterval::Elapsed(halves.try_into().expect("positive count"))
                );
            }
        }
    }

    #[test]
    fn long_preemption_preserves_ambiguity() {
        let p = 69_632;
        let prev = observation(10, 20, false);
        let next = observation(p, 4 * p, true);
        assert_eq!(next.interval_since(prev, p), RefillInterval::Uncertain);
        let next = observation(100, 110, false);
        assert_eq!(next.interval_since(prev, p), RefillInterval::Uncertain);
    }

    #[test]
    fn an_estimate_matches_the_edges_for_every_phase() {
        let period = 16;
        for count in 1..=5 {
            for first in 0..period {
                for last in 0..period {
                    let a = period + first;
                    let b = (count + 1) * period + last;
                    for width in [0, 1, 4, 15, 19] {
                        if a + width > b {
                            continue;
                        }
                        let prev = observation(a - 3, a + width, false);
                        let next = observation(b, b + width, count % 2 == 1);
                        if let RefillInterval::Elapsed(n) = next.interval_since(prev, period) {
                            assert_eq!(n.get(), count);
                        }
                    }
                }
            }
        }
    }
}
