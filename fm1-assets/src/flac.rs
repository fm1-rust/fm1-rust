//! Optional, allocation-free FLAC support for flash-resident streams.
//! Supply a maximum block size that matches the host encoder.

pub use nanoaudio_core::{
    Error, Result,
    codec::{Decoder, Progress},
};
pub use nanoaudio_flac::FlacDecoder;

/// Mono playback of streams up to 16 bits, with a narrow internal block buffer.
/// The default holds one 128-frame FM-1 audio block.
pub type Mono16<const BLOCK: usize = 128> = FlacDecoder<i16, BLOCK, 1, i16>;

/// Decode one batch and convert its samples into the caller's output type.
/// Both scratch and output are measured in interleaved samples. The shorter
/// slice limits this call. Advance input by `consumed`, use only `produced`
/// output samples, and retain the decoder between calls. For native PCM
/// output, call [`Decoder::decode`] directly and omit the scratch slice.
///
/// # Errors
///
/// Returns the decoder's error, including [`Error::BufferTooSmall`] when
/// there is insufficient space for one frame.
pub fn decode_as<D: Decoder, T>(
    decoder: &mut D,
    input: &[u8],
    scratch: &mut [D::Sample],
    output: &mut [T],
    mut convert: impl FnMut(D::Sample) -> T,
) -> Result<Progress> {
    let len = scratch.len().min(output.len());
    let progress = decoder.decode(input, &mut scratch[..len])?;
    if progress.consumed > input.len() {
        return Err(Error::InvalidData);
    }
    let samples = scratch[..len]
        .get(..progress.produced)
        .ok_or(Error::InvalidData)?;
    for (out, &sample) in output.iter_mut().zip(samples) {
        *out = convert(sample);
    }
    Ok(progress)
}
