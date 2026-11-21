use fm1_rticx::bsp::panel::PianoKey;

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
