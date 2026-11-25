//! Transport for the one-shot AC791N write-range experiment.
use super::{DATA_TIMEOUT, PAGE, control_timeout, invalid, open_handle};
use crate::Device;
use core::{num::NonZeroU16, time::Duration};
use rusb::{Context, DeviceHandle, TransferType};
use std::io;

const WND1_HELLO: &[u8; 12] = b"WND1\x01\x02\x80\x00\x40\x00\x00\x20";
const WND3_HELLO: &[u8; 12] = b"WND3\x03\x02\x80\x00\x40\x00\x00\x20";
/// Number of retained bytes across two header pages and the 8 KiB arena.
pub const RECORD_LEN: usize = 8320;
/// Host stimulus on EP1, indexed by the offset in the expected payload.
pub const EP1_DATA: [u8; PAGE] = {
    let mut bytes = [0; PAGE];
    let mut value = 16u8;
    let mut i = 0usize;
    while i < PAGE {
        bytes[i] = value;
        i = i.wrapping_add(1);
        value = value.wrapping_add(1);
    }
    bytes
};
/// Exact eight bytes sent on the wire for the EP0 SETUP stimulus.
pub const EP0_SETUP: [u8; 8] = [0x40, 0x79, 0x32, 0x31, 0x34, 0x33, 0, 0];

/// A window probe verified by its USB product, protocol and vendor bulk alternate.
#[derive(Debug)]
pub struct WindowProbe {
    handle: DeviceHandle<Context>,
    timeout: Duration,
    protocol: &'static str,
}

impl WindowProbe {
    /// Verify the window probe identity before any acquisition command. Readback needs no interface claim.
    ///
    /// # Errors
    /// Returns an error for an invalid timeout, failed USB operation, or incorrect
    /// image or descriptor.
    pub fn open(device: &Device, timeout: Duration) -> io::Result<Self> {
        let timeout = control_timeout(timeout)?;
        let handle = open_handle(device)?;
        let dev = handle.device();
        let descriptor = dev.device_descriptor().map_err(io::Error::other)?;
        let name = handle
            .read_product_string_ascii(&descriptor)
            .map_err(io::Error::other)?;
        if name != "FM-1 USB Window" {
            return Err(invalid(&format!("expected FM-1 USB Window, found {name}")));
        }
        if handle.active_configuration().map_err(io::Error::other)? == 0 {
            handle
                .set_active_configuration(1)
                .map_err(io::Error::other)?;
        }
        let config = dev.active_config_descriptor().map_err(io::Error::other)?;
        let valid = config.number() == 1
            && config
                .interfaces()
                .flat_map(|i| i.descriptors())
                .any(|alt| {
                    alt.interface_number() == 0
                        && alt.setting_number() == 1
                        && alt.class_code() == 0xff
                        && alt.num_endpoints() == 1
                        && alt.endpoint_descriptors().any(|ep| {
                            ep.address() == 1
                                && ep.transfer_type() == TransferType::Bulk
                                && ep.max_packet_size() == 64
                        })
                });
        if !valid {
            return Err(invalid("wrong window probe bulk alternate"));
        }
        let mut hello = [0; 12];
        let count = handle
            .read_control(0xc0, 0x70, 0, 0, &mut hello, timeout)
            .map_err(io::Error::other)?;
        let protocol = match hello.get(..count) {
            Some(bytes) if bytes == WND1_HELLO => "WND1",
            Some(bytes) if bytes == WND3_HELLO => "WND3",
            _ => return Err(invalid("unexpected window probe handshake")),
        };
        Ok(Self {
            handle,
            timeout,
            protocol,
        })
    }

    /// The record format established by the device handshake.
    #[must_use]
    pub fn protocol(&self) -> &'static str {
        self.protocol
    }

    fn read(&self, request: u8, page: u16, bytes: &mut [u8]) -> io::Result<()> {
        let n = self
            .handle
            .read_control(0xc0, request, page, 0, bytes, self.timeout)
            .map_err(io::Error::other)?;
        if n != bytes.len() {
            return Err(invalid("short window probe control reply"));
        }
        Ok(())
    }

    /// Read the retained first page. Its first twelve bytes contain magic, token and case.
    ///
    /// # Errors
    /// Returns an error for an incomplete transfer.
    pub fn header(&self) -> io::Result<[u8; PAGE]> {
        let mut head = [0; PAGE];
        self.read(0x71, 0, &mut head)?;
        Ok(head)
    }

    /// Retrieve all 130 pages for one challenge and verify that page zero stayed stable.
    ///
    /// # Errors
    /// Returns an error if the record is stale or changed, or any transfer fails.
    pub fn snapshot(&self, case: u8, token: NonZeroU16) -> io::Result<Vec<u8>> {
        let head = self.header()?;
        if &head[..4] != self.protocol.as_bytes()
            || head[4..8] != u32::from(token.get()).to_le_bytes()
            || head[8..12] != u32::from(case).to_le_bytes()
        {
            return Err(invalid("pending window record is missing or stale"));
        }
        let mut record = vec![0; RECORD_LEN];
        record[..PAGE].copy_from_slice(&head);
        for (page, bytes) in (1u16..).zip(record[PAGE..].as_chunks_mut::<PAGE>().0) {
            self.read(0x71, page, bytes)?;
        }
        if self.header()? != head {
            return Err(invalid("window record changed during readback"));
        }
        Ok(record)
    }

    /// Claim the vendor interface and enable its bulk alternate before recording a pending challenge.
    ///
    /// # Errors
    /// Returns an error if the interface is busy or the device rejects its alternate.
    pub fn select(&mut self) -> io::Result<()> {
        self.handle.claim_interface(0).map_err(io::Error::other)?;
        self.handle
            .set_alternate_setting(0, 1)
            .map_err(io::Error::other)
    }

    /// Issue GO once. The caller keeps this handle open and the bus quiet during the acquisition.
    ///
    /// # Errors
    /// Returns an error for a failed GO, including an uncertain acknowledgement. Never retry GO.
    pub fn go(&mut self, case: u8, token: NonZeroU16) -> io::Result<()> {
        if case >= 16 {
            return Err(invalid("invalid window case"));
        }
        let n = self
            .handle
            .write_control(
                0x40,
                0x72,
                u16::from(case),
                token.get(),
                &[],
                Duration::from_millis(350),
            )
            .map_err(io::Error::other)?;
        if n != 0 {
            return Err(invalid("unexpected window GO status length"));
        }
        Ok(())
    }

    /// Send one EP0 SETUP after GO. The firmware deliberately leaves its status stage pending.
    ///
    /// # Errors
    /// Returns the host transfer error, including an expected status timeout.
    pub fn setup_stimulus(&mut self) -> io::Result<usize> {
        let [kind, req, vl, vh, il, ih, _, _] = EP0_SETUP;
        self.handle
            .write_control(
                kind,
                req,
                u16::from_le_bytes([vl, vh]),
                u16::from_le_bytes([il, ih]),
                &[],
                DATA_TIMEOUT,
            )
            .map_err(io::Error::other)
    }

    /// Send exactly one legal 64-byte EP1 OUT stimulus after GO.
    ///
    /// # Errors
    /// Returns the host transfer error, including timeout.
    pub fn bulk_stimulus(&mut self) -> io::Result<usize> {
        self.handle
            .write_bulk(1, &EP1_DATA, DATA_TIMEOUT)
            .map_err(io::Error::other)
    }
}
