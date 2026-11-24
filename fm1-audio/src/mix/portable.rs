use super::Gain;

#[inline]
#[allow(
    clippy::arithmetic_side_effects,
    reason = "A signed 16-by-16 product fits in i32."
)]
fn accumulate(dst: &mut [i32; 2], input: [i16; 2], gains: [Gain; 2]) {
    for ((dst, sample), gain) in dst.iter_mut().zip(input).zip(gains) {
        let product = i32::from(sample) * i32::from(gain.to_bits());
        *dst = dst.saturating_add(product >> 6);
    }
}

pub(super) fn mono_i16(out: &mut [[i32; 2]], input: &[i16], gains: [Gain; 2]) -> usize {
    for (dst, &sample) in out.iter_mut().zip(input) {
        accumulate(dst, [sample; 2], gains);
    }
    out.len().min(input.len())
}

pub(super) fn stereo_i16(out: &mut [[i32; 2]], input: &[[i16; 2]], gains: [Gain; 2]) -> usize {
    for (dst, &frame) in out.iter_mut().zip(input) {
        accumulate(dst, frame, gains);
    }
    out.len().min(input.len())
}

pub(super) fn stereo_q23(out: &mut [[i32; 2]], input: &[[i32; 2]]) -> usize {
    for (dst, &[left, right]) in out.iter_mut().zip(input) {
        dst[0] = dst[0].saturating_add(left);
        dst[1] = dst[1].saturating_add(right);
    }
    out.len().min(input.len())
}

pub(super) fn planar_q23(out: &mut [[i32; 2]], left: &[i32], right: &[i32]) -> usize {
    for ((dst, &l), &r) in out.iter_mut().zip(left).zip(right) {
        dst[0] = dst[0].saturating_add(l);
        dst[1] = dst[1].saturating_add(r);
    }
    out.len().min(left.len()).min(right.len())
}
