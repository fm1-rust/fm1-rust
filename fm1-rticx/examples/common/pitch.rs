use fm1_rticx::bsp::{audio::FRAME_RATE, panel::PianoKey};

pub(super) const FIRST_NOTE: u8 = 53;

#[allow(
    clippy::arithmetic_side_effects,
    reason = "Physical key indices are 0..=26."
)]
pub(super) fn midi_note(key: PianoKey) -> u8 {
    // Middle C is 60. The physical keyboard starts on the F below it.
    FIRST_NOTE + key.index()
}

#[allow(
    clippy::arithmetic_side_effects,
    reason = "MIDI notes 41..=91 give octave digits 2..=6."
)]
pub(super) fn note_name(note: u8) -> [u8; 3] {
    const NAMES: [[u8; 2]; 12] = [
        *b"C ", *b"C#", *b"D ", *b"D#", *b"E ", *b"F ", *b"F#", *b"G ", *b"G#", *b"A ", *b"A#",
        *b"B ",
    ];
    let [letter, accidental] = NAMES[usize::from(note % 12)];
    let octave = b'0' + (note / 12 - 1);
    if accidental == b'#' {
        [letter, accidental, octave]
    } else {
        [letter, octave, b' ']
    }
}

// Equal-tempered phase increments for the BSP frame rate, starting at first_hz.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "Every rounded phase increment is positive and fits in u32."
)]
pub(super) const fn phase_steps<const N: usize>(first_hz: f64) -> [u32; N] {
    let mut steps = [0; N];
    let mut step = first_hz * 4_294_967_296.0 * FRAME_RATE.1 as f64 / FRAME_RATE.0 as f64;
    let mut i = 0;
    while i < steps.len() {
        steps[i] = (step + 0.5) as u32;
        step *= 1.059_463_094_359_295_3; // 2^(1/12)
        i = i.wrapping_add(1);
    }
    steps
}
