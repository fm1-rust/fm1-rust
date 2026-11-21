//! Two prepared WaveGraph programs sharing one phase-continuous oscillator.
use super::{render::RenderRequest, voice::Voice};
use fm1_audio::{FRAME_RATE, I24, OUTPUT_FRAMES, Refill};
use fm1_rticx::rticx::asynchronous::channel::{Receiver, Sender};
use wavegraph::{Event, Process, bytecode::Program, dsp::Software};
use wavegraph_monosynth::{Message, Monosynth, Tuning};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Selection {
    pub(super) half_gain: bool,
    pub(super) oscillator_tap: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct Applied {
    pub(super) selection: Selection,
    /// Render timeline, including blocks which later missed DMA submission.
    pub(super) first_frame: u64,
    pub(super) submitted_at: u32,
}

/// A single controller task submits one edit and awaits its first successful refill.
/// The completion channel is independent of the lossy scope queue.
pub(super) struct Controller<'a> {
    requests: Sender<'a, Selection, 1>,
    applied: Receiver<'a, Applied, 1>,
    awaiting_receipt: bool,
}

impl<'a> Controller<'a> {
    pub(super) fn new(
        requests: Sender<'a, Selection, 1>,
        applied: Receiver<'a, Applied, 1>,
    ) -> Self {
        Self {
            requests,
            applied,
            awaiting_receipt: false,
        }
    }

    pub(super) async fn commit(&mut self, selection: Selection) -> Applied {
        // Dropping a waiter leaves an accepted edit in flight. Drain its receipt before reusing the slot.
        if self.awaiting_receipt {
            self.applied.recv().await.expect("audio owns sender");
        }
        self.awaiting_receipt = false;
        self.requests
            .send(selection)
            .await
            .expect("audio owns receiver");
        self.awaiting_receipt = true;
        let applied = self.applied.recv().await.expect("audio owns sender");
        self.awaiting_receipt = false;
        applied
    }
}

pub(super) struct AudioEdits<'a> {
    requests: Receiver<'a, Selection, 1>,
    receipts: Sender<'a, Applied, 1>,
    pending: Option<Selection>,
    active: Applied,
    next_frame: u64,
}

impl<'a> AudioEdits<'a> {
    pub(super) fn new(
        requests: Receiver<'a, Selection, 1>,
        receipts: Sender<'a, Applied, 1>,
    ) -> Self {
        Self {
            requests,
            receipts,
            pending: None,
            active: Applied::default(),
            next_frame: 0,
        }
    }

    pub(super) fn selection(&mut self) -> Selection {
        if self.pending.is_none() {
            self.pending = self.requests.try_recv().ok();
        }
        self.pending.unwrap_or(self.active.selection)
    }

    pub(super) fn render_request(&self, voice: Voice, selection: Selection) -> RenderRequest {
        RenderRequest {
            first_frame: self.next_frame,
            voice,
            half_gain: selection.half_gain,
        }
    }

    pub(super) fn finish(&mut self, result: Refill, submitted_at: u32) -> Applied {
        if result == Refill::Written
            && let Some(selection) = self.pending.take()
        {
            self.active = Applied {
                selection,
                first_frame: self.next_frame,
                submitted_at,
            };
            self.receipts
                .try_send(self.active)
                .expect("one outstanding commit");
        }
        if result != Refill::WouldBlock {
            self.next_frame = self.next_frame.wrapping_add(OUTPUT_FRAMES as u64);
        }
        self.active
    }
}

type Plan = Program<'static, 2, 1>;
static TUNING: Tuning = Tuning::new(FRAME_RATE.0, FRAME_RATE.1).expect("FM-1 sample clock");

pub(super) type Buses = [[I24; OUTPUT_FRAMES]; 2];

#[derive(Clone, Debug)]
pub(super) struct Oscillator {
    synth: Monosynth<'static>,
    controls: Option<Voice>,
}

impl Oscillator {
    pub(super) fn new() -> Self {
        Self {
            synth: Monosynth::new(&TUNING),
            controls: None,
        }
    }
}

pub(super) struct Graph {
    plans: [Plan; 2],
}

impl Graph {
    pub(super) fn new() -> Self {
        // Call object 0 into bus 0. The second program appends gain(0, 64/128) on bus 1.
        let dry = Plan::decode(&[1, 1, 0, 0, 1]).expect("dry program");
        let half = Plan::decode(&[1, 1, 0, 0, 1, 2, 0, 64]).expect("half-gain program");
        Self { plans: [dry, half] }
    }

    pub(super) fn render(
        &self,
        state: &mut Oscillator,
        voice: Voice,
        half_gain: bool,
        buses: &mut Buses,
    ) {
        let mut nodes: [&mut dyn Process<Message, I24>; 1] = [&mut state.synth];
        let mut patch = self.plans[usize::from(half_gain)]
            .bind(&mut nodes)
            .expect("monosynth layout");
        let events = voice.messages().map(|message| Event {
            frame: 0,
            port: 0,
            message,
        });
        let count = if state.controls.replace(voice) == Some(voice) {
            0
        } else if voice.playing && voice.velocity != 0 {
            2
        } else {
            3
        };
        let input = [&events[..count]];
        patch
            .render(
                &mut Software,
                buses,
                if count == 0 { &[] } else { &input },
                |_, e| Err(e),
            )
            .expect("fixed program and buses");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    use fm1_rticx::rticx::asynchronous::channel::Channel;

    #[test]
    fn commit_waits_for_a_written_block_and_reports_its_first_frame() {
        let mut commands = Channel::new();
        let mut replies = Channel::new();
        let (requests, edits) = commands.split();
        let (receipts, applied) = replies.split();
        let mut control = Controller::new(requests, applied);
        let mut audio = AudioEdits::new(edits, receipts);
        let mut cx = Context::from_waker(Waker::noop());
        let selection = Selection {
            half_gain: true,
            oscillator_tap: true,
        };
        assert_eq!(audio.selection(), Selection::default());
        audio.finish(Refill::Written, 5);
        let mut commit = pin!(control.commit(selection));
        assert_eq!(commit.as_mut().poll(&mut cx), Poll::Pending);
        for (result, at) in [(Refill::WouldBlock, 7), (Refill::Late, 10)] {
            assert_eq!(audio.selection(), selection);
            assert_eq!(audio.finish(result, at), Applied::default());
            assert_eq!(commit.as_mut().poll(&mut cx), Poll::Pending);
        }
        assert_eq!(audio.selection(), selection);
        let request = audio.render_request(Voice::default(), selection);
        assert_eq!(request.first_frame, 2 * OUTPUT_FRAMES as u64);
        assert!(request.half_gain);
        assert_eq!(
            request,
            audio.render_request(
                Voice::default(),
                Selection {
                    oscillator_tap: false,
                    ..selection
                }
            )
        );
        let receipt = audio.finish(Refill::Written, 15);
        assert_eq!(
            receipt,
            Applied {
                selection,
                first_frame: 2 * OUTPUT_FRAMES as u64,
                submitted_at: 15
            }
        );
        assert_eq!(commit.as_mut().poll(&mut cx), Poll::Ready(receipt));
        assert_eq!(audio.finish(Refill::Written, 20), receipt);
    }

    #[test]
    fn dropping_a_waiter_keeps_the_next_receipt_paired_with_its_edit() {
        let mut commands = Channel::new();
        let mut replies = Channel::new();
        let (requests, edits) = commands.split();
        let (receipts, applied) = replies.split();
        let mut control = Controller::new(requests, applied);
        let mut audio = AudioEdits::new(edits, receipts);
        let mut cx = Context::from_waker(Waker::noop());
        let first = Selection {
            half_gain: true,
            ..Selection::default()
        };
        {
            let mut abandoned = pin!(control.commit(first));
            assert!(abandoned.as_mut().poll(&mut cx).is_pending());
        }
        let next = Selection {
            oscillator_tap: true,
            ..first
        };
        let mut commit = pin!(control.commit(next));
        assert!(commit.as_mut().poll(&mut cx).is_pending());
        assert_eq!(audio.selection(), first);
        audio.finish(Refill::Written, 2);
        assert!(commit.as_mut().poll(&mut cx).is_pending());
        assert_eq!(audio.selection(), next);
        let applied = audio.finish(Refill::Written, 3);
        assert_eq!(commit.as_mut().poll(&mut cx), Poll::Ready(applied));
        assert_eq!(applied.first_frame, OUTPUT_FRAMES as u64);
    }

    #[test]
    fn route_edits_preserve_phase_and_the_raw_oscillator_bus() {
        let graph = Graph::new();
        let mut reference = Oscillator::new();
        let mut live = Oscillator::new();
        let mut voice = Voice::default();
        voice.velocity = 101;
        let mut dry = [[I24::default(); OUTPUT_FRAMES]; 2];
        let mut buses = dry;
        for half_gain in [false, true, true, false] {
            graph.render(&mut reference, voice, false, &mut dry);
            graph.render(&mut live, voice, half_gain, &mut buses);
            assert_eq!(buses[0], dry[0]);
            assert!(dry[0].iter().any(|s| s.inner() > 0));
            let expected = dry[0].map(|s| s.inner() >> u32::from(half_gain));
            assert_eq!(buses[usize::from(half_gain)].map(I24::inner), expected);
        }
    }
}
