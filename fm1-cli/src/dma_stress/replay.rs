//! Replay the fixed RXS1 schedule from the original raw register and SRAM records.
use super::{ARENA, BOUND, Case, Outcome, Status, Traffic, assess, cases, transfer_complete};
use serde_json::{Value, json};
use std::{fs, io, path::Path};

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn raw(row: &Value, field: &str) -> io::Result<Status> {
    let bytes: [u8; 24] = serde_json::from_value(row[field].clone())?;
    let token = u16::from_le_bytes([bytes[0], bytes[1]]);
    if token == 0 || row["token"] != token {
        return Err(invalid("status token mismatch"));
    }
    Ok(Status::decode(bytes))
}

fn trial(dir: &Path, row: &Value, id: usize, c: Case, fill: u8) -> io::Result<Outcome> {
    let name = format!("trial-{id:03}.bin");
    let data = c.packets();
    let lengths: Vec<_> = data.iter().map(Vec::len).collect();
    if row["id"] != id
        || row["snapshot"] != name
        || row["case"] != format!("{:?}", c.traffic)
        || row["alt"] != c.alt()
        || row["fill"] != fill
        || row["rx_max"] != c.max
        || row["delayed"] != c.delayed
        || row["lengths"] != json!(lengths)
    {
        return Err(invalid("row differs from the RXS1 schedule"));
    }
    let before = raw(row, "before")?;
    let s = raw(row, "at_freeze")?;
    if before.phase != 1
        || before.alt != c.alt()
        || before.delayed != c.delayed
        || before.sentinel != fill
        || s.phase != 3
        || s.alt != c.alt()
        || s.sentinel != fill
        || s.delayed != c.delayed
        || s.max != before.max
        || row["readback"] != s.max
        || row["irq"] != s.saw_rx_irq
        || row["ready_packets"] != s.packets
        || row["counts"] != json!([s.ready[0].count, s.ready[1].count])
    {
        return Err(invalid("arm, freeze, or journal fields disagree"));
    }
    let bytes = fs::read(dir.join(name))?;
    if bytes.len() != ARENA {
        return Err(invalid("expected an 8192-byte snapshot"));
    }
    let high = bytes.iter().rposition(|b| *b != fill);
    let guard = bytes
        .iter()
        .enumerate()
        .skip(BOUND)
        .find(|(_, b)| **b != fill)
        .map(|(i, &b)| (i, b));
    if row["highest_changed"] != json!(high)
        || row["first_guard"] != json!(guard.map(|(i, _)| i))
        || row["first_guard_byte"] != json!(guard.map(|(_, b)| b))
    {
        return Err(invalid(&format!(
            "snapshot disagrees with reported footprint: highest={high:?}, first_guard={guard:?}"
        )));
    }
    let sent = transfer_complete(c, &lengths, &row["completion"]);
    let verdict = assess(c, &s, fill, &bytes, &data, sent);
    println!(
        "{id:03} {:?} max={:#04x} ready={} highest={high:?} guard={guard:?} {verdict:?}",
        c.traffic, c.max, s.packets
    );
    Ok(verdict)
}

pub(crate) fn replay(dir: &Path) -> io::Result<()> {
    let report: Value = serde_json::from_slice(&fs::read(dir.join("report.json"))?)?;
    if report["protocol"] != "RXS1"
        || report["arena_bytes"] != ARENA
        || report["guard_offset"] != BOUND
        || report["complete"] != true
    {
        return Err(invalid("expected a completed RXS1 8192/2048 acquisition"));
    }
    let trials = report["trials"]
        .as_array()
        .ok_or_else(|| invalid("missing trial list"))?;
    let mut schedule: Vec<_> = [0xa5, 0x5a]
        .into_iter()
        .flat_map(|fill| cases().into_iter().map(move |c| (c, fill)))
        .collect();
    if trials.len() != schedule.len() {
        return Err(invalid("incomplete RXS1 trial list"));
    }
    let reconnect = report.get("reconnect");
    // Early RXS1 runners recorded the reconnect row without a request flag.
    if let Some(requested) = report.get("reconnect_requested")
        && requested.as_bool() != Some(reconnect.is_some())
    {
        return Err(invalid("reconnect request and result disagree"));
    }
    if reconnect.is_some() {
        schedule.push((
            Case {
                traffic: Traffic::Recover,
                max: 0xff,
                delayed: false,
            },
            0xa5,
        ));
    }
    let mut partial = 0usize;
    let mut ok = true;
    for (id, ((case, fill), row)) in
        (1usize..).zip(schedule.into_iter().zip(trials.iter().chain(reconnect)))
    {
        let verdict = trial(dir, row, id, case, fill)
            .map_err(|e| io::Error::new(e.kind(), format!("trial {id}: {e}")))?;
        ok &= verdict.observed();
        if matches!(verdict, Outcome::ObservedSilentPartial(_)) {
            partial = partial.saturating_add(1);
        }
    }
    if !ok {
        return Err(invalid(
            "one or more trials changed a guard, failed a control, or lacked a qualified stimulus",
        ));
    }
    println!(
        "{} captures verified with intact guards, including {partial} silent partial writes.",
        trials
            .len()
            .saturating_add(usize::from(reconnect.is_some()))
    );
    Ok(())
}
