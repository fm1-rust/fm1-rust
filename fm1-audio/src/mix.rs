//! Add PCM sources to a borrowed stereo mix in signed Q23 units.
//!
//! All operations return the smallest frame count among their slices and
//! leave any remaining output frames alone. Additions saturate at the `i32`
//! limits. [`crate::Output`] applies MASTER and the DAC's 24-bit clip after all sources.
//! These functions can also run on ordinary application buffers on either core.
//!
//! # Sample layout
//!
//! A stereo frame is a Rust array `[left, right]` of native-endian integers.
//! PCM16 frames occupy four bytes (channel offsets 0 and 2); Q23 frames occupy
//! eight bytes (offsets 0 and 4). The array has the alignment of one channel,
//! including when held inside a larger struct. Slices contain consecutive frames
//! without padding. On the FM-1, PCM16 requires only two-byte alignment and Q23
//! requires four. A planar Q23 input has one native `i32` word per sample.
//! The [Rust array layout](https://doc.rust-lang.org/reference/type-layout.html#array-layout)
//! is part of this API.

#[cfg(fm1_dsp)]
#[allow(
    unsafe_code,
    reason = "The native kernels borrow initialized PCM for one synchronous call."
)]
#[path = "mix/pi32v2.rs"]
mod backend;

#[cfg(not(fm1_dsp))]
#[path = "mix/portable.rs"]
mod backend;

/// Linear gain for one channel, stored as a signed integer with 14 fractional bits.
/// The range is −2 through 2 − 2⁻¹⁴. Negative gains invert polarity.
/// `Gain` has the same layout and ABI as its native-endian `i16` bits.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gain(i16);

impl Gain {
    /// Silence.
    pub const ZERO: Self = Self(0);
    /// Pass the input at its original level.
    pub const UNITY: Self = Self(1 << 14);

    /// Construct a gain from its Q2.14 bits. For example, 8192 is half level.
    #[must_use]
    pub const fn from_bits(bits: i16) -> Self {
        Self(bits)
    }

    /// The signed Q2.14 representation.
    #[must_use]
    pub const fn to_bits(self) -> i16 {
        self.0
    }
}

/// Mix mono PCM16 into both output channels with independent left and right gains.
/// In each lane the Q23 contribution is `(sample * gain.to_bits()) >> 6`,
/// using a signed product and rounding toward negative infinity.
pub fn mono_i16(out: &mut [[i32; 2]], input: &[i16], gains: [Gain; 2]) -> usize {
    backend::mono_i16(out, input, gains)
}

/// Mix interleaved stereo PCM16. Each input channel uses its corresponding gain
/// and the same rounding as [`mono_i16`].
pub fn stereo_i16(out: &mut [[i32; 2]], input: &[[i16; 2]], gains: [Gain; 2]) -> usize {
    backend::stereo_i16(out, input, gains)
}

/// Add an interleaved stereo Q23 bus at unity gain. Both buses may use the full
/// signed `i32` headroom.
pub fn stereo_q23(out: &mut [[i32; 2]], input: &[[i32; 2]]) -> usize {
    backend::stereo_q23(out, input)
}

/// Add separate left and right Q23 planes at unity gain, up to the shortest
/// of the three slices. The input planes may be the same slice to center a mono bus.
pub fn planar_q23(out: &mut [[i32; 2]], left: &[i32], right: &[i32]) -> usize {
    backend::planar_q23(out, left, right)
}

#[cfg(test)]
mod tests;
