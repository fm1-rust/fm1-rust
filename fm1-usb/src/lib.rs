#![doc = include_str!("../README.md")]
#![deny(unsafe_code)]

#[cfg(feature = "dma-probe")]
pub mod dma_probe;
mod log;
mod midi;
mod timing;

pub use midi::{MidiConnection, MidiPorts};
pub use nusb::DeviceInfo;

use core::time::Duration;
use nusb::MaybeFuture;
use std::io::{self, ErrorKind};

/// USB vendor and product IDs used by the FM-1 development firmware.
pub const USB_ID: (u16, u16) = (0x1209, 0x0001);

/// A connected USB device with the FM-1 development identity.
#[derive(Clone, Debug)]
pub struct Device(DeviceInfo);

impl Device {
    /// Find the sole USB device using [`USB_ID`].
    ///
    /// # Errors
    /// Returns an error if USB enumeration fails or the match is missing or ambiguous.
    pub fn find() -> io::Result<Self> {
        let devices = nusb::list_devices().wait().map_err(io::Error::other)?;
        let matches = devices.filter(|d| (d.vendor_id(), d.product_id()) == USB_ID);
        only(matches, "USB device with ID 1209:0001").map(Self)
    }

    /// USB descriptors and location reported by the host.
    #[must_use]
    pub fn info(&self) -> &DeviceInfo {
        &self.0
    }

    /// Locate the MIDI input and output that answer the log request, and return
    /// their snapshot. A name filter explicitly selects a unique port in each
    /// direction. Automatic discovery can probe other MIDI outputs with the
    /// FM-1 log request. `timeout` applies to each request.
    ///
    /// # Errors
    /// Returns an error if the ports cannot be opened or no valid reply arrives.
    pub fn log_ports(
        &self,
        port: Option<&str>,
        timeout: Duration,
    ) -> io::Result<(MidiPorts, String)> {
        if let Some(name) = port {
            let ports = MidiPorts::by_name(name)?;
            let snapshot = ports.connect(|_, _| {})?.read_log(timeout)?;
            return Ok((ports, snapshot));
        }
        midi::probe(self.0.product_string(), timeout)
    }
}

fn only<T>(mut matches: impl Iterator<Item = T>, what: &str) -> io::Result<T> {
    let first = matches
        .next()
        .ok_or_else(|| io::Error::new(ErrorKind::NotFound, format!("no {what} found")))?;
    if matches.next().is_some() {
        return Err(io::Error::new(
            ErrorKind::InvalidInput,
            format!("multiple matches for {what}"),
        ));
    }
    Ok(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_development_id_requires_exactly_one_device() {
        let find = |devices: &[((u16, u16), u8)]| {
            only(
                devices
                    .iter()
                    .filter(|(id, _)| *id == USB_ID)
                    .map(|(_, address)| *address),
                "device",
            )
        };
        let bootloader = ((0x4c4a, 0x8057), 1);
        assert_eq!(
            find(&[bootloader]).expect_err("bootloader").kind(),
            ErrorKind::NotFound
        );
        assert_eq!(find(&[bootloader, (USB_ID, 2)]).expect("one match"), 2);
        assert_eq!(
            find(&[(USB_ID, 1), (USB_ID, 2)])
                .expect_err("ambiguous")
                .kind(),
            ErrorKind::InvalidInput
        );
    }
}
