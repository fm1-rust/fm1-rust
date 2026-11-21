//! A monophonic DDS oscillator and a rising-edge trigger for the selected signal.
use fm1_audio::{OUTPUT_FRAMES, OutputBlock};
use fm1_rticx::bsp::panel::PianoKey;

pub(super) const WIDTH: usize = 224;
const LAST_ROW: u8 = 127;
pub(super) const HEIGHT: usize = LAST_ROW as usize + 1;
pub(super) type Trace = [u8; WIDTH];
pub(super) type Samples = [i32; OUTPUT_FRAMES];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum Wave {
    #[default]
    Sine,
    Triangle,
    Sawtooth,
    Square,
}

impl Wave {
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "An i8 detent count plus a preset index in 0..4 fits in i16."
    )]
    pub(super) fn turn(self, detents: i8) -> Self {
        const WAVES: [Wave; 4] = [Wave::Sine, Wave::Triangle, Wave::Sawtooth, Wave::Square];
        WAVES[(self as i16 + i16::from(detents)).rem_euclid(4) as usize]
    }

    pub(super) fn label(self) -> &'static [u8] {
        match self {
            Self::Sine => b"SINE    ",
            Self::Triangle => b"TRIANGLE",
            Self::Sawtooth => b"SAWTOOTH",
            Self::Square => b"SQUARE  ",
        }
    }

    #[allow(
        clippy::arithmetic_side_effects,
        reason = "The ramp is 16 bits and the quarter-wave interpolation uses an 8-bit fraction; all intermediates fit in i32."
    )]
    fn sample(self, phase: u32) -> i32 {
        let ramp = (phase >> 16).cast_signed();
        match self {
            Self::Sawtooth => ramp - 32768,
            Self::Square => {
                if phase < 1 << 31 {
                    32767
                } else {
                    -32767
                }
            }
            Self::Triangle => 32767 - (2 * ramp - 65535).abs(),
            Self::Sine => {
                let part = phase & 0x3fff_ffff;
                let folded = if phase & (1 << 30) == 0 {
                    part
                } else {
                    (1 << 30) - part
                };
                let i = (folded >> 24) as usize;
                let a = i32::from(SINE[i]);
                let b = i32::from(SINE[(i + 1).min(64)]);
                let fraction = ((folded >> 16) & 255).cast_signed();
                let value = a + (((b - a) * fraction) >> 8);
                if phase & (1 << 31) == 0 {
                    value
                } else {
                    -value
                }
            }
        }
    }
}

// One quarter of a sine, round(32767 * sin(i * π / 128)).
const SINE: [i16; 65] = [
    0, 804, 1608, 2410, 3212, 4011, 4808, 5602, 6393, 7179, 7962, 8739, 9512, 10278, 11039, 11793,
    12539, 13279, 14010, 14732, 15446, 16151, 16846, 17530, 18204, 18868, 19519, 20159, 20787,
    21403, 22005, 22594, 23170, 23731, 24279, 24811, 25329, 25832, 26319, 26790, 27245, 27683,
    28105, 28510, 28898, 29268, 29621, 29956, 30273, 30571, 30852, 31113, 31356, 31580, 31785,
    31971, 32137, 32285, 32412, 32521, 32609, 32678, 32728, 32757, 32767,
];
// F2 through G6 covers the keyboard and one octave of transposition in either direction.
const STEPS: [u32; 51] = crate::pitch::phase_steps(87.307_057_858_251);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Voice {
    pub(super) wave: Wave,
    pub(super) playing: bool,
    pub(super) velocity: u8,
    pub(super) transpose: i8,
    key: u8,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            wave: Wave::Sine,
            playing: true,
            velocity: 0,
            transpose: 0,
            key: 69,
        }
    }
}

impl Voice {
    pub(super) fn select_key(&mut self, key: PianoKey) {
        self.key = crate::pitch::midi_note(key);
    }
    pub(super) fn transpose_by(&mut self, delta: i8) {
        self.transpose = self.transpose.saturating_add(delta).clamp(-12, 12);
    }
    pub(super) fn note(self) -> u8 {
        self.key.saturating_add_signed(self.transpose)
    }
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "Notes are 41..=91 and multiplying a Q15 waveform by a 7-bit velocity fits in i32."
    )]
    pub(super) fn render(self, mut phase: u32, block: &mut Samples) -> u32 {
        let note = self.note();
        let step = STEPS[usize::from(note - 41)];
        let gain = if self.playing {
            i32::from(self.velocity)
        } else {
            0
        };
        for sample in block {
            // Peak output is about -24 dBFS at velocity 127.
            *sample = (self.wave.sample(phase) * gain) >> 3;
            phase = phase.wrapping_add(step);
        }
        phase
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Selection {
    pub(super) half_gain: bool,
    pub(super) oscillator_tap: bool,
    pub(super) changed_at: u32,
}

impl Selection {
    pub(super) fn write_stereo(self, oscillator: &Samples, output: &mut OutputBlock) {
        let shift = u32::from(self.half_gain);
        for (frame, sample) in output.iter_mut().zip(oscillator) {
            *frame = [sample >> shift; 2];
        }
    }
}

/// One scope tap from a successful DMA submission.
pub(super) struct ScopeBlock {
    pub(super) voice: Voice,
    pub(super) selection: Selection,
    submitted_at: u32,
    samples: Samples,
    gap: bool,
}

impl ScopeBlock {
    pub(super) fn new(
        dac: &OutputBlock,
        oscillator: &Samples,
        voice: Voice,
        selection: Selection,
        submitted_at: u32,
        gap: bool,
    ) -> Self {
        let samples = if selection.oscillator_tap {
            *oscillator
        } else {
            core::array::from_fn(|i| dac[i][0])
        };
        Self {
            voice,
            selection,
            submitted_at,
            samples,
            gap,
        }
    }
}

pub(super) struct Capture {
    pub(super) trace: Trace,
    pub(super) started_at: u32,
    selection: Option<Selection>,
    count: usize,
    previous: i32,
    skip: u8,
    voice: Option<Voice>,
}

impl Capture {
    pub(super) const fn new() -> Self {
        Self {
            trace: [64; WIDTH],
            started_at: 0,
            selection: None,
            count: 0,
            previous: 0,
            skip: 0,
            voice: None,
        }
    }

    /// Consume only blocks accepted by the DAC. A trace can span two or more refills.
    #[allow(
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Block indices are bounded, samples are signed Q23 and the display coordinate is clamped to 0..=127 before conversion."
    )]
    pub(super) fn push(&mut self, block: &ScopeBlock) -> bool {
        if self.selection.replace(block.selection) != Some(block.selection) || block.gap {
            // A gain change, a new tap or a missing block starts a fresh trace.
            self.count = 0;
            self.skip = 0;
            self.previous = 0;
        }
        let voice = block.voice;
        if self.voice.replace(voice) != Some(voice) {
            // Controls can change every block. Finish any in-progress trace of the
            // transition, just as an oscilloscope would on the physical output.
            self.skip = 0;
        }
        if self.skip != 0 {
            self.skip -= 1;
            self.previous = block.samples[OUTPUT_FRAMES - 1];
            return false;
        }
        let silent = !voice.playing || voice.velocity == 0;
        for &sample in &block.samples {
            let rising = self.previous < 0 && sample >= 0;
            self.previous = sample;
            if self.count == 0 {
                if !silent && !rising {
                    continue;
                }
                self.started_at = block.submitted_at;
            }
            // About eight rows of headroom keep both rails clear of the labels.
            let offset = (((sample >> 8) * 7) + 128) >> 8;
            self.trace[self.count] = (64 - offset).clamp(0, i32::from(LAST_ROW)) as u8;
            self.count += 1;
            if self.count == WIDTH {
                self.count = 0;
                self.skip = 10; // About 29 ms between traces at 128 frames per half.
                self.previous = block.samples[OUTPUT_FRAMES - 1];
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dac(block: &OutputBlock, voice: Voice, gap: bool) -> ScopeBlock {
        ScopeBlock::new(
            block,
            &[0; OUTPUT_FRAMES],
            voice,
            Selection::default(),
            0,
            gap,
        )
    }

    #[test]
    fn preset_and_keyboard_controls_reach_the_oscillator() {
        let mut voice = Voice {
            velocity: 127,
            ..Voice::default()
        };
        assert_eq!(crate::pitch::note_name(voice.note()), *b"A4 ");
        let mut samples = [0; OUTPUT_FRAMES];
        for label in [b"SINE    ", b"TRIANGLE", b"SAWTOOTH", b"SQUARE  "] {
            assert_eq!(voice.wave.label(), label);
            voice.render(0, &mut samples);
            assert!(samples.iter().all(|s| s.abs() < (1 << 19)));
            assert!(samples.iter().any(|&s| s > 100_000));
            voice.wave = voice.wave.turn(1);
        }
        assert_eq!(voice.wave.turn(-1), Wave::Square);
        voice.select_key(PianoKey::new(26).expect("rightmost key"));
        voice.transpose_by(127);
        assert_eq!(crate::pitch::note_name(voice.note()), *b"G6 ");
        voice.select_key(PianoKey::new(0).expect("leftmost key"));
        voice.transpose_by(-127);
        assert_eq!(crate::pitch::note_name(voice.note()), *b"F2 ");
    }

    #[test]
    fn capture_spans_blocks_even_during_a_velocity_change() {
        let mut capture = Capture::new();
        let mut block = [[-8192; 2]; OUTPUT_FRAMES];
        block[OUTPUT_FRAMES - 1] = [8192; 2];
        let mut voice = Voice {
            velocity: 80,
            ..Voice::default()
        };
        assert!(!capture.push(&dac(&block, voice, false)));
        block.fill([16384; 2]);
        voice.velocity = 81;
        assert!(!capture.push(&dac(&block, voice, false)));
        assert!(capture.push(&dac(&block, voice, false)));
        assert_eq!(capture.trace[0], 63);
        assert_eq!(capture.trace[1..], [62; WIDTH - 1]);
        assert!(!capture.push(&dac(&block, voice, true))); // Arm on a fresh crossing.
        voice.playing = false;
        let mut samples = [1; OUTPUT_FRAMES];
        let end_phase = voice.render(0, &mut samples);
        assert_ne!(end_phase, 0);
        assert_eq!(samples, [0; OUTPUT_FRAMES]);
        Selection::default().write_stereo(&samples, &mut block);
        assert!(!capture.push(&dac(&block, voice, false)));
        assert!(capture.push(&dac(&block, voice, false)));
        assert_eq!(usize::from(capture.trace[0]), HEIGHT / 2);
        assert_eq!(capture.trace, [64; WIDTH]);
    }

    #[test]
    fn gap_discards_partial_trace_and_rearms_on_the_left_channel() {
        let voice = Voice {
            velocity: 80,
            ..Voice::default()
        };
        let mut capture = Capture::new();
        let mut pcm = [[8192, 0]; OUTPUT_FRAMES];
        pcm[0][0] = -8192;
        pcm[OUTPUT_FRAMES - 1][0] = -8192;
        assert!(!capture.push(&dac(&pcm, voice, false)));

        // A rising edge on the right channel cannot complete the pre-gap trace.
        pcm.fill([16384, 8192]);
        pcm[0][1] = -8192;
        assert!(!capture.push(&dac(&pcm, voice, true)));
        assert!(!capture.push(&dac(&pcm, voice, false)));

        pcm[0][0] = -8192;
        assert!(!capture.push(&dac(&pcm, voice, false)));
        pcm[0][0] = 16384;
        assert!(capture.push(&dac(&pcm, voice, false)));
        assert_eq!(capture.trace, [62; WIDTH]);
    }

    #[test]
    fn fx_scales_both_dac_channels_and_preserves_the_oscillator_tap() {
        let voice = Voice {
            velocity: 97,
            ..Voice::default()
        };
        let mut oscillator = [0; OUTPUT_FRAMES];
        voice.render(0, &mut oscillator);
        let mut dac = [[0; 2]; OUTPUT_FRAMES];
        for half_gain in [false, true] {
            let selection = Selection {
                half_gain,
                ..Selection::default()
            };
            selection.write_stereo(&oscillator, &mut dac);
            let expected = oscillator.map(|sample| if half_gain { sample >> 1 } else { sample });
            assert_eq!(dac, expected.map(|sample| [sample; 2]));
            assert_eq!(
                ScopeBlock::new(&dac, &oscillator, voice, selection, 0, false).samples,
                expected
            );
            let tap = Selection {
                oscillator_tap: true,
                ..selection
            };
            assert_eq!(
                ScopeBlock::new(&dac, &oscillator, voice, tap, 0, false).samples,
                oscillator
            );
        }
    }

    #[test]
    fn switching_taps_discards_partial_traces_and_dates_the_fresh_trigger() {
        let voice = Voice {
            velocity: 80,
            ..Voice::default()
        };
        let mut capture = Capture::new();
        let mut dac = [[8192, 0]; OUTPUT_FRAMES];
        dac[0][0] = -8192;
        let mut osc = [16384; OUTPUT_FRAMES];
        let old = Selection::default();
        let new = Selection {
            changed_at: 20,
            half_gain: true,
            oscillator_tap: true,
        };
        assert!(!capture.push(&ScopeBlock::new(&dac, &osc, voice, old, 10, false)));
        // DAC has a crossing, the newly selected oscillator does not.
        assert!(!capture.push(&ScopeBlock::new(&dac, &osc, voice, new, 20, false)));
        osc[0] = -8192;
        assert!(!capture.push(&ScopeBlock::new(&dac, &osc, voice, new, 30, false)));
        osc[0] = 16384;
        assert!(capture.push(&ScopeBlock::new(&dac, &osc, voice, new, 40, false)));
        assert_eq!(capture.started_at, 30);
        assert_eq!(capture.trace, [62; WIDTH]);
    }
}
