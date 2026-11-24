// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Leo Kuroshita (@kurogedelic), Hügelton Instruments
//! USB-MIDI 1.0, one virtual cable. Interface 0 is AudioControl and
//! interface 1 is MIDIStreaming. Each bulk endpoint connects to one embedded jack.

use super::{Identity, controller::MAX_PACKET};

#[rustfmt::skip]
const CONFIG: [u8; 101] = [
    9, 2, 101, 0, 2, 1, 0, 0x80, 50,          // Configuration (100 mA)
    9, 4, 0, 0, 0, 1, 1, 0, 0,                // AudioControl interface
    9, 0x24, 1, 0, 1, 9, 0, 1, 1,             // AC 1.0, one streaming interface
    9, 4, 1, 0, 2, 1, 3, 0, 0,                // MIDIStreaming interface
    7, 0x24, 1, 0, 1, 65, 0,                  // MS 1.0 header
    6, 0x24, 2, 1, 1, 0,                      // Embedded MIDI IN jack 1
    6, 0x24, 2, 2, 2, 0,                      // External MIDI IN jack 2
    9, 0x24, 3, 1, 3, 1, 2, 1, 0,             // Embedded MIDI OUT jack 3, from 2
    9, 0x24, 3, 2, 4, 1, 1, 1, 0,             // External MIDI OUT jack 4, from 1
    9, 5, 0x01, 2, 64, 0, 0, 0, 0,            // Bulk OUT (host to FM-1)
    5, 0x25, 1, 1, 1,                         // Cable 0 to jack 1
    9, 5, 0x81, 2, 64, 0, 0, 0, 0,            // Bulk IN (FM-1 to host)
    5, 0x25, 1, 1, 3,                         // Cable 0 from jack 3
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Reply {
    Data(usize),
    Address(u8),
    Configure(bool),
    #[cfg(feature = "usb-dma-probe")]
    Alternate(u8),
    #[cfg(feature = "usb-dma-probe")]
    Arm {
        token: u16,
        max: u8,
        flags: u8,
        requested: u16,
    },
    #[cfg(feature = "usb-window-probe")]
    WindowTrial {
        case: u8,
        token: u16,
    },
    Halt {
        input: bool,
        halted: bool,
    },
    Ack,
    Stall,
}

#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Interface {
    pub(super) configured: bool,
    pub(super) in_halted: bool,
    pub(super) out_halted: bool,
}

#[derive(Debug)]
pub(super) struct Control {
    bytes: [u8; CONFIG.len()],
    offset: usize,
    end: usize,
    zlp: bool,
    pub(super) sending: bool,
}

impl Control {
    pub(super) fn new() -> Self {
        Self {
            bytes: [0; CONFIG.len()],
            offset: 0,
            end: 0,
            zlp: false,
            sending: false,
        }
    }

    pub(super) fn data(&mut self, bytes: &[u8]) -> Reply {
        self.bytes[..bytes.len()].copy_from_slice(bytes);
        Reply::Data(bytes.len())
    }

    #[allow(
        clippy::arithmetic_side_effects,
        reason = "The descriptor has at most 64 bytes; each scalar adds at most four."
    )]
    fn string(&mut self, s: &str) -> Reply {
        // Limit to one packet; keep pairs for non-BMP characters together.
        let mut len = 2;
        for c in s.chars() {
            let mut units = [0; 2];
            let encoded = c.encode_utf16(&mut units);
            let end = len + size_of_val(encoded);
            if end > MAX_PACKET {
                break;
            }
            for (pair, unit) in self.bytes[len..end]
                .as_chunks_mut::<2>()
                .0
                .iter_mut()
                .zip(encoded)
            {
                pair.copy_from_slice(&unit.to_le_bytes());
            }
            len = end;
        }
        self.bytes[0] = u8::try_from(len).expect("one string packet");
        self.bytes[1] = 3;
        Reply::Data(len)
    }

    pub(super) fn device(&mut self, id: &Identity, revision: u8) -> Reply {
        let [vl, vh] = id.vendor_id.to_le_bytes();
        let [pl, ph] = id.product_id.to_le_bytes();
        self.data(&[
            18, 1, 0x10, 1, 0, 0, 0, 64, vl, vh, pl, ph, revision, 1, 1, 2, 0, 1,
        ])
    }

    fn descriptor(&mut self, id: &Identity, value: u16, index: u16) -> Reply {
        match (value, index) {
            (0x0100, 0) => self.device(id, 0),
            (0x0200, 0) => self.data(&CONFIG),
            (0x0300, _) => self.data(&[4, 3, 0x09, 0x04]),
            (0x0301, 0x0409 | 0) => self.string(id.manufacturer),
            (0x0302, 0x0409 | 0) => self.string(id.product),
            _ => Reply::Stall,
        }
    }

    pub(super) fn request(&mut self, id: &Identity, state: Interface, setup: [u8; 8]) -> Reply {
        let [kind, request, vl, vh, il, ih, ll, lh] = setup;
        let value = u16::from_le_bytes([vl, vh]);
        let index = u16::from_le_bytes([il, ih]);
        let len = u16::from_le_bytes([ll, lh]);
        match (kind, request) {
            (0x80, 6) => self.descriptor(id, value, index),
            (0, 5) if value <= 127 && index == 0 && len == 0 && !state.configured => {
                Reply::Address(vl)
            }
            (0, 9) if value <= 1 && index == 0 && len == 0 => Reply::Configure(value == 1),
            (0x80, 8) if value == 0 && index == 0 && len == 1 => {
                self.data(&[u8::from(state.configured)])
            }
            (0x80, 0) if value == 0 && index == 0 && len == 2 => self.data(&[0, 0]),
            (0x81, 0) if state.configured && value == 0 && index <= 1 && len == 2 => {
                self.data(&[0, 0])
            }
            (0x82, 0) if value == 0 && len == 2 => match index {
                0 | 0x80 => self.data(&[0, 0]),
                1 if state.configured => self.data(&[u8::from(state.out_halted), 0]),
                0x81 if state.configured => self.data(&[u8::from(state.in_halted), 0]),
                _ => Reply::Stall,
            },
            (0x81, 10) if state.configured && value == 0 && index <= 1 && len == 1 => {
                self.data(&[0])
            }
            (1, 11) if state.configured && value == 0 && index <= 1 && len == 0 => Reply::Ack,
            (2, 1 | 3)
                if state.configured && value == 0 && matches!(index, 1 | 0x81) && len == 0 =>
            {
                Reply::Halt {
                    input: index == 0x81,
                    halted: request == 3,
                }
            }
            _ => Reply::Stall,
        }
    }

    pub(super) fn start(&mut self, available: usize, requested: u16) {
        self.offset = 0;
        self.end = available.min(usize::from(requested));
        self.zlp = available < usize::from(requested) && self.end.is_multiple_of(MAX_PACKET);
        self.sending = true;
    }

    pub(super) fn chunk(&mut self) -> (&[u8], bool) {
        let start = self.offset;
        self.offset = start.saturating_add(MAX_PACKET).min(self.end);
        let last = self.offset == self.end && (!self.zlp || start == self.end);
        self.sending = !last;
        (&self.bytes[start..self.offset], last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const ID: Identity = Identity {
        vendor_id: 0x1209,
        product_id: 1,
        manufacturer: "fm1-rust",
        product: "FM-1 MIDI Piano",
    };

    #[test]
    fn config_spans_two_packets_and_short_requests_end_immediately() {
        let mut c = Control::new();
        assert_eq!(
            c.request(&ID, Interface::default(), [0x80, 6, 0, 2, 0, 0, 255, 0]),
            Reply::Data(101)
        );
        c.start(101, 255);
        assert_eq!(c.chunk(), (&CONFIG[..64], false));
        assert_eq!(c.chunk(), (&CONFIG[64..], true));
        c.start(101, 9);
        assert_eq!(c.chunk(), (&CONFIG[..9], true));
    }

    #[test]
    fn configured_interfaces_and_bidirectional_endpoint_halt() {
        let mut c = Control::new();
        let state = Interface {
            configured: true,
            in_halted: true,
            out_halted: false,
        };
        assert_eq!(
            c.request(&ID, state, [0x81, 10, 0, 0, 1, 0, 1, 0]),
            Reply::Data(1)
        );
        assert_eq!(c.bytes[0], 0);
        assert_eq!(
            c.request(&ID, state, [0x82, 0, 0, 0, 0x81, 0, 2, 0]),
            Reply::Data(2)
        );
        assert_eq!(&c.bytes[..2], &[1, 0]);
        assert_eq!(
            c.request(&ID, state, [2, 1, 0, 0, 0x81, 0, 0, 0]),
            Reply::Halt {
                input: true,
                halted: false
            }
        );
        assert_eq!(
            c.request(&ID, state, [2, 3, 0, 0, 1, 0, 0, 0]),
            Reply::Halt {
                input: false,
                halted: true
            }
        );
        assert_eq!(
            c.request(&ID, state, [2, 3, 0, 0, 2, 0, 0, 0]),
            Reply::Stall
        );
        assert_eq!(
            c.request(&ID, Interface::default(), [1, 11, 0, 0, 1, 0, 0, 0]),
            Reply::Stall
        );
    }

    #[test]
    fn full_string_ends_with_a_zlp_only_if_host_requested_more() {
        let mut c = Control::new();
        assert_eq!(c.string("12345678901234567890123456789🎹"), Reply::Data(64));
        assert_eq!(&c.bytes[60..64], &[0x3c, 0xd8, 0xb9, 0xdf]);
        c.start(64, 255);
        assert!(!c.chunk().1);
        assert_eq!(c.chunk(), (&[][..], true));
        c.start(64, 64);
        assert!(c.chunk().1);
    }
}
