// Apply emulator/usb-host.patch and then window/usb-wo-model.patch to the pinned fm1-emu.
// cargo run -p fm1-soc --release --example check_window -- APP.bin CASE STIMULUS OUT.bin [MODEL]
use fm1_core::Bus;
use fm1_soc::machine::{Machine, Options};

const RECORD: u32 = 0x01c7c000 - fm1_soc::RAM_BASE;
const VECTOR1: u32 = 0x01c7fe04;
const CASE_RESULT: u32 = RECORD + 4 * 3;
const STARTED_GO: u32 = RECORD + 4 * 29;

fn until(m: &mut Machine, deadline_ms: u64, mut done: impl FnMut(&Machine) -> bool) {
    let start = m.soc.periph.ms();
    while !done(m) {
        assert!(
            m.soc.periph.ms() - start < deadline_ms,
            "deadline, pc={:#x}",
            m.cpu.pc
        );
        m.run(10000).unwrap();
    }
}

fn control(m: &mut Machine, setup: [u8; 8]) -> Vec<u8> {
    m.soc.usb.test_request(setup);
    until(m, 100, |m| m.soc.usb.test_ctl.is_some());
    m.soc.usb.test_ctl.take().unwrap().unwrap()
}

// Apply the hypothetical readback after every instruction, from SETUP until
// the firmware starts GO or records its refusal. Firmware does the checks.
fn go(m: &mut Machine, request: [u8; 8], model: &str) -> bool {
    if matches!(
        model,
        "nominal" | "drop-address" | "zero-read-vector" | "zero-read-no-vector"
    ) {
        control(m, request);
        return true;
    }
    let original_vector = m.soc.ram.read32(VECTOR1 - fm1_soc::RAM_BASE);
    let start = m.soc.periph.ms();
    m.soc.usb.test_request(request);
    while m.soc.ram.read32(STARTED_GO) == 0 && m.soc.ram.read32(CASE_RESULT) == 0 {
        match model {
            // A priority bit held high on disabled source 7. R1 compared all
            // 32 words. R2 checks just the two source enables it depends on.
            "held-priority" | "r1-held-priority" => m.soc.periph.icfg[0] |= 1 << 31,
            "vector-readonly" => m
                .soc
                .ram
                .write32(VECTOR1 - fm1_soc::RAM_BASE, original_vector),
            "exception-disabled" => m.soc.periph.icfg[0] &= !(1 << 4),
            "reject-limit" => m.soc.write32(0x01eee000 + 4 * 0xd4, 0),
            "r2-address-zero" | "r3-exception-zero" => {} // the peripheral model supplies the readback
            _ => panic!("unknown readback model {model}"),
        }
        assert!(m.soc.periph.ms() - start < 500, "setup deadline");
        m.run(1).unwrap();
    }
    if m.soc.ram.read32(STARTED_GO) == 0 {
        return false;
    }
    until(m, 100, |m| m.soc.usb.test_ctl.is_some());
    m.soc.usb.test_ctl.take().unwrap().unwrap();
    true
}

fn boot(image: &[u8]) -> Machine {
    let mut m = Machine::from_image(
        image.to_vec(),
        fm1_soc::XIP_BASE + 0x120,
        &Options {
            strict: true,
            ..Options::default()
        },
    );
    m.soc.usb_plug();
    until(&mut m, 3500, |m| m.soc.usb.product().is_some());
    m
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    let [_, app, case, stimulus, out, options @ ..] = args.as_slice() else {
        panic!("APP.bin CASE STIMULUS OUT.bin [MODEL]");
    };
    let model = match options {
        [] => "nominal",
        [model] => model.as_str(),
        _ => panic!("expected at most one readback model"),
    };
    let case: u8 = case.parse().unwrap();
    assert!(case < 16);
    let setup = [0x40, 0x79, 0x32, 0x31, 0x34, 0x33, 0, 0];
    let payload: Vec<u8> = if case & 4 != 0 {
        setup.to_vec()
    } else {
        (16..80).collect()
    };
    let mut image = vec![0xff; 0x120];
    image.extend(std::fs::read(app).unwrap());
    let mut m = boot(&image);
    let hello = control(&mut m, [0xc0, 0x70, 0, 0, 0, 0, 12, 0]);
    let wnd3 = match hello.as_slice() {
        b"WND3\x03\x02\x80\x00\x40\x00\x00\x20" => true,
        b"WND1\x01\x02\x80\x00\x40\x00\x00\x20" => false,
        _ => panic!("unknown window protocol"),
    };
    let magic = u32::from_le_bytes(hello[..4].try_into().unwrap());
    assert_eq!(m.soc.usb.test_config().len(), 34);
    control(&mut m, [1, 11, 1, 0, 0, 0, 0, 0]);
    let flavor = match stimulus.as_str() {
        "quiet" => 1,
        "wire" => 2,
        "fault-stop" => 3,
        "fault-cross" => 4,
        "prefix-only" => 5,
        _ => panic!("choose quiet, wire, fault-stop, fault-cross or prefix-only"),
    };
    let token = 0x3000 + 8 * u16::from(case) + flavor;
    let [lo, hi] = token.to_le_bytes();
    let address_reg = if case & 4 == 0 { 0x11820 } else { 0x11818 };
    m.soc.usb.test_wo_rx = wnd3 || model == "r2-address-zero";
    m.soc.periph.test_vector1_write_accepted = match model {
        "zero-read-vector" => Some(true),
        "r3-exception-zero" | "zero-read-no-vector" => Some(false),
        _ => None,
    };
    if model == "drop-address" {
        assert!(wnd3 && stimulus == "wire");
        m.soc.usb.test_drop_address = Some(address_reg);
    }
    let started = go(&mut m, [0x40, 0x72, case, 0, lo, hi, 0, 0], model);
    let rec = RECORD;
    match (started, stimulus.as_str()) {
        (false, _) | (true, "quiet") => {}
        (true, "wire") if case & 4 != 0 => m.soc.usb.test_request(setup),
        (true, "wire") => m.soc.usb.test_q.push_back(payload),
        (true, "fault-stop" | "fault-cross" | "prefix-only") => {
            // The emulator has no peripheral-range enforcement. Inject the
            // chosen SRAM result independently of fault latching or dispatch.
            let dest = m.soc.ram.read32(rec + 4 * 4) - fm1_soc::RAM_BASE;
            let cut = m.soc.ram.read32(rec + 4 * 5) as usize;
            let extent = if stimulus == "fault-cross" {
                payload.len()
            } else {
                cut
            };
            for (i, &b) in payload[..extent].iter().enumerate() {
                m.soc.ram.write8(dest + i as u32, b);
            }
            if stimulus != "prefix-only" {
                m.soc.write32(0x01eee000 + 4 * 0xda, 1 << 7);
                m.soc.write32(0x01eee000 + 4 * 0xdb, 0x81);
                m.soc.dbg_cause = 4;
                m.soc.periph.exc_pending = true;
            }
        }
        _ => unreachable!("stimulus validated above"),
    }
    until(&mut m, 1800, |m| m.soc.ram.read32(rec) == magic);
    if wnd3 {
        assert_eq!(m.soc.usb.test_rx_address_reads, 0);
    } else if model == "r2-address-zero" {
        assert_eq!(m.soc.usb.test_rx_address_reads, 1);
    }
    let bytes: Vec<u8> = (0..8320).map(|i| m.soc.ram.read8(rec + i)).collect();
    let result = match (model, case & 3 != 0) {
        ("r1-held-priority", _) => 3,
        ("r2-address-zero", _) => {
            if case & 4 == 0 {
                0x14
            } else {
                0x13
            }
        }
        ("vector-readonly", true) => 0x10,
        ("exception-disabled" | "r3-exception-zero", true) => 0x12,
        ("reject-limit", true) => 0x1a,
        _ => 1,
    };
    assert_eq!(m.soc.ram.read32(CASE_RESULT), result);
    assert_eq!(started, result == 1);
    if !started || model == "drop-address" {
        let fill = if result == 3 {
            0
        } else if case & 8 != 0 {
            0x5a
        } else {
            0xa5
        };
        assert!(bytes[128..].iter().all(|&b| b == fill));
        if !started && result != 3 {
            let word = |index: u32| m.soc.ram.read32(rec + 4 * index);
            let (register, mask) = match result {
                0x10 => (VECTOR1, u32::MAX),
                0x12 => (0x01eef100, 1 << 4),
                0x13 | 0x14 => (address_reg, u32::MAX),
                0x1a => (0x01eee000 + 4 * 0xd4, 1 << 7),
                _ => unreachable!(),
            };
            assert_eq!(word(25), register);
            assert_ne!((word(27) ^ word(28)) & mask, 0);
            if model == "r2-address-zero" {
                assert_eq!(word(26), 0);
                assert_eq!(word(28), 0);
            }
            if model == "r3-exception-zero" {
                assert_eq!((word(27), word(28)), (0xf0, 0));
            }
            assert_eq!((word(7), word(9)), (u32::MAX, u32::MAX));
        }
    }
    assert_eq!(
        m.soc.ram.read32(rec + 4 * 14),
        u32::from(started && stimulus.starts_with("fault-") && model != "zero-read-no-vector")
    );
    if started && stimulus.starts_with("fault-") {
        assert_eq!(m.soc.ram.read32(rec + 4 * 10), 4);
        assert_eq!(m.soc.ram.read32(rec + 4 * 11), 1 << 7);
        assert_eq!(m.soc.ram.read32(rec + 4 * 12), 0x81);
        assert!(m.soc.ram.read32(rec + 4 * 30) >= 24_000_000);
    }
    // Model a fresh boot with preserved bytes explicitly. Actual retention
    // and physical USB reconnect are separate device observations.
    let mut m = boot(&image);
    for (i, &b) in bytes.iter().enumerate() {
        m.soc.ram.write8(rec + i as u32, b);
    }
    let mut reply = Vec::new();
    for page in 0..130 {
        reply.extend(control(&mut m, [0xc0, 0x71, page, 0, 0, 0, 64, 0]));
    }
    assert_eq!(reply, bytes);
    std::fs::write(out, bytes).unwrap();
}
