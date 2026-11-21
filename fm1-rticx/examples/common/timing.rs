//! TIMER4 observations for the two 10 kHz panel scanners paired with audio.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PanelReport {
    peak_ticks: u32,
    peak_gap_ticks: u32,
    home_held: bool,
}

#[derive(Debug, Default)]
pub struct PanelTiming {
    last_start: Option<u32>,
    report: PanelReport,
}

impl PanelTiming {
    /// Observe every column interrupt. The HOME state is available once per full sweep.
    /// Publish only changes so the common scan path takes no additional resource lock.
    pub fn record(&mut self, started: u32, ended: u32, home: Option<bool>) -> Option<PanelReport> {
        let previous = self.report;
        if let Some(last) = self.last_start.replace(started) {
            self.report.peak_gap_ticks = self.report.peak_gap_ticks.max(started.wrapping_sub(last));
        }
        self.report.peak_ticks = self.report.peak_ticks.max(ended.wrapping_sub(started));
        if let Some(held) = home {
            self.report.home_held = held;
        }
        (self.report != previous).then_some(self.report)
    }
}

#[cfg(target_os = "none")]
#[derive(Default)]
pub struct TimingFooter {
    visible: bool,
}

#[cfg(target_os = "none")]
impl TimingFooter {
    /// Draw the four timing rows while HOME is held. Otherwise the caller draws its usual footer.
    pub fn draw(
        &mut self,
        screen: &mut fm1_graphics::TextGrid,
        panel: PanelReport,
        audio: &fm1_audio::Stats,
    ) -> bool {
        use fm1_graphics::{TextGrid, palette};
        use fm1_rticx::bsp::{panel::Controls, time::Timers};

        if self.visible != panel.home_held {
            for row in 11..=14 {
                screen.text(0, row, [b' '; TextGrid::COLUMNS], palette::MUTED);
            }
            self.visible = panel.home_held;
        }
        if self.visible {
            let us = |ticks: u32| ticks.div_ceil(Timers::CRYSTAL_HZ / 1_000_000);
            screen.text(0, 11, b"PEAK US  WORK    GAP", palette::MUTED);
            for (row, label, work, gap, budget) in [
                (
                    12,
                    &b"AUDIO"[..],
                    audio.peak_ticks,
                    audio.peak_gap_ticks,
                    fm1_audio::BLOCK_TICKS,
                ),
                (
                    13,
                    &b"PANEL"[..],
                    panel.peak_ticks,
                    panel.peak_gap_ticks,
                    Timers::CRYSTAL_HZ / Controls::SCAN_HZ,
                ),
            ] {
                screen.text(0, row, label, palette::MUTED);
                let color = if work >= budget {
                    palette::RED
                } else {
                    palette::WHITE
                };
                screen.number(7, row, 6, us(work), color);
                screen.number(14, row, 6, us(gap), palette::WHITE);
            }
            screen.text(0, 14, b"COMMIT", palette::MUTED);
            screen.number(6, 14, 5, us(audio.peak_commit_ticks), palette::WHITE);
            screen.text(12, 14, b"XR", palette::MUTED);
            screen.number(14, 14, 6, audio.overruns, palette::WHITE);
        }
        self.visible
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_scans_contribute_to_peaks_across_wrap_and_preserve_button_state() {
        let mut timing = PanelTiming::default();
        let start = u32::MAX - 3;
        let first = timing.record(start, 2, Some(true)).expect("first sample");
        assert_eq!((first.peak_ticks, first.peak_gap_ticks), (6, 0));
        let second = timing.record(16, 19, None).expect("first gap");
        assert_eq!((second.peak_ticks, second.peak_gap_ticks), (6, 20));
        assert!(second.home_held);
        assert_eq!(timing.record(32, 35, None), None);
        let last = timing.record(48, 51, Some(false)).expect("HOME released");
        assert!(!last.home_held);
        assert_eq!((last.peak_ticks, last.peak_gap_ticks), (6, 20));
    }
}
