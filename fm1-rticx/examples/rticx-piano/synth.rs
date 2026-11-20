use fm1_rticx::bsp::{
    audio::{BLOCK_FRAMES, Block, FRAME_RATE},
    panel::{MasterLevel, PianoKey},
};

#[allow(
    clippy::arithmetic_side_effects,
    reason = "Physical key indices are 0..=26."
)]
pub(super) fn midi_note(key: PianoKey) -> u8 {
    // Middle C is 60. The physical keyboard starts on the F below it.
    53 + key.index()
}

#[allow(
    clippy::arithmetic_side_effects,
    reason = "MIDI notes 53..=79 give octave digits 3..=5."
)]
pub(super) fn note_name(key: PianoKey) -> [u8; 3] {
    const NAMES: [[u8; 2]; 12] = [
        *b"C ", *b"C#", *b"D ", *b"D#", *b"E ", *b"F ", *b"F#", *b"G ", *b"G#", *b"A ", *b"A#",
        *b"B ",
    ];
    let note = midi_note(key);
    let [letter, accidental] = NAMES[usize::from(note % 12)];
    let octave = b'0' + (note / 12 - 1);
    if accidental == b'#' {
        [letter, accidental, octave]
    } else {
        [letter, octave, b' ']
    }
}

// Equal-tempered F3 through G5 at A4 = 440 Hz. Compute the table at compile
// time for the BSP's actual frame rate. The audio interrupt only uses integers.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Every rounded phase increment is positive and fits in u32."
)]
const STEPS: [u32; PianoKey::COUNT as usize] = {
    let mut steps = [0; PianoKey::COUNT as usize];
    let mut step =
        174.614_115_716_502 * 4_294_967_296.0 * FRAME_RATE.1 as f64 / FRAME_RATE.0 as f64;
    let mut i = 0;
    while i < steps.len() {
        steps[i] = (step + 0.5) as u32;
        step *= 1.059_463_094_359_295_3; // 2^(1/12)
        i += 1;
    }
    steps
};

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
            assert_eq!(note_name(key), name);
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
