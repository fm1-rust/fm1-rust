//! Lossless text representation of the existing report and wire records.
extern crate alloc;

use super::{Report, audit_records, hex, invalid};
use alloc::collections::BTreeMap;
use fm1_usb::dma_probe::window::RECORD_LEN;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

const FORMAT: &str = "fm1-usb-window-v1";
const HEADER_LEN: usize = 128;

#[derive(Deserialize, Serialize)]
struct Archive {
    format: String,
    runs: Vec<Run>,
}

#[derive(Deserialize, Serialize)]
struct Run {
    name: String,
    report: Report,
    records: BTreeMap<String, Record>,
}

#[derive(Deserialize, Serialize)]
struct Record {
    header_hex: String,
    arena_fill: u8,
    arena_offset: usize,
    arena_hex: String,
}

impl Record {
    fn pack(bytes: &[u8]) -> io::Result<Self> {
        if bytes.len() != RECORD_LEN {
            return Err(invalid("wrong window record length"));
        }
        let (header, arena) = bytes.split_at(HEADER_LEN);
        // Use an observed byte. Early setup failures can precede the requested fill.
        let fill = arena[0];
        let offset = arena.iter().position(|&b| b != fill).unwrap_or(0);
        let changed = arena
            .iter()
            .rposition(|&b| b != fill)
            .map_or(&[][..], |last| &arena[offset..=last]);
        Ok(Self {
            header_hex: hex(header),
            arena_fill: fill,
            arena_offset: offset,
            arena_hex: hex(changed),
        })
    }

    fn unpack(&self) -> io::Result<Vec<u8>> {
        let mut bytes = vec![self.arena_fill; RECORD_LEN];
        let (header, arena) = bytes.split_at_mut(HEADER_LEN);
        decode_hex(&self.header_hex, header)?;
        let changed = arena
            .get_mut(self.arena_offset..)
            .and_then(|tail| tail.get_mut(..self.arena_hex.len() / 2))
            .ok_or_else(|| invalid("archived bytes extend past the window arena"))?;
        decode_hex(&self.arena_hex, changed)?;
        Ok(bytes)
    }
}

fn decode_hex(hex: &str, dest: &mut [u8]) -> io::Result<()> {
    let (pairs, tail) = hex.as_bytes().as_chunks::<2>();
    if !tail.is_empty() || pairs.len() != dest.len() || !hex.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("invalid hexadecimal window data"));
    }
    for (pair, byte) in pairs.iter().zip(dest) {
        let digits = core::str::from_utf8(pair).map_err(io::Error::other)?;
        *byte = u8::from_str_radix(digits, 16).map_err(io::Error::other)?;
    }
    Ok(())
}

pub(super) fn pack(dirs: &[PathBuf]) -> io::Result<()> {
    let mut runs = Vec::with_capacity(dirs.len());
    for dir in dirs {
        let report = Report::read(dir)?;
        if !report.complete {
            return Err(invalid(
                "capture is unfinished; keep its recovery directory",
            ));
        }
        let mut records = BTreeMap::new();
        audit_records(
            &report,
            |row| {
                let bytes = fs::read(dir.join(&row.file))?;
                records.insert(row.file.clone(), Record::pack(&bytes)?);
                Ok(bytes)
            },
            &mut io::stderr().lock(),
        )?;
        let name = dir
            .canonicalize()?
            .file_name()
            .ok_or_else(|| invalid("capture directory has no name"))?
            .to_string_lossy()
            .into_owned();
        runs.push(Run {
            name,
            report,
            records,
        });
    }
    let archive = Archive {
        format: FORMAT.into(),
        runs,
    };
    let mut out = io::BufWriter::new(io::stdout().lock());
    serde_json::to_writer_pretty(&mut out, &archive)?;
    writeln!(out)?;
    out.flush()
}

pub(super) fn replay(path: &Path) -> io::Result<bool> {
    let archive: Archive = serde_json::from_slice(&fs::read(path)?)?;
    if archive.format != FORMAT || archive.runs.is_empty() {
        return Err(invalid("expected a nonempty fm1-usb-window-v1 archive"));
    }
    let mut good = true;
    let mut log = io::stdout().lock();
    for Run {
        name,
        report,
        mut records,
    } in archive.runs
    {
        writeln!(log, "{name} ({} trials)", report.trials.len())?;
        if !report.complete {
            return Err(invalid("unfinished report in window archive"));
        }
        let (_, passed) = audit_records(
            &report,
            |row| {
                records
                    .remove(&row.file)
                    .ok_or_else(|| invalid("window archive is missing a record"))?
                    .unpack()
            },
            &mut log,
        )?;
        if !records.is_empty() {
            return Err(invalid("unreferenced record in window archive"));
        }
        good &= passed;
    }
    Ok(good)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retains_every_wire_byte_even_when_the_capture_has_gaps() {
        let mut bytes = vec![0xa5; RECORD_LEN];
        for byte in [None, Some(256), Some(RECORD_LEN - 1)] {
            if let Some(index) = byte {
                bytes[index] = 0x42;
            }
            let record = Record::pack(&bytes).expect("complete record");
            assert_eq!(record.unpack().expect("valid encoding"), bytes);
        }
    }

    #[test]
    fn rejects_invalid_hex_and_out_of_bounds_bytes() {
        let mut record = Record::pack(&vec![0; RECORD_LEN]).expect("complete record");
        for bad in ["1", "ø", "+1", "az"] {
            record.arena_hex = bad.into();
            assert!(record.unpack().is_err());
        }
        record.arena_hex = "00".into();
        for offset in [RECORD_LEN - HEADER_LEN, usize::MAX] {
            record.arena_offset = offset;
            assert!(record.unpack().is_err());
        }
        record.arena_offset = 0;
        record.header_hex.truncate(2);
        assert!(record.unpack().is_err());
    }
}
