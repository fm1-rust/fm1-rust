//! FM-1 UsbLog over native MIDI messages (the host driver handles USB packets).

use core::time::Duration;
use std::{
    io::{self, ErrorKind},
    sync::mpsc::{Receiver, RecvTimeoutError},
};

pub(crate) const REQUEST: &[u8] = &[0xf0, 0x7d, 0xf7];

pub(crate) fn snapshot(message: &[u8]) -> Option<&str> {
    let [0xf0, 0x7d, 0x01, text @ .., 0xf7] = message else {
        return None;
    };
    // UsbLog holds 16 three-byte events minus the four framing bytes.
    if text.len() > 44 || !text.iter().all(|b| matches!(b, b'\n' | b' '..=b'~')) {
        return None;
    }
    str::from_utf8(text).ok()
}

pub(crate) fn request<T>(
    replies: &Receiver<T>,
    timeout: Duration,
    send: impl FnOnce(&[u8]) -> io::Result<()>,
) -> io::Result<T> {
    // The callback queues at most one reply. Discard a previous snapshot before
    // sending. A fast reply may already be waiting when send returns.
    let _ = replies.try_recv();
    send(REQUEST)?;
    replies.recv_timeout(timeout).map_err(|err| match err {
        RecvTimeoutError::Timeout => io::Error::new(
            ErrorKind::TimedOut,
            "no FM-1 log reply. Check that the running firmware serves USB logs",
        ),
        RecvTimeoutError::Disconnected => {
            io::Error::new(ErrorKind::NotConnected, "MIDI input closed")
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn accepts_only_framed_ascii_snapshots() {
        assert_eq!(snapshot(b"\xf0\x7d\x01\xf7"), Some(""));
        assert_eq!(
            snapshot(b"\xf0\x7d\x01ready\nplay+\n\xf7"),
            Some("ready\nplay+\n")
        );
        for invalid in [
            &[0x90, 60, 100][..],
            REQUEST,
            b"\xf0\x7e\x01x\xf7",
            b"\xf0\x7d\x02x\xf7",
            b"\xf0\x7d\x01ready",
            b"\xf0\x7d\x01\x1b\xf7",
            b"\xf0\x7d\x01\x80\xf7",
            b"\xf0\x7d\x01x\xf7\x00",
        ] {
            assert_eq!(snapshot(invalid), None);
        }
        let mut full = [b"\xf0\x7d\x01", &[b'x'; 43][..], b"\n\xf7"].concat();
        assert_eq!(snapshot(&full).map(str::len), Some(44));
        full.insert(3, b'y');
        assert_eq!(snapshot(&full), None);
    }

    #[test]
    fn drops_a_queued_snapshot_and_accepts_an_immediate_reply() {
        let (tx, rx) = mpsc::sync_channel(1);
        tx.try_send("old").expect("empty slot");
        let text = request(&rx, Duration::ZERO, |bytes| {
            assert_eq!(bytes, [0xf0, 0x7d, 0xf7]);
            tx.try_send("new").expect("slot cleared before send");
            Ok(())
        })
        .expect("fresh snapshot");
        assert_eq!(text, "new");
        let missing = request(&rx, Duration::ZERO, |_| Ok(())).expect_err("timeout");
        assert_eq!(missing.kind(), ErrorKind::TimedOut);
        drop(tx);
        let closed = request(&rx, Duration::ZERO, |_| Ok(())).expect_err("disconnect");
        assert_eq!(closed.kind(), ErrorKind::NotConnected);
    }
}
