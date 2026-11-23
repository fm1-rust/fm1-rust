//! Physical trial runner. All control/status/paging traffic uses EP0. The only
//! EP1 OUT traffic inside the observation window is the selected stimulus.
use core::time::Duration;
use fm1_usb::{
    Device, MidiConnection,
    dma_probe::{ARENA, BOUND, DmaProbe, Profile, Sample, Status},
};
use serde_json::{Value, json};
use std::{
    fs, io,
    path::{Path, PathBuf},
    thread,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pipe {
    Midi,
    Bulk,
    Iso,
}
#[derive(Clone, Copy, Debug)]
struct Trial {
    pipe: Pipe,
    max: u8,
    sentinel: u8,
    bytes: usize,
    pair: bool,
    delayed: bool,
}
impl Trial {
    fn alt(self) -> u8 {
        match self.pipe {
            Pipe::Midi => 0,
            Pipe::Bulk => 1,
            Pipe::Iso => 2,
        }
    }
    fn payload(self) -> Vec<u8> {
        if self.pipe == Pipe::Midi && self.bytes != 0 {
            return midi_payload().1;
        }
        (0..self.bytes)
            .map(|i| {
                let base: u8 = if self.pair && i >= 64 { 0x80 } else { 0x10 };
                base | u8::try_from(i % 32).expect("five bits")
            })
            .collect()
    }
}

// 48 MIDI bytes make exactly sixteen USB-MIDI SysEx events when the class
// driver coalesces them. The device's raw count and bytes verify the grouping.
fn midi_payload() -> (Vec<u8>, Vec<u8>) {
    let mut msg = vec![0xf0, 0x7d];
    msg.extend(1..=45);
    msg.push(0xf7);
    let mut wire = Vec::with_capacity(64);
    for chunk in msg.as_chunks::<3>().0 {
        wire.push(if chunk[2] == 0xf7 { 0x07 } else { 0x04 });
        wire.extend_from_slice(chunk);
    }
    (msg, wire)
}

#[derive(Debug)]
struct Injection {
    json: Value,
    complete: bool,
}
fn inject(
    probe: &mut DmaProbe,
    midi: &mut Option<MidiConnection>,
    t: Trial,
    data: &[u8],
) -> Injection {
    if t.bytes == 0 {
        return Injection {
            json: json!({"submitted":false}),
            complete: true,
        };
    }
    let result = match t.pipe {
        Pipe::Midi => midi
            .as_mut()
            .ok_or_else(|| io::Error::other("no MIDI connection"))
            .and_then(|m| m.send(&midi_payload().0))
            .map(|()| Injection {
                // CoreMIDI does not report USB transfer lengths.
                json: json!({"midi_bytes":48}),
                complete: true,
            }),
        Pipe::Bulk => probe.bulk(data).map(|n| Injection {
            json: json!({"actual":n}),
            complete: n == data.len(),
        }),
        Pipe::Iso => probe.iso(data).map(|r| Injection {
            json: json!({"iso_packets":1,"transfer_status":r.transfer_status,
                "packet_status":r.packet_status,"packet_actual":r.actual}),
            complete: r.transfer_status == 0
                && r.packet_status == 0
                && usize::try_from(r.actual) == Ok(data.len()),
        }),
    };
    result.unwrap_or_else(|e| Injection {
        json: json!({"error":e.to_string()}),
        complete: false,
    })
}

fn sample(s: Sample) -> Value {
    json!({"rxcsr":s.csr, "count":s.count, "ms":s.ms})
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Verdict {
    Observed,
    Overrun,
    Inconclusive,
}

fn classify(
    t: Trial,
    arena: &[u8],
    data: &[u8],
    status: &Status,
    transport_ok: bool,
) -> (Verdict, &'static str) {
    let highest = arena.iter().rposition(|&b| b != t.sentinel);
    if status.phase != 3
        || status.sentinel != t.sentinel
        || status.delayed != t.delayed
        || status.alt != t.alt()
    {
        return (Verdict::Inconclusive, "mismatched frozen trial");
    }
    if t.bytes == 0 && (highest.is_some() || status.packets != 0) {
        return (Verdict::Inconclusive, "no-OUT control contaminated");
    }
    if highest.is_some_and(|i| i >= BOUND) {
        return (
            Verdict::Overrun,
            "write beyond the original 68-byte allocation",
        );
    }
    if !transport_ok {
        return (
            Verdict::Inconclusive,
            "host transfer failed, was short, or missed the window",
        );
    }
    if t.pair {
        if status.packets != 2
            || status.ready[0].count != 64
            || status.ready[1].count != 64
            || !arena.starts_with(&data[64..])
        {
            return (
                Verdict::Inconclusive,
                "two distinct serviced packets were not verified",
            );
        }
        if t.delayed && status.ready[1].ms.saturating_sub(status.ready[0].ms) < 50 {
            return (
                Verdict::Inconclusive,
                "delayed service interval was not observed",
            );
        }
    } else if t.bytes == 64
        && t.max != 0x20
        && (!arena.starts_with(data)
            || (t.pipe == Pipe::Midi
                && (status.packets != 1
                    || status.ready[0].csr & 1 == 0
                    || status.ready[0].count != 64)))
    {
        return (
            Verdict::Inconclusive,
            "64-byte positive control did not reproduce the exact packet",
        );
    }
    if t.bytes != 0 && status.packets == 0 {
        return (
            Verdict::Observed,
            if highest.is_some() {
                "SRAM changed without RXPKTRDY; guard intact"
            } else {
                "host completed; no device-side evidence of this packet"
            },
        );
    }
    (Verdict::Observed, "guard intact in this trial")
}

fn record(
    probe: &mut DmaProbe,
    midi: &mut Option<MidiConnection>,
    t: Trial,
    sequence: u16,
    dir: &Path,
    row: &mut Value,
) -> io::Result<(Verdict, &'static str)> {
    if t.pipe != Pipe::Midi {
        probe.select(t.alt())?;
    }
    let token = probe.arm(t.max, t.sentinel, t.delayed, t.bytes)?;
    row["token"] = json!(token);
    let start = Instant::now();
    let before = probe.status(token)?;
    if before.phase != 1 {
        return Err(io::Error::other("arm window expired before stimulus"));
    }
    let data = t.payload();
    let sent = inject(probe, midi, t, &data);
    row["host"] = sent.json;
    row["expected_wire_bytes"] = json!(data);
    let after = probe.status(token)?;
    row["phase_after_submission"] = json!(after.phase);
    let mut status = after;
    while status.phase != 3 {
        if start.elapsed() >= Duration::from_secs(3) || !matches!(status.phase, 1 | 2) {
            return Err(io::Error::other("firmware did not freeze the trial"));
        }
        thread::sleep(Duration::from_millis(15));
        status = probe.status(token)?;
    }
    let arena = probe.snapshot(token)?;
    if arena.len() != ARENA {
        return Err(io::Error::other("truncated snapshot"));
    }
    let name = format!("trial-{sequence:03}.bin");
    fs::write(dir.join(&name), &arena)?;
    let highest = arena.iter().rposition(|&b| b != t.sentinel);
    row["snapshot"] = json!(name);
    row["raw_status"] = json!(status.raw);
    row["rx_max_readback"] = json!(status.max);
    row["ready_packets"] = json!(status.packets);
    row["saw_rx_irq"] = json!(status.saw_rx_irq);
    row["first"] = sample(status.ready[0]);
    row["second"] = sample(status.ready[1]);
    row["at_gate"] = sample(status.at_gate);
    row["highest_changed"] = json!(highest);
    row["first_guard_change"] = json!(
        arena
            .iter()
            .enumerate()
            .skip(BOUND)
            .find(|&(_, b)| *b != t.sentinel)
            .map(|(i, _)| i)
    );
    row["matching_prefix"] = json!(arena.iter().zip(&data).take_while(|(a, b)| a == b).count());
    Ok(classify(
        t,
        &arena,
        &data,
        &status,
        sent.complete && after.phase == 1,
    ))
}

struct Run {
    dir: PathBuf,
    report: Value,
    sequence: u16,
    overrun: bool,
    incomplete: bool,
}
impl Run {
    fn save(&self) -> io::Result<()> {
        let tmp = self.dir.join("report.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(&self.report)?)?;
        fs::rename(tmp, self.dir.join("report.json"))
    }

    fn trial(
        &mut self,
        probe: &mut DmaProbe,
        midi: &mut Option<MidiConnection>,
        t: Trial,
    ) -> io::Result<Verdict> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("trial number exhausted"))?;
        let mut row = json!({"trial":self.sequence,"pipe":format!("{:?}",t.pipe),"rx_max":t.max,"sentinel":t.sentinel,
            "bytes":t.bytes,"pair":t.pair,"delayed":t.delayed});
        let outcome = record(probe, midi, t, self.sequence, &self.dir, &mut row);
        let (verdict, why) = match &outcome {
            Ok(result) => *result,
            Err(e) => {
                row["error"] = json!(e.to_string());
                (Verdict::Inconclusive, "could not complete trial")
            }
        };
        row["verdict"] = json!(format!("{verdict:?}"));
        row["reason"] = json!(why);
        println!(
            "{:03} {:?} max={:#04x} readback={} fill={:#04x} bytes={} counts=[{},{}] highest={} {verdict:?} ({why})",
            self.sequence,
            t.pipe,
            t.max,
            row["rx_max_readback"],
            t.sentinel,
            t.bytes,
            row["first"]["count"],
            row["second"]["count"],
            row["highest_changed"]
        );
        self.overrun |= verdict == Verdict::Overrun;
        self.incomplete |= verdict == Verdict::Inconclusive;
        self.report["overrun_observed"] = json!(self.overrun);
        self.report["inconclusive_trials"] = json!(self.incomplete);
        self.report["trials"]
            .as_array_mut()
            .expect("trial list")
            .push(row);
        self.save()?;
        outcome.map(|(v, _)| v)
    }
}

pub(super) fn run(
    device: &Device,
    board: &str,
    out: Option<PathBuf>,
    port: Option<&str>,
    timeout: Duration,
) -> io::Result<()> {
    let mut probe = DmaProbe::open(device, timeout)?;
    let profile = probe.profile();
    if profile == Profile::Stress {
        return Err(io::Error::other("use fm1 dma-stress for RXS1"));
    }
    let mut midi = if profile == Profile::Midi {
        let (ports, marker) = device.log_ports(port, timeout)?;
        if marker.lines().next() != Some("RXD3 MIDI READY") {
            return Err(io::Error::other(
                "selected MIDI ports did not return RXD3 MIDI READY",
            ));
        }
        Some(ports.connect(|_, _| {})?)
    } else {
        None
    };
    let epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(io::Error::other)?;
    let dir = out.unwrap_or_else(|| {
        PathBuf::from(format!(
            "fm1-dma-{}-{}",
            epoch.as_millis(),
            std::process::id()
        ))
    });
    fs::create_dir(&dir)?;
    eprintln!("RXD3 {profile:?}; saving {}", dir.display());
    let mut run = Run {
        dir,
        sequence: 0,
        overrun: false,
        incomplete: false,
        report: json!({
        "protocol":"RXD3","profile":format!("{profile:?}"),"board_revision":board,
        "host_os":std::env::consts::OS,"host_arch":std::env::consts::ARCH,
        "bus":device.info().bus_id(),"address":device.info().device_address(),
        "start_unix_ms":u64::try_from(epoch.as_millis()).map_err(io::Error::other)?,
        "comparison_revision":"27210227909df5ad0433787895dc1776fe7c2c83",
        "arena_bytes":ARENA,"first_out_of_bounds_offset":BOUND,"window_ms":500,
        "completed":false,"trials":[]}),
    };
    run.save()?;
    let pipes: &[_] = match profile {
        Profile::Midi => &[Pipe::Midi],
        Profile::Bulk => &[Pipe::Bulk],
        Profile::Iso => &[Pipe::Bulk, Pipe::Iso],
        Profile::Stress => unreachable!("validated above"),
    };
    for &pipe in pipes {
        let maxima: &[_] = if pipe == Pipe::Iso {
            &[0xff, 0x40]
        } else {
            &[0xff]
        };
        for &max in maxima {
            for sentinel in [0xa5, 0x5a] {
                let base = Trial {
                    pipe,
                    max,
                    sentinel,
                    bytes: 0,
                    pair: false,
                    delayed: false,
                };
                if run.trial(&mut probe, &mut midi, base)? != Verdict::Observed {
                    continue;
                }
                let single = Trial { bytes: 64, ..base };
                if run.trial(&mut probe, &mut midi, single)? != Verdict::Observed {
                    continue;
                }
                if pipe == Pipe::Bulk {
                    for delayed in [false, true] {
                        run.trial(
                            &mut probe,
                            &mut midi,
                            Trial {
                                bytes: 128,
                                pair: true,
                                delayed,
                                ..base
                            },
                        )?;
                    }
                    let reduced = Trial { max: 0x20, ..base };
                    if run.trial(&mut probe, &mut midi, reduced)? != Verdict::Observed {
                        continue;
                    }
                    for bytes in [32, 33, 64] {
                        run.trial(
                            &mut probe,
                            &mut midi,
                            Trial {
                                max: 0x20,
                                bytes,
                                ..base
                            },
                        )?;
                    }
                } else if pipe == Pipe::Iso {
                    for bytes in [65, 68, 69, 96, 128, 255, 256, 512, 1023] {
                        run.trial(&mut probe, &mut midi, Trial { bytes, ..base })?;
                    }
                }
            }
        }
    }
    run.report["completed"] = json!(true);
    run.report["overrun_observed"] = json!(run.overrun);
    run.report["inconclusive_trials"] = json!(run.incomplete);
    run.save()?;
    if run.overrun {
        Err(io::Error::other(
            "EP1 OUT crossed the original 68-byte allocation; see trial snapshots and RX_MAX",
        ))
    } else if run.incomplete {
        Err(io::Error::other(
            "inconclusive controls or transfers; see report.json",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iso64() -> (Trial, Status, Vec<u8>) {
        let trial = Trial {
            pipe: Pipe::Iso,
            max: 0xff,
            sentinel: 0xa5,
            bytes: 64,
            pair: false,
            delayed: false,
        };
        let state = Status {
            raw: [0; 24],
            phase: 3,
            sentinel: 0xa5,
            max: 0xff,
            delayed: false,
            packets: 1,
            alt: 2,
            ready: [
                Sample {
                    csr: 1,
                    count: 64,
                    ms: 2,
                },
                Sample::default(),
            ],
            at_gate: Sample::default(),
            saw_rx_irq: true,
        };
        let mut arena = vec![trial.sentinel; ARENA];
        arena[..64].copy_from_slice(&trial.payload());
        (trial, state, arena)
    }

    #[test]
    fn first_out_of_bounds_byte_is_68_even_after_a_failed_transfer() {
        let (trial, status, mut arena) = iso64();
        let data = trial.payload();
        arena[67] = 0;
        assert_eq!(
            classify(trial, &arena, &data, &status, true).0,
            Verdict::Observed
        );
        arena[68] = 0;
        assert_eq!(
            classify(trial, &arena, &data, &status, false).0,
            Verdict::Overrun
        );
        let quiet = Trial { bytes: 0, ..trial };
        assert_eq!(
            classify(quiet, &arena, &[], &status, true).0,
            Verdict::Inconclusive
        );
    }

    #[test]
    fn count_alone_cannot_pass_or_fail_the_guard() {
        let (trial, mut status, mut arena) = iso64();
        let data = trial.payload();
        status.ready[0].count = 1023;
        arena[63] = trial.sentinel;
        assert_eq!(
            classify(trial, &arena, &data, &status, true).0,
            Verdict::Inconclusive
        );
        assert!(arena[BOUND..].iter().all(|&b| b == trial.sentinel));
        arena[63] = data[63];
        status.packets = 0;
        status.saw_rx_irq = false;
        status.max = 0; // A read-as-zero register must not suppress the experiment.
        assert_eq!(
            classify(trial, &arena, &data, &status, true).0,
            Verdict::Observed
        );
        assert_eq!(
            classify(trial, &arena, &data, &status, false).0,
            Verdict::Inconclusive
        );
    }

    #[test]
    fn midi_requires_one_verified_packet_and_payloads_avoid_both_sentinels() {
        let (base, mut status, mut arena) = iso64();
        let trial = Trial {
            pipe: Pipe::Midi,
            ..base
        };
        let (msg, wire) = midi_payload();
        assert_eq!(msg.len(), 48);
        assert_eq!(wire.len(), 64);
        assert_eq!(&wire[..3], &[4, 0xf0, 0x7d]);
        assert_eq!(&wire[60..], &[7, 44, 45, 0xf7]);
        arena[..64].copy_from_slice(&wire);
        status.alt = 0;
        assert_eq!(
            classify(trial, &arena, &wire, &status, true).0,
            Verdict::Observed
        );
        status.packets = 2;
        assert_eq!(
            classify(trial, &arena, &wire, &status, true).0,
            Verdict::Inconclusive
        );
        for payload in [
            wire,
            Trial {
                bytes: 1023,
                ..base
            }
            .payload(),
            Trial {
                pair: true,
                bytes: 128,
                ..base
            }
            .payload(),
        ] {
            assert!(payload.iter().all(|b| !matches!(b, 0x5a | 0xa5)));
        }
    }

    #[test]
    fn delayed_pair_requires_two_counts_elapsed_time_and_second_payload() {
        let (base, mut status, mut arena) = iso64();
        let trial = Trial {
            pipe: Pipe::Bulk,
            bytes: 128,
            pair: true,
            delayed: true,
            ..base
        };
        let data = trial.payload();
        status.alt = 1;
        status.delayed = true;
        status.packets = 2;
        status.ready[1] = Sample {
            csr: 1,
            count: 64,
            ms: 53,
        };
        assert_eq!(
            classify(trial, &arena, &data, &status, true).0,
            Verdict::Inconclusive
        );
        arena[..64].copy_from_slice(&data[64..]);
        assert_eq!(
            classify(trial, &arena, &data, &status, true).0,
            Verdict::Observed
        );
        status.ready[1].ms = 20;
        assert_eq!(
            classify(trial, &arena, &data, &status, true).0,
            Verdict::Inconclusive
        );
    }
}
