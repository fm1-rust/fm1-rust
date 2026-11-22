//! Read the frozen report from the rticx-timing diagnostic.

use crate::MidiConnection;
use core::time::Duration;
use std::io::{self, ErrorKind};

impl MidiConnection {
    /// Read all 25 pages of the `rticx-timing` report, including line endings.
    /// Each page is selected with CC 112 on channel 1, then requested as a log
    /// snapshot. The timeout applies separately to each reply.
    ///
    /// # Errors
    /// Fails if the diagnostic is still running, the header does not match, or
    /// a MIDI transfer fails. A failed read can be restarted from page zero.
    pub fn read_timing(&mut self, timeout: Duration) -> io::Result<String> {
        read_pages(|page| {
            self.send(&[0xb0, 112, page])?;
            // Allow USB0 to process the CC before requesting its snapshot.
            std::thread::sleep(Duration::from_millis(30));
            self.read_log(timeout)
        })
    }
}

fn read_pages(mut read_page: impl FnMut(u8) -> io::Result<String>) -> io::Result<String> {
    let mut report = String::new();
    for number in 0..25 {
        let page = read_page(number)
            .map_err(|e| io::Error::new(e.kind(), format!("timing page {number}: {e}")))?;
        let line = page.strip_suffix('\n').unwrap_or(&page);
        if number == 0 && line != "24MHZ AUDIO 128F PANEL 10KHZ" {
            return Err(io::Error::new(
                ErrorKind::InvalidData,
                "unexpected page 0. Run the rticx-timing firmware",
            ));
        }
        if number == 1 && line.starts_with("RUNNING") {
            return Err(io::Error::other(
                "diagnostic is still running. Read again when the LCD says DONE",
            ));
        }
        report.push_str(line);
        report.push('\n');
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "24MHZ AUDIO 128F PANEL 10KHZ\n";

    #[test]
    fn reads_pages_in_order_and_preserves_line_boundaries() {
        let mut requested = Vec::new();
        let report = read_pages(|n| {
            requested.push(n);
            Ok(if n == 0 {
                HEADER.to_owned()
            } else {
                format!("page {n}\n")
            })
        })
        .expect("completed report");
        assert!(requested.into_iter().eq(0..25));
        assert_eq!(report.lines().count(), 25);
        assert!(report.starts_with(HEADER));
        assert!(report.ends_with("page 24\n"));
    }

    #[test]
    fn stops_on_wrong_firmware_unfinished_or_missing_page() {
        let wrong = read_pages(|n| {
            assert_eq!(n, 0);
            Ok("ready\n".to_owned())
        })
        .expect_err("ordinary USB log");
        assert_eq!(wrong.kind(), ErrorKind::InvalidData);

        for unfinished in [true, false] {
            let error = read_pages(|n| match n {
                0 => Ok(HEADER.to_owned()),
                1 if unfinished => Ok("RUNNING - READ AFTER DONE\n".to_owned()),
                1 => Err(io::Error::from(ErrorKind::TimedOut)),
                _ => unreachable!("stop at the first failed page"),
            })
            .expect_err("incomplete report");
            if unfinished {
                assert!(error.to_string().contains("DONE"));
            } else {
                assert_eq!(error.kind(), ErrorKind::TimedOut);
                assert!(error.to_string().contains("page 1"));
            }
        }
    }
}
