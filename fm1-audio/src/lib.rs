#![no_std]
#![forbid(unsafe_code)]
#![doc = include_str!("../README.md")]

pub use fm1::audio::*;
pub use fm1::panel::MasterLevel;
use fm1::time::cycles;
pub use wavegraph_sample::I24;
use wavegraph_sample::types::i24;

const FRAMES_PER_HALF: u16 = 128;

/// Stereo frames per DMA half used by [`Output`].
pub const OUTPUT_FRAMES: usize = FRAMES_PER_HALF as usize;
/// One interleaved block for [`Output`]. Each frame is `[left, right]`.
pub type OutputBlock = Block<OUTPUT_FRAMES>;
/// TIMER4 ticks available per output block (24 MHz crystal).
pub const BLOCK_TICKS: u32 = FRAMES_PER_HALF as u32 * FRAME_RATE.1;

/// A 128-frame DMA stream with one reusable staging block and smoothed MASTER gain.
#[derive(Debug)]
pub struct Output {
    stream: fm1::Cpu0<Stream<'static, OUTPUT_FRAMES>>,
    block: OutputBlock,
    gain: MasterLevel,
}

impl Output {
    /// Take ownership of ALNK0 and start playback with silence.
    ///
    /// # Errors
    /// Propagates the BSP's clock and busy errors.
    pub fn start(port: AudioPort) -> Result<Self, StartError> {
        Ok(Self {
            stream: port.into_stream_with_frames()?.into(),
            block: [[0; 2]; OUTPUT_FRAMES],
            gain: MasterLevel::MIN,
        })
    }

    /// Render one ready half. The callback receives a cleared block and runs
    /// exactly once on `Written` or `Late`, and zero times on `WouldBlock`.
    /// Add sources in signed Q23 units with i32 headroom. MASTER ramps to the
    /// requested level across the block before the final clip to signed 24-bit PCM.
    pub fn render(&mut self, master: MasterLevel, render: impl FnOnce(&mut OutputBlock)) -> Refill {
        let block = &mut self.block;
        let previous = self.gain;
        let result = self
            .stream
            .try_write_with(|| prepare_block(block, previous, master, render));
        self.finish(master, result)
    }

    /// Render a ready half with its DAC interval. The interval is available before selecting
    /// the samples so a sequencer can skip expired work. The callback receives the same
    /// cleared block and MASTER handling as [`Self::render`].
    ///
    /// [`Stream::try_write_with_interval`] describes the clock assumptions and when the
    /// count is uncertain. A serviced call to [`Self::render`] resets the timing baseline.
    pub fn render_with_interval(
        &mut self,
        master: MasterLevel,
        render: impl FnOnce(&mut OutputBlock, RefillInterval),
    ) -> Refill {
        let block = &mut self.block;
        let previous = self.gain;
        let result = self.stream.try_write_with_interval(|interval| {
            prepare_block(block, previous, master, |block| render(block, interval))
        });
        self.finish(master, result)
    }

    fn finish(&mut self, master: MasterLevel, result: Refill) -> Refill {
        if result == Refill::Written {
            self.gain = master;
        }
        result
    }

    /// The most recently prepared post-MASTER stereo mix. After [`Refill::Written`]
    /// these are the samples submitted to the DAC, suitable for metering or recording.
    #[must_use]
    pub fn block(&self) -> &OutputBlock {
        &self.block
    }
}

fn prepare_block(
    block: &mut OutputBlock,
    from: MasterLevel,
    to: MasterLevel,
    render: impl FnOnce(&mut OutputBlock),
) -> &OutputBlock {
    block.fill([0; 2]);
    render(block);
    apply_gain(block, from, to);
    block
}

/// Add separate left and right signed 24-bit input planes to an interleaved mix.
/// Accumulation saturates at i32 limits. [`Output`] applies the 24-bit clip after MASTER.
pub fn mix_stereo<const N: usize>(out: &mut Block<N>, left: &[I24; N], right: &[I24; N]) {
    for ((frame, l), r) in out.iter_mut().zip(left).zip(right) {
        frame[0] = frame[0].saturating_add(l.inner());
        frame[1] = frame[1].saturating_add(r.inner());
    }
}

#[allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "Q10 gains times 128 frames fit in i32, scaling an i32 sample fits in i64, and the result is clipped to 24 bits."
)]
fn apply_gain(block: &mut OutputBlock, from: MasterLevel, to: MasterLevel) {
    const { assert!(OUTPUT_FRAMES.is_power_of_two()) };
    let fixed = |level: MasterLevel| {
        let raw = i32::from(level.raw());
        raw + (raw >> 9) // 0..=1023 to Q10, including exact unity.
    };
    let from = fixed(from);
    let to = fixed(to);
    let min = i24::MIN.inner();
    let max = i24::MAX.inner();
    if from == 1024 && to == 1024 {
        for channel in block.as_flattened_mut() {
            *channel = (*channel).clamp(min, max);
        }
        return;
    }
    let mut gain = from * i32::from(FRAMES_PER_HALF);
    for frame in block {
        gain += to - from;
        for channel in frame {
            let scaled = (i64::from(*channel) * i64::from(gain)) >> (10 + OUTPUT_FRAMES.ilog2());
            *channel = scaled.clamp(i64::from(min), i64::from(max)) as i32;
        }
    }
}

/// Timing of serviced 128-frame output interrupts. Snapshot this for a lower-priority UI.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Stats {
    /// Service calls which crossed a DMA boundary, exhausted the block budget,
    /// or started at least two periods after the preceding serviced interrupt.
    /// Several coalesced hardware events can count as a single overrun.
    pub overruns: u32,
    /// Longest serviced interrupt in ticks of the 24 MHz TIMER4, including preemption.
    pub peak_ticks: u32,
    /// Longest entry-to-entry interval between serviced audio interrupts, in TIMER4 ticks.
    pub peak_gap_ticks: u32,
    /// Longest time from entry to a successful refill return sampled by
    /// [`record_with_refill`](Self::record_with_refill), in TIMER4 ticks.
    pub peak_commit_ticks: u32,
    last_start: Option<u32>,
}

impl Stats {
    /// At the end of the interrupt, pass the TIMER4 reading taken at entry and
    /// the refill result. Returns true if this service call observed an overrun.
    pub fn record(&mut self, started: u32, result: Refill) -> bool {
        self.record_at(started, None, cycles(), result)
    }

    /// Also record the timestamp taken immediately after [`Output::render`] returned.
    /// Call this once at the end of service, after any output taps. Only a
    /// [`Refill::Written`] contributes to the peak commit time.
    pub fn record_with_refill(&mut self, started: u32, refill_ended: u32, result: Refill) -> bool {
        self.record_at(started, Some(refill_ended), cycles(), result)
    }

    fn record_at(
        &mut self,
        started: u32,
        refill_ended: Option<u32>,
        ended: u32,
        result: Refill,
    ) -> bool {
        if result == Refill::WouldBlock {
            return false;
        }
        if result == Refill::Written
            && let Some(at) = refill_ended
        {
            self.peak_commit_ticks = self.peak_commit_ticks.max(at.wrapping_sub(started));
        }
        let elapsed = ended.wrapping_sub(started);
        self.peak_ticks = self.peak_ticks.max(elapsed);
        let gap = self
            .last_start
            .replace(started)
            .map_or(0, |last| started.wrapping_sub(last));
        self.peak_gap_ticks = self.peak_gap_ticks.max(gap);
        let overrun = result == Refill::Late || elapsed >= BLOCK_TICKS || gap >= 2 * BLOCK_TICKS;
        if overrun {
            self.overruns = self.overruns.saturating_add(1);
        }
        overrun
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_sources_sum_before_master_and_clip() {
        let mut mix = [[0; 2]; OUTPUT_FRAMES];
        let left = [i24::MAX; OUTPUT_FRAMES];
        let right = [i24::MIN; OUTPUT_FRAMES];
        mix_stereo(&mut mix, &left, &right);
        mix_stereo(&mut mix, &left, &right);
        assert_eq!(mix[0], [16_777_214, -16_777_216]);
        let mut unity = mix;
        apply_gain(&mut unity, MasterLevel::MAX, MasterLevel::MAX);
        assert_eq!(unity, [[8_388_607, -8_388_608]; OUTPUT_FRAMES]);
        apply_gain(&mut mix, MasterLevel::MAX, MasterLevel::MIN);
        assert_eq!(mix[OUTPUT_FRAMES / 2 - 1], [8_388_607, -8_388_608]);
        assert_eq!(mix[OUTPUT_FRAMES - 1], [0; 2]);
        assert!(
            mix.windows(2)
                .all(|pair| pair[0][0] >= pair[1][0] && pair[0][1] <= pair[1][1])
        );
    }

    #[test]
    fn timing_wraps_and_counts_each_late_service_once() {
        let mut stats = Stats::default();
        let start = u32::MAX - 10;
        assert!(!stats.record_at(start, Some(2), 10, Refill::Written));
        assert_eq!(stats.peak_ticks, 21);
        assert_eq!((stats.peak_commit_ticks, stats.peak_gap_ticks), (13, 0));
        let before = stats;
        assert!(!stats.record_at(50, Some(100), 120, Refill::WouldBlock));
        assert_eq!(stats, before);
        let next = start.wrapping_add(2 * BLOCK_TICKS);
        assert!(stats.record_at(next, None, next + 1, Refill::Written));
        let next = next + BLOCK_TICKS;
        assert!(stats.record_at(next, Some(next + 20), next + BLOCK_TICKS, Refill::Late));
        let next = next + BLOCK_TICKS + 1;
        assert!(!stats.record_at(next, Some(next + 3), next + 5, Refill::Written));
        assert_eq!(
            (stats.peak_commit_ticks, stats.peak_gap_ticks),
            (13, 2 * BLOCK_TICKS)
        );
        assert_eq!((stats.overruns, stats.peak_ticks), (2, BLOCK_TICKS));
    }
}
