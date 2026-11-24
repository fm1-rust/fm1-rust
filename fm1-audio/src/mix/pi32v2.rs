//! AC791N kernels in `kernels.S`. Only the native PCM and accumulator element
//! types cross the ABI. The low word in each accumulator pair is the left lane.
use super::Gain;

const _: () = {
    assert!(cfg!(target_endian = "little") && usize::BITS == 32);
    assert!(align_of::<i16>() == 2 && align_of::<i32>() == 4);
};

// SAFETY: kernels.S implements these leaf functions with the pi32v2 C ABI.
// They restore the stack and callee-saved registers, return synchronously and
// retain no pointers. Each skips all buffer accesses if frames is zero.
unsafe extern "C" {
    /// # Safety
    /// For this call, `out` uniquely borrows `frames` initialized stereo i32
    /// frames and `input` borrows that many disjoint initialized mono i16 samples.
    fn fm1_audio_mix_v1_mono_i16(out: *mut [i32; 2], input: *const i16, frames: usize, gains: u32);
    /// # Safety
    /// As for the mono kernel, with one initialized stereo i16 frame per iteration.
    fn fm1_audio_mix_v1_stereo_i16(
        out: *mut [i32; 2],
        input: *const [i16; 2],
        frames: usize,
        gains: u32,
    );
    /// # Safety
    /// As for the mono kernel, with one initialized stereo i32 frame per iteration.
    fn fm1_audio_mix_v1_stereo_q23(out: *mut [i32; 2], input: *const [i32; 2], frames: usize);
    /// # Safety
    /// For the call, `out` exclusively covers `frames` initialized stereo i32
    /// frames. Both input planes cover that many initialized i32 words and may
    /// overlap each other. Each must be disjoint from the output.
    fn fm1_audio_mix_v1_planar_q23(
        out: *mut [i32; 2],
        left: *const i32,
        frames: usize,
        right: *const i32,
    );
}

fn pack([left, right]: [Gain; 2]) -> u32 {
    let [l0, l1] = left.to_bits().to_le_bytes();
    let [r0, r1] = right.to_bits().to_le_bytes();
    u32::from_le_bytes([l0, l1, r0, r1])
}

pub(super) fn mono_i16(out: &mut [[i32; 2]], input: &[i16], gains: [Gain; 2]) -> usize {
    let n = out.len().min(input.len());
    // SAFETY: the slices cover n disjoint live frames, with exclusive access to out.
    // Each iteration reads one aligned halfword and updates one word-aligned pair.
    unsafe { fm1_audio_mix_v1_mono_i16(out.as_mut_ptr(), input.as_ptr(), n, pack(gains)) };
    n
}

pub(super) fn stereo_i16(out: &mut [[i32; 2]], input: &[[i16; 2]], gains: [Gain; 2]) -> usize {
    let n = out.len().min(input.len());
    // SAFETY: the slices cover n disjoint live frames, with exclusive access to out.
    // Each input frame is two adjacent initialized halfwords. The kernel reads a
    // word only after checking four-byte alignment, or uses two halfword loads.
    // A four-byte stride preserves that alignment class throughout the call.
    unsafe { fm1_audio_mix_v1_stereo_i16(out.as_mut_ptr(), input.as_ptr(), n, pack(gains)) };
    n
}

pub(super) fn stereo_q23(out: &mut [[i32; 2]], input: &[[i32; 2]]) -> usize {
    let n = out.len().min(input.len());
    // SAFETY: both slices cover n initialized, word-aligned pairs. Each iteration
    // reads one pair from each and writes one to the exclusively borrowed out.
    unsafe { fm1_audio_mix_v1_stereo_q23(out.as_mut_ptr(), input.as_ptr(), n) };
    n
}

pub(super) fn planar_q23(out: &mut [[i32; 2]], left: &[i32], right: &[i32]) -> usize {
    let n = out.len().min(left.len()).min(right.len());
    // SAFETY: n is bounded by all three borrows. Each iteration reads one
    // aligned i32 per shared plane and updates one exclusively borrowed pair.
    unsafe { fm1_audio_mix_v1_planar_q23(out.as_mut_ptr(), left.as_ptr(), n, right.as_ptr()) };
    n
}
