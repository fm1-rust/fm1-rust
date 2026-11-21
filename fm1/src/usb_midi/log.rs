//! Bounded ASCII log snapshots on USB-MIDI cable zero.

use super::{EventPacket, MAX_EVENTS, MidiDevice};
use core::{fmt, iter::once};
use heapless::{Deque, Vec};

// Three MIDI bytes per event. Reserve F0 7D 01 for the header and F7 for the end.
const TEXT_BYTES: usize = MAX_EVENTS * 3 - 4;
const REQUEST: EventPacket = [0x07, 0xf0, 0x7d, 0xf7];

/// The line exceeds [`UsbLog::MAX_LINE_LEN`] or contains non-printable ASCII.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidLogLine;

impl fmt::Display for InvalidLogLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "expected at most {} printable ASCII bytes",
            UsbLog::MAX_LINE_LEN
        )
    }
}

impl core::error::Error for InvalidLogLine {}

/// Whole log lines retained in RAM for non-destructive reads over USB-MIDI.
///
/// Own one log per MIDI device. Its history survives a USB reconnect and newer
/// lines evict the oldest complete lines when space runs out. Recording a line
/// only touches RAM. [`Self::service`] handles host requests in the USB owner.
#[derive(Debug, Default)]
pub struct UsbLog {
    text: Deque<u8, TEXT_BYTES>,
    pending: Vec<EventPacket, MAX_EVENTS>,
    session: Option<u32>,
}

impl UsbLog {
    /// Maximum printable ASCII bytes per line, excluding its trailing newline.
    pub const MAX_LINE_LEN: usize = TEXT_BYTES - 1;

    /// Create an empty log.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            text: Deque::new(),
            pending: Vec::new(),
            session: None,
        }
    }

    /// Append one line and its newline.
    ///
    /// # Errors
    /// Returns [`InvalidLogLine`] for an overlong line or any byte outside
    /// ASCII space through tilde. A rejected write preserves the history.
    pub fn record(&mut self, line: &str) -> Result<(), InvalidLogLine> {
        if line.len() > Self::MAX_LINE_LEN || !line.bytes().all(|b| matches!(b, b' '..=b'~')) {
            return Err(InvalidLogLine);
        }
        while TEXT_BYTES.saturating_sub(self.text.len()) <= line.len() {
            // Evict one complete line, including its newline.
            while self.text.pop_front().is_some_and(|b| b != b'\n') {}
        }
        self.text.extend(line.bytes().chain(once(b'\n')));
        Ok(())
    }

    /// Inspect the application's received packets and try one nonblocking send.
    ///
    /// Call after each USB service pass, including passes with empty input, so
    /// a busy endpoint can retry. The caller retains all received MIDI events.
    /// A new session discards any outstanding reply and keeps the log.
    pub fn service(&mut self, device: &mut MidiDevice, received: &[EventPacket]) {
        self.service_with(device.session(), received, |packets| device.send(packets));
    }

    fn service_with(
        &mut self,
        session: Option<u32>,
        received: &[EventPacket],
        send: impl FnOnce(&[EventPacket]) -> usize,
    ) {
        if self.session != session {
            self.pending.clear();
            self.session = session;
        }
        if session.is_none() {
            return;
        }
        if self.pending.is_empty() && received.contains(&REQUEST) {
            let (chunks, tail) = self.text.make_contiguous().as_chunks::<3>();
            let end = match tail {
                [a, b] => [7, *a, *b, 0xf7],
                [a] => [6, *a, 0xf7, 0],
                _ => [5, 0xf7, 0, 0],
            };
            self.pending = once([4, 0xf0, 0x7d, 1])
                .chain(chunks.iter().map(|&[a, b, c]| [4, a, b, c]))
                .chain([end])
                .collect();
        }
        if !self.pending.is_empty() {
            let sent = send(&self.pending);
            self.pending.drain(..sent);
        }
    }
}

#[cfg(test)]
mod tests;
