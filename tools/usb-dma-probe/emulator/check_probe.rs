use fm1_soc::machine::{Machine, Options};
fn until(m: &mut Machine, deadline_ms: u64, mut done: impl FnMut(&Machine) -> bool) {
    let start = m.soc.periph.ms();
    while !done(m) {
        assert!(
            m.soc.periph.ms() - start < deadline_ms,
            "deadline at {} ms events {:?}",
            m.soc.periph.ms(),
            m.soc.usb.events
        );
        m.run(10000).unwrap();
    }
}
fn control(m: &mut Machine, kind: u8, req: u8, value: u16, idx: u16, len: u16) -> Vec<u8> {
    let [v0, v1] = value.to_le_bytes();
    let [i0, i1] = idx.to_le_bytes();
    let [l0, l1] = len.to_le_bytes();
    m.soc.usb.test_request([kind, req, v0, v1, i0, i1, l0, l1]);
    until(m, 100, |m| m.soc.usb.test_ctl.is_some());
    m.soc.usb.test_ctl.take().unwrap().unwrap()
}
fn advance(m: &mut Machine, ms: u64) {
    let target = m.soc.periph.ms() + ms;
    until(m, ms + 10, |m| m.soc.periph.ms() >= target);
}
fn head(m: &mut Machine) -> Vec<u8> {
    let h = control(m, 0xc0, 0x55, 0, 0, 16);
    assert_eq!(&h[..4], b"DLG3");
    h
}
fn record(m: &mut Machine, seq: u16) -> Vec<u8> {
    control(m, 0xc0, 0x56, seq, 0, 32)
}
fn field(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes([bytes[offset], bytes[offset + 1]])
}
fn stress(m: &mut Machine) {
    let info = control(m, 0xc0, 0x50, 0, 0, 12);
    assert_eq!(&info, b"RXS1\x03\x00\x00\x20\x00\x08\xf4\x01");
    let mut seq = 0;
    for (n, code, max, silent, fill) in [
        (0, 0, 0xff, false, 0xa5),
        (69, 0, 0xff, false, 0x5a),
        (2048, 0, 0xff, true, 0xa5),
        (3072, 2, 0x08, true, 0x5a),
        (64, 2, 0x08, false, 0xa5),
    ] {
        seq += 1;
        let alt = if n == 64 { 1 } else { 2 };
        control(m, 1, 11, 0, 0, 0);
        control(m, 1, 11, alt, 0, 0);
        let token = field(&control(m, 0xc0, 0x54, 0, 0, 2), 0);
        control(
            m,
            0x40,
            0x60 + 2 * code,
            n | if fill == 0x5a { 0x8000 } else { 0 },
            token,
            0,
        );
        let armed = control(m, 0xc0, 0x52, 0, token, 24);
        assert_eq!(&armed[2..6], &[1, fill, max, 0]);
        m.soc.usb.test_silent = silent;
        let data = vec![0x39; usize::from(n)];
        if n != 0 {
            m.soc.usb.test_q.push_back(data.clone());
        }
        advance(m, 535);
        let row = record(m, seq);
        assert_eq!(
            (row[2], row[3], row[4], row[6], row[23]),
            (
                3,
                fill,
                max,
                u8::from(n != 0 && !silent),
                u8::from(n != 0 && !silent)
            )
        );
        let mut snap = Vec::new();
        for page in 0..128 {
            snap.extend(control(m, 0xc0, 0x53, page, token, 64));
        }
        assert_eq!(snap.len(), 8192);
        assert_eq!(&snap[..usize::from(n)], data);
        assert!(snap[usize::from(n)..].iter().all(|b| *b == fill));
        let first = field(&row, 30);
        assert_eq!(field(&row, 28), n.checked_sub(1).unwrap_or(0xffff));
        assert_eq!(first, if n > 2048 { 2048 } else { 0xffff });
        println!(
            "injected {n} bytes; silent={silent}, register={max:#04x}, first_guard={first:#06x}"
        );
        if n == 3072 {
            advance(m, 200);
            assert!(m.soc.lcd.fb.iter().any(|&b| b != 0));
            if let Ok(path) = std::env::var("FM1_PROBE_LCD") {
                fm1_soc::write_bmp(&path, &m.soc.lcd.fb, m.soc.lcd.inverted).unwrap();
            }
        }
    }
    let interrupted = field(&control(m, 0xc0, 0x54, 0, 0, 2), 0);
    control(m, 0x40, 0x60, 0, interrupted, 0);
    m.soc.usb.cable = false;
    advance(m, 300);
    m.soc.usb_plug();
    until(m, 1000, |m| m.soc.usb.product().is_some());
    let h = head(m);
    assert_eq!((field(&h, 4), field(&h, 14)), (seq, interrupted));
    control(m, 1, 11, 1, 0, 0);
    let next = field(&control(m, 0xc0, 0x54, 0, 0, 2), 0);
    control(m, 0x40, 0x60, 64, next, 0);
    m.soc.usb.test_q.push_back(vec![0x45; 64]);
    advance(m, 535);
    let row = record(m, seq + 1);
    assert_eq!(
        (row[6], field(&row, 9), field(&row, 28), field(&row, 30)),
        (1, 64, 63, 0xffff)
    );
    println!("re-enumerated after interrupting an arm; received a fresh 64-byte bulk packet");
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let mut xip = vec![0xff; 0x120];
    xip.extend(std::fs::read(&args[1]).unwrap());
    let mut m = Machine::from_image(
        xip,
        fm1_soc::XIP_BASE + 0x120,
        &Options {
            strict: true,
            ..Options::default()
        },
    );
    m.soc.usb_plug();
    until(&mut m, 1000, |m| m.soc.usb.product().is_some());
    let prod = m.soc.usb.product().unwrap().to_owned();
    let conf = m.soc.usb.test_config().to_vec();
    println!("enum {:?}, config {} bytes", prod, conf.len());
    if args[2] == "stress" {
        assert_eq!((prod.as_str(), conf.len()), ("FM-1 DMA Stress", 50));
        stress(&mut m);
        return;
    }
    if args[2] == "stock" {
        assert_eq!(conf.len(), 101);
        std::fs::write(&args[3], &conf).unwrap();
        return;
    }
    if matches!(
        args[2].as_str(),
        "normal" | "normal-count" | "normal-silent" | "normal-ep0"
    ) {
        assert_eq!(conf, std::fs::read(&args[3]).unwrap());
        let cycles = m.soc.periph.cycles;
        let ep0 = m.soc.usb.read(0x11818, cycles);
        let ep1 = m.soc.usb.read(0x11820, cycles);
        assert!(ep0.abs_diff(ep1) >= 2048 + 16);
        let start = ep1.checked_sub(fm1_soc::RAM_BASE).unwrap();
        m.soc.usb.test_q.push_back(vec![0x32; 64]);
        advance(&mut m, 30);
        assert!((0..64).all(|i| m.soc.ram.read8(start + i) == 0x32));
        assert_eq!(control(&mut m, 0x80, 8, 0, 0, 1), [1]);
        match args[2].as_str() {
            "normal" => {
                // Exactly the data budget, no IRQ, no reported count, no guard change.
                m.soc.usb.test_silent = true;
                m.soc.usb.test_q.push_back(vec![0x45; 2048]);
                advance(&mut m, 140);
                assert_eq!(m.soc.ram.read8(start + 2047), 0x45);
                assert_eq!(control(&mut m, 0x80, 8, 0, 0, 1), [1]);
                println!("64-byte MIDI and a silent write contained in staging kept USB online");
                return;
            }
            "normal-count" => m.soc.usb.test_q.push_back(vec![0x32; 65]),
            "normal-silent" => {
                m.soc.usb.test_silent = true;
                m.soc.usb.test_q.push_back(vec![0x45; 2049]);
            }
            "normal-ep0" => {
                // Inject a changed guard word with no EP0 event.
                let offset = ep0 - fm1_soc::RAM_BASE + 2048 + 15;
                m.soc.ram.write8(offset, m.soc.ram.read8(offset) ^ 1);
            }
            _ => unreachable!(),
        }
        // The examples' panic handler leaves this mailbox in retained RAM.
        let mailbox = 0x01c7_fd80 - fm1_soc::RAM_BASE;
        until(&mut m, 300, |m| {
            b"usb_update_mode\0"
                .iter()
                .enumerate()
                .all(|(i, &b)| m.soc.ram.read8(mailbox + i as u32) == b)
        });
        assert_eq!(m.soc.usb.read(0x11800, m.soc.periph.cycles) & 1, 0);
        println!("{} detached USB0 and entered the panic handler", args[2]);
        return;
    }
    if args[2] == "midi" {
        assert_eq!(conf, std::fs::read(&args[3]).unwrap());
        m.soc.usb.midi_send(&[0xf0, 0x7d, 0xf7]);
        let t = m.soc.periph.ms() + 30;
        until(&mut m, 100, |m| m.soc.periph.ms() >= t);
        let replies = m.soc.usb.midi_take();
        let framed = [&[0xf0, 0x7d, 1][..], b"RXD3 MIDI READY\n", &[0xf7]].concat();
        assert!(replies.contains(&framed), "{replies:02x?}");
        println!("configuration byte-identical to stock and MIDI log marker received");
    }
    let info = control(&mut m, 0xc0, 0x50, 0, 0, 12);
    assert_eq!(&info[..4], b"RXD3");
    let profile = info[4];
    assert_eq!(conf.len(), [101, 34, 50][profile as usize]);
    let types = if profile == 2 {
        vec![(2, 69, false), (2, 1023, true)]
    } else {
        vec![
            (profile, 0, false),
            (profile, 64, false),
            (profile, 128, false),
        ]
    };
    let mut retained = Vec::new();
    for (trial, (alt, len, silent)) in (1u16..).zip(types) {
        if alt != 0 {
            control(&mut m, 1, 11, u16::from(alt), 0, 0);
            assert_eq!(control(&mut m, 0x81, 10, 0, 0, 1), [alt]);
        }
        let delayed = len == 128;
        let arm = len as u16 | if delayed { 1 << 13 } else { 0 };
        assert_eq!(control(&mut m, 0xc0, 0x54, 0, 0, 2), trial.to_le_bytes());
        control(&mut m, 0x40, 0x51, arm, trial, 0);
        let pre = control(&mut m, 0xc0, 0x52, 0, trial, 24);
        assert_eq!(pre[2], 1);
        m.soc.usb.test_silent = silent;
        let mut data = (0..len)
            .map(|i| if i < 64 { 0x19 } else { 0x37 })
            .collect::<Vec<u8>>();
        if profile == 0 && len == 64 {
            let msg = [&[0xf0, 0x7d][..], &(1..=45).collect::<Vec<u8>>(), &[0xf7]].concat();
            data = fm1_soc::usb::usb_midi_packets(&msg)
                .into_iter()
                .flatten()
                .collect();
            m.soc.usb.midi_send(&msg);
        } else if len == 128 {
            m.soc.usb.test_q.extend(data.chunks(64).map(|d| d.to_vec()));
        } else if len != 0 {
            m.soc.usb.test_q.push_back(data.clone());
        }
        let target = m.soc.periph.ms() + 520;
        until(&mut m, 600, |m| m.soc.periph.ms() >= target);
        let post = control(&mut m, 0xc0, 0x52, 0, trial, 24);
        assert_eq!(post[2], 3);
        assert_eq!(
            post[6],
            if silent || len == 0 {
                0
            } else if delayed {
                2
            } else {
                1
            }
        );
        if delayed {
            assert!(u16::from_le_bytes([post[16], post[17]]) >= 50);
        }
        let mut arena = Vec::new();
        for p in 0..32 {
            arena.extend(control(&mut m, 0xc0, 0x53, p, trial, 64));
        }
        if len == 1023 {
            assert_eq!(post[23], 0);
        }
        if len != 0 && !silent {
            assert_ne!(post[23], 0);
        }
        if !silent && len != 0 {
            assert_eq!(
                u16::from_le_bytes([post[9], post[10]]),
                if delayed { 64 } else { len as u16 }
            );
        }
        let expected = if delayed { &data[64..] } else { &data[..] };
        assert_eq!(&arena[..expected.len()], expected);
        assert!(arena[expected.len()..].iter().all(|b| *b == 0xa5));
        let h = head(&mut m);
        assert_eq!(field(&h, 4), trial);
        assert_eq!(h[8..12], [trial as u8, profile, 4, 3]);
        let row = record(&mut m, trial);
        assert_eq!(&row[..24], &post);
        assert_eq!(field(&row, 24), len as u16);
        assert_eq!((row[26], row[4]), (0xff, 0xff));
        assert_eq!(
            field(&row, 28),
            expected.len().checked_sub(1).map_or(0xffff, |n| n as u16)
        );
        if expected.len() > 68 {
            assert_eq!((field(&row, 30), row[27]), (68, expected[68]));
        } else {
            assert_eq!(field(&row, 30), 0xffff);
        }
        retained.push(row);
        println!(
            "{} len {len} silent {silent} events {} peak {:?}",
            prod,
            post[6],
            arena.iter().rposition(|b| *b != 0xa5)
        );
    }
    m.soc.usb.test_silent = false;
    if profile == 0 {
        use fm1_soc::periph::{Action, key_id};
        let id = key_id("play").unwrap();
        m.soc.periph.apply(&Action::Key { id, down: true });
        advance(&mut m, 100);
        m.soc.periph.apply(&Action::Key { id, down: false });
        let trial = retained.len() as u16 + 1;
        let h = head(&mut m);
        assert_eq!(
            (field(&h, 4), h[11]),
            (trial - 1, 1),
            "PLAY did not arm: {h:?}"
        );
        m.soc.usb.midi_send(&[0x90, 0x34, 0x40]);
        advance(&mut m, 100);
        let row = record(&mut m, trial);
        assert_eq!(
            (field(&row, 0), field(&row, 24), field(&row, 28)),
            (trial, 0xffff, 3)
        );
        assert_eq!((row[6], field(&row, 9), field(&row, 19)), (1, 4, 4));
        retained.push(row);
        m.soc.usb.midi_send(&[0xf0, 0x7d, 0xf7]);
        advance(&mut m, 40);
        let replies = m.soc.usb.midi_take();
        let framed = [
            &[0xf0, 0x7d, 1][..],
            b"RXD3 MIDI READY\n#4 HI3 G-\n",
            &[0xf7],
        ]
        .concat();
        assert!(replies.contains(&framed), "{replies:02x?}");
        println!("PLAY captured an ordinary MIDI note; standalone MIDI log returned the offsets");
    }
    advance(&mut m, 200);
    assert!(m.soc.lcd.fb.iter().any(|&p| p != 0), "LCD stayed black");
    if let Ok(path) = std::env::var("FM1_PROBE_LCD") {
        fm1_soc::write_bmp(&path, &m.soc.lcd.fb, m.soc.lcd.inverted).unwrap();
    }
    let interrupted = retained.len() as u16 + 1;
    control(&mut m, 0x40, 0x51, 69, interrupted, 0);
    m.soc.usb.cable = false;
    advance(&mut m, 300);
    m.soc.usb_plug();
    until(&mut m, 1000, |m| m.soc.usb.product().is_some());
    let h = head(&mut m);
    assert_eq!(field(&h, 4) as usize, retained.len());
    assert_eq!(field(&h, 14), interrupted);
    for (seq, row) in (1u16..).zip(retained) {
        assert_eq!(record(&mut m, seq), row);
    }
    if profile != 0 {
        control(&mut m, 1, 11, u16::from(profile), 0, 0);
    }
    let next = interrupted + 1;
    control(&mut m, 0x40, 0x51, 0, next, 0);
    advance(&mut m, 530);
    let quiet = record(&mut m, interrupted);
    assert_eq!((field(&quiet, 0), field(&quiet, 28)), (next, 0xffff));
    println!(
        "LCD painted, records survived re-enumeration, and the first arm after interruption captured a clean quiet window"
    );
}
