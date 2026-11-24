use super::*;
use crate::{I24, mix_stereo};

#[test]
fn pcm16_gain_preserves_q23_headroom_and_channel_order() {
    let input = [[i16::MIN, i16::MAX], [1, -1]];
    let mut out = [[0; 2]; 2];
    assert_eq!(stereo_i16(&mut out, &input, [Gain::UNITY; 2]), 2);
    assert_eq!(out, [[-8_388_608, 8_388_352], [256, -256]]);
    stereo_i16(&mut out, &input, [Gain::UNITY; 2]);
    assert_eq!(out[0], [-16_777_216, 16_776_704]);
    mono_i16(
        &mut out[1..],
        &[1],
        [Gain::from_bits(-1), Gain::from_bits(1)],
    );
    assert_eq!(out[1], [511, -512]);
}

#[test]
fn mono_and_stereo_agree_with_wide_reference() {
    let samples = [i16::MIN, -23, -1, 0, 1, 42, i16::MAX];
    let gains = [i16::MIN, -16384, -1, 0, 1, 8192, 16384, i16::MAX];
    let seed = [i32::MIN + 3, i32::MAX - 5];
    for &l in &gains {
        for &r in &gains {
            let gains = [l, r].map(Gain::from_bits);
            let mut out = [seed; 7];
            let mut stereo = out;
            mono_i16(&mut out, &samples, gains);
            stereo_i16(&mut stereo, &samples.map(|s| [s, s]), gains);
            assert_eq!(out, stereo);
            for (&s, frame) in samples.iter().zip(out) {
                let expected = core::array::from_fn(|i| {
                    let product = i64::from(s) * i64::from(gains[i].to_bits());
                    let sum = i64::from(seed[i]) + product.div_euclid(64);
                    i32::try_from(sum.clamp(i64::from(i32::MIN), i64::from(i32::MAX)))
                        .expect("clamped to i32")
                });
                assert_eq!(frame, expected);
            }
        }
    }
}

#[test]
fn lengths_and_empty_inputs_leave_the_tail_untouched() {
    let mut out = [[7, 9]; 131];
    for n in [0, 1, 15, 16, 17, 31, 32, 33, 127, 128, 129] {
        let sentinel = out;
        let left = [123; 130];
        let right = [-345; 130];
        assert_eq!(planar_q23(&mut out[1..], &left[..n], &right), n);
        assert_eq!(out[0], sentinel[0]);
        assert_eq!(out[n + 1..], sentinel[n + 1..]);
        assert_eq!(stereo_q23(&mut out[..n], &[[0; 2]; 130]), n);
        assert_eq!(mono_i16(&mut out[..n], &[0; 130], [Gain::ZERO; 2]), n);
        assert_eq!(
            stereo_i16(&mut out, &[[0; 2]; 130][..n], [Gain::ZERO; 2]),
            n
        );
        assert_eq!(planar_q23(&mut out, &left, &right[..n]), n);
    }
}

#[test]
fn q23_paths_match_legacy_including_full_i32_inputs() {
    let values = [
        i32::MIN,
        i32::MIN + 1,
        -8_388_608,
        -1,
        0,
        1,
        8_388_607,
        i32::MAX,
    ];
    const N: usize = 8;
    let left = values;
    let right = left.map(|v| !v);
    for value in values {
        let seed = [[value, !value]; N];
        let mut planar = seed;
        let mut stereo = seed;
        let mut legacy = seed;
        let mut both = seed;
        let left_i24 = left.map(I24::new_unchecked);
        let right_i24 = right.map(I24::new_unchecked);
        mix_stereo(&mut legacy, &left_i24, &right_i24);
        planar_q23(&mut planar, &left, &right);
        planar_q23(&mut both, &left, &left);
        let frames: [[i32; 2]; N] = core::array::from_fn(|i| [left[i], right[i]]);
        stereo_q23(&mut stereo, &frames);
        for (i, frame) in frames.iter().enumerate() {
            let expected = [
                value.saturating_add(frame[0]),
                (!value).saturating_add(frame[1]),
            ];
            assert_eq!(legacy[i], expected);
            assert_eq!(stereo[i], expected);
            assert_eq!(planar[i], expected);
            assert_eq!(both[i][1], (!value).saturating_add(frame[0]));
        }
    }
}
