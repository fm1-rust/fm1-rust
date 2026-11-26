extern crate std;
use self::std::{cell::RefCell, vec::Vec};
use super::*;

const RX_START: usize = CON + 0x1c;
const RX_END: usize = CON + 0x20;

struct Uart {
    ring: [u8; 256],
    write: u8,
    since: u32,
    timeout: bool,
    during_copy: Vec<u8>,
}
impl Uart {
    const fn new() -> Self {
        Self {
            ring: [0; 256],
            write: 0,
            since: 0,
            timeout: false,
            during_copy: Vec::new(),
        }
    }
    fn push(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.ring[usize::from(self.write)] = b;
            self.write = self.write.wrapping_add(1);
            self.since = self.since.wrapping_add(1);
        }
    }
}
std::thread_local! { static UART: RefCell<Uart> = const { RefCell::new(Uart::new()) }; }

pub(super) fn set_ring() {
    // Programming RXSADR on every (re)arm resets the model's write cursor.
    UART.with_borrow_mut(|u| *u = Uart::new());
    registers::write(RX_START, 0x01c4_f000);
    registers::write(RX_END, 0x01c4_f100);
}
pub(super) fn capture() -> u32 {
    UART.with_borrow_mut(|u| {
        let watermark = u.since >= WATERMARK;
        if !watermark && !u.timeout {
            return 0;
        }
        u.timeout = false;
        (core::mem::take(&mut u.since) & 0xffff) | if watermark { RX_PENDING } else { 0 }
    })
}
pub(super) fn copy(start: u8, out: &mut [u8]) {
    UART.with_borrow_mut(|u| {
        let mut cursor = start;
        for b in out {
            *b = u.ring[usize::from(cursor)];
            cursor = cursor.wrapping_add(1);
            if !u.during_copy.is_empty() {
                let incoming = core::mem::take(&mut u.during_copy);
                u.push(&incoming);
            }
        }
    });
}
fn burst(bytes: &[u8]) {
    UART.with_borrow_mut(|u| {
        u.push(bytes);
        u.timeout = true;
    });
}

#[test]
fn uart_reads_arbitrary_bytes_and_reports_a_gap_before_publishing() {
    use registers::{DIE, DIR, PD, PH, PU, read, write};
    let board = Board::init();
    write(0x10010, 0x55);
    write(0x51024, 0xff00_00ff);
    let mut rx = board.take_trs_midi().expect("available UART1");
    assert!(board.take_trs_midi().is_none());
    assert_eq!(
        (read(RX_START), read(RX_END), read(RX_COUNT)),
        (0x01c4_f000, 0x01c4_f100, 8)
    );
    assert_eq!((read(CON), read(CON + 8)), (0x41, 383));
    assert_eq!((read(0x10010), read(0x51024)), (0x455, 0xff00_31ff));
    assert_eq!(read(0x51028) & 0xf0, 0x50);
    assert_ne!(
        read(PH + DIE) & read(PH + DIR) & read(PH + PU) & (1 << 8),
        0
    );
    assert_eq!(read(PH + PD) & (1 << 8), 0);
    assert_eq!(serial::Read::read(&mut rx), Err(nb::Error::WouldBlock));

    let alphabet: Vec<_> = (0..=255u8).cycle().take(513).collect();
    let mut output = Vec::new();
    let mut small = [0; 9];
    for batch in alphabet.chunks(37) {
        burst(batch);
        assert_eq!(rx.try_read(&mut []), Ok(0));
        loop {
            let n = rx.try_read(&mut small).expect("timely reads");
            if n == 0 {
                break;
            }
            output.extend_from_slice(&small[..n]);
        }
    }
    assert_eq!(output, alphabet);
    UART.with_borrow_mut(|u| u.push(&[0x80, 60, 0]));
    assert_eq!(rx.try_read(&mut small), Ok(0));
    UART.with_borrow_mut(|u| u.timeout = true);
    assert_eq!(rx.try_read(&mut small), Ok(3));
    assert_eq!(&small[..3], &[0x80, 60, 0]);

    burst(&[4; 240]);
    UART.with_borrow_mut(|u| u.during_copy.extend_from_slice(&[5; 256]));
    small.fill(0xaa);
    assert_eq!(rx.try_read(&mut small), Err(RxError::Overrun));
    assert_eq!(small, [0xaa; 9]);
    assert_eq!(rx.try_read(&mut small), Ok(0));
    burst(&[0xfd]);
    assert_eq!(serial::Read::read(&mut rx), Ok(0xfd));

    // A complete turn of HRXCNT can look like eight fresh bytes.
    UART.with_borrow_mut(|u| u.since = 65_536 + WATERMARK);
    rx.checked.cycles = time::cycles().wrapping_sub(17 * time::Timers::CRYSTAL_HZ);
    assert_eq!(rx.try_read(&mut small), Err(RxError::Overrun));
    drop(rx);
    assert_eq!(read(CON), 0x3400);
    let mut rx = board.take_trs_midi().expect("released UART1");
    assert_eq!(rx.try_read(&mut small), Ok(0));

    #[cfg(feature = "embedded-io")]
    {
        use embedded_io::{Read, ReadReady};
        let mut reader = rx.blocking();
        assert_eq!(reader.read_ready(), Ok(false));
        assert_eq!(reader.read(&mut []), Ok(0));
        burst(&[0xfe]);
        assert_eq!(reader.read_ready(), Ok(true));
        assert_eq!(reader.read(&mut small), Ok(1));
        assert_eq!(small[0], 0xfe);
    }

    #[cfg(feature = "midly")]
    {
        use midly::live::LiveEvent;
        let mut rx = rx.into_midly();
        burst(&[0x90, 60]);
        assert_eq!(rx.read_events(|_| unreachable!()), Ok(2));
        burst(&[0; 256]);
        assert_eq!(rx.read_events(|_| unreachable!()), Err(RxError::Overrun));
        burst(&[100, 61, 100, 0x91, 64, 42]);
        let mut notes = 0u32;
        rx.read_events(|e| {
            assert!(matches!(e, LiveEvent::Midi {channel, message: midly::MidiMessage::NoteOn {key, vel}} if (channel.as_int(),key.as_int(),vel.as_int()) == (1,64,42)));
            notes = notes.wrapping_add(1);
        }).expect("resynchronized");
        assert_eq!(notes, 1);
    }
}
