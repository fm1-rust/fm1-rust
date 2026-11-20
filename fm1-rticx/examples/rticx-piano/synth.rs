use fm1_rticx::bsp::{
    audio::{BLOCK_FRAMES, Block},
    panel::{MasterLevel, PianoKey},
};

// Equal-tempered F3 through G5 at A4 = 440 Hz.
const STEPS: [u32; PianoKey::COUNT as usize] = crate::pitch::phase_steps(174.614_115_716_502);

#[derive(Clone, Copy, Default)]
pub(super) struct Synth {
    phases: [u32; PianoKey::COUNT as usize],
    held: u32,
}

impl Synth {
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "One triangle is ±1024. The 256-frame ramp and 27 voices fit in i32, including the scaled MASTER multiplication."
    )]
    pub(super) fn render(&mut self, held: u32, master: MasterLevel, block: &mut Block) {
        const { assert!(BLOCK_FRAMES == 256 && PianoKey::COUNT <= 32) };
        block.fill([0; 2]);
        for (i, (&step, phase)) in STEPS.iter().zip(&mut self.phases).enumerate() {
            let down = held & (1 << i) != 0;
            let was_down = self.held & (1 << i) != 0;
            if !down && !was_down {
                continue;
            }
            if !was_down {
                *phase = 1 << 30; // Start the triangle at its rising zero crossing.
            }

            // Ramp each voice over one block on attack and release. Held
            // voices keep their phase and amplitude when a chord changes.
            let mut level = if was_down { 256 } else { 0 };
            let delta = i32::from(down) - i32::from(was_down);
            let mut position = *phase;
            for frame in &mut *block {
                let ramp = (position >> 20).cast_signed();
                frame[0] += (1024 - (ramp - 2048).abs()) * level;
                position = position.wrapping_add(step);
                level += delta;
            }
            *phase = position;
        }
        self.held = held;
        for frame in block {
            // Fixed headroom for all 27 voices at full MASTER, in stereo Q23.
            let sample = ((frame[0] >> 2) * i32::from(master.raw())) >> 8;
            *frame = [sample; 2];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitch::{midi_note, note_name};
    use fm1_rticx::bsp::audio::FRAME_RATE;

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
        let mut block = [[0; 2]; BLOCK_FRAMES];
        let mut reference = block;
        let all_keys = (1 << PianoKey::COUNT) - 1;
        let c4 = 1 << 7;
        for keys in [all_keys, all_keys, c4, c4] {
            chord.render(keys, MasterLevel::MAX, &mut block);
            solo.render(c4, MasterLevel::MAX, &mut reference);
            assert!(block.iter().any(|f| f[0] != 0));
            assert!(block.iter().all(|f| f[0] == f[1] && f[0].abs() < (1 << 23)));
        }
        assert_eq!(block, reference);
        chord.render(0, MasterLevel::MAX, &mut block); // Release ramp.
        chord.render(0, MasterLevel::MAX, &mut block);
        assert_eq!(block, [[0; 2]; BLOCK_FRAMES]);
        chord.render(all_keys, MasterLevel::MIN, &mut block);
        assert_eq!(block, [[0; 2]; BLOCK_FRAMES]);
    }
}
