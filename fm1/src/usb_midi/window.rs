//! One-shot WL82 comparator experiment. Only the vendor bench personality
//! recognizes these requests. The application never lends Rust memory to USB0.
use super::{
    Identity, MidiDevice, ProbeProfile, UsbPort,
    control::{Control, Interface, Reply},
    controller,
};

// Pages 0..2 hold the 128-byte header. Pages 2..130 hold the 8192-byte capture.
const PAGES: u16 = 130;
const HELLO: &[u8; 12] = b"WND3\x03\x02\x80\x00\x40\x00\x00\x20";

#[cfg(fm1)]
#[link(name = "fm1_window", kind = "static")]
unsafe extern "C" {
    fn fm1_window_run(case: u32, token: u32) -> !;
    fn fm1_window_page(page: u32, output: *mut u8);
}

impl UsbPort {
    /// Start the WND3 vendor-bulk bench firmware. One GO takes over USB0 and
    /// the exception vector until soft reset. PLAY in the example returns to the flasher.
    ///
    /// # Safety
    /// Uphold the normal EP0/EP1 bounds and ownership rules of
    /// [`Self::assume_midi_dma`] on an isolated board for every attachment.
    /// CPU0 must exclusively own its interrupt configuration, vector 1, debug
    /// slot 7 and global fault rearm, USB0, TIMER4 and retained SRAM
    /// 0x01c7c000..0x01c7e080. CPU1 must be inactive. Through GO, every
    /// CPU0 interrupt source except TIMER5 and exception vector 1 must be
    /// disabled. A guarded shot masks TIMER5 and requests exception delivery
    /// before opening the global IRQ gates. TIMER4 ends the shot even when
    /// the CPU does not enter the exception handler.
    /// The sole host must finish each scheduled transfer before the 1 s capture
    /// ends. All writes, including retries and trailers, must stay in their
    /// reserved allocations even with slot 7 disabled. Gating and detaching
    /// USB0 must quiesce old writes within the 10 ms drain before the CPU
    /// copies the 8 KiB arena. These premises cover faults, reset, cable changes
    /// and any later use of the handle. The comparator under test supplies none
    /// of them. This firmware takes ownership of the reserved record on boot.
    ///
    /// # Panics
    /// A changed guard from an earlier USB session is fatal.
    #[must_use]
    pub unsafe fn into_window_probe(self, id: Identity) -> MidiDevice {
        // SAFETY: the caller supplies the normal DMA guarantees plus exclusive
        // diagnostic resources. Only standard vendor interface selection is
        // reused. WND3 intercepts every vendor request before the RXD3 handler.
        let mut device = unsafe { self.into_dma_probe(id, ProbeProfile::Bulk) };
        device.window = true;
        device
    }
}

fn page(index: u16) -> Option<[u8; 64]> {
    if index >= PAGES {
        return None;
    }
    let mut bytes = [0; 64];
    #[cfg(fm1)]
    // SAFETY: only a window probe handle calls this. The fixed retained record belongs
    // to this firmware. Capture never returns into this boot, so no writer is
    // live. C copies exactly 64 initialized byte values into this local array.
    unsafe {
        fm1_window_page(u32::from(index), bytes.as_mut_ptr());
    }
    #[cfg(not(fm1))]
    bytes.fill(0);
    Some(bytes)
}

impl MidiDevice {
    /// The retained window probe header for a local display. A zero magic means no
    /// committed record. The Mac runner checks the nonce and the full checksum.
    #[must_use]
    pub fn window_header(&self) -> Option<[u8; 64]> {
        if self.window { page(0) } else { None }
    }

    /// The register address, requested word and observed word when firmware
    /// aborted a shot on a failed setup check.
    #[must_use]
    pub fn window_setup_check(&self) -> Option<(u32, u32, u32)> {
        let header = self.window_header()?;
        let status = u32::from_le_bytes(header.as_chunks::<4>().0[3]);
        if !(0x10..=0x1a).contains(&status) {
            return None;
        }
        let page = page(1)?;
        let words = page.as_chunks::<4>().0;
        Some((
            u32::from_le_bytes(words[9]),
            u32::from_le_bytes(words[11]),
            u32::from_le_bytes(words[12]),
        ))
    }
}

pub(super) fn request(c: &mut Control, state: Interface, alt: u8, setup: [u8; 8]) -> Reply {
    let [kind, req, vl, vh, il, ih, ll, lh] = setup;
    let value = u16::from_le_bytes([vl, vh]);
    let token = u16::from_le_bytes([il, ih]);
    if !state.configured {
        return Reply::Stall;
    }
    match (kind, req, ll, lh) {
        (0xc0, 0x70, 12, 0) if value == 0 && token == 0 => c.data(HELLO),
        (0xc0, 0x71, 64, 0) if token == 0 => page(value).map_or(Reply::Stall, |p| c.data(&p)),
        // Bits 0..1 select off / in-range / aligned / within-beat. Bit 2
        // selects EP0 instead of EP1. Bit 3 selects the complementary fill.
        (0x40, 0x72, 0, 0) if alt == 1 && !state.out_halted && value < 16 && token != 0 => {
            Reply::WindowTrial { case: vl, token }
        }
        _ => Reply::Stall,
    }
}

pub(super) fn run(case: u8, token: u16) -> controller::Result<()> {
    #[cfg(fm1)]
    // SAFETY: only the privately marked window probe returns WindowTrial. The
    // USB owner has copied a validated, eight-byte GO SETUP, still selected
    // EP0, and has not acknowledged it. No other task selects its registers.
    // The unsafe constructor supplies exclusive bench resources and the DMA
    // bounds independently of slot 7. C never resumes this service frame.
    unsafe {
        fm1_window_run(u32::from(case), u32::from(token))
    }
    #[cfg(not(fm1))]
    {
        let _ = (case, token);
        controller::wr(controller::CSR0, 0x60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_requires_the_selected_alternate_and_a_bounded_challenge() {
        let mut c = Control::new();
        let mut state = Interface::default();
        let mut go = [0x40, 0x72, 15, 0, 0x34, 0x12, 0, 0];
        assert_eq!(request(&mut c, state, 1, go), Reply::Stall);
        state.configured = true;
        assert_eq!(request(&mut c, state, 0, go), Reply::Stall);
        state.out_halted = true;
        assert_eq!(request(&mut c, state, 1, go), Reply::Stall);
        state.out_halted = false;
        assert_eq!(
            request(&mut c, state, 1, go),
            Reply::WindowTrial {
                case: 15,
                token: 0x1234
            }
        );
        for (offset, value) in [(2, 16), (3, 1), (6, 1), (7, 1), (0, 0xc0), (1, 0x79)] {
            let mut invalid = go;
            invalid[offset] = value;
            assert_eq!(request(&mut c, state, 1, invalid), Reply::Stall);
        }
        go[4..6].fill(0);
        assert_eq!(request(&mut c, state, 1, go), Reply::Stall);
    }

    #[test]
    fn retained_reads_stop_at_the_end_of_the_record() {
        let mut c = Control::new();
        let state = Interface {
            configured: true,
            ..Interface::default()
        };
        let read = |p| [0xc0, 0x71, p, 0, 0, 0, 64, 0];
        assert_eq!(request(&mut c, state, 0, read(129)), Reply::Data(64));
        assert_eq!(request(&mut c, state, 0, read(130)), Reply::Stall);
        assert_eq!(
            request(&mut c, state, 0, [0x40, 0x70, 0xff, 0, 1, 0, 0, 0]),
            Reply::Stall
        );
    }
}
