use super::*;
extern crate std;
use self::std::vec::Vec;
use static_cell::ConstStaticCell;

#[test]
fn host_sees_capture_and_the_original_midi_jacks() {
    static BUFFER: ConstStaticCell<CaptureBuffer> = ConstStaticCell::new(CaptureBuffer::new());
    let (_, mut stream) = BUFFER.take().split();
    let mut cfg = Control::new();
    let id = Identity {
        vendor_id: 0x1209,
        product_id: 1,
        manufacturer: "fm1-rust",
        product: "WaveGraph FM-1",
    };
    let off = Interface::default();
    let on = Interface {
        configured: true,
        ..off
    };
    let request = [0x80, 6, 0, 2, 0, 0, 255, 0];
    let Some(Reply::Data(n)) = stream.request(&mut cfg, &id, off, request) else {
        unreachable!()
    };
    cfg.start(n, 255);
    let mut data = Vec::new();
    loop {
        let (part, last) = cfg.chunk();
        data.extend_from_slice(part);
        if last {
            break;
        }
    }
    assert_eq!(n, usize::from(u16::from_le_bytes([data[2], data[3]])));
    assert_eq!(data[4], 3);
    let mut number = 0;
    let mut alt = 0;
    let mut interfaces = Vec::new();
    let mut eps = Vec::new();
    let mut bytes = data.as_slice();
    while !bytes.is_empty() {
        let (d, rest) = bytes.split_at(usize::from(bytes[0]));
        match d[1] {
            4 => {
                number = d[2];
                alt = d[3];
                interfaces.push((number, alt, d[4], d[6]));
            }
            5 => eps.push((number, alt, d[2], d[3], d[4], d[6])),
            0x24 if number == 0 && d[2] == 1 => assert_eq!(&d[5..], &[31, 0, 2, 1, 2]),
            0x24 if number == 1 && d[2] == 2 => {
                assert_eq!(&d[3..], &[1, 2, 2, 16, 1, 0x44, 0xac, 0])
            }
            _ => (),
        }
        bytes = rest;
    }
    assert_eq!(
        interfaces,
        [(0, 0, 0, 1), (1, 0, 0, 2), (1, 1, 1, 2), (2, 0, 2, 3)]
    );
    assert_eq!(
        eps,
        [
            (1, 1, 0x84, 5, 184, 1),
            (2, 0, 1, 2, 64, 0),
            (2, 0, 0x81, 2, 64, 0)
        ]
    );
    let alt1 = [1, 11, 1, 0, 1, 0, 0, 0];
    assert_eq!(stream.request(&mut cfg, &id, off, alt1), None);
    assert_eq!(cfg.request(&id, off, alt1), Reply::Stall);
    assert_eq!(
        stream.request(&mut cfg, &id, on, alt1),
        Some(Reply::CaptureAlternate(true))
    );
    stream.select(true);
    assert_eq!(
        stream.request(&mut cfg, &id, on, [0x81, 10, 0, 0, 1, 0, 1, 0]),
        Some(Reply::Data(1))
    );
    cfg.start(1, 1);
    assert_eq!(cfg.chunk(), (&[1][..], true));
    assert_eq!(
        stream.request(&mut cfg, &id, on, [1, 11, 0, 0, 2, 0, 0, 0]),
        Some(Reply::Ack)
    );
    assert_eq!(
        cfg.request(&id, on, [2, 3, 0, 0, 0x84, 0, 0, 0]),
        Reply::Stall
    );
}

#[test]
fn capture_is_stereo_bounded_and_restarts_with_fresh_samples() {
    static BUFFER: ConstStaticCell<CaptureBuffer> = ConstStaticCell::new(CaptureBuffer::new());
    let (mut writer, mut rx) = BUFFER.take().split();
    let old = [[i32::MIN, i32::MAX]; 512];
    let fresh = [[0x123456, -256]; 256];
    let mut data = [0x55; MAX_BYTES];
    assert_eq!(writer.write_q23(&old), 0);
    rx.select(true);
    assert_eq!(writer.write_q23(&old), SLOTS as usize);
    assert_eq!(writer.write_q23(&fresh), 0);
    let n = usize::from(rx.packet(0, &mut data));
    assert!(
        data[..n]
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p == &[0, 0x80, 0xff, 0x7f])
    );
    rx.select(false);
    assert_eq!(writer.write_q23(&fresh), 0);
    rx.select(true);
    let n = usize::from(rx.packet(24_000, &mut data));
    assert!(data[..n].iter().all(|&b| b == 0));
    assert_eq!(writer.write_q23(&fresh), 256);
    let n = usize::from(rx.packet(48_000, &mut data));
    assert!(
        data[..n]
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p == &[0x34, 0x12, 0xff, 0xff])
    );
    let n = usize::from(rx.packet(2_000_000, &mut data));
    assert!(data[..n].iter().all(|&b| b == 0));
}

#[test]
#[allow(
    clippy::arithmetic_side_effects,
    reason = "The fixture models at most 12 seconds at a bounded clock offset."
)]
fn paced_packets_keep_order_across_clock_drift_and_cursor_wrap() {
    // Miri still crosses both cursor laps and several packet-size periods.
    // Native tests exercise the full 12 seconds of accumulated clock drift.
    let (duration_ms, min_received) = if cfg!(miri) {
        (96, 3_840)
    } else {
        (12_000, 500_000)
    };
    static BUFFER: ConstStaticCell<CaptureBuffer> = ConstStaticCell::new(CaptureBuffer::new());
    let (mut writer, mut rx) = BUFFER.take().split();
    for scale in [998_800u64, 1_000_000, 1_001_200] {
        rx.select(true);
        let mut ticks = 0u64;
        let mut produced = 0u32;
        let mut received = 0u32;
        let mut packet = [0; MAX_BYTES];
        let sample = |i: u32| i32::try_from(i % 30_000 + 1).expect("bounded ramp");
        for ms in 0u32..duration_ms {
            ticks += 24_000 * scale;
            if ticks >= 128 * 544 * 1_000_000 {
                ticks -= 128 * 544 * 1_000_000;
                let block: [[i32; 2]; 128] = core::array::from_fn(|_| {
                    let v = sample(produced) << 8;
                    produced += 1;
                    [v, -v]
                });
                assert_eq!(writer.write_q23(&block), 128);
            }
            let n = usize::from(rx.packet(ms * 24_000, &mut packet));
            assert!(matches!(n, 172 | 176 | 180 | 184));
            for &[a, b, c, d] in packet[..n].as_chunks::<4>().0 {
                let l = i32::from(i16::from_le_bytes([a, b]));
                let r = i32::from(i16::from_le_bytes([c, d]));
                if received == 0 && l == 0 {
                    assert_eq!(r, 0);
                    continue;
                }
                assert_eq!((l, r), (sample(received), -sample(received)));
                received += 1;
            }
        }
        assert!(received > min_received && produced - received < SLOTS);
    }
}
