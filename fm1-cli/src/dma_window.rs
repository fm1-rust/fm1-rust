//! Capture, recover and replay the AC791N USB write-range experiment.
mod archive;
mod evidence;
#[cfg(test)]
mod tests;

use clap::ValueEnum;
use core::{num::NonZeroU16, time::Duration};
use evidence::{Controls, Header, Outcome};
use fm1_usb::{Device, dma_probe::window::WindowProbe};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs, io,
    path::{Path, PathBuf},
    thread,
    time::Instant,
};

const QUIET_INTERVAL: Duration = Duration::from_millis(1500);

#[derive(clap::Args)]
pub(super) struct Options {
    /// Board color, revision or markings to record.
    #[arg(long, visible_alias = "board", required_unless_present_any = ["replay","recover","pack"])]
    board_revision: Option<String>,
    /// New capture directory (defaults to usb-window-results).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Choose EP1, EP0, or both (default).
    #[arg(long, value_enum)]
    endpoint: Option<Scope>,
    /// Verify a capture directory or a combined JSON file offline.
    #[arg(long, value_name="PATH", conflicts_with_all = ["recover","board_revision","out","endpoint"])]
    replay: Option<PathBuf>,
    /// Write completed capture directories to stdout as one lossless JSON file.
    #[arg(long, value_name = "DIR", num_args = 1.., conflicts_with_all = ["replay", "recover", "board_revision", "out", "endpoint"])]
    pack: Vec<PathBuf>,
    /// Retrieve the last pending challenge without issuing GO.
    #[arg(long, value_name="DIR", conflicts_with_all = ["board_revision","out","endpoint"])]
    recover: Option<PathBuf>,
}

#[derive(Clone, Copy, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
enum Scope {
    Ep1,
    Ep0,
    Both,
}
impl Scope {
    fn endpoints(self) -> &'static [Endpoint] {
        match self {
            Self::Both => &[Endpoint::Ep1, Endpoint::Ep0],
            Self::Ep1 => &[Endpoint::Ep1],
            Self::Ep0 => &[Endpoint::Ep0],
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Endpoint {
    Ep1,
    Ep0,
}
impl Endpoint {
    fn index(self) -> usize {
        usize::from(self == Self::Ep0)
    }
    fn name(self) -> &'static str {
        match self {
            Self::Ep1 => "ep1",
            Self::Ep0 => "ep0",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Step {
    QuietOff,
    Baseline,
    InRange,
    Quiet,
    Aligned,
    WithinBeat,
}
impl Step {
    const ALL: [Self; 6] = [
        Self::QuietOff,
        Self::Baseline,
        Self::InRange,
        Self::Quiet,
        Self::Aligned,
        Self::WithinBeat,
    ];
    fn mode(self) -> u8 {
        match self {
            Self::QuietOff | Self::Baseline => 0,
            Self::InRange => 1,
            Self::Quiet | Self::Aligned => 2,
            Self::WithinBeat => 3,
        }
    }
    fn name(self) -> &'static str {
        match self {
            Self::QuietOff => "quiet_off",
            Self::Baseline => "baseline",
            Self::InRange => "in_range",
            Self::Quiet => "quiet",
            Self::Aligned => "aligned",
            Self::WithinBeat => "within_beat",
        }
    }
    fn is_quiet(self) -> bool {
        matches!(self, Self::QuietOff | Self::Quiet)
    }
    fn is_control(self) -> bool {
        !matches!(self, Self::Aligned | Self::WithinBeat)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
struct Shot {
    endpoint: Endpoint,
    polarity: u8,
    name: Step,
}
impl Shot {
    fn case(self) -> u8 {
        (u8::from(self.endpoint == Endpoint::Ep0) << 2) | (self.polarity << 3) | self.name.mode()
    }
    fn filename(self) -> String {
        format!(
            "{}-{}-{}.bin",
            self.endpoint.name(),
            self.polarity,
            self.name.name()
        )
    }
}

#[derive(Deserialize, Serialize)]
struct Row {
    #[serde(flatten)]
    shot: Shot,
    case: u8,
    token: NonZeroU16,
    file: String,
    go: String,
    stimulus: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    host_quiet_complete: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    header: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observation: Option<String>,
    #[serde(flatten)]
    location: Map<String, Value>,
}
impl Row {
    fn fill(&self) -> u8 {
        if self.shot.polarity == 0 { 0xa5 } else { 0x5a }
    }
    fn cut(&self) -> u8 {
        if self.shot.name == Step::InRange {
            return 128;
        }
        let base: u8 = if self.shot.endpoint == Endpoint::Ep0 {
            4
        } else {
            32
        };
        base | u8::from(self.shot.name == Step::WithinBeat)
    }
}

#[derive(Deserialize, Serialize)]
struct Report {
    protocol: String,
    endpoint: Scope,
    complete: bool,
    trials: Vec<Row>,
    #[serde(flatten)]
    meta: Map<String, Value>,
}
fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

fn atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(tmp, path)
}

impl Report {
    fn read(dir: &Path) -> io::Result<Self> {
        Ok(serde_json::from_slice(&fs::read(dir.join("report.json"))?)?)
    }
    fn save(&self, dir: &Path) -> io::Result<()> {
        let mut data = serde_json::to_vec_pretty(self)?;
        data.push(b'\n');
        atomic(&dir.join("report.json"), &data)
    }
    fn check_format(&self, record: &[u8]) -> io::Result<()> {
        if record.get(..4) == Some(self.protocol.as_bytes()) {
            Ok(())
        } else {
            Err(invalid("record and report have different window formats"))
        }
    }
}

fn directory_lock(dir: &Path) -> io::Result<fs::File> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join("run.lock"))?;
    file.try_lock().map_err(|e| {
        io::Error::other(format!(
            "cannot lock {} for capture or recovery: {e}",
            dir.display()
        ))
    })?;
    Ok(file)
}

fn hex(bytes: &[u8]) -> String {
    use core::fmt::Write;
    let mut hex = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        write!(hex, "{byte:02x}").expect("writing into a String");
    }
    hex
}

fn digest(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn open(timeout: Duration) -> io::Result<(WindowProbe, Map<String, Value>)> {
    let dev = Device::find()?;
    let probe = WindowProbe::open(&dev, timeout)?;
    let info = dev.info();
    Ok((
        probe,
        Map::from_iter([
            ("bus".into(), json!(info.bus_id())),
            ("address".into(), json!(info.device_address())),
        ]),
    ))
}

fn readback(row: &Row, timeout: Duration) -> io::Result<Vec<u8>> {
    let start = Instant::now();
    loop {
        match open(timeout).and_then(|(p, _)| p.snapshot(row.case, row.token)) {
            Ok(record) => return Ok(record),
            Err(e) if start.elapsed() >= Duration::from_secs(15) => {
                return Err(io::Error::other(format!(
                    "Window readback failed ({e}). Photograph the LCD before cycling power, then use dma-window --recover."
                )));
            }
            Err(_) => thread::sleep(Duration::from_millis(150)),
        }
    }
}

fn token(rows: &[Row], previous: [u8; 64]) -> io::Result<NonZeroU16> {
    let old = u32::from_le_bytes(previous.as_chunks::<4>().0[1]);
    loop {
        let mut bytes = [0; 2];
        getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
        if let Some(t) = NonZeroU16::new(u16::from_le_bytes(bytes))
            && u32::from(t.get()) != old
            && rows.iter().all(|r| r.token != t)
        {
            return Ok(t);
        }
    }
}

fn report_capture(
    dir: &Path,
    report: &mut Report,
    controls: &mut Controls,
    data: &[u8],
) -> io::Result<Outcome> {
    let row = report
        .trials
        .last_mut()
        .ok_or_else(|| invalid("missing pending shot"))?;
    // Preserve the captured bytes even when the device checksum or CPU summary is bad.
    atomic(&dir.join(row.shot.filename()), data)?;
    row.sha256 = Some(digest(data));
    report.save(dir)?;
    report.check_format(data)?;
    let row = report.trials.last_mut().expect("pending shot retained");
    let (header, result) = controls.evaluate(row, data)?;
    row.header = Some(header.json()?);
    row.observation = Some(result.to_string());
    let shown = header.show(&mut io::stdout().lock(), &row.file, result);
    report.save(dir)?;
    shown?;
    Ok(result)
}

fn trial(
    dir: &Path,
    report: &mut Report,
    shot: Shot,
    controls: &mut Controls,
    (mut probe, location): (WindowProbe, Map<String, Value>),
    timeout: Duration,
) -> io::Result<Outcome> {
    if probe.protocol() != report.protocol {
        return Err(invalid("window firmware changed during acquisition"));
    }
    let token = token(&report.trials, probe.header()?)?;
    probe.select()?;
    report.trials.push(Row {
        shot,
        case: shot.case(),
        token,
        file: shot.filename(),
        go: "pending".into(),
        stimulus: "not attempted".into(),
        host_quiet_complete: Some(false),
        sha256: None,
        header: None,
        observation: None,
        location,
    });
    report.save(dir)?;
    let row = report.trials.last_mut().expect("just pushed");
    println!("{} token {:04x}", row.file, token.get());
    // GO may have reached the controller even if its status transfer failed. Never retry it.
    match probe.go(row.case, token) {
        Ok(()) => {
            row.go = "ok".into();
            row.stimulus = if shot.name.is_quiet() {
                "skipped".into()
            } else {
                match shot.endpoint {
                    Endpoint::Ep0 => probe
                        .setup_stimulus()
                        .map(|n| format!("status returned {n}")),
                    Endpoint::Ep1 => probe.bulk_stimulus().map(|n| format!("wrote {n}/64")),
                }
                .unwrap_or_else(|e| format!("USB transfer: {e}"))
            };
        }
        Err(e) => {
            row.go = e.to_string();
            row.stimulus = "skipped".into();
        }
    }
    let saved = report.save(dir);
    // Releasing the claimed interface can generate EP0 traffic. Keep it alive until even the
    // timeout path has gated, drained and reset. An on-disk error must not shorten this interval.
    thread::sleep(QUIET_INTERVAL);
    drop(probe);
    saved?;
    report
        .trials
        .last_mut()
        .expect("current trial")
        .host_quiet_complete = Some(true);
    report.save(dir)?;
    let data = readback(report.trials.last().expect("current trial"), timeout)?;
    report_capture(dir, report, controls, &data)
}

fn capture(opts: &Options, timeout: Duration) -> io::Result<bool> {
    let board = opts
        .board_revision
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| invalid("missing --board-revision"))?;
    let dir = opts
        .out
        .as_deref()
        .unwrap_or_else(|| Path::new("usb-window-results"));
    let initial = open(timeout)?;
    let protocol = initial.0.protocol();
    println!("Connected to {protocol} window firmware.");
    let mut first = Some(initial);
    if let Some(parent) = dir.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    fs::create_dir(dir).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("new capture directory {}: {e}", dir.display()),
        )
    })?;
    let _lock = directory_lock(dir)?;
    let scope = opts.endpoint.unwrap_or(Scope::Both);
    let mut meta = Map::from_iter([
        ("board".into(), json!(board)),
        (
            "host".into(),
            json!(format!(
                "{} {}",
                std::env::consts::OS,
                std::env::consts::ARCH
            )),
        ),
        ("fm1_cli".into(), json!(env!("CARGO_PKG_VERSION"))),
    ]);
    if cfg!(target_os = "macos")
        && let Ok(output) = std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
        && output.status.success()
    {
        meta.insert(
            "macos".into(),
            json!(String::from_utf8_lossy(&output.stdout).trim()),
        );
    }
    let mut report = Report {
        protocol: protocol.into(),
        endpoint: scope,
        complete: false,
        trials: vec![],
        meta,
    };
    report.save(dir)?;
    let mut good = true;
    for &endpoint in scope.endpoints() {
        for polarity in 0..=1 {
            let mut controls = Controls::default();
            for name in Step::ALL {
                if protocol == "WND3" && !controls.can_attempt(name) {
                    println!("Skipping dependent trials for this fill.");
                    break;
                }
                let device = match first.take() {
                    Some(d) => d,
                    None => open(timeout)?,
                };
                let outcome = trial(
                    dir,
                    &mut report,
                    Shot {
                        endpoint,
                        polarity,
                        name,
                    },
                    &mut controls,
                    device,
                    timeout,
                )?;
                if !outcome.passed() {
                    good = false;
                    if protocol == "WND1" && name.is_control() {
                        println!("Skipping dependent trials for this fill.");
                        break;
                    }
                }
            }
        }
    }
    report.complete = true;
    report.save(dir)?;
    println!(
        "Capture saved in {}. Use dma-window --pack DIR to export one JSON file.",
        dir.display()
    );
    Ok(good)
}

type ControlGroups = [[Controls; 2]; 2];

fn audit(dir: &Path, report: &Report) -> io::Result<(ControlGroups, bool)> {
    audit_records(
        report,
        |row| fs::read(dir.join(&row.file)),
        &mut io::stdout().lock(),
    )
}

/// Check journal identity before loading each record or restoring its controls.
fn audit_records(
    report: &Report,
    mut load: impl FnMut(&Row) -> io::Result<Vec<u8>>,
    log: &mut impl io::Write,
) -> io::Result<(ControlGroups, bool)> {
    if !matches!(report.protocol.as_str(), "WND1" | "WND3") {
        return Err(invalid("expected a WND1 or WND3 report"));
    }
    let mut groups = ControlGroups::default();
    let schedule: Vec<_> = report
        .endpoint
        .endpoints()
        .iter()
        .flat_map(|&endpoint| {
            (0..=1).flat_map(move |polarity| {
                Step::ALL.map(|name| Shot {
                    endpoint,
                    polarity,
                    name,
                })
            })
        })
        .collect();
    let mut remaining = schedule.iter();
    let mut tokens = Vec::with_capacity(schedule.len());
    let mut good = report.complete && report.trials.len() == schedule.len();
    for (i, row) in report.trials.iter().enumerate() {
        let shot = row.shot;
        if !remaining.any(|s| s == &shot)
            || tokens.contains(&row.token)
            || row.case != shot.case()
            || row.file != shot.filename()
        {
            return Err(invalid("unexpected, duplicate or mislabeled window trial"));
        }
        tokens.push(row.token);
        let (Some(sum), Some(recorded)) = (&row.sha256, &row.observation) else {
            if report.complete || Some(i) != report.trials.len().checked_sub(1) {
                return Err(invalid("unfinished shot inside a completed sequence"));
            }
            writeln!(log, "{} incomplete acquisition", row.file)?;
            good = false;
            continue;
        };
        let data = load(row)?;
        if &digest(&data) != sum {
            return Err(invalid("archived hash mismatch"));
        }
        report.check_format(&data)?;
        let (header, outcome) =
            groups[shot.endpoint.index()][usize::from(shot.polarity)].evaluate(row, &data)?;
        if &outcome.to_string() != recorded {
            return Err(invalid("recorded verdict mismatch"));
        }
        if let Some(saved) = &row.header
            && saved != &header.json()?
        {
            return Err(invalid("recorded header mismatch"));
        }
        header.show(log, &row.file, outcome)?;
        good &= outcome.passed();
    }
    Ok((groups, good))
}

fn recover(dir: &Path, timeout: Duration) -> io::Result<()> {
    let _lock = directory_lock(dir)?;
    let mut report = Report::read(dir)?;
    let (mut groups, _) = audit(dir, &report)?;
    let row = report
        .trials
        .last()
        .ok_or_else(|| invalid("no window shot to recover"))?;
    if row.observation.is_some() {
        return Err(invalid("last trial was already recovered"));
    }
    let pending = row.shot;
    // A lost process can leave the durable capture in place before updating the report.
    let data = match fs::read(dir.join(pending.filename())) {
        Ok(data)
            if row.sha256.as_ref().is_none_or(|s| s == &digest(&data))
                && report.check_format(&data).is_ok()
                && Header::decode(&data, row).is_ok() =>
        {
            data
        }
        _ => {
            // The previous process may have been interrupted immediately after GO.
            thread::sleep(QUIET_INTERVAL);
            readback(row, timeout)?
        }
    };
    report_capture(
        dir,
        &mut report,
        &mut groups[pending.endpoint.index()][usize::from(pending.polarity)],
        &data,
    )?;
    Ok(())
}

pub(super) fn run(opts: &Options, timeout: Duration) -> io::Result<bool> {
    if !opts.pack.is_empty() {
        archive::pack(&opts.pack)?;
        return Ok(true);
    }
    if let Some(path) = &opts.replay {
        if path.is_dir() {
            return audit(path, &Report::read(path)?).map(|(_, good)| good);
        }
        return archive::replay(path);
    }
    if let Some(dir) = &opts.recover {
        recover(dir, timeout)?;
        return Ok(true);
    }
    capture(opts, timeout)
}
