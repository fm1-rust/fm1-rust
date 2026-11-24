//! Classify one frozen WND1 or WND3 record using the USB observations and SRAM bytes.
use super::{Endpoint, Row, Step, invalid};
use core::fmt;
use fm1_usb::dma_probe::window::{EP0_SETUP, EP1_DATA, RECORD_LEN};
use serde::{Deserialize, Serialize, de::value::SeqDeserializer};
use std::io;

const WND3: u32 = u32::from_le_bytes(*b"WND3");
const SLOT: u32 = 1 << 7;
const NONE: u32 = u32::MAX;

/// Fields in firmware wire order. The archived JSON preserves the original names.
#[derive(Debug, Deserialize, Serialize)]
pub(super) struct Header {
    tag: u32,
    token: u32,
    case: u32,
    result: u32,
    dest: u32,
    cut: u32,
    fill: u32,
    first: u32,
    at_cut: u32,
    last: u32,
    cause: u32,
    slots: u32,
    master: u32,
    wid: u32,
    trapped: u32,
    emu: u32,
    low: u32,
    high: u32,
    device: u32,
    limit_mask: u32,
    allow_mask: u32,
    out_mask: u32,
    dbg_enable: u32,
    initial_msg: u32,
    bus: u32,
    count: u32,
    #[serde(rename = "address_readback")]
    address_word: u32, // WND1 MMIO readback; WND3 CPU address of arena[0]
    csr0: u32,
    rxcsr: u32,
    started_go: u32,
    elapsed: u32,
    hash: u32,
}

#[derive(Default)]
pub(super) struct Controls {
    quiet_off: bool,
    quiet: bool,
    in_range: bool,
    baseline: Option<(u32, Vec<u8>)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Quiet,
    Delivered,
    ObservedStop,
    CrossedWithFault,
    CrossedWithoutAttribution,
    Inconclusive(&'static str),
}
impl Outcome {
    pub(super) fn passed(self) -> bool {
        matches!(self, Self::Quiet | Self::Delivered | Self::ObservedStop)
    }
}
impl fmt::Display for Outcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Quiet => "QUIET",
            Self::Delivered => "DELIVERED",
            Self::ObservedStop => "OBSERVED_STOP",
            Self::CrossedWithFault => "CROSSED_WITH_FAULT",
            Self::CrossedWithoutAttribution => "CROSSED_WITHOUT_ATTRIBUTED_FAULT",
            Self::Inconclusive(reason) => return write!(f, "INCONCLUSIVE: {reason}"),
        })
    }
}

fn index(n: Option<usize>) -> u32 {
    n.and_then(|i| u32::try_from(i).ok()).unwrap_or(NONE)
}

impl Header {
    fn prefix(&self) -> usize {
        if self.tag == WND3 && self.case & 8 != 0 {
            128
        } else {
            64
        }
    }

    pub(super) fn json(&self) -> io::Result<serde_json::Value> {
        let mut value = serde_json::to_value(self)?;
        if self.tag == WND3 {
            let fields = value
                .as_object_mut()
                .expect("header serializes to an object");
            fields.remove("address_readback");
            fields.insert("arena_base".into(), self.address_word.into());
        }
        Ok(value)
    }

    fn failed_check(&self) -> Option<&'static str> {
        Some(match self.result {
            0x10 => "exception vector",
            0x11 => "TIMER5 disable",
            0x12 => "exception enable",
            0x13 => "EP0_ADR readback (write-only)",
            0x14 => "EP1_RADR readback (write-only)",
            0x15 => "slot lower bound",
            0x16 => "slot upper bound",
            0x17 => "slot master ID",
            0x18 => "slot allow enable",
            0x19 => "slot out-of-range error enable",
            0x1a => "slot limit enable",
            _ => return None,
        })
    }

    pub(super) fn show(
        &self,
        log: &mut impl io::Write,
        file: &str,
        outcome: Outcome,
    ) -> io::Result<()> {
        if let Some(check) = self.failed_check() {
            writeln!(
                log,
                "{file} INCONCLUSIVE: {check} at {:#010x}, requested {:#010x}, read {:#010x}",
                self.count, self.csr0, self.rxcsr
            )
        } else {
            writeln!(log, "{file} {outcome}")
        }
    }

    pub(super) fn decode(data: &[u8], row: &Row) -> io::Result<Self> {
        if data.len() != RECORD_LEN || !(data.starts_with(b"WND1") || data.starts_with(b"WND3")) {
            return Err(invalid("missing committed window record"));
        }
        let words = data[..128]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&bytes| u32::from_le_bytes(bytes));
        let h = Header::deserialize(SeqDeserializer::<_, serde::de::value::Error>::new(words))
            .map_err(io::Error::other)?;
        if h.case != u32::from(row.case) || h.token != u32::from(row.token.get()) {
            return Err(invalid("wrong trial identity"));
        }
        let arena = &data[128..];
        let checksum = data[4..124]
            .iter()
            .chain(arena)
            .fold(2166136261u32, |v, &byte| {
                (v ^ u32::from(byte)).wrapping_mul(16777619)
            });
        if h.hash != checksum {
            return Err(invalid("record checksum mismatch"));
        }
        let cut = row.cut();
        let fill = row.fill();
        if h.cut != u32::from(cut) || h.fill != u32::from(fill) {
            return Err(invalid("wrong cut/fill"));
        }
        let tail = &arena[h.prefix()..];
        let first = tail
            .iter()
            .enumerate()
            .skip(usize::from(cut))
            .find(|(_, b)| **b != fill)
            .map(|(i, _)| i);
        if h.first != index(first)
            || h.last != index(tail.iter().rposition(|&b| b != fill))
            || h.at_cut != u32::from(tail[usize::from(cut)])
        {
            return Err(invalid("CPU summary disagrees with frozen bytes"));
        }
        Ok(h)
    }

    fn assess(&self, row: &Row, arena: &[u8], controls: &Controls) -> Outcome {
        use Outcome::Inconclusive;
        let h = self;
        let ep0 = row.shot.endpoint == Endpoint::Ep0;
        let quiet = row.shot.name.is_quiet();
        let cut = usize::from(row.cut());
        let payload: &[u8] = if ep0 { &EP0_SETUP } else { &EP1_DATA };
        let prefix = self.prefix();
        let tail = &arena[prefix..];
        if h.result != 1 {
            return Inconclusive(match h.result {
                2 => "slot 7 in use",
                3 => "register readback",
                4 => "GO/exception during setup or cleanup",
                5 => "timer/drain",
                6 => "preexisting range fault",
                7 => "CPU1 running",
                _ if h.tag == WND3 => h.failed_check().unwrap_or("state"),
                _ => "state", // preserve the archived R1/R2 verdict; show() expands the code
            });
        }
        if row.host_quiet_complete == Some(false) {
            return Inconclusive("host acquisition interrupted");
        }
        if row.go != "ok" || h.started_go != 1 || h.elapsed < 24_000_000 {
            return Inconclusive("GO or acquisition interval");
        }
        let Some(base) = h
            .dest
            .checked_sub(u32::try_from(prefix).expect("64 or 128"))
        else {
            return Inconclusive("DMA destination");
        };
        let expected_word = if h.tag == WND3 { base } else { h.dest };
        if !base.is_multiple_of(64)
            || !(0x01c4_0000..=0x01c7_a000).contains(&base)
            || h.address_word != expected_word
            || h.tag == WND3
                && controls
                    .baseline
                    .as_ref()
                    .is_some_and(|(dest, _)| h.dest != *dest)
        {
            return Inconclusive("DMA destination");
        }
        if arena[..prefix].iter().any(|&b| b != row.fill()) {
            return Inconclusive("bytes before DMA base changed");
        }
        if h.initial_msg & 4 != 0
            || h.bus == NONE
            || h.bus & 0x27 != 0
            || h.csr0 == NONE
            || !ep0 && h.csr0 & 1 != 0
        {
            return Inconclusive("bus change, stale fault or unexpected SETUP");
        }
        if row.case & 3 != 0 {
            let upper = h.dest.wrapping_add(h.cut).wrapping_sub(1);
            if h.low != h.dest
                || h.high >> 3 != upper >> 3
                || h.device != 0x81
                || h.limit_mask & h.allow_mask & h.out_mask & SLOT == 0
            {
                return Inconclusive("comparator configuration");
            }
            if !row.shot.name.is_control() && h.high != upper {
                return Inconclusive("high-address readback differs from the requested boundary");
            }
        } else if (h.limit_mask | h.allow_mask | h.out_mask) & SLOT != 0 {
            return Inconclusive("control slot enabled");
        }
        let new_cause = h.cause & !h.initial_msg;
        let fault = new_cause == 4
            && h.slots == SLOT
            && h.master == 0x81
            && matches!(h.wid, 0 | 0x81)
            && h.emu == 0
            && h.limit_mask & !SLOT == 0;
        if quiet {
            if h.trapped == 0
                && new_cause == 0
                && h.last == NONE
                && h.csr0 & 1 == 0
                && (ep0 || h.rxcsr != NONE && h.rxcsr & 1 == 0 && h.count == 0)
            {
                return Outcome::Quiet;
            }
            return Inconclusive("quiet capture changed or signaled");
        }
        let submitted = !matches!(row.stimulus.as_str(), "skipped" | "not attempted");
        if row.shot.name.is_control() {
            if !submitted || h.trapped != 0 || new_cause != 0 || !tail.starts_with(payload) {
                return Inconclusive("positive control failed to reproduce the payload");
            }
            if h.count as usize != payload.len() || h.last >= 128 {
                return Inconclusive("positive control count or extent");
            }
            if row.shot.name == Step::InRange
                && controls.baseline.as_ref().map(|(_, data)| data.as_slice()) != Some(arena)
            {
                return Inconclusive("in-range capture differs from comparator-off control");
            }
            return Outcome::Delivered;
        }
        if !(controls.quiet_off
            && controls.quiet
            && controls.in_range
            && controls.baseline.is_some())
        {
            return Inconclusive("missing controls");
        }
        if !submitted {
            return Inconclusive("missing host stimulus");
        }
        if h.first != NONE {
            let i = h.first as usize; // decode checked this against an actual changed byte in the arena
            if payload.get(i) != Some(&tail[i]) || !tail.starts_with(&payload[..4]) {
                return Inconclusive("unexpected changed bytes");
            }
            return if fault {
                Outcome::CrossedWithFault
            } else {
                Outcome::CrossedWithoutAttribution
            };
        }
        if tail[..cut] == payload[..cut] && fault {
            return Outcome::ObservedStop;
        }
        Inconclusive("no crossing, incomplete prefix or unattributed fault")
    }
}

impl Controls {
    pub(super) fn can_attempt(&self, step: Step) -> bool {
        let have_unprotected = self.quiet_off && self.baseline.is_some();
        match step {
            Step::QuietOff | Step::Baseline => true,
            Step::InRange | Step::Quiet => have_unprotected,
            Step::Aligned | Step::WithinBeat => have_unprotected && self.in_range && self.quiet,
        }
    }

    pub(super) fn evaluate(&mut self, row: &Row, data: &[u8]) -> io::Result<(Header, Outcome)> {
        let header = Header::decode(data, row)?;
        let arena = &data[128..];
        let result = header.assess(row, arena, self);
        match (row.shot.name, result) {
            (Step::QuietOff, Outcome::Quiet) => self.quiet_off = true,
            (Step::Quiet, Outcome::Quiet) => self.quiet = true,
            (Step::Baseline, Outcome::Delivered) => {
                self.baseline = Some((header.dest, arena.to_vec()))
            }
            (Step::InRange, Outcome::Delivered) => self.in_range = true,
            _ => {}
        }
        Ok((header, result))
    }
}
