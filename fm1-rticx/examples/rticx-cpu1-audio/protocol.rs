//! CPU0's one-slot protocol. Only a completed job can return the slot.
use fm1_audio::{OUTPUT_FRAMES, OutputBlock};

pub(super) const JOBS: u32 = 512;
pub(super) const DELAYED: u32 = 64;
const _: () = assert!(OUTPUT_FRAMES == 128); // native ABI in cpu1_audio.c

// SAFETY: this binary supplies the sole C ABI sample function called by the
// fixed worker. CRT copies this section into SRAM. Its two integer arguments
// and wrapping and bitwise operations need no globals or CPU0 services.
#[cfg_attr(target_os = "none", unsafe(no_mangle))]
#[cfg_attr(target_os = "none", unsafe(link_section = ".cpu1.text.rust"))]
#[inline(never)]
pub(super) extern "C" fn fm1_cpu1_pcm_sample(job: u32, frame: u32) -> u32 {
    let square = if frame & 64 == 0 { 32_768 } else { 0xffff_8000 };
    square | ((job.wrapping_mul(73) ^ frame.wrapping_mul(17)) & 0x3ff)
}

pub(super) fn valid(job: u32, block: &OutputBlock, claimed: u32) -> bool {
    let mut digest = job;
    for (i, &[left, right]) in (0_u32..).zip(block) {
        let word = u32::from_ne_bytes(left.to_ne_bytes());
        if left != right || word != fm1_cpu1_pcm_sample(job, i) {
            return false;
        }
        digest = digest.rotate_left(5).wrapping_add(word);
    }
    digest == claimed
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Action {
    Play,
    Silence,
    Discard,
}

#[derive(Default)]
pub(super) struct Pipeline {
    pub(super) accepted: u32,
    pub(super) silenced: bool,
    pub(super) discarded: bool,
}

impl Pipeline {
    pub(super) fn job(&self) -> u32 {
        self.accepted
            .saturating_add(if self.discarded { 2 } else { 1 })
    }

    pub(super) fn try_take(&self, done: u32, held: u32) -> Result<Action, &'static str> {
        let job = self.job();
        if done == job {
            return if job != DELAYED {
                Ok(Action::Play)
            } else if self.silenced {
                Ok(Action::Discard)
            } else {
                Err("GATE")
            };
        }
        if done != job.wrapping_sub(1) {
            return Err("SEQ");
        }
        if job == DELAYED && held == job && !self.silenced {
            Ok(Action::Silence)
        } else {
            Err("MISS")
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Report {
    pub(super) status: &'static str,
    pub(super) accepted: u32,
    pub(super) silenced: bool,
    pub(super) discarded: bool,
    pub(super) commit: u32,
    pub(super) gap: u32,
    pub(super) worker: u32,
}

impl Report {
    pub(super) fn at_boot(code: u32) -> Self {
        let status = match code {
            0 => "BT0",
            1 => "RUN",
            2 => "BT2",
            3 => "BT3",
            _ => "BT4",
        };
        Self {
            status,
            accepted: 0,
            silenced: false,
            discarded: false,
            commit: 0,
            gap: 0,
            worker: 0,
        }
    }

    pub(super) fn line(&self) -> heapless::String<43> {
        use core::fmt::Write;
        let mut line = heapless::String::new();
        write!(
            line,
            "C1 {} {:03X} {}{} {:08X} {:08X} {:08X}",
            self.status,
            self.accepted,
            u8::from(self.silenced),
            u8::from(self.discarded),
            self.commit,
            self.gap,
            self.worker
        )
        .expect("fixed-width probe report");
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delayed_job_needs_a_written_silence_then_a_fresh_job() {
        let mut p = Pipeline {
            accepted: DELAYED - 1,
            ..Pipeline::default()
        };
        assert_eq!(p.try_take(DELAYED, 0), Err("GATE"));
        assert_eq!(p.try_take(DELAYED - 1, 0), Err("MISS"));
        assert_eq!(p.try_take(DELAYED - 1, DELAYED), Ok(Action::Silence));
        p.silenced = true;
        assert_eq!(p.job(), DELAYED);
        assert_eq!(p.try_take(DELAYED, DELAYED), Ok(Action::Discard));
        p.discarded = true;
        assert_eq!(p.try_take(DELAYED - 1, DELAYED), Err("SEQ"));
        assert_eq!(p.try_take(DELAYED, DELAYED), Err("MISS"));
        assert_eq!(p.try_take(DELAYED + 1, DELAYED), Ok(Action::Play));
    }

    #[test]
    fn verify_every_sample_and_the_claimed_digest() {
        let mut block = [[0; 2]; OUTPUT_FRAMES];
        let job = 19;
        let mut digest = job;
        for (i, pair) in (0_u32..).zip(&mut block) {
            let sample = fm1_cpu1_pcm_sample(job, i);
            *pair = [i32::from_ne_bytes(sample.to_ne_bytes()); 2];
            digest = digest.rotate_left(5).wrapping_add(sample);
        }
        assert!(valid(job, &block, digest));
        assert!(!valid(job + 1, &block, digest));
        assert!(!valid(job, &block, digest ^ 1));
        block[127][1] ^= 1;
        assert!(!valid(job, &block, digest));
    }

    #[test]
    fn final_report_fits_one_usb_reply_even_with_maximum_timings() {
        let mut r = Report::at_boot(1);
        r.status = "PASS";
        r.accepted = JOBS - 1;
        r.silenced = true;
        r.discarded = true;
        r.commit = u32::MAX;
        r.gap = u32::MAX;
        r.worker = u32::MAX;
        assert_eq!(r.line(), "C1 PASS 1FF 11 FFFFFFFF FFFFFFFF FFFFFFFF");
        assert!(r.line().len() <= fm1_rticx::bsp::usb_midi::UsbLog::MAX_LINE_LEN);
    }
}
