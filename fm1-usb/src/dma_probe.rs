//! Raw host transport for the RXD3 diagnostic. MIDI devices keep their class
//! driver. Only the vendor diagnostic interface is claimed for raw transfers.
extern crate alloc;
mod history;
#[allow(
    unsafe_code,
    reason = "libusb exposes isochronous transfers through its C API."
)]
mod iso;
pub use history::read_log;

use crate::{Device, USB_ID, only};
use alloc::sync::Arc;
use core::time::Duration;
use rusb::{Context, DeviceHandle, TransferType, UsbContext};
use std::io;

/// Descriptor personality reported by the diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Profile {
    /// Stock USB-MIDI timing device.
    Midi,
    /// Vendor interface with a bulk alternate.
    Bulk,
    /// Vendor interface with bulk and isochronous alternates.
    Iso,
    /// RXS1 with an 8 KiB arena and a 2 KiB comparison boundary.
    Stress,
}

/// One packet's libusb completion, including both independent status fields.
#[derive(Debug, Clone, Copy)]
pub struct IsoCompletion {
    /// Overall libusb transfer status (zero means completed).
    pub transfer_status: i32,
    /// Per-packet libusb status (zero means completed).
    pub packet_status: i32,
    /// Per-packet actual length reported by the host controller.
    pub actual: u32,
}

const PAGE: usize = 64;
/// Number of bytes in one complete, frozen EP1 OUT snapshot.
pub const ARENA: usize = 2048;
/// First byte outside the original 64-byte payload and four-byte spare word.
pub const BOUND: usize = 68;
const DATA_TIMEOUT: Duration = Duration::from_millis(200);

/// RXCSR and the complete 16-bit COUNT before the firmware acknowledged a
/// packet, or immediately before it gated EP1 at the end of the trial.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    /// Raw low byte of RXCSR.
    pub csr: u8,
    /// Both bytes of RXCOUNT.
    pub count: u16,
    /// Device time in milliseconds since arming.
    pub ms: u16,
}
impl Sample {
    fn decode([csr, lo, hi, t0, t1]: [u8; 5]) -> Self {
        Self {
            csr,
            count: u16::from_le_bytes([lo, hi]),
            ms: u16::from_le_bytes([t0, t1]),
        }
    }
}

/// An RXD3 trial status reply.
#[derive(Clone, Copy, Debug)]
pub struct Status {
    /// Original response for the measurement record.
    pub raw: [u8; 24],
    /// 0 idle, 1 armed, 2 draining, 3 frozen.
    pub phase: u8,
    /// Byte used to fill the live arena.
    pub sentinel: u8,
    /// Value read back after writing RX_MAX.
    pub max: u8,
    /// Whether firmware deferred service of a ready packet for 50 ms.
    pub delayed: bool,
    /// Number of distinct ready packets seen, saturating at 255.
    pub packets: u8,
    /// Active alternate setting.
    pub alt: u8,
    /// First two packets observed with RXPKTRDY set.
    pub ready: [Sample; 2],
    /// Last register sample before the DMA gate closed, including on an idle endpoint.
    pub at_gate: Sample,
    /// An EP1 OUT interrupt was latched during the observation window.
    pub saw_rx_irq: bool,
}
impl Status {
    /// Decode the register samples from a live or archived 24-byte diagnostic status.
    #[must_use]
    pub fn decode(raw: [u8; 24]) -> Self {
        let [_, _, phase, sentinel, max, delay, packets, alt, rest @ ..] = raw;
        let (samples, _) = rest.as_chunks::<5>();
        Self {
            raw,
            phase,
            sentinel,
            max,
            delayed: delay != 0,
            packets,
            alt,
            ready: [Sample::decode(samples[0]), Sample::decode(samples[1])],
            at_gate: Sample::decode(samples[2]),
            saw_rx_irq: raw[23] != 0,
        }
    }
}

/// A validated RXD3 protocol session on the numeric device found by fm1-usb.
#[derive(Debug)]
pub struct DmaProbe {
    handle: Arc<DeviceHandle<Context>>,
    timeout: Duration,
    profile: Profile,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn open_handle(device: &Device) -> io::Result<DeviceHandle<Context>> {
    let context = Context::new().map_err(io::Error::other)?;
    let list = context.devices().map_err(io::Error::other)?;
    let raw = only(
        list.iter().filter(|d| {
            d.device_descriptor()
                .is_ok_and(|desc| (desc.vendor_id(), desc.product_id()) == USB_ID)
        }),
        "libusb device with ID 1209:0001",
    )?;
    if raw.address() != device.info().device_address() {
        return Err(invalid("USB address changed during discovery"));
    }
    raw.open().map_err(io::Error::other)
}

impl DmaProbe {
    /// Verify the protocol over EP0 before claiming the vendor interface.
    ///
    /// # Errors
    /// Returns an error on device change, failed control request or protocol mismatch.
    pub fn open(device: &Device, timeout: Duration) -> io::Result<Self> {
        let handle = open_handle(device)?;
        let mut hello = [0; 12];
        let len = handle
            .read_control(0xc0, 0x50, 0, 0, &mut hello, timeout)
            .map_err(|e| {
                io::Error::other(format!(
                    "RXD3 handshake failed ({e}). Check the image and its LCD enumeration stage"
                ))
            })?;
        if len != hello.len() {
            return Err(invalid("short diagnostic handshake"));
        }
        let profile = match (&hello[..4], hello[4], &hello[6..]) {
            (b"RXD3", n, [0, 8, 68, 0, 0xf4, 1]) => match n {
                0 => Profile::Midi,
                1 => Profile::Bulk,
                2 => Profile::Iso,
                _ => return Err(invalid("unknown RXD3 profile")),
            },
            (b"RXS1", 3, [0, 32, 0, 8, 0xf4, 1]) => Profile::Stress,
            _ => {
                return Err(invalid(
                    "unsupported diagnostic profile, arena, boundary or window",
                ));
            }
        };
        if profile != Profile::Midi {
            handle.claim_interface(0).map_err(io::Error::other)?;
        }
        Ok(Self {
            handle: Arc::new(handle),
            timeout,
            profile,
        })
    }

    /// Descriptor personality of the running image.
    #[must_use]
    pub fn profile(&self) -> Profile {
        self.profile
    }

    /// Reset EP1 to DATA0 on both sides via alternate zero, then select bulk
    /// (1) or isochronous (2). The physical receiver is bulk-configured in both.
    ///
    /// # Errors
    /// Returns an error for a missing/wrong endpoint or failed SET_INTERFACE.
    pub fn select(&mut self, alt: u8) -> io::Result<()> {
        let (kind, max) = match (self.profile, alt) {
            (Profile::Bulk | Profile::Iso | Profile::Stress, 1) => (TransferType::Bulk, 64),
            (Profile::Iso | Profile::Stress, 2) => (TransferType::Isochronous, 1023),
            _ => return Err(invalid("unsupported EP1 alternate")),
        };
        let config = self
            .handle
            .device()
            .active_config_descriptor()
            .map_err(io::Error::other)?;
        let valid = config
            .interfaces()
            .flat_map(|i| i.descriptors())
            .filter(|i| {
                i.interface_number() == 0 && i.setting_number() == alt && i.class_code() == 0xff
            })
            .any(|i| {
                i.endpoint_descriptors().any(|ep| {
                    ep.address() == 1
                        && ep.transfer_type() == kind
                        && ep.max_packet_size() == max
                        && (alt != 2 || ep.interval() == 1)
                })
            });
        if !valid {
            return Err(invalid(
                "active descriptor differs from requested EP1 transfer",
            ));
        }
        self.handle
            .set_alternate_setting(0, 0)
            .map_err(io::Error::other)?;
        self.handle
            .set_alternate_setting(0, alt)
            .map_err(io::Error::other)
    }

    /// Start a 500 ms observation window. Arms use only the default control pipe.
    ///
    /// # Errors
    /// Returns an error if the device rejects the arm or its status stage fails.
    pub fn arm(&mut self, max: u8, sentinel: u8, delayed: bool, bytes: usize) -> io::Result<u16> {
        let mut id = [0; 2];
        self.read(0x54, 0, 0, &mut id)?;
        let token = u16::from_le_bytes(id);
        if token == 0 {
            return Err(invalid("invalid next trial token"));
        }
        let polarity = match sentinel {
            0xa5 => 0,
            0x5a => 1,
            _ => return Err(invalid("invalid sentinel")),
        };
        if self.profile == Profile::Stress {
            let request: u8 = match max {
                0xff => 0x60,
                0x40 => 0x62,
                0x08 => 0x64,
                0x09 => 0x66,
                0x20 => 0x68,
                _ => return Err(invalid("unsupported stress RX_MAX")),
            };
            let length = u16::try_from(bytes)
                .ok()
                .filter(|n| *n <= 4096)
                .ok_or_else(|| invalid("stress payload exceeds 4096 bytes"))?;
            let request = request | u8::from(delayed);
            self.handle
                .write_control(
                    0x40,
                    request,
                    length | (u16::from(polarity) << 15),
                    token,
                    &[],
                    self.timeout,
                )
                .map_err(io::Error::other)?;
            return Ok(token);
        }
        let flags = polarity | (u8::from(delayed) << 1);
        let register: u16 = match max {
            0xff => 0,
            0x20 => 1,
            0x40 => 2,
            _ => return Err(invalid("unsupported RX_MAX")),
        };
        let length = u16::try_from(bytes)
            .ok()
            .filter(|n| *n <= 1023)
            .ok_or_else(|| invalid("invalid trial length"))?;
        let plan = length | (register << 10) | (u16::from(flags) << 12);
        self.handle
            .write_control(0x40, 0x51, plan, token, &[], self.timeout)
            .map_err(io::Error::other)
            .map(|_| token)
    }

    fn read(&self, request: u8, value: u16, token: u16, bytes: &mut [u8]) -> io::Result<()> {
        let n = self
            .handle
            .read_control(0xc0, request, value, token, bytes, self.timeout)
            .map_err(io::Error::other)?;
        if n != bytes.len() {
            return Err(invalid("short diagnostic control reply"));
        }
        Ok(())
    }

    /// Read the trial token, phase and three pre-flush register samples.
    ///
    /// # Errors
    /// Returns an error on a short, stale or failed status reply.
    pub fn status(&self, token: u16) -> io::Result<Status> {
        let mut status = [0; 24];
        self.read(0x52, 0, token, &mut status)?;
        if status[..2] != token.to_le_bytes() {
            return Err(invalid("stale trial token"));
        }
        Ok(Status::decode(status))
    }

    /// Download every page of the CPU-owned frozen snapshot for this token.
    ///
    /// # Errors
    /// Returns an error if the snapshot expires, the session resets or a page is short.
    pub fn snapshot(&self, token: u16) -> io::Result<Vec<u8>> {
        let mut bytes = vec![
            0;
            if self.profile == Profile::Stress {
                8192
            } else {
                ARENA
            }
        ];
        for (page, dest) in (0u16..).zip(bytes.as_chunks_mut::<PAGE>().0) {
            self.read(0x53, page, token, dest)?;
        }
        Ok(bytes)
    }

    /// Submit one bulk transfer. The host splits a 128-byte buffer at the
    /// advertised 64-byte maximum. The two packets can contain distinct data.
    ///
    /// # Errors
    /// Returns the host transfer error, including timeout.
    pub fn bulk(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.handle
            .write_bulk(1, bytes, DATA_TIMEOUT)
            .map_err(io::Error::other)
    }

    /// Schedule exactly one full-speed isochronous OUT packet.
    ///
    /// # Errors
    /// Returns an error on invalid length, submission failure or event backend failure.
    pub fn iso(&mut self, bytes: &[u8]) -> io::Result<IsoCompletion> {
        Ok(iso::send(&self.handle, &[bytes])?[0])
    }

    /// Schedule up to four full-speed OUT packets in consecutive service intervals.
    ///
    /// # Errors
    /// Returns an error on invalid packet lengths or a libusb submission or dispatch failure.
    pub fn iso_train(&mut self, packets: &[&[u8]]) -> io::Result<Vec<IsoCompletion>> {
        iso::send(&self.handle, packets)
    }
}
