use super::*;
use crate::{
    panel::{Button, Play},
    registers::{IN, PB, PH, read, write},
};

// Model a complete sweep through the real register driver. Each entry is a pressed key's
// driven ADC reading (lower rows) or rise above its open probe reading (upper row).
#[allow(
    clippy::arithmetic_side_effects,
    reason = "Rows are 2–4 and the simulated 10-bit probe rises at most 43 counts above 960."
)]
fn sweep(
    controls: &mut TouchControls,
    keys: &[Option<u16>; 27],
    adc_ready: bool,
) -> (Scan, PianoTouch) {
    let mut leds = Leds::default();
    for key in PianoKey::all() {
        leds.set(key, true)
    }
    loop {
        let column = usize::from(controls.panel.column);
        let mut pins = u32::MAX;
        for key in PianoKey::all() {
            let (col, row) = key.position();
            if col == column && keys[usize::from(key.index())].is_some() {
                pins &= !(1 << (row + 4));
            }
        }
        if column == 7 {
            pins &= !(1 << 5); // HOME beside the keys.
        }
        write(PA + IN, pins);
        write(PB + IN, u32::MAX);

        let adc = read(0x13100);
        if adc & (1 << 4) != 0 {
            let channel = (adc >> 8) & 15;
            let value = match channel {
                0..=2 => {
                    let row = [3u8, 4, 2][channel as usize];
                    let key = PianoKey::BY_ROW[usize::from(row - 2)][column].expect("piano slot");
                    let reading = keys[usize::from(key.index())];
                    let pin = 1 << (row + 4);
                    if channel == 2 {
                        assert_eq!(read(PA + OUT) & (1 << 10), 0);
                        assert_ne!(read(PA + DIR) & (1 << 10), 0);
                        assert_ne!(read(PA + PU) & (1 << 10), 0);
                        assert_eq!((read(PA + HD0) | read(PA + HD) | read(PA + DIR)) & pin, 0);
                        assert_ne!(read(PA + OUT) & pin, 0);
                        960 + reading.unwrap_or(0)
                    } else if read(PA + DIR) & pin == 0 {
                        assert_eq!(read(PA + OUT) & (3 << 9), 0);
                        assert_eq!(read(PH + OUT) & ((1 << 6) | (1 << 9)), 0);
                        assert_ne!(read(PA + OUT) & pin, 0);
                        assert_ne!(read(PA + HD0) & pin, 0);
                        assert_eq!(read(PA + HD) & pin, 0);
                        reading.unwrap_or(1023)
                    } else if reading.is_some() {
                        250
                    } else {
                        1023
                    }
                }
                4 => 789,
                _ => unreachable!("unexpected SARADC channel"),
            };
            assert_eq!(adc & 0xf00f, 0x200f);
            if adc_ready {
                write(0x13104, u32::from(value));
                set(0x13100, 1 << 7);
            }
        }
        if let Some(report) = controls.scan(&leds) {
            return report;
        }
    }
}

#[test]
fn chord_edges_match_velocity_and_only_the_leaning_key_reports_pressure() {
    let mut controls = Play::init().into_touch_controls();
    let mut keys = [None; 27];
    let a = PianoKey::new(2).expect("G3");
    let b = PianoKey::new(14).expect("G4");
    let c = PianoKey::new(26).expect("G5");
    for _ in 0..8 {
        sweep(&mut controls, &keys, true);
    }
    // All three keys share column five and use different ADC paths.
    keys[2] = Some(830);
    keys[14] = Some(530);
    keys[26] = Some(28);
    let mut presses = [0; 3];
    for _ in 0..256 {
        let (scan, touch) = sweep(&mut controls, &keys, true);
        for (i, k) in [a, b, c].into_iter().enumerate() {
            if scan.pressed(k) {
                presses[i] += 1;
                assert_eq!(touch.velocity(k), Some([strike_velocity(830), 127, 46][i]));
                assert_eq!(touch.pressure(k), k.supports_pressure().then_some(0));
            }
        }
        assert!(touch.pressure_changes().next().is_none());
        assert!(scan.is_down(Button::Home));
        assert_eq!(scan.master().map(MasterLevel::raw), Some(789));
    }
    assert_eq!(presses, [1, 1, 1]);
    keys[2] = Some(550);
    let mut last_pressure = 0;
    for _ in 0..64 {
        let (scan, touch) = sweep(&mut controls, &keys, true);
        assert!(scan.is_down(a) && scan.is_down(b) && scan.is_down(c));
        assert_eq!(touch.velocity(a), Some(strike_velocity(830)));
        assert_eq!(touch.pressure(b), Some(0));
        assert_eq!(touch.pressure(c), None);
        if let Some((key, p)) = touch.pressure_changes().next() {
            assert_eq!(key, a);
            assert!(p > last_pressure);
            assert_eq!(touch.pressure_changes().count(), 1);
            assert_eq!(touch.pressure(a), Some(p));
            last_pressure = p;
        } else {
            assert_eq!(touch.pressure(a), Some(last_pressure));
        }
    }
    assert!(last_pressure > 100);
    keys = [None; 27];
    let mut releases = [0; 3];
    for _ in 0..32 {
        let (scan, touch) = sweep(&mut controls, &keys, false);
        for (i, k) in [a, b, c].into_iter().enumerate() {
            if scan.released(k) {
                releases[i] += 1;
                assert_eq!((touch.velocity(k), touch.pressure(k)), (None, None));
            }
        }
    }
    assert_eq!(releases, [1, 1, 1]);
    keys[2] = Some(530);
    let mut pressed = false;
    for _ in 0..32 {
        let (scan, touch) = sweep(&mut controls, &keys, true);
        if scan.pressed(a) {
            assert_eq!((touch.velocity(a), touch.pressure(a)), (Some(127), Some(0)));
            pressed = true;
        }
    }
    assert!(pressed);
}

#[test]
fn graze_and_unfinished_strike_do_not_emit_a_press() {
    let mut key = AnalogKey::default();
    let (mut v, mut p) = (0, 0);
    for (sample, driven, closed) in [
        (Some(750), false, true),
        (None, true, true),
        (Some(850), true, true),
        (Some(1023), false, false),
    ] {
        key.sample(sample, driven, closed, &mut v, &mut p);
        assert_eq!((v, p), (0, 0));
    }
    assert!(matches!(key.contact, Contact::Open));
}

#[test]
fn normal_settling_does_not_sustain_pressure_but_a_later_lean_does() {
    let mut key = AnalogKey::default();
    let (mut velocity, mut pressure) = (0, 0);
    key.sample(Some(250), false, true, &mut velocity, &mut pressure);
    key.sample(Some(750), true, true, &mut velocity, &mut pressure);
    key.sample(Some(250), false, true, &mut velocity, &mut pressure);
    let mut hold = |value| {
        key.sample(Some(value), true, true, &mut velocity, &mut pressure);
        key.sample(Some(250), false, true, &mut velocity, &mut pressure);
        (velocity, pressure)
    };
    for _ in 0..64 {
        hold(700);
    }
    assert_eq!(hold(700), (strike_velocity(750), 0));
    for _ in 0..64 {
        hold(520);
    }
    assert_eq!(hold(520), (strike_velocity(750), 127));
    for _ in 0..64 {
        hold(700);
    }
    assert_eq!(hold(700), (strike_velocity(750), 0));
}

#[test]
fn a_top_key_held_at_boot_gets_an_open_reference_on_its_first_release() {
    let mut probe = Probe::default();
    let mut velocity = 0;
    probe.sample(Some(988), true, &mut velocity);
    assert_eq!(velocity, 0);
    probe.sample(Some(988), true, &mut velocity);
    assert_eq!(velocity, 64);
    probe.sample(Some(960), false, &mut velocity);
    probe.sample(Some(960), false, &mut velocity);
    assert_eq!(velocity, 0);
    probe.sample(Some(988), true, &mut velocity);
    probe.sample(Some(980), true, &mut velocity);
    assert_eq!(velocity, 46);
}

#[test]
fn dropping_during_a_probe_restores_both_pins_and_stops_the_adc() {
    let mut controls = Play::init().into_touch_controls();
    while !matches!(controls.slot, Slot::Key { key, .. } if !key.supports_pressure()) {
        controls.scan(&Leds::default());
    }
    drop(controls);
    assert_eq!(read(0x13100), 0);
    assert_eq!(read(PA + DIR) & ((1 << 6) | (1 << 10)), 1 << 6);
    assert_eq!(read(PA + PU) & ((1 << 6) | (1 << 10)), 1 << 6);
    assert_eq!(read(PA + OUT) & ((1 << 6) | (1 << 10)), 0);
}
