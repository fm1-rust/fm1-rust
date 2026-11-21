use super::*;

#[test]
fn snapshot_uses_the_correct_end_packet_for_each_length() {
    for (line, end) in [
        ("", [6, b'\n', 0xf7, 0]),
        ("a", [7, b'a', b'\n', 0xf7]),
        ("ab", [5, 0xf7, 0, 0]),
    ] {
        let mut log = UsbLog::new();
        log.record(line).expect("valid line");
        log.service_with(Some(0), &[REQUEST], |packets| {
            assert_eq!(packets.first(), Some(&[4, 0xf0, 0x7d, 1]));
            assert_eq!(packets.last(), Some(&end));
            if line == "ab" {
                assert_eq!(packets[1], [4, b'a', b'b', b'\n']);
            }
            packets.len()
        });
    }
}

#[test]
fn full_history_evicts_whole_lines_and_rejects_invalid_writes() {
    let mut log = UsbLog::new();
    log.record("one").expect("valid");
    log.record("two").expect("valid");
    let max_line = core::str::from_utf8(&[b'x'; UsbLog::MAX_LINE_LEN]).expect("ASCII");
    let line = &max_line[4..];
    log.record(line).expect("fits beside two");
    assert_eq!(&log.text.make_contiguous()[..4], b"two\n");
    assert_eq!(log.text.len(), TEXT_BYTES);
    let full = log.text.clone();
    for s in [
        "this record deliberately exceeds forty-three bytes",
        "bad\nline",
        "café",
        "\x7f",
    ] {
        assert_eq!(log.record(s), Err(InvalidLogLine));
        assert_eq!(log.text, full);
    }
    log.record(max_line).expect("maximum");
    log.service_with(Some(1), &[REQUEST], |packets| {
        assert_eq!(packets.len(), MAX_EVENTS);
        assert_eq!(packets.last(), Some(&[7, b'x', b'\n', 0xf7]));
        MAX_EVENTS
    });
}

#[test]
fn retries_keep_the_snapshot_and_session_changes_discard_pending_data() {
    let mut log = UsbLog::new();
    let note = [9, 0x90, 60, 100];
    log.record("a").expect("valid");
    log.service_with(Some(1), &[note, [7, 0xf0, 0x7e, 0xf7]], |_| unreachable!());
    log.service_with(Some(1), &[note, REQUEST], |_| 0);
    log.record("b").expect("valid");
    log.service_with(Some(1), &[], |packets| {
        assert_eq!(packets, [[4, 0xf0, 0x7d, 1], [7, b'a', b'\n', 0xf7]]);
        1
    });
    log.service_with(Some(1), &[], |packets| {
        assert_eq!(packets, [[7, b'a', b'\n', 0xf7]]);
        1
    });
    log.service_with(Some(1), &[REQUEST], |_| 0);
    log.service_with(None, &[REQUEST], |_| unreachable!());
    log.service_with(Some(2), &[], |_| unreachable!());
    log.service_with(Some(2), &[REQUEST], |packets| {
        assert_eq!(
            packets,
            [
                [4, 0xf0, 0x7d, 1],
                [4, b'a', b'\n', b'b'],
                [6, b'\n', 0xf7, 0]
            ]
        );
        packets.len()
    });
}
