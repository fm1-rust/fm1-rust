//! Independent groups of 32 signed PCM16 samples. Each group uses the
//! smallest signed bit width that represents every sample in it.

const GROUP: u8 = 32;

/// A validated mono clip in the PCM1 format. Copies share the encoded bytes.
#[derive(Clone, Copy, Debug)]
pub struct PackedPcm16<'a> {
    data: &'a [u8],
    frames: u32,
}

impl<'a> PackedPcm16<'a> {
    /// Verify the header, every group's width and length, and the end of
    /// the clip. Returns `None` for malformed data. A zero-frame clip is valid.
    /// A generated asset can be validated in a static initializer.
    #[must_use]
    #[allow(
        clippy::arithmetic_side_effects,
        reason = "A group has at most 32 samples of at most 16 bits, and count never exceeds remaining."
    )]
    pub const fn parse(bytes: &'a [u8]) -> Option<Self> {
        let Some((header, data)) = bytes.split_first_chunk::<8>() else {
            return None;
        };
        let [b'P', b'C', b'M', b'1', a, b, c, d] = *header else {
            return None;
        };
        let frames = u32::from_le_bytes([a, b, c, d]);
        let mut remaining = frames;
        let mut tail = data;
        while remaining != 0 {
            let Some((&width, payload)) = tail.split_first() else {
                return None;
            };
            if width == 0 || width > 16 {
                return None;
            }
            let count = if remaining < GROUP as u32 {
                remaining
            } else {
                GROUP as u32
            };
            let len = (count as usize * width as usize).div_ceil(8);
            let Some((_, rest)) = payload.split_at_checked(len) else {
                return None;
            };
            tail = rest;
            remaining -= count;
        }
        if tail.is_empty() {
            Some(Self { data, frames })
        } else {
            None
        }
    }

    /// Number of mono samples.
    #[must_use]
    pub const fn frames(self) -> u32 {
        self.frames
    }

    /// Start an independent decoder at the first frame, in constant time.
    #[must_use]
    pub const fn samples(self) -> Samples<'a> {
        Samples {
            data: self.data,
            remaining: self.frames,
            bits: 0,
            available: 0,
            width: 0,
            group_left: 0,
        }
    }
}

/// Sequential PCM samples. Copying a cursor preserves its playback position.
#[derive(Clone, Copy, Debug)]
pub struct Samples<'a> {
    data: &'a [u8],
    remaining: u32,
    bits: u32,
    available: u8,
    width: u8,
    group_left: u8,
}

impl Samples<'_> {
    /// Number of samples left in this cursor.
    #[must_use]
    pub const fn remaining(&self) -> u32 {
        self.remaining
    }
}

impl Iterator for Samples<'_> {
    type Item = i16;

    #[inline]
    #[allow(
        clippy::arithmetic_side_effects,
        clippy::cast_possible_truncation,
        reason = "Widths are validated in 1..=16. At most 23 reservoir bits are live; the low 16 contain the sample."
    )]
    fn next(&mut self) -> Option<i16> {
        if self.remaining == 0 {
            return None;
        }
        if self.group_left == 0 {
            let (&width, data) = self.data.split_first()?;
            self.width = width;
            self.data = data;
            self.group_left = GROUP;
        }
        while self.available < self.width {
            let (&byte, data) = self.data.split_first()?;
            self.bits |= u32::from(byte) << self.available;
            self.available += 8;
            self.data = data;
        }
        let shift = 16 - self.width;
        let sample = ((self.bits as u16) << shift).cast_signed() >> shift;
        self.bits >>= self.width;
        self.available -= self.width;
        self.remaining -= 1;
        self.group_left -= 1;
        Some(sample)
    }
}

impl core::iter::FusedIterator for Samples<'_> {}

/// Encode mono PCM16 on the host. Returns `None` if the count exceeds `u32::MAX`.
/// The encoded bytes include the format identifier and frame count.
#[cfg(all(feature = "encode", not(target_os = "none")))]
#[must_use]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "Widths are 1..=16, the reservoir uses at most 23 bits, and each byte write emits its low eight."
)]
pub fn encode(samples: &[i16]) -> Option<alloc::vec::Vec<u8>> {
    let frames = u32::try_from(samples.len()).ok()?;
    let mut out = alloc::vec::Vec::from(*b"PCM1");
    out.extend_from_slice(&frames.to_le_bytes());
    for group in samples.chunks(usize::from(GROUP)) {
        let width = group
            .iter()
            .map(|&s| 17 - s.leading_ones().max(s.leading_zeros()))
            .max()? as u8;
        out.push(width);
        let mask = (1_u32 << width) - 1;
        let mut bits = 0_u32;
        let mut count = 0_u8;
        for sample in group {
            bits |= (u32::from(sample.cast_unsigned()) & mask) << count;
            count += width;
            while count >= 8 {
                out.push(bits as u8);
                bits >>= 8;
                count -= 8;
            }
        }
        if count > 0 {
            out.push(bits as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_bits_and_short_final_group() {
        // -1, -2, 0, 1 in two-bit two's complement, least-significant bits first.
        let bytes = b"PCM1\x04\0\0\0\x02\x4b";
        let clip = PackedPcm16::parse(bytes).expect("valid clip");
        assert_eq!(clip.frames(), 4);
        assert!(clip.samples().eq([-1, -2, 0, 1]));
        let mut a = clip.samples();
        assert_eq!(a.next(), Some(-1));
        assert!(a.eq([-2, 0, 1]));
        assert!(clip.samples().eq([-1, -2, 0, 1]));
    }

    #[test]
    fn malformed_streams_are_rejected() {
        let clip = b"PCM1\x01\0\0\0\x10\xff\x7f";
        for end in 0..clip.len() {
            assert!(PackedPcm16::parse(&clip[..end]).is_none());
        }
        for header in [
            b"PCM0\0\0\0\0".as_slice(),
            b"PCM1\0\0\0\0!",
            b"PCM1\x01\0\0\0\x00",
            b"PCM1\x01\0\0\0\x11",
            b"PCM1\xff\xff\xff\xff",
        ] {
            assert!(PackedPcm16::parse(header).is_none());
        }
        assert!(
            PackedPcm16::parse(b"PCM1\0\0\0\0")
                .expect("empty clip")
                .samples()
                .next()
                .is_none()
        );
    }

    #[cfg(all(feature = "encode", not(target_os = "none")))]
    #[test]
    fn every_width_and_group_boundary_round_trips() {
        let mut pcm = alloc::vec::Vec::new();
        for width in 1..=16 {
            let min = i16::MIN >> (16 - width);
            let max = !min;
            for pattern in [min, max, 0, -1].into_iter().cycle().take(32) {
                pcm.push(pattern);
            }
        }
        for len in [0, 1, 31, 32, 33, 511, 512] {
            let input = &pcm[..len];
            let bytes = encode(input).expect("short clip");
            assert!(
                PackedPcm16::parse(&bytes)
                    .expect("valid")
                    .samples()
                    .eq(input.iter().copied())
            );
        }
    }
}
