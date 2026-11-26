//! Optional bounded adapter for the serial MIDI input.
pub use midly;
use midly::{live::LiveEvent, stream::MidiStream};

use super::{READ_CHUNK, RxError, TrsMidiRx};

midly::stack_buffer! {
    struct Buffer([u8; 16]);
}

#[derive(Debug, Default)]
struct Decoder {
    stream: MidiStream<Buffer>,
    common_remaining: u8,
}

impl Decoder {
    fn feed(&mut self, bytes: &[u8], mut event: impl FnMut(LiveEvent<'_>)) {
        for &byte in bytes {
            if (0x80..0xf8).contains(&byte) {
                if self.common_remaining != 0 {
                    // A new status interrupts an incomplete System Common message.
                    *self = Self::default();
                }
                self.common_remaining = match byte {
                    0xf1 | 0xf3 => 1,
                    0xf2 => 2,
                    _ => 0,
                };
            }
            self.stream.feed(&[byte], &mut event);
            // midly 0.5.3 waits for the next non-realtime status to emit
            // System Common. Publish a complete Song Position before a Start
            // byte, even when both arrive in the same UART batch.
            if byte == 0xf6 {
                self.stream.flush(&mut event);
            } else if byte < 0x80 && self.common_remaining != 0 {
                self.common_remaining = self.common_remaining.saturating_sub(1);
                if self.common_remaining == 0 {
                    self.stream.flush(&mut event);
                }
            }
        }
    }
}

/// A TRS receiver with a persistent, heap-free MIDI 1.0 stream parser.
/// It accepts up to 16 SysEx payload bytes. Longer SysEx messages are
/// discarded. Use [`TrsMidiRx::try_read`] for arbitrary-length SysEx.
#[derive(Debug)]
pub struct MidlyRx {
    rx: TrsMidiRx,
    decoder: Decoder,
}

impl MidlyRx {
    pub(super) fn new(rx: TrsMidiRx) -> Self {
        Self {
            rx,
            decoder: Decoder::default(),
        }
    }

    /// Read one batch and call `event` for each completed message, in
    /// wire order. Returns the number of raw bytes consumed, including
    /// incomplete messages. Zero means there is no completed UART batch.
    ///
    /// # Errors
    /// A receive overrun resets all parser state before returning the error.
    pub fn read_events(&mut self, event: impl FnMut(LiveEvent<'_>)) -> Result<usize, RxError> {
        let mut bytes = [0; READ_CHUNK];
        match self.rx.try_read(&mut bytes) {
            Ok(n) => {
                self.decoder.feed(&bytes[..n], event);
                Ok(n)
            }
            Err(e) => {
                self.decoder = Decoder::default();
                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use self::std::vec::Vec;
    use super::*;
    use midly::live::{SystemCommon, SystemRealtime};

    #[test]
    fn messages_keep_their_wire_order_across_reads() {
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        for bytes in [
            &[0x91, 60][..],
            &[0xf8, 100, 61],
            &[0, 0xf2, 1],
            &[2, 0xfa, 0xf3, 9, 0xf6, 0xf1, 0xf8, 0x25],
            &[0xf2, 1, 0x91, 64, 127, 0xf7, 65, 127],
        ] {
            decoder.feed(bytes, |e| {
                let value = match e {
                    LiveEvent::Midi {
                        channel,
                        message: midly::MidiMessage::NoteOn { key, vel },
                    } => {
                        assert_eq!(channel.as_int(), 1);
                        (0x91, u16::from(key.as_int()), vel.as_int())
                    }
                    LiveEvent::Realtime(SystemRealtime::TimingClock) => (0xf8, 0, 0),
                    LiveEvent::Realtime(SystemRealtime::Start) => (0xfa, 0, 0),
                    LiveEvent::Common(SystemCommon::SongPosition(v)) => (0xf2, v.as_int(), 0),
                    LiveEvent::Common(SystemCommon::SongSelect(v)) => {
                        (0xf3, u16::from(v.as_int()), 0)
                    }
                    LiveEvent::Common(SystemCommon::TuneRequest) => (0xf6, 0, 0),
                    LiveEvent::Common(SystemCommon::MidiTimeCodeQuarterFrame(_, v)) => {
                        (0xf1, u16::from(v.as_int()), 0)
                    }
                    _ => unreachable!("unexpected event {e:?}"),
                };
                events.push(value);
            });
        }
        assert_eq!(
            events,
            [
                (0xf8, 0, 0),
                (0x91, 60, 100),
                (0x91, 61, 0),
                (0xf2, 257, 0),
                (0xfa, 0, 0),
                (0xf3, 9, 0),
                (0xf6, 0, 0),
                (0xf8, 0, 0),
                (0xf1, 5, 0),
                (0x91, 64, 127)
            ]
        );
    }

    #[test]
    fn sysex_is_bounded_and_realtime_survives_oversized_messages() {
        let mut decoder = Decoder::default();
        let mut events = Vec::new();
        let mut event = |e: LiveEvent<'_>| match e {
            LiveEvent::Common(SystemCommon::SysEx(data)) => events.push(data.len()),
            LiveEvent::Realtime(SystemRealtime::TimingClock) => events.push(0xf8),
            LiveEvent::Midi { .. } => events.push(0x90),
            _ => unreachable!(),
        };
        for n in [16, 17] {
            decoder.feed(&[0xf0], &mut event);
            decoder.feed(&[1; 17][..n], &mut event);
            decoder.feed(&[0xf8, 0xf7], &mut event);
        }
        decoder.feed(&[0x90, 60, 127], &mut event);
        assert_eq!(events, [0xf8, 16, 0xf8, 0x90]);
    }
}
