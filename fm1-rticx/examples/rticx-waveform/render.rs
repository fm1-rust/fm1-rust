//! One block of lookahead. Only the refill can advance the live oscillator.
use crate::{
    graph::{Buses, Graph, Oscillator},
    voice::Voice,
};
use fm1_audio::{I24, OUTPUT_FRAMES, OutputBlock, mix_stereo};

/// At a given frame the starting phase is fixed by the preceding refills.
/// Both scope taps remain available, so switching taps doesn't invalidate PCM.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct RenderRequest {
    pub(super) first_frame: u64,
    pub(super) voice: Voice,
    pub(super) half_gain: bool,
}

#[derive(Debug)]
pub(super) struct Renderer<'a> {
    state: Oscillator,
    buses: &'a mut Buses,
}

impl<'a> Renderer<'a> {
    pub(super) fn new(buses: &'a mut Buses) -> Self {
        Self {
            state: Oscillator::new(),
            buses,
        }
    }

    pub(super) fn oscillator(&self) -> &[I24; OUTPUT_FRAMES] {
        &self.buses[0]
    }

    pub(super) fn job(&self, request: RenderRequest, mut spare: Self) -> RenderJob<'a> {
        spare.state.clone_from(&self.state);
        RenderJob {
            request,
            renderer: spare,
        }
    }

    /// Adopt a matching prediction, including its end state, or render this block inline.
    pub(super) fn refill(
        &mut self,
        graph: &Graph,
        request: RenderRequest,
        mut ready: Option<PreparedBlock<'a>>,
        output: &mut OutputBlock,
    ) -> (bool, Option<Self>) {
        let hit = if let Some(block) = ready.as_mut().filter(|block| block.request == request) {
            core::mem::swap(self, &mut block.renderer);
            true
        } else {
            graph.render(
                &mut self.state,
                request.voice,
                request.half_gain,
                self.buses,
            );
            false
        };
        let bus = &self.buses[usize::from(request.half_gain)];
        mix_stereo(output, bus, bus);
        (hit, ready.map(|block| block.renderer))
    }
}

#[derive(Debug)]
pub(super) struct RenderJob<'a> {
    request: RenderRequest,
    pub(super) renderer: Renderer<'a>,
}

impl<'a> RenderJob<'a> {
    pub(super) fn prepare(mut self, graph: &Graph) -> PreparedBlock<'a> {
        graph.render(
            &mut self.renderer.state,
            self.request.voice,
            self.request.half_gain,
            self.renderer.buses,
        );
        PreparedBlock {
            request: self.request,
            renderer: self.renderer,
        }
    }
}

#[derive(Debug)]
pub struct PreparedBlock<'a> {
    request: RenderRequest,
    renderer: Renderer<'a>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_predictions_and_edits_preserve_pcm_taps_and_the_next_phase() {
        let graph = Graph::new();
        let mut live_buses = [[I24::default(); OUTPUT_FRAMES]; 2];
        let mut spare_buses = live_buses;
        let mut reference_buses = live_buses;
        let mut live = Renderer::new(&mut live_buses);
        let mut spare = Some(Renderer::new(&mut spare_buses));
        let mut reference = Renderer::new(&mut reference_buses);
        let mut voice = Voice::default();
        voice.velocity = 97;
        let mut request = RenderRequest {
            first_frame: 0,
            voice,
            half_gain: false,
        };
        let mut ready = None;
        for block in 0..9 {
            // The producer misses one refill, FX changes, then pitch and STOP
            // change after preparation. The following blocks must still match.
            if block == 3 {
                request.half_gain = true;
            }
            if block == 5 {
                request.voice.transpose_by(7);
            }
            if block == 7 {
                request.voice.playing = false;
            }
            if block == 8 {
                request.voice.playing = true;
            }
            let mut actual = [[0; 2]; OUTPUT_FRAMES];
            let mut expected = actual;
            let prediction = if block == 1 { None } else { ready.take() };
            let (hit, returned) = live.refill(&graph, request, prediction, &mut actual);
            assert_eq!(hit, matches!(block, 4 | 6));
            reference.refill(&graph, request, None, &mut expected);
            assert_eq!(actual, expected);
            assert_eq!(live.oscillator(), reference.oscillator());
            if let Some(renderer) = returned {
                spare = Some(renderer);
            }
            request.first_frame = request.first_frame.wrapping_add(OUTPUT_FRAMES as u64);
            if let Some(renderer) = spare.take() {
                ready = Some(live.job(request, renderer).prepare(&graph));
            }
        }
    }
}
