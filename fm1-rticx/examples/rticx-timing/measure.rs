//! Small, allocation-free, TIMER4-based measurements and paged ASCII reports.
use core::fmt::{self, Write};
use fm1_audio::Refill;

pub const CRYSTAL_HZ: u32 = 24_000_000;
pub const AUDIO_TICKS: u32 = fm1_audio::BLOCK_TICKS;
pub const PANEL_TICKS: u32 = CRYSTAL_HZ / 10_000;
pub const PHASE_TICKS: u32 = 5 * CRYSTAL_HZ;
pub const PAGE_COUNT: u8 = 25;
const LINE_CAPACITY: usize = 43; // UsbLog::MAX_LINE_LEN (one USB-MIDI transfer)

#[derive(Clone, Copy, Debug, Default)]
pub struct AudioMetrics {
    pub blocks: u32,
    pub late: u32,
    pub overruns: u32,
    pub twice_late: u32,
    pub max_work: u32,
    pub max_copy: u32,
    pub max_gap: u32,
    pub max_jitter: u32,
    pub work_sum: u32,
    last: Option<u32>,
}

impl AudioMetrics {
    pub fn record(&mut self, start: u32, copied: u32, end: u32, result: Refill) {
        if result == Refill::WouldBlock {
            return;
        }
        self.blocks = self.blocks.saturating_add(1);
        let work = end.wrapping_sub(start);
        self.max_work = self.max_work.max(work);
        self.work_sum = self.work_sum.saturating_add(work);
        if result == Refill::Written {
            self.max_copy = self.max_copy.max(copied.wrapping_sub(start));
        } else {
            self.late = self.late.saturating_add(1);
        }
        let mut missed = result == Refill::Late || work >= AUDIO_TICKS;
        if let Some(previous) = self.last.replace(start) {
            let gap = start.wrapping_sub(previous);
            self.max_gap = self.max_gap.max(gap);
            self.max_jitter = self.max_jitter.max(gap.abs_diff(AUDIO_TICKS));
            if gap >= 2 * AUDIO_TICKS {
                self.twice_late = self.twice_late.saturating_add(1);
                missed = true;
            }
        }
        if missed {
            self.overruns = self.overruns.saturating_add(1);
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PanelMetrics {
    pub interrupts: u32,
    pub dropped: u32,
    pub long_gaps: u32,
    pub max_work: u32,
    pub max_gap: u32,
    pub max_jitter: u32,
    pub work_sum: u32,
    last: Option<u32>,
}

impl PanelMetrics {
    pub fn record(&mut self, start: u32, end: u32) {
        self.interrupts = self.interrupts.saturating_add(1);
        let work = end.wrapping_sub(start);
        self.max_work = self.max_work.max(work);
        self.work_sum = self.work_sum.saturating_add(work);
        if let Some(previous) = self.last.replace(start) {
            let gap = start.wrapping_sub(previous);
            self.max_gap = self.max_gap.max(gap);
            self.max_jitter = self.max_jitter.max(gap.abs_diff(PANEL_TICKS));
            if gap >= 2 * PANEL_TICKS {
                self.long_gaps = self.long_gaps.saturating_add(1);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct InputMetrics {
    pub sweeps: u32,
    pub max_dispatch: u32,
}

impl InputMetrics {
    pub fn record(&mut self, captured_at: u32, received_at: u32) {
        let elapsed = received_at.wrapping_sub(captured_at);
        self.sweeps = self.sweeps.saturating_add(1);
        self.max_dispatch = self.max_dispatch.max(elapsed);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WorkMetrics {
    pub count: u32,
    pub max_ticks: u32,
    pub sum_ticks: u32,
}

impl WorkMetrics {
    pub fn record(&mut self, elapsed: u32) {
        self.count = self.count.saturating_add(1);
        self.max_ticks = self.max_ticks.max(elapsed);
        self.sum_ticks = self.sum_ticks.saturating_add(elapsed);
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WakeMetrics {
    pub wakes: u32,
    pub max_late: u32,
    pub sum_late: u32,
    pub missed: u32,
}

impl WakeMetrics {
    pub fn record(&mut self, expected: u32, actual: u32) {
        let delay = actual.wrapping_sub(expected);
        // An early wake is not a late wake; avoid treating wrapping subtraction
        // as a multi-minute delay near TIMER4 rollover.
        let delay = if delay > i32::MAX as u32 { 0 } else { delay };
        self.wakes = self.wakes.saturating_add(1);
        self.max_late = self.max_late.max(delay);
        self.sum_late = self.sum_late.saturating_add(delay);
        if delay >= 5 * CRYSTAL_HZ / 1_000 {
            self.missed = self.missed.saturating_add(1);
        }
    }
}

pub const fn micros(ticks: u32) -> u32 {
    ticks.div_ceil(24)
}

pub const fn mean_us(sum: u32, count: u32) -> u32 {
    match sum.checked_div(count) {
        Some(mean) => micros(mean),
        None => 0,
    }
}

pub struct Line {
    bytes: [u8; LINE_CAPACITY],
    len: usize,
}

impl Line {
    pub(super) fn new() -> Self {
        Self {
            bytes: [0; LINE_CAPACITY],
            len: 0,
        }
    }

    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).expect("ASCII output")
    }
}

impl Write for Line {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        if !text.is_ascii() {
            return Err(fmt::Error);
        }
        let end = self.len.checked_add(text.len()).ok_or(fmt::Error)?;
        let dst = self.bytes.get_mut(self.len..end).ok_or(fmt::Error)?;
        dst.copy_from_slice(text.as_bytes());
        self.len = end;
        Ok(())
    }
}

pub struct Report {
    pub audio: [AudioMetrics; 3],
    pub panel: [PanelMetrics; 3],
    pub input: [InputMetrics; 3],
    pub wake: [WakeMetrics; 3],
    pub lcd: WorkMetrics,
    pub cpu: WorkMetrics,
    pub usb: WorkMetrics,
}

impl Report {
    /// Each page fits one `UsbLog` snapshot; the host selects a page using MIDI CC 112.
    pub fn page(&self, number: u8, done: bool) -> Line {
        let mut line = Line::new();
        let formatted = match number {
            0 => write!(line, "24MHZ AUDIO 128F PANEL 10KHZ"),
            1..=21 => {
                let offset = number.saturating_sub(1);
                let index = usize::from(offset / 7);
                let name = ["IDLE", "DRAW", "BUSY"][index];
                let a = self.audio[index];
                let p = self.panel[index];
                let i = self.input[index];
                let w = self.wake[index];
                match offset % 7 {
                    0 => write!(
                        line,
                        "{name} A N={} XR={} LATE={}",
                        a.blocks, a.overruns, a.late
                    ),
                    1 => write!(
                        line,
                        "{name} A AVG={} MAX={} COPY={}",
                        mean_us(a.work_sum, a.blocks),
                        micros(a.max_work),
                        micros(a.max_copy)
                    ),
                    2 => write!(
                        line,
                        "{name} A GAP={} JIT={} G2={}",
                        micros(a.max_gap),
                        micros(a.max_jitter),
                        a.twice_late
                    ),
                    3 => write!(
                        line,
                        "{name} P N={} AVG={} MAX={}",
                        p.interrupts,
                        mean_us(p.work_sum, p.interrupts),
                        micros(p.max_work)
                    ),
                    4 => write!(
                        line,
                        "{name} P GAP={} JIT={} G2={}",
                        micros(p.max_gap),
                        micros(p.max_jitter),
                        p.long_gaps
                    ),
                    5 => write!(
                        line,
                        "{name} INPUT N={} MAX={} DROP={}",
                        i.sweeps,
                        micros(i.max_dispatch),
                        p.dropped
                    ),
                    _ => write!(
                        line,
                        "{name} W AVG={} MAX={} M={}",
                        mean_us(w.sum_late, w.wakes),
                        micros(w.max_late),
                        w.missed
                    ),
                }
            }
            22 => write!(
                line,
                "LCD N={} AVG={} MAX={} US",
                self.lcd.count,
                mean_us(self.lcd.sum_ticks, self.lcd.count),
                micros(self.lcd.max_ticks)
            ),
            23 => write!(
                line,
                "CPU N={} AVG={} MAX={} US",
                self.cpu.count,
                mean_us(self.cpu.sum_ticks, self.cpu.count),
                micros(self.cpu.max_ticks)
            ),
            24 => write!(
                line,
                "USB N={} AVG={} MAX={} US",
                self.usb.count,
                mean_us(self.usb.sum_ticks, self.usb.count),
                micros(self.usb.max_ticks)
            ),
            _ => write!(line, "BAD PAGE"),
        };
        if formatted.is_err() {
            let mut fallback = Line::new();
            write!(fallback, "PAGE {number} TOO LONG").expect("short ASCII fallback");
            return fallback;
        }
        if !done && number > 0 {
            let mut pending = Line::new();
            write!(pending, "RUNNING - READ AFTER DONE").expect("short ASCII warning");
            return pending;
        }
        line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaps_and_deadlines_survive_timer_wrap() {
        let mut a = AudioMetrics::default();
        let start = u32::MAX - 100;
        a.record(
            start,
            start.wrapping_add(24),
            start.wrapping_add(48),
            Refill::Written,
        );
        let next = start.wrapping_add(AUDIO_TICKS + 24);
        a.record(next, next + 30, next + 60, Refill::Written);
        assert_eq!((a.blocks, a.max_jitter, a.overruns), (2, 24, 0));
        a.record(
            next + 2 * AUDIO_TICKS,
            next,
            next + 2 * AUDIO_TICKS + 99,
            Refill::Late,
        );
        assert_eq!((a.overruns, a.late, a.twice_late), (1, 1, 1));

        let mut p = PanelMetrics::default();
        p.record(u32::MAX - 8, 17);
        p.record(PANEL_TICKS - 9, PANEL_TICKS + 11);
        assert_eq!(p.max_jitter, 0);
        assert_eq!(p.max_work, 26);
    }

    #[test]
    fn delayed_async_wake_counts_missed_periods_across_wrap() {
        let mut w = WakeMetrics::default();
        w.record(u32::MAX - 20, 5 * CRYSTAL_HZ / 1_000);
        assert_eq!(w.wakes, 1);
        assert_eq!(w.missed, 1);
        w.record(1_000, 1_024);
        assert_eq!(w.max_late, 5 * CRYSTAL_HZ / 1_000 + 21);
    }

    #[test]
    fn records_input_latency_and_work() {
        let mut input = InputMetrics::default();
        input.record(u32::MAX - 7, 16);
        input.record(100, 112);
        assert_eq!((input.sweeps, input.max_dispatch), (2, 24));

        let mut work = WorkMetrics::default();
        assert_eq!(mean_us(work.sum_ticks, work.count), 0);
        work.record(24);
        work.record(72);
        assert_eq!((work.count, work.max_ticks, work.sum_ticks), (2, 72, 96));
        assert_eq!(mean_us(work.sum_ticks, work.count), 2);
    }

    #[test]
    fn every_page_is_short_ascii_and_reports_are_non_destructive() {
        let mut a = [AudioMetrics::default(); 3];
        let mut p = [PanelMetrics::default(); 3];
        let mut i = [InputMetrics::default(); 3];
        let mut w = [WakeMetrics::default(); 3];
        for n in 0..3 {
            a[n].blocks = 50_000;
            a[n].overruns = 50_000;
            a[n].late = 50_000;
            a[n].max_work = PHASE_TICKS;
            a[n].max_gap = PHASE_TICKS;
            a[n].max_jitter = PHASE_TICKS;
            a[n].max_copy = PHASE_TICKS;
            a[n].twice_late = 50_000;
            p[n].interrupts = 50_000;
            p[n].long_gaps = 50_000;
            p[n].dropped = 50_000;
            p[n].max_work = PHASE_TICKS;
            p[n].max_gap = PHASE_TICKS;
            p[n].max_jitter = PHASE_TICKS;
            i[n].sweeps = 50_000;
            i[n].max_dispatch = PHASE_TICKS;
            w[n].wakes = 50_000;
            w[n].missed = 50_000;
            w[n].max_late = PHASE_TICKS;
        }
        let work = WorkMetrics {
            count: 50_000,
            max_ticks: PHASE_TICKS,
            sum_ticks: PHASE_TICKS,
        };
        let report = Report {
            audio: a,
            panel: p,
            input: i,
            wake: w,
            lcd: work,
            cpu: work,
            usb: work,
        };
        for n in 0..PAGE_COUNT {
            let line = report.page(n, true);
            assert!(line.len <= LINE_CAPACITY);
            assert!(line.as_str().is_ascii());
            assert!(
                !line.as_str().contains("TOO LONG"),
                "page {n}: {}",
                line.as_str()
            );
        }
        assert_eq!(report.page(1, false).as_str(), "RUNNING - READ AFTER DONE");
    }
}
