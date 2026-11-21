//! Panel settings and a rising-edge trigger for the selected signal.
use super::graph::Applied;
use fm1_audio::{I24, OUTPUT_FRAMES, OutputBlock};
use fm1_rticx::bsp::panel::PianoKey;
use wavegraph_monosynth::{Message, Waveform};

pub(super) const WIDTH: usize = 224;
const LAST_ROW: u8 = 127;
pub(super) const HEIGHT: usize = LAST_ROW as usize + 1;
pub(super) type Trace = [u8; WIDTH];

const PRESETS: [(Waveform, &[u8]); 4] = [
    (Waveform::Sine, b"SINE    "),
    (Waveform::Triangle, b"TRIANGLE"),
    (Waveform::Sawtooth, b"SAWTOOTH"),
    (Waveform::Square, b"SQUARE  "),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Voice {
    preset: usize,
    pub(super) playing: bool,
    pub(super) velocity: u8,
    pub(super) transpose: i8,
    key: u8,
}

impl Default for Voice {
    fn default() -> Self {
        Self {
            preset: 0,
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
    pub(super) fn turn_preset(&mut self, detents: i8) {
        const MASK: usize = {
            assert!(PRESETS.len().is_power_of_two());
            PRESETS.len() - 1
        };
        self.preset = self.preset.wrapping_add_signed(isize::from(detents)) & MASK;
    }
    pub(super) fn label(self) -> &'static [u8] {
        PRESETS[self.preset].1
    }

    // On a muted pitch change, set the new increment and release at the same frame.
    // Unmuting then resumes at the phase the oscillator reached during silence.
    pub(super) fn messages(self) -> [Message; 3] {
        let note = self.note();
        [
            Message::Waveform(PRESETS[self.preset].0),
            Message::NoteOn {
                note,
                velocity: self.velocity.max(1),
            },
            Message::NoteOff(note),
        ]
    }
}

/// One selected signal block and the identity of the edit that produced it.
pub(super) struct ScopeBlock {
    pub(super) voice: Voice,
    pub(super) applied: Applied,
    submitted_at: u32,
    samples: [i32; OUTPUT_FRAMES],
    gap: bool,
}

impl ScopeBlock {
    pub(super) fn new(
        dac: &OutputBlock,
        oscillator: &[I24; OUTPUT_FRAMES],
        voice: Voice,
        applied: Applied,
        submitted_at: u32,
        gap: bool,
    ) -> Self {
        let samples = if applied.selection.oscillator_tap {
            core::array::from_fn(|i| oscillator[i].inner())
        } else {
            core::array::from_fn(|i| dac[i][0])
        };
        Self {
            voice,
            applied,
            submitted_at,
            samples,
            gap,
        }
    }
}

pub(super) struct Capture {
    pub(super) trace: Trace,
    pub(super) started_at: u32,
    applied: Option<Applied>,
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
            applied: None,
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
        if self.applied.replace(block.applied) != Some(block.applied) || block.gap {
            // A graph edit, a new tap or a missing block starts a fresh trace.
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
    use super::super::graph::{Graph, Oscillator, Selection};
    use super::*;

    fn dac(block: &OutputBlock, voice: Voice, gap: bool) -> ScopeBlock {
        ScopeBlock::new(
            block,
            &[I24::default(); OUTPUT_FRAMES],
            voice,
            Applied::default(),
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
        let mut block = [[0; 2]; OUTPUT_FRAMES];
        for label in [b"SINE    ", b"TRIANGLE", b"SAWTOOTH", b"SQUARE  "] {
            assert_eq!(voice.label(), label);
            block.fill([0; 2]);
            let mut buses = [[I24::default(); OUTPUT_FRAMES]; 2];
            Graph::new().render(&mut Oscillator::new(), voice, false, &mut buses);
            fm1_audio::mix_stereo(&mut block, &buses[0], &buses[0]);
            assert!(block.iter().all(|f| f[0] == f[1] && f[0].abs() < (1 << 19)));
            assert!(block.iter().any(|f| f[0] > 100_000));
            voice.turn_preset(1);
        }
        voice.turn_preset(-1);
        assert_eq!(voice.label(), b"SQUARE  ");
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
        block.fill([0; 2]);
        let mut buses = [[I24::default(); OUTPUT_FRAMES]; 2];
        Graph::new().render(&mut Oscillator::new(), voice, false, &mut buses);
        fm1_audio::mix_stereo(&mut block, &buses[0], &buses[0]);
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
    fn switching_taps_discards_partial_traces_and_dates_the_fresh_trigger() {
        let voice = Voice {
            velocity: 80,
            ..Voice::default()
        };
        let mut capture = Capture::new();
        let mut dac = [[8192, 0]; OUTPUT_FRAMES];
        dac[0][0] = -8192;
        let mut osc = [I24::new_unchecked(16384); OUTPUT_FRAMES];
        let old = Applied::default();
        let new = Applied {
            first_frame: 128,
            submitted_at: 20,
            selection: Selection {
                half_gain: true,
                oscillator_tap: true,
            },
        };
        assert!(!capture.push(&ScopeBlock::new(&dac, &osc, voice, old, 10, false)));
        // DAC has a crossing, the newly selected oscillator does not.
        assert!(!capture.push(&ScopeBlock::new(&dac, &osc, voice, new, 20, false)));
        osc[0] = I24::new_unchecked(-8192);
        assert!(!capture.push(&ScopeBlock::new(&dac, &osc, voice, new, 30, false)));
        osc[0] = I24::new_unchecked(16384);
        assert!(capture.push(&ScopeBlock::new(&dac, &osc, voice, new, 40, false)));
        assert_eq!(capture.started_at, 30);
        assert_eq!(capture.trace, [62; WIDTH]);
    }
}
