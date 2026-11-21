use crate::pitch::phase_steps;
use fm1_audio::{OUTPUT_FRAMES, OutputBlock};
use fm1_rticx::bsp::panel::PianoKey;

// Equal-tempered F3 through G5 at A4 = 440 Hz.
const STEPS: [u32; PianoKey::COUNT as usize] = phase_steps(174.614_115_716_502);

#[derive(Clone, Copy, Default)]
pub(super) struct Synth {
    phases: [u32; PianoKey::COUNT as usize],
    gains: crate::midi::Velocities,
}

impl Synth {
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "One triangle is ±1024. The 128-frame ramp and 27 voices fit in i32 at maximum velocity."
    )]
    pub(super) fn render(&mut self, levels: &crate::midi::Velocities, block: &mut OutputBlock) {
        const { assert!(OUTPUT_FRAMES == 128 && PianoKey::COUNT <= 32) };
        for (i, (&step, phase)) in STEPS.iter().zip(&mut self.phases).enumerate() {
            let velocity = levels[i];
            let gain = velocity + (velocity >> 6); // 0..=128
            let previous = core::mem::replace(&mut self.gains[i], gain);
            if gain == 0 && previous == 0 {
                continue;
            }
            if previous == 0 {
                *phase = 1 << 30; // Start the triangle at its rising zero crossing.
            }

            // Ramp each voice over one block on attack and release. Held
            // voices keep their phase and amplitude when a chord changes.
            let mut level = i32::from(previous) << 7;
            let delta = i32::from(gain) - i32::from(previous);
            let mut position = *phase;
            for frame in &mut *block {
                let ramp = (position >> 20).cast_signed();
                frame[0] += (1024 - (ramp - 2048).abs()) * (level >> 6);
                position = position.wrapping_add(step);
                level += delta;
            }
            *phase = position;
        }
        for frame in block {
            frame[1] = frame[0];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::{midi_note, note_name};
    use fm1_audio::FRAME_RATE;

    const SILENT: crate::midi::Velocities = [0; PianoKey::COUNT as usize];

    #[test]
    fn note_labels_and_concert_pitch_agree() {
        for (index, number, name) in [
            (0, 53, *b"F3 "),
            (7, 60, *b"C4 "),
            (16, 69, *b"A4 "),
            (25, 78, *b"F#5"),
            (26, 79, *b"G5 "),
        ] {
            let key = PianoKey::new(index).expect("physical key");
            assert_eq!(midi_note(key), number);
            assert_eq!(note_name(number), name);
        }
        let a4 = f64::from(STEPS[16]) * f64::from(FRAME_RATE.0)
            / (4_294_967_296.0 * f64::from(FRAME_RATE.1));
        assert!((a4 - 440.0).abs() < 0.001);
    }

    #[test]
    fn releasing_a_chord_preserves_the_remaining_voice() {
        let mut chord = Synth::default();
        let mut solo = Synth::default();
        let mut block = [[0; 2]; OUTPUT_FRAMES];
        let mut reference = block;
        let all_keys = [127; PianoKey::COUNT as usize];
        let mut c4 = SILENT;
        c4[7] = 127;
        for keys in [all_keys, all_keys, c4, c4] {
            block.fill([0; 2]);
            reference.fill([0; 2]);
            chord.render(&keys, &mut block);
            solo.render(&c4, &mut reference);
            assert!(block.iter().any(|f| f[0] != 0));
            assert!(block.iter().all(|f| f[0] == f[1] && f[0].abs() < (1 << 23)));
        }
        assert_eq!(block, reference);
        block.fill([0; 2]);
        chord.render(&SILENT, &mut block); // Release ramp.
        block.fill([0; 2]);
        chord.render(&SILENT, &mut block);
        assert_eq!(block, [[0; 2]; OUTPUT_FRAMES]);
    }
}
