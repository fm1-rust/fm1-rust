//! Exercise the production boundary with a separately instrumented 8 KiB arena.
use core::time::Duration;
use fm1_usb::{
    Device,
    dma_probe::{DmaProbe, Profile, Status, read_log},
};
use serde_json::{Value, json};
use std::{fs, io, path::Path, thread, time::Instant};

mod replay;
pub(super) use replay::replay;

const BOUND: usize = 2048;
const ARENA: usize = 8192;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Traffic {
    Quiet,
    Iso(&'static [usize]),
    BulkTrain,
    Recover,
}

#[derive(Clone, Copy, Debug)]
struct Case {
    traffic: Traffic,
    max: u8,
    delayed: bool,
}

fn cases() -> Vec<Case> {
    let mut list = Vec::new();
    for max in [0xff, 0x40, 0x08, 0x09] {
        let mut add = |traffic, delayed| {
            list.push(Case {
                traffic,
                max,
                delayed,
            })
        };
        add(Traffic::Quiet, false);
        if max == 0xff {
            add(Traffic::Iso(&[69]), false);
        }
        if matches!(max, 0x08 | 0x09) {
            for n in [&[64][..], &[65], &[72], &[73]] {
                add(Traffic::Iso(n), false);
            }
        }
        add(Traffic::Iso(&[1023]), false);
        add(Traffic::Iso(&[1023; 4]), false);
        add(Traffic::Iso(&[1023; 4]), true);
        if matches!(max, 0xff | 0x08) {
            add(Traffic::Iso(&[1023, 64]), false);
        }
        if max == 0xff {
            add(Traffic::BulkTrain, false);
        }
        add(Traffic::Recover, false);
    }
    list
}

// 16 well-formed USB-MIDI events, eight matched note-on/off pairs.
fn notes() -> Vec<u8> {
    (48..56u8)
        .flat_map(|i| [9, 0x90, i, 80, 8, 0x80, i, 0])
        .collect()
}

fn payload(len: usize, packet: u8) -> Vec<u8> {
    let mut x = 0x4389_ac51u32 ^ u32::from(packet).wrapping_mul(0x9e37_79b1);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            match x.to_le_bytes()[1] {
                0xa5 => 0xa4,
                0x5a => 0x5b,
                b => b,
            }
        })
        .collect()
}

impl Case {
    fn alt(self) -> u8 {
        if matches!(self.traffic, Traffic::Recover | Traffic::BulkTrain) {
            1
        } else {
            2
        }
    }
    fn packets(self) -> Vec<Vec<u8>> {
        match self.traffic {
            Traffic::Quiet => Vec::new(),
            Traffic::Recover => vec![notes()],
            Traffic::BulkTrain => (0..64).map(|i| payload(64, i)).collect(),
            Traffic::Iso(lengths) => (0u8..).zip(lengths).map(|(i, &n)| payload(n, i)).collect(),
        }
    }
}

fn transmit(p: &mut DmaProbe, c: Case, data: &[Vec<u8>]) -> Value {
    if c.traffic == Traffic::Quiet {
        return json!({"pipe":"none"});
    }
    if c.alt() == 1 {
        return match p.bulk(&data.concat()) {
            Ok(n) => json!({"pipe":"bulk","actual":n}),
            Err(e) => json!({"pipe":"bulk","error":e.to_string()}),
        };
    }
    let parts: Vec<_> = data.iter().map(Vec::as_slice).collect();
    match p.iso_train(&parts) {
        Ok(done) => json!({"pipe":"iso","packets":done.iter().map(|r| json!({
            "transfer_status":r.transfer_status,"packet_status":r.packet_status,"actual":r.actual
        })).collect::<Vec<_>>()}),
        Err(e) => json!({"pipe":"iso","error":e.to_string()}),
    }
}

// Each recorded libusb completion has the same meaning in a live run and in a replay.
fn transfer_complete(c: Case, lengths: &[usize], completion: &Value) -> bool {
    if completion.get("error").is_some() {
        return false;
    }
    if c.traffic == Traffic::Quiet {
        return completion["pipe"] == "none";
    }
    if c.alt() == 1 {
        return completion["pipe"] == "bulk"
            && completion["actual"] == lengths.iter().sum::<usize>();
    }
    completion["pipe"] == "iso"
        && completion["packets"].as_array().is_some_and(|packets| {
            packets.len() == lengths.len()
                && packets.iter().zip(lengths).all(|(packet, &n)| {
                    packet["transfer_status"] == 0
                        && packet["packet_status"] == 0
                        && packet["actual"] == n
                })
        })
}

fn crc16_usb(payload: &[u8]) -> u16 {
    let mut crc = u16::MAX;
    for &b in payload {
        crc ^= u16::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 0 { 0 } else { 0xa001 };
        }
    }
    !crc
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Outcome {
    Observed,
    // Index in the submitted ISO train whose bytes survived in the frozen arena.
    ObservedSilentPartial(usize),
    GuardChanged,
    QuietContaminated,
    Inconclusive,
    RecoveryFailed,
    BulkLoss,
}

impl Outcome {
    fn observed(self) -> bool {
        matches!(self, Self::Observed | Self::ObservedSilentPartial(_))
    }
}

fn assess(c: Case, s: &Status, fill: u8, bytes: &[u8], data: &[Vec<u8>], sent: bool) -> Outcome {
    let quiet = c.traffic == Traffic::Quiet;
    if quiet && bytes.iter().any(|b| *b != fill) {
        return Outcome::QuietContaminated;
    }
    if bytes
        .get(BOUND..)
        .is_some_and(|tail| tail.iter().any(|b| *b != fill))
    {
        return Outcome::GuardChanged;
    }
    if bytes.len() != ARENA
        || s.phase != 3
        || s.sentinel != fill
        || s.max != c.max
        || s.alt != c.alt()
        || s.delayed != c.delayed
    {
        return Outcome::Inconclusive;
    }
    if quiet {
        return if sent && s.packets == 0 && !s.saw_rx_irq {
            Outcome::Observed
        } else {
            Outcome::QuietContaminated
        };
    }
    if c.traffic == Traffic::Recover {
        return if sent && s.packets == 1 && s.ready[0].count == 64 && bytes.starts_with(&data[0]) {
            Outcome::Observed
        } else {
            Outcome::RecoveryFailed
        };
    }
    if c.traffic == Traffic::BulkTrain {
        if !sent {
            return Outcome::Inconclusive;
        }
        return if usize::from(s.packets) == data.len()
            && s.ready.iter().all(|sample| sample.count == 64)
            && data.last().is_some_and(|last| bytes.starts_with(last))
        {
            Outcome::Observed
        } else {
            Outcome::BulkLoss
        };
    }
    if !sent {
        return Outcome::Inconclusive;
    }
    if c.traffic == Traffic::Iso(&[69]) && c.max == 0xff {
        return if bytes.starts_with(&data[0]) {
            Outcome::Observed
        } else {
            Outcome::Inconclusive
        };
    }
    // On the black FM-1 byte zero can hold the high USB CRC byte. Verify
    // this silent write against a complete observed footprint.
    if matches!(c.max, 0x08 | 0x09 | 0x40)
        && matches!(c.traffic, Traffic::Iso(&[1023] | &[1023, 1023, 1023, 1023]))
        && s.packets == 0
        && !s.saw_rx_irq
    {
        if s.ready
            .iter()
            .chain(core::iter::once(&s.at_gate))
            .all(|r| r.count == 0 && r.csr == 0)
            && bytes[512..].iter().all(|b| *b == fill)
            && let Some(i) = data.iter().position(|p| {
                p.len() == 1023
                    && bytes[1..512] == p[1..512]
                    && bytes[0] == crc16_usb(p).to_le_bytes()[1]
            })
        {
            return Outcome::ObservedSilentPartial(i);
        }
        return Outcome::Inconclusive;
    }
    // Some packets change SRAM without setting READY or IRQ. Require a payload
    // fingerprint even in that case. One final snapshot cannot prove receipt of
    // every member of a train. Individual host completions are recorded above.
    let marker = data
        .iter()
        .any(|p| bytes.windows(16).any(|w| w == &p[..16]));
    if marker {
        Outcome::Observed
    } else {
        Outcome::Inconclusive
    }
}

fn trial(
    p: &mut DmaProbe,
    out: &Path,
    id: usize,
    c: Case,
    fill: u8,
) -> io::Result<(Outcome, Value)> {
    p.select(c.alt())?;
    let data = c.packets();
    let lengths: Vec<_> = data.iter().map(Vec::len).collect();
    let token = p.arm(c.max, fill, c.delayed, lengths.iter().sum())?;
    let before = p.status(token)?;
    if before.phase != 1
        || before.alt != c.alt()
        || before.sentinel != fill
        || before.delayed != c.delayed
    {
        return Err(io::Error::other("arm status mismatch before the stimulus"));
    }
    let completion = transmit(p, c, &data);
    let sent = transfer_complete(c, &lengths, &completion);
    let started = Instant::now();
    let status = loop {
        let s = p.status(token)?;
        if s.phase == 3 {
            break s;
        }
        if started.elapsed() >= Duration::from_secs(2) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "freeze timeout"));
        }
        thread::sleep(Duration::from_millis(10));
    };
    let bytes = p.snapshot(token)?;
    let name = format!("trial-{id:03}.bin");
    fs::write(out.join(&name), &bytes)?;
    let first = bytes
        .iter()
        .enumerate()
        .skip(BOUND)
        .find(|(_, b)| **b != fill)
        .map(|(i, _)| i);
    let highest = bytes.iter().rposition(|b| *b != fill);
    let markers: Vec<_> = data
        .iter()
        .map(|p| bytes.windows(16).position(|w| w == &p[..16]))
        .collect();
    let result = assess(c, &status, fill, &bytes, &data, sent);
    println!(
        "{id:03} {:?} max={:#04x}/{:#04x} fill={fill:#04x} {} packets={} high={highest:?} first>=2048={first:?} {result:?}",
        c.traffic,
        c.max,
        status.max,
        if c.delayed { "delayed" } else { "prompt" },
        status.packets
    );
    let (outcome, silent_packet_index) = match result {
        Outcome::ObservedSilentPartial(i) => ("ObservedSilentPartial".into(), Some(i)),
        other => (format!("{other:?}"), None),
    };
    Ok((
        result,
        json!({"id":id,"token":token,"case":format!("{:?}",c.traffic),"alt":c.alt(),"lengths":lengths,
        "delayed":c.delayed,"fill":fill,"rx_max":c.max,"readback":status.max,"before":before.raw,"at_freeze":status.raw,
        "ready_packets":status.packets,"counts":[status.ready[0].count,status.ready[1].count],"irq":status.saw_rx_irq,
        "completion":completion,"marker_offsets":markers,"highest_changed":highest,"first_guard":first,
        "first_guard_byte":first.map(|i| bytes[i]),"snapshot":name,"outcome":outcome,"silent_packet_index":silent_packet_index}),
    ))
}

pub(super) fn run(
    device: &Device,
    board: &str,
    out: &Path,
    timeout: Duration,
    reconnect: bool,
) -> io::Result<()> {
    let mut p = DmaProbe::open(device, timeout)?;
    if p.profile() != Profile::Stress {
        return Err(io::Error::other("flash rticx-dma-stress.fwsc (RXS1)"));
    }
    fs::create_dir(out)?;
    let mut report = json!({"protocol":"RXS1","board_revision":board,"arena_bytes":ARENA,"guard_offset":BOUND,
        "address":device.info().device_address(),"bus":device.info().bus_id(),"os":std::env::consts::OS,
        "arch":std::env::consts::ARCH,"reconnect_requested":reconnect,"complete":false,"trials":[]});
    let save = |report: &Value| -> io::Result<()> {
        fs::write(out.join("report.tmp"), serde_json::to_vec_pretty(report)?)?;
        fs::rename(out.join("report.tmp"), out.join("report.json"))
    };
    save(&report)?;
    let mut ok = true;
    let mut id = 0usize;
    for fill in [0xa5, 0x5a] {
        for c in cases() {
            id = id.saturating_add(1);
            let row = match trial(&mut p, out, id, c, fill) {
                Ok((outcome, row)) => {
                    ok &= outcome.observed();
                    row
                }
                Err(e) => {
                    report["interruption"] = json!({"id":id,"case":format!("{:?}",c.traffic),"fill":fill,
                        "rx_max":c.max,"error":e.to_string()});
                    save(&report)?;
                    return Err(e);
                }
            };
            report["trials"]
                .as_array_mut()
                .ok_or_else(|| io::Error::other("invalid report"))?
                .push(row);
            save(&report)?;
        }
    }
    drop(p);
    let journal = read_log(device, timeout).and_then(|log| {
        let log = log.ok_or_else(|| io::Error::other("RXS1 did not return its USB journal"))?;
        fs::write(out.join("usb-log.txt"), log)
    });
    if let Err(error) = &journal {
        report["usb_log_error"] = json!(error.to_string());
        save(&report)?;
    }
    if reconnect {
        eprintln!(
            "Disconnect the FM-1 USB cable now, wait for the prompt, then reconnect the same board."
        );
        let result = reconnect_trial(out, id.saturating_add(1), timeout);
        match result {
            Ok((outcome, row)) => {
                ok &= outcome.observed();
                report["reconnect"] = row;
            }
            Err(err) => {
                report["reconnect"] = json!({"error":err.to_string()});
                report["all_trials_observed_with_intact_guard"] = json!(false);
                save(&report)?;
                return Err(err);
            }
        }
    }
    report["complete"] = json!(true);
    report["all_trials_observed_with_intact_guard"] = json!(ok);
    save(&report)?;
    if ok {
        journal
    } else {
        Err(io::Error::other(
            "one or more trials changed a guard, failed recovery or were inconclusive; see report.json",
        ))
    }
}

fn reconnect_trial(out: &Path, id: usize, timeout: Duration) -> io::Result<(Outcome, Value)> {
    let since = Instant::now();
    loop {
        match Device::find() {
            Err(e) if e.kind() == io::ErrorKind::NotFound => break,
            Err(e) => return Err(e),
            Ok(_) => (),
        }
        if since.elapsed() > Duration::from_secs(60) {
            return Err(io::Error::other("did not observe USB detach"));
        }
        thread::sleep(Duration::from_millis(100));
    }
    eprintln!("Detach observed. Reconnect the FM-1 now.");
    let since = Instant::now();
    loop {
        match Device::find() {
            Ok(dev) => {
                if let Ok(mut p) = DmaProbe::open(&dev, timeout) {
                    if p.profile() != Profile::Stress {
                        return Err(io::Error::other("unexpected device after reconnect"));
                    }
                    return trial(
                        &mut p,
                        out,
                        id,
                        Case {
                            traffic: Traffic::Recover,
                            max: 0xff,
                            delayed: false,
                        },
                        0xa5,
                    );
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        if since.elapsed() > Duration::from_secs(60) {
            return Err(io::Error::other("RXS1 did not re-enumerate"));
        }
        thread::sleep(Duration::from_millis(150));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state(c: Case) -> Status {
        let mut raw = [0; 24];
        raw[2..8].copy_from_slice(&[3, 0xa5, c.max, u8::from(c.delayed), 0, c.alt()]);
        Status {
            raw,
            phase: 3,
            sentinel: 0xa5,
            max: c.max,
            delayed: c.delayed,
            packets: 0,
            alt: c.alt(),
            ready: Default::default(),
            at_gate: Default::default(),
            saw_rx_irq: false,
        }
    }
    #[test]
    fn boundaries_silent_writes_and_transport_errors() {
        let c = Case {
            traffic: Traffic::Iso(&[1023]),
            max: 0xff,
            delayed: false,
        };
        let mut s = state(c);
        let data = c.packets();
        let mut a = vec![0xa5; ARENA];
        assert_eq!(assess(c, &s, 0xa5, &a, &data, true), Outcome::Inconclusive);
        a[..1023].copy_from_slice(&data[0]);
        a[2047] = 42;
        assert_eq!(assess(c, &s, 0xa5, &a, &data, true), Outcome::Observed);
        s.max = 0x08;
        assert_eq!(assess(c, &s, 0xa5, &a, &data, true), Outcome::Inconclusive);
        a[2048] = 42;
        assert_eq!(assess(c, &s, 0xa5, &a, &data, false), Outcome::GuardChanged);
        let quiet = Case {
            traffic: Traffic::Quiet,
            ..c
        };
        assert_eq!(
            assess(quiet, &state(quiet), 0xa5, &a, &[], true),
            Outcome::QuietContaminated
        );
    }
    #[test]
    fn silent_clip_requires_the_crc_payload_and_complete_transfer() {
        assert_eq!(crc16_usb(b"123456789"), 0xb4c8);
        let c = Case {
            traffic: Traffic::Iso(&[1023; 4]),
            max: 0x40,
            delayed: true,
        };
        let data = c.packets();
        let s = state(c);
        let mut a = vec![0xa5; ARENA];
        a[1..512].copy_from_slice(&data[3][1..512]);
        assert_eq!(crc16_usb(&data[3]).to_le_bytes(), [0x15, 0x42]);
        a[0] = 0x42;
        assert_eq!(
            assess(c, &s, 0xa5, &a, &data, true),
            Outcome::ObservedSilentPartial(3)
        );
        assert_eq!(assess(c, &s, 0xa5, &a, &data, false), Outcome::Inconclusive);
        a[0] = 0x43;
        assert_eq!(assess(c, &s, 0xa5, &a, &data, true), Outcome::Inconclusive);
        a[0] = data[3][0];
        assert_eq!(assess(c, &s, 0xa5, &a, &data, true), Outcome::Inconclusive);
        a[0] = 0x42;
        a[511] ^= 1;
        assert_eq!(assess(c, &s, 0xa5, &a, &data, true), Outcome::Inconclusive);
        a[511] ^= 1;
        a[512] = 0;
        assert_eq!(assess(c, &s, 0xa5, &a, &data, true), Outcome::Inconclusive);
        a[2048] = 1;
        assert_eq!(assess(c, &s, 0xa5, &a, &data, true), Outcome::GuardChanged);
    }

    #[test]
    fn calibrations_and_resource_budget() {
        let c = Case {
            traffic: Traffic::Iso(&[69]),
            max: 0xff,
            delayed: false,
        };
        let data = c.packets();
        let mut a = vec![0xa5; ARENA];
        a[..68].copy_from_slice(&data[0][..68]);
        assert_eq!(
            assess(c, &state(c), 0xa5, &a, &data, true),
            Outcome::Inconclusive
        );
        a[68] = data[0][68];
        assert_eq!(
            assess(c, &state(c), 0xa5, &a, &data, true),
            Outcome::Observed
        );
        let r = Case {
            traffic: Traffic::Recover,
            ..c
        };
        let mut s = state(r);
        let notes = r.packets();
        a[..64].copy_from_slice(&notes[0]);
        assert_eq!(
            assess(r, &s, 0xa5, &a, &notes, true),
            Outcome::RecoveryFailed
        );
        s.packets = 1;
        s.ready[0].count = 64;
        assert_eq!(assess(r, &s, 0xa5, &a, &notes, true), Outcome::Observed);
        let train = Case {
            traffic: Traffic::BulkTrain,
            ..c
        };
        let packets = train.packets();
        let mut state = state(train);
        let last = packets.last().expect("train contains packets");
        a[..64].copy_from_slice(last);
        assert_eq!(
            assess(train, &state, 0xa5, &a, &packets, true),
            Outcome::BulkLoss
        );
        state.packets = 64;
        state.ready[0].count = 64;
        state.ready[1].count = 64;
        assert_eq!(
            assess(train, &state, 0xa5, &a, &packets, true),
            Outcome::Observed
        );
        for c in cases() {
            assert!(c.packets().iter().map(Vec::len).sum::<usize>() <= 4096);
        }
    }
}
