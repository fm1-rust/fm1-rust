//! Classifier tests for WND1 and WND3 use controlled wire bytes and register snapshots.
use super::*;
use crate::Args;
use clap::Parser;
use fm1_usb::dma_probe::window::{EP0_SETUP, EP1_DATA, RECORD_LEN};

fn row(ep: Endpoint, polarity: u8, name: Step) -> Row {
    let shot = Shot {
        endpoint: ep,
        polarity,
        name,
    };
    Row {
        shot,
        case: shot.case(),
        token: NonZeroU16::MIN,
        file: shot.filename(),
        go: "ok".into(),
        stimulus: if name.is_quiet() {
            "skipped"
        } else {
            "host transfer"
        }
        .into(),
        host_quiet_complete: None,
        header: None,
        observation: None,
        sha256: None,
        location: Map::new(),
    }
}

fn record(r: &Row, payload: &[u8], edit: impl FnOnce(&mut [u32; 32])) -> Vec<u8> {
    record_for(*b"WND1", r, payload, edit)
}

fn record_for(tag: [u8; 4], r: &Row, payload: &[u8], edit: impl FnOnce(&mut [u32; 32])) -> Vec<u8> {
    let mut data = vec![r.fill(); RECORD_LEN];
    let prefix: u8 = if tag == *b"WND3" && r.shot.polarity == 1 {
        128
    } else {
        64
    };
    let base = 0x01c4_4000;
    let dest = base + u32::from(prefix);
    let start = 128 + usize::from(prefix);
    let cut = u32::from(r.cut());
    data[start..][..payload.len()].copy_from_slice(payload);
    let mut h = [0; 32];
    let n = u32::try_from(payload.len()).expect("test payload");
    h[..10].copy_from_slice(&[
        u32::from_le_bytes(tag),
        u32::from(r.token.get()),
        u32::from(r.case),
        1,
        dest,
        cut,
        u32::from(r.fill()),
        if n > cut { cut } else { u32::MAX },
        u32::from(data[start..][usize::from(r.cut())]),
        n.wrapping_sub(1),
    ]);
    if r.shot.name.mode() != 0 {
        h[16..22].copy_from_slice(&[
            dest,
            dest.wrapping_add(cut).wrapping_sub(1),
            0x81,
            128,
            128,
            128,
        ]);
    }
    h[25..31].copy_from_slice(&[
        n,
        if tag == *b"WND3" { base } else { dest },
        0,
        0,
        1,
        24_000_000,
    ]);
    edit(&mut h);
    for (src, dst) in h.iter().zip(data[..128].as_chunks_mut::<4>().0) {
        *dst = src.to_le_bytes();
    }
    let hash = data[4..124]
        .iter()
        .chain(&data[128..])
        .fold(2166136261u32, |a, &b| {
            (a ^ u32::from(b)).wrapping_mul(16777619)
        });
    data[124..128].copy_from_slice(&hash.to_le_bytes());
    data
}

#[test]
fn wnd3_needs_a_delivered_payload_at_each_programmed_offset() -> io::Result<()> {
    for ep in [Endpoint::Ep0, Endpoint::Ep1] {
        let payload: &[u8] = if ep == Endpoint::Ep0 {
            &EP0_SETUP
        } else {
            &EP1_DATA
        };
        for polarity in [0, 1] {
            let mut controls = Controls::default();
            let quiet = row(ep, polarity, Step::QuietOff);
            let wire = row(ep, polarity, Step::Baseline);
            assert!(controls.can_attempt(Step::Baseline));
            let (header, quiet_outcome) =
                controls.evaluate(&quiet, &record_for(*b"WND3", &quiet, &[], |_| {}))?;
            assert_eq!(quiet_outcome, Outcome::Quiet);
            let json = header.json()?;
            assert_eq!(json["arena_base"], 0x01c4_4000);
            assert!(json.get("address_readback").is_none());
            assert!(!controls.can_attempt(Step::Aligned));
            // A host ACK and a controller count with no payload in this arena.
            let dropped = record_for(*b"WND3", &wire, &[], |h| {
                h[25] = u32::try_from(payload.len()).expect("at most 64 bytes")
            });
            assert_eq!(
                controls.evaluate(&wire, &dropped)?.1,
                Outcome::Inconclusive("positive control failed to reproduce the payload")
            );
            assert!(!controls.can_attempt(Step::InRange));
            let mislocated = record_for(*b"WND3", &wire, payload, |h| h[26] = h[4]);
            assert_eq!(
                controls.evaluate(&wire, &mislocated)?.1,
                Outcome::Inconclusive("DMA destination")
            );
            let delivered = record_for(*b"WND3", &wire, payload, |_| {});
            assert_eq!(controls.evaluate(&wire, &delivered)?.1, Outcome::Delivered);
            assert!(controls.can_attempt(Step::InRange));
            assert!(!controls.can_attempt(Step::Aligned));
        }
    }
    Ok(())
}

fn fault(h: &mut [u32; 32]) {
    h[10..13].copy_from_slice(&[4, 128, 0x81]);
}

#[test]
fn distinguishes_stopped_and_committed_writes_even_on_identical_faults() {
    for ep in [Endpoint::Ep0, Endpoint::Ep1] {
        let payload: &[u8] = if ep == Endpoint::Ep0 {
            &EP0_SETUP
        } else {
            &EP1_DATA
        };
        for polarity in [0, 1] {
            let mut refs = Controls::default();
            for name in Step::ALL {
                let r = row(ep, polarity, name);
                if name.is_control() {
                    let (p, expected) = if name.is_quiet() {
                        (&[][..], Outcome::Quiet)
                    } else {
                        (payload, Outcome::Delivered)
                    };
                    assert_eq!(
                        refs.evaluate(&r, &record(&r, p, |_| {}))
                            .expect("control")
                            .1,
                        expected
                    );
                } else {
                    let prefix = &payload[..usize::from(r.cut())];
                    for (data, expected) in [
                        (record(&r, prefix, fault), Outcome::ObservedStop),
                        (record(&r, payload, fault), Outcome::CrossedWithFault),
                        (
                            record(&r, payload, |_| {}),
                            Outcome::CrossedWithoutAttribution,
                        ),
                    ] {
                        assert_eq!(refs.evaluate(&r, &data).expect("valid record").1, expected);
                    }
                    for (field, value) in [
                        (10, 0),
                        (11, 64),
                        (12, 0x82),
                        (13, 0x82),
                        (23, 4),
                        (24, 4),
                        (29, 0),
                        (30, 11),
                    ] {
                        let data = record(&r, prefix, |h| {
                            fault(h);
                            h[field] = value;
                        });
                        assert!(matches!(
                            refs.evaluate(&r, &data).expect("valid record").1,
                            Outcome::Inconclusive(_)
                        ));
                    }
                    let data = record(&r, prefix, fault);
                    assert!(matches!(
                        Controls::default()
                            .evaluate(&r, &data)
                            .expect("no controls")
                            .1,
                        Outcome::Inconclusive(_)
                    ));
                }
            }
        }
    }
}

#[test]
fn rejects_stale_damaged_or_interrupted_evidence() {
    let mut r = row(Endpoint::Ep1, 0, Step::QuietOff);
    let mut raw = record(&r, &[], |_| {});
    r.host_quiet_complete = Some(false);
    assert_eq!(
        Controls::default()
            .evaluate(&r, &raw)
            .expect("valid record")
            .1,
        Outcome::Inconclusive("host acquisition interrupted")
    );
    raw[1024] ^= 1;
    assert!(Header::decode(&raw, &r).is_err());
    raw[1024] ^= 1;
    r.token = NonZeroU16::new(2).expect("nonzero");
    assert!(Header::decode(&raw, &r).is_err());
}

#[test]
fn modes_and_board_alias_work() {
    for flag in [
        "--board",
        "--board-revision",
        "--recover",
        "--replay",
        "--pack",
    ] {
        assert!(Args::try_parse_from(["fm1", "dma-window", flag, "example"]).is_ok());
    }
    assert!(Args::try_parse_from(["fm1", "dma-window"]).is_err());
    assert!(Args::try_parse_from(["fm1", "dma-window", "--pack", "r1", "r2"]).is_ok());
    assert!(Args::try_parse_from(["fm1", "dma-window", "--pack", "r1", "--out", "dir"]).is_err());
    assert!(
        Args::try_parse_from(["fm1", "dma-window", "--replay", "dir", "--board", "black"]).is_err()
    );
    assert!(
        Args::try_parse_from(["fm1", "dma-window", "--recover", "dir", "--endpoint", "ep0"])
            .is_err()
    );
}

#[test]
fn replay_rejects_unsafe_filenames_before_opening_a_device_or_file() {
    let mut r = row(Endpoint::Ep0, 0, Step::QuietOff);
    r.file = "../not-a-shot".into();
    let report = Report {
        protocol: "WND1".into(),
        endpoint: Scope::Both,
        complete: false,
        trials: vec![r],
        meta: Map::new(),
    };
    assert!(audit(Path::new("missing"), &report).is_err());
}
