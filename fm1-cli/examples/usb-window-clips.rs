//! Generate the four Standard MIDI Files for the WaveGraph mixer test.
use std::{fs, io, io::Write, path::Path};

// The four-bar clips use only one- and two-byte MIDI delta times.
fn delta(track: &mut Vec<u8>, ticks: u16) {
    if ticks >= 128 {
        track.push(0x80 | u8::try_from(ticks >> 7).expect("delta fits in two MIDI bytes"));
    }
    track.push(ticks.to_le_bytes()[0] & 0x7f);
}

fn main() -> io::Result<()> {
    let out = Path::new("target/usb-window-clips");
    fs::create_dir_all(out)?;
    for (channel, name, filename, notes) in [
        (0u8, "Bass low", "01-bass-low.mid", [48, 48]),
        (1, "Bass high", "02-bass-high.mid", [60, 60]),
        (2, "808", "03-808.mid", [36, 42]),
        (3, "909", "04-909.mid", [39, 51]),
    ] {
        let bar = usize::from(channel);
        let mut track = vec![
            0,
            0xff,
            0x03,
            u8::try_from(name.len()).map_err(io::Error::other)?,
        ];
        track.extend_from_slice(name.as_bytes());
        track.extend([0, 0xff, 0x51, 3, 7, 0xa1, 0x20]); // 120 BPM
        track.extend([0, 0xff, 0x58, 4, 4, 2, 24, 8]); // 4/4

        let mut delay = [0, 384, 768, 1152][bar];
        for (note, cutoff) in notes.into_iter().zip([32, 100]) {
            delta(&mut track, delay);
            delay = 96;
            if channel < 2 {
                track.extend([0xb0 | channel, 74, cutoff, 0]);
            }
            track.extend([0x90 | channel, note, 105, 96, 0x80 | channel, note, 0]);
        }
        delta(&mut track, [1248, 864, 480, 96][bar]);
        track.extend([0xff, 0x2f, 0]);

        let mut file = fs::File::create(out.join(filename))?;
        // Format 0, one track, 96 ticks per quarter note.
        file.write_all(b"MThd\x00\x00\x00\x06\x00\x00\x00\x01\x00\x60MTrk")?;
        let length = u32::try_from(track.len()).map_err(io::Error::other)?;
        file.write_all(&length.to_be_bytes())?;
        file.write_all(&track)?;
    }
    Ok(())
}
