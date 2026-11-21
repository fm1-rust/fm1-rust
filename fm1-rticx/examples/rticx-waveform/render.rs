//! One block of oscillator lookahead. The refill owns the authoritative phase.
use crate::voice::Voice;
use fm1_audio::OutputBlock;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RenderRequest {
    number: u32,
    phase: u32,
    pub(super) voice: Voice,
}

#[derive(Debug)]
pub struct PreparedBlock<'a> {
    request: RenderRequest,
    end_phase: u32,
    pub(super) samples: &'a mut OutputBlock,
}

impl RenderRequest {
    pub(super) fn new(voice: Voice) -> Self {
        Self {
            number: 0,
            phase: 0,
            voice,
        }
    }

    pub(super) fn prepare(self, samples: &mut OutputBlock) -> PreparedBlock<'_> {
        PreparedBlock {
            request: self,
            end_phase: self.voice.render(self.phase, samples),
            samples,
        }
    }

    /// Fill one released half and advance the oscillator exactly once, including on a late refill.
    pub(super) fn render_and_advance(
        &mut self,
        output: &mut OutputBlock,
        ready: Option<&PreparedBlock<'_>>,
    ) -> bool {
        let matching = ready.filter(|block| block.request == *self);
        self.phase = if let Some(block) = matching {
            output.copy_from_slice(block.samples);
            block.end_phase
        } else {
            self.voice.render(self.phase, output)
        };
        self.number = self.number.wrapping_add(1);
        matching.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fm1_audio::OUTPUT_FRAMES;

    #[test]
    fn only_the_current_block_phase_and_controls_can_supply_pcm() {
        let mut voice = Voice::default();
        voice.velocity = 97;
        let initial = RenderRequest {
            number: u32::MAX,
            phase: u32::MAX - 31,
            ..RenderRequest::new(voice)
        };
        let mut stopped = voice;
        stopped.playing = false;
        let mut scratch = [[0; 2]; OUTPUT_FRAMES];
        for prediction in [
            Some(initial),
            Some(RenderRequest {
                number: 0,
                ..initial
            }),
            Some(RenderRequest {
                phase: 0,
                ..initial
            }),
            Some(RenderRequest {
                voice: stopped,
                ..initial
            }),
            None,
        ] {
            let prepared = prediction.map(|request| request.prepare(&mut scratch));
            let mut next = initial;
            let mut actual = [[0; 2]; OUTPUT_FRAMES];
            let mut expected = [[0; 2]; OUTPUT_FRAMES];
            let end_phase = voice.render(initial.phase, &mut expected);
            assert_eq!(
                next.render_and_advance(&mut actual, prepared.as_ref()),
                prediction == Some(initial)
            );
            assert_eq!(actual, expected);
            assert_eq!((next.number, next.phase), (0, end_phase));
        }
    }
}
