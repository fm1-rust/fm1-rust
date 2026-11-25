use super::*;
extern crate alloc;
use alloc::vec::Vec;

const OWNER: [u8; 4] = *b"TEST";

#[derive(Clone, Debug)]
struct Model {
    bytes: [u8; 4 * SECTOR],
    // Cut the nth mutation after k bytes, including k=0 and k=length.
    cut: Option<(usize, usize)>,
    ops: Vec<usize>,
    fail_read: Option<usize>,
    ignore_write: bool,
    distort_second_read: Option<(usize, bool)>,
}

impl Default for Model {
    fn default() -> Self {
        Self {
            bytes: [0xff; 4 * SECTOR],
            cut: None,
            ops: Vec::new(),
            fail_read: None,
            ignore_write: false,
            distort_second_read: None,
        }
    }
}

impl Model {
    fn store(&mut self) -> Store<'_> {
        Store {
            flash: self,
            owner: OWNER,
        }
    }

    fn mutation(&mut self, count: usize) -> (usize, Result<(), Error>) {
        let index = self.ops.len();
        self.ops.push(count);
        if let Some((when, prefix)) = self.cut
            && when == index
        {
            (prefix, Err(Error::Io))
        } else {
            (if self.ignore_write { 0 } else { count }, Ok(()))
        }
    }
}

#[allow(
    clippy::arithmetic_side_effects,
    reason = "The model intentionally traps out-of-bounds driver requests."
)]
impl Nor for Model {
    fn read(&mut self, off: usize, out: &mut [u8]) -> Result<(), Error> {
        assert!(out.len() <= PAGE);
        if self.fail_read == Some(off) {
            return Err(Error::Io);
        }
        out.copy_from_slice(&self.bytes[off..off + out.len()]);
        if let Some((address, seen)) = &mut self.distort_second_read
            && *address == off
        {
            if *seen {
                out[0] ^= 1;
            }
            *seen = true;
        }
        Ok(())
    }

    fn program(&mut self, off: usize, bytes: &[u8]) -> Result<(), Error> {
        assert!(!bytes.is_empty() && off % PAGE + bytes.len() <= PAGE);
        let (prefix, status) = self.mutation(bytes.len());
        for (cell, &byte) in self.bytes[off..off + prefix].iter_mut().zip(bytes) {
            assert_eq!(*cell, 0xff, "one program per page between erases");
            *cell &= byte;
        }
        status
    }

    fn erase(&mut self, off: usize) -> Result<(), Error> {
        assert_eq!(off % SECTOR, 0);
        let (prefix, status) = self.mutation(SECTOR);
        self.bytes[off..off + prefix].fill(0xff);
        status
    }
}

#[cfg(not(fm1))]
#[test]
fn port_is_exclusive_and_can_be_reacquired() {
    let board = Board::init();
    let port = board.take_storage().expect("first owner");
    assert!(board.take_storage().is_none());
    drop(port);
    assert!(board.take_storage().is_some());
}

#[test]
fn sizes_slots_and_identical_saves() {
    let mut m = Model::default();
    let mut out = [0x55; MAX_LEN + 1];
    assert_eq!(m.store().load(0, &mut out), Ok(None));
    let payload = [0x9a; MAX_LEN];
    for size in [0, 1, PAGE - 1, PAGE, PAGE + 1, MAX_LEN] {
        let bytes = &payload[..size];
        m.store().save(1, bytes).expect("save");
        let calls = m.ops.len();
        m.store().save(1, bytes).expect("skip duplicate");
        assert_eq!(m.ops.len(), calls);
        assert_eq!(m.store().load(1, &mut out), Ok(Some(size)));
        assert_eq!(&out[..size], bytes);
        assert_eq!(out[size], 0x55);
        assert_eq!(m.store().load(0, &mut out), Ok(None));
    }
    let mut short = [0xeb];
    assert_eq!(
        m.store().load(1, &mut short),
        Err(Error::BufferTooSmall(MAX_LEN))
    );
    assert_eq!(short, [0xeb]);
    let calls = m.ops.len();
    for index in [SLOT_COUNT, u8::MAX] {
        assert_eq!(m.store().save(index, b"bad"), Err(Error::OutOfRange));
        assert_eq!(m.store().load(index, &mut out), Err(Error::OutOfRange));
        assert_eq!(m.store().reclaim(index), Err(Error::OutOfRange));
    }
    assert_eq!(m.store().save(0, &out), Err(Error::OutOfRange));
    assert_eq!(m.ops.len(), calls);
}

#[test]
fn foreign_bytes_even_after_a_blank_header_require_explicit_reclamation() {
    let mut m = Model::default();
    m.store().save(0, b"patch").expect("seed");
    m.store().save(1, b"phrase").expect("seed neighbor");
    m.bytes[2 * SECTOR - 1] = 0x11;
    let before = m.bytes;
    assert_eq!(m.store().save(0, b"new patch"), Err(Error::Occupied));
    let mut foreign = Store {
        flash: &mut m,
        owner: *b"UPDT",
    };
    assert_eq!(foreign.load(0, &mut []), Err(Error::Occupied));
    assert_eq!(foreign.save(1, b"foreign"), Err(Error::Occupied));
    assert_eq!(m.bytes, before);
    m.store().reclaim(0).expect("deliberate reclaim");
    assert_eq!(&m.bytes[..SECTOR], &before[..SECTOR]);
    assert_eq!(&m.bytes[2 * SECTOR..], &before[2 * SECTOR..]);
    assert!(m.bytes[SECTOR..2 * SECTOR].iter().all(|b| *b == 0xff));
    let mut out = [0; 5];
    assert_eq!(m.store().load(0, &mut out), Ok(Some(5)));
    assert_eq!(&out, b"patch");
}

#[test]
fn crc_covers_generation_length_and_payload_and_can_fall_back() {
    assert_eq!(!crc32(u32::MAX, b"123456789"), 0xcbf4_3926);
    let mut m = Model::default();
    m.store().save(0, b"old").expect("old");
    m.store().save(0, b"new").expect("new");
    for at in [PAGE, COMMIT, COMMIT + 4, COMMIT + 8, COMMIT + 12] {
        let mut bad = m.clone();
        bad.bytes[SECTOR + at] ^= 0x80;
        let mut out = [0; 3];
        assert_eq!(bad.store().load(0, &mut out), Ok(Some(3)));
        assert_eq!(&out, b"old");
        bad.bytes[PAGE] ^= 0x80;
        assert_eq!(bad.store().load(0, &mut out), Err(Error::Corrupt));
    }
    let mut out = [0; 3];
    m.fail_read = Some(SECTOR + COMMIT);
    assert_eq!(m.store().load(0, &mut out), Err(Error::Io));
    assert_eq!(m.store().save(0, b"X"), Err(Error::Io));
}

#[test]
fn load_checks_the_bytes_actually_returned() {
    let mut chip = Model::default();
    chip.store().save(0, b"patch").expect("save");
    chip.distort_second_read = Some((PAGE, false));
    assert_eq!(chip.store().load(0, &mut [0; 5]), Err(Error::Io));
}

#[test]
fn generation_rollover() {
    let rec = |generation| {
        Bank::Valid(Record {
            generation,
            len: 0,
            payload_crc: u32::MAX,
        })
    };
    for (a, b, winner) in [(u32::MAX, 0, 1), (0, u32::MAX, 0), (41, 42, 1), (42, 41, 0)] {
        assert_eq!(newest([rec(a), rec(b)]).map(|(i, _)| i), Some(winner));
    }
}

#[test]
fn a_silent_program_or_erase_failure_never_commits() {
    let mut m = Model {
        ignore_write: true,
        ..Model::default()
    };
    assert_eq!(m.store().save(0, b"first"), Err(Error::Io));
    assert_eq!(m.store().load(0, &mut []), Ok(None));
    m.ignore_write = false;
    m.store().save(0, b"old").expect("old");
    m.store().save(0, b"now").expect("current");
    m.ignore_write = true;
    assert_eq!(m.store().save(0, b"bad"), Err(Error::Io));
    let mut out = [0; 3];
    assert_eq!(m.store().load(0, &mut out), Ok(Some(3)));
    assert_eq!(&out, b"now");
}

#[test]
fn power_cut_at_every_changed_byte() {
    let next = [0x51; PAGE + 11];
    let mut initial = Model::default();
    initial.store().save(1, b"unrelated").expect("neighbor");
    for replace in [false, true] {
        if replace {
            initial.store().save(0, b"older").expect("seed old target");
            initial.store().save(0, b"latest").expect("seed current");
        }
        initial.ops.clear();
        let mut complete = initial.clone();
        complete.store().save(0, &next).expect("measure writes");
        for (op, count) in complete.ops.into_iter().enumerate() {
            for prefix in 0..=count {
                let mut chip = initial.clone();
                chip.cut = Some((op, prefix));
                assert_eq!(chip.store().save(0, &next), Err(Error::Io));
                chip.cut = None; // cold mount: only the flash contents survive
                let mut out = [0; PAGE + 11];
                let recovered = chip.store().load(0, &mut out);
                let intact = match recovered {
                    Ok(Some(n)) => out[..n] == next || (replace && &out[..n] == b"latest"),
                    Ok(None) | Err(Error::Corrupt | Error::Occupied) => !replace,
                    _ => false,
                };
                assert!(intact, "cut {op} at {prefix} recovered {recovered:?}");
                assert_eq!(&chip.bytes[2 * SECTOR..], &initial.bytes[2 * SECTOR..]);
                // A torn erase/ownership tag is indistinguishable from foreign
                // data. Explicit reclamation must preserve the committed peer.
                if chip.store().save(0, &next) == Err(Error::Occupied) {
                    chip.store()
                        .reclaim(0)
                        .expect("recover interrupted ownership");
                }
                chip.store().save(0, &next).expect("retry");
                assert_eq!(chip.store().load(0, &mut out), Ok(Some(next.len())));
                assert_eq!(out, next);
            }
        }
    }
}
