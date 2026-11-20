//! A monophonic DDS oscillator and a rising-edge trigger on its submitted PCM.
use fm1_rticx::bsp::{audio::Block, panel::PianoKey};

pub(super) const WIDTH: usize = 224;
const LAST_ROW: u8 = 127;
pub(super) const HEIGHT: usize = LAST_ROW as usize + 1;
pub(super) type Trace = [u8; WIDTH];

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
    pub(super) fn render(self, mut phase: u32, block: &mut Block) -> u32 {
        let note = self.note();
        let step = STEPS[usize::from(note - 41)];
        let gain = if self.playing {
            i32::from(self.velocity)
        } else {
            0
        };
        for frame in block {
            // Peak output is about -24 dBFS at velocity 127.
            let sample = (self.wave.sample(phase) * gain) >> 3;
            *frame = [sample; 2];
            phase = phase.wrapping_add(step);
        }
        phase
    }
}

pub(super) struct Capture {
    pub(super) trace: Trace,
    count: usize,
    previous: i32,
    skip: u8,
    voice: Option<Voice>,
}

impl Capture {
    pub(super) const fn new() -> Self {
        Self {
            trace: [64; WIDTH],
            count: 0,
            previous: 0,
            skip: 0,
            voice: None,
        }
    }

    pub(super) fn discontinuity(&mut self) {
        self.count = 0;
        self.skip = 0;
        self.previous = 0;
        self.voice = None;
    }

    /// Consume only blocks accepted by the DAC. A trace can span two or more refills.
    #[allow(
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "Block indices are bounded, samples are signed Q23 and the display coordinate is clamped to 0..=127 before conversion."
    )]
    pub(super) fn push(&mut self, block: &Block, voice: Voice) -> bool {
        if self.voice.replace(voice) != Some(voice) {
            // Controls can change every block. Finish any in-progress trace of the
            // transition, just as an oscilloscope would on the physical output.
            self.skip = 0;
        }
        if self.skip != 0 {
            self.skip -= 1;
            self.previous = block[block.len() - 1][0];
            return false;
        }
        let silent = !voice.playing || voice.velocity == 0;
        for &[sample, _] in block {
            let rising = self.previous < 0 && sample >= 0;
            self.previous = sample;
            if self.count == 0 && !silent && !rising {
                continue;
            }
            // About eight rows of headroom keep both rails clear of the labels.
            let offset = (((sample >> 8) * 7) + 128) >> 8;
            self.trace[self.count] = (64 - offset).clamp(0, i32::from(LAST_ROW)) as u8;
            self.count += 1;
            if self.count == WIDTH {
                self.count = 0;
                self.skip = 5;
                self.previous = block[block.len() - 1][0];
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm1_rticx::bsp::audio::BLOCK_FRAMES;

    #[test]
    fn preset_and_keyboard_controls_reach_the_oscillator() {
        let mut voice = Voice {
            velocity: 127,
            ..Voice::default()
        };
        assert_eq!(crate::pitch::note_name(voice.note()), *b"A4 ");
        let mut block = [[0; 2]; BLOCK_FRAMES];
        for label in [b"SINE    ", b"TRIANGLE", b"SAWTOOTH", b"SQUARE  "] {
            assert_eq!(voice.wave.label(), label);
            voice.render(0, &mut block);
            assert!(block.iter().all(|f| f[0] == f[1] && f[0].abs() < (1 << 19)));
            assert!(block.iter().any(|f| f[0] > 100_000));
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
        let mut block = [[-8192; 2]; BLOCK_FRAMES];
        block[BLOCK_FRAMES - 1] = [8192; 2];
        let mut voice = Voice {
            velocity: 80,
            ..Voice::default()
        };
        assert!(!capture.push(&block, voice));
        block.fill([16384; 2]);
        voice.velocity = 81;
        assert!(capture.push(&block, voice));
        assert_eq!(capture.trace[0], 63);
        assert_eq!(capture.trace[1..], [62; WIDTH - 1]);
        capture.discontinuity();
        assert!(!capture.push(&block, voice)); // Arm on a fresh crossing after a late refill.
        voice.playing = false;
        voice.render(0, &mut block);
        assert!(capture.push(&block, voice));
        assert_eq!(usize::from(capture.trace[0]), HEIGHT / 2);
        assert_eq!(capture.trace, [64; WIDTH]);
    }
}
