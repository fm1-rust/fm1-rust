use super::*;
use midir::os::unix::{VirtualInput, VirtualOutput};

#[test]
#[cfg_attr(
    target_os = "linux",
    ignore = "requires an ALSA sequencer or JACK server"
)]
fn virtual_midi_round_trip() -> Result<(), Box<dyn core::error::Error>> {
    let name = format!("FM1-test-{}", std::process::id());
    let timeout = Duration::from_secs(2);
    let mut source = MidiOutput::new(&name)?.create_virtual(&name)?;
    let _destination = MidiInput::new(&name)?.create_virtual(
        &name,
        move |_, bytes, page: &mut Option<u8>| match bytes {
            [0xb0, 112, n] => *page = Some(*n),
            [0xf0, 0x7d, 0xf7] => {
                let text = match *page {
                    Some(0) => "24MHZ AUDIO 128F PANEL 10KHZ\n".to_owned(),
                    Some(n) => format!("page {n}\n"),
                    None => "ready\n".to_owned(),
                };
                let reply = [b"\xf0\x7d\x01", text.as_bytes(), b"\xf7"].concat();
                source.send(&[0xf8]).expect("interleaved clock");
                source.send(&reply).expect("log reply");
            }
            _ => source.send(bytes).expect("MIDI echo"),
        },
        None,
    )?;

    // Limit discovery to our own ports even if real instruments are attached.
    let deadline = std::time::Instant::now();
    let ports = loop {
        match MidiPorts::by_name(&name) {
            Ok(ports) => break ports,
            Err(_) if deadline.elapsed() < timeout => std::thread::sleep(Duration::from_millis(10)),
            Err(e) => return Err(e.into()),
        }
    };
    let (ports, text) = probe_ports(&[ports.input], vec![ports.output], timeout)?;
    assert_eq!(text, "ready\n");
    let (tx, rx) = mpsc::channel();
    let mut midi = ports.connect(move |_, bytes| {
        if bytes.first() == Some(&0x90) {
            tx.send(bytes.to_vec()).expect("test still listening");
        }
    })?;
    let note = [0x90, 60, 100];
    midi.send(&note)?;
    assert_eq!(rx.recv_timeout(timeout)?, note);
    assert_eq!(midi.read_log(timeout)?, "ready\n");
    let report = midi.read_timing(timeout)?;
    assert_eq!(report.lines().count(), 25);
    assert!(report.ends_with("page 24\n"));
    Ok(())
}
