//! Read the board's retained diagnostic records over EP0. Reading this log
//! neither claims an interface nor sends traffic to the measured endpoint.
use super::{Status, invalid};
use crate::Device;
use core::time::Duration;
use std::io::{self, Write};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use nusb::{
    MaybeFuture,
    transfer::{ControlIn, ControlType, Recipient, TransferError},
};

/// Read the on-board probe log, or return None for firmware that stalls this
/// request. All records and the index use read-only control transfers.
///
/// # Errors
/// Returns an error for a short page, a log overwritten during reading, or a USB error.
pub fn read_log(device: &Device, timeout: Duration) -> io::Result<Option<String>> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        let handle = device.info().open().wait().map_err(io::Error::other)?;
        read_records(|request, seq, length| {
            let [a, b, c, d] = seq.to_le_bytes();
            let req = ControlIn {
                control_type: ControlType::Vendor,
                recipient: Recipient::Device,
                request,
                value: u16::from_le_bytes([a, b]),
                index: u16::from_le_bytes([c, d]),
                length,
            };
            match handle.control_in(req, timeout).wait() {
                Ok(data) => Ok(Some(data)),
                Err(TransferError::Stall) => Ok(None),
                Err(e) => Err(io::Error::other(e)),
            }
        })
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let handle = super::open_handle(device)?;
        read_records(|request, seq, length| {
            let [a, b, c, d] = seq.to_le_bytes();
            let mut buf = vec![0; usize::from(length)];
            match handle.read_control(
                0xc0,
                request,
                u16::from_le_bytes([a, b]),
                u16::from_le_bytes([c, d]),
                &mut buf,
                timeout,
            ) {
                Ok(n) => {
                    buf.truncate(n);
                    Ok(Some(buf))
                }
                Err(rusb::Error::Pipe) => Ok(None),
                Err(e) => Err(io::Error::other(e)),
            }
        })
    }
}

fn read_records(
    mut read: impl FnMut(u8, u32, u16) -> io::Result<Option<Vec<u8>>>,
) -> io::Result<Option<String>> {
    let Some(head) = read(0x55, 0, 16)? else {
        return Ok(None);
    };
    if !head.starts_with(b"DLG3") {
        return Ok(None);
    }
    let h: [u8; 16] = head
        .try_into()
        .map_err(|_| invalid("short probe log header"))?;
    let total = u32::from_le_bytes([h[4], h[5], h[6], h[7]]);
    let count = h[8];
    if count > 64 {
        return Err(invalid("invalid probe log count"));
    }
    let profile = match h[9] {
        0 => "MIDI",
        1 => "BULK",
        2 => "ISO",
        _ => return Err(invalid("unknown probe profile")),
    };
    let mut out = Vec::new();
    writeln!(out, "RXD3 {profile} READY")?;
    writeln!(
        out,
        "enum={} retries={} phase={} completed={} retained={} last_aborted={}",
        h[10],
        u16::from_le_bytes([h[12], h[13]]),
        h[11],
        total,
        count,
        u16::from_le_bytes([h[14], h[15]])
    )?;
    for age in (0..u32::from(count)).rev() {
        let seq = total.wrapping_sub(age);
        let data =
            read(0x56, seq, 32)?.ok_or_else(|| invalid("log advanced or USB reset during read"))?;
        let row: [u8; 32] = data
            .try_into()
            .map_err(|_| invalid("short probe log entry"))?;
        let (raw, extra) = row
            .split_first_chunk::<24>()
            .ok_or_else(|| invalid("short probe status"))?;
        let st = Status::decode(*raw);
        let token = u16::from_le_bytes([raw[0], raw[1]]);
        let optional = |a, b| {
            let v = u16::from_le_bytes([a, b]);
            (v != u16::MAX).then_some(v)
        };
        let req = optional(extra[0], extra[1]);
        let high = optional(extra[4], extra[5]);
        let guard = optional(extra[6], extra[7]);
        let word = |n: Option<u16>| n.map_or_else(|| "-".to_owned(), |x| x.to_string());
        writeln!(
            out,
            "#{seq} token={token} alt={} requested={} RX_MAX={:#04x}/{:#04x} fill={:#04x} delayed={}",
            st.alt,
            req.map_or_else(|| "manual".to_owned(), |n| n.to_string()),
            extra[2],
            st.max,
            st.sentinel,
            st.delayed
        )?;
        writeln!(
            out,
            "  ready={} irq={} first(csr,count,ms)=({:#04x},{},{}) second=({:#04x},{},{}) gate=({:#04x},{},{})",
            st.packets,
            st.saw_rx_irq,
            st.ready[0].csr,
            st.ready[0].count,
            st.ready[0].ms,
            st.ready[1].csr,
            st.ready[1].count,
            st.ready[1].ms,
            st.at_gate.csr,
            st.at_gate.count,
            st.at_gate.ms
        )?;
        writeln!(
            out,
            "  highest={} first_guard={} guard_byte={}",
            word(high),
            word(guard),
            guard.map_or_else(|| "-".to_owned(), |_| format!("{:#04x}", extra[3]))
        )?;
    }
    String::from_utf8(out).map(Some).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_the_retained_range_and_reports_a_silent_write() {
        let mut header = vec![0; 16];
        header[..4].copy_from_slice(b"DLG3");
        header[4] = 79;
        header[8] = 2;
        header[9] = 2;
        let mut queries = Vec::new();
        let text = read_records(|req, id, _| {
            if req == 0x55 {
                return Ok(Some(header.clone()));
            }
            queries.push(id);
            let mut row = vec![0; 32];
            row[2] = 3;
            row[3] = 0xa5;
            row[4] = 0xff;
            row[7] = 2;
            row[24] = 69;
            row[26] = 0xff;
            row[27] = 0x19;
            row[28] = 68;
            row[30] = 68;
            Ok(Some(row))
        })
        .expect("log")
        .expect("probe");
        assert_eq!(queries, [78, 79]);
        assert!(text.contains("ready=0 irq=false"));
        assert!(text.contains("highest=68 first_guard=68 guard_byte=0x19"));
    }
}
