//! Read FM-1 diagnostics from the host.

mod dma_probe;

use clap::{Parser, Subcommand};
use core::time::Duration;
use fm1_usb::{Device, dma_probe::read_log};
use std::{
    io::{self, Write},
    process::ExitCode,
};

#[derive(Parser)]
#[command(name = "fm1", version, about = "Read FM-1 diagnostics over USB")]
struct Args {
    /// Match a unique substring in both MIDI port names.
    #[arg(long, global = true)]
    port: Option<String>,

    /// Seconds to wait for each control or MIDI reply.
    #[arg(long, global = true, default_value = "2", value_parser = parse_timeout, value_name = "SECONDS")]
    timeout: Duration,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read the probe's trial history or the firmware's USB-MIDI log.
    Log,
    /// Read the 25-page rticx-timing report after the LCD shows DONE.
    Timing,
    /// Measure EP1 OUT DMA on a board running an rticx-dma image.
    DmaProbe {
        /// Board identification to record with the measurements.
        #[arg(long)]
        board_revision: String,
        /// New directory for report.json and all 2 KiB snapshots.
        #[arg(long)]
        out: Option<std::path::PathBuf>,
    },
}

fn parse_timeout(s: &str) -> Result<Duration, &'static str> {
    let seconds = s
        .parse::<f64>()
        .map_err(|_| "expected a positive number of seconds")?;
    Duration::try_from_secs_f64(seconds)
        .ok()
        .filter(|d| !d.is_zero())
        .ok_or("expected a positive, finite duration")
}

fn run(args: Args) -> io::Result<()> {
    let device = Device::find()?;
    let info = device.info();
    eprintln!(
        "Using {:04x}:{:04x} on bus {}, address {}",
        info.vendor_id(),
        info.product_id(),
        info.bus_id(),
        info.device_address()
    );
    let log_ports = || device.log_ports(args.port.as_deref(), args.timeout);
    let text = match args.command {
        Command::Log => {
            // Probe history is read only over EP0, even while EP1 is armed.
            let vendor = matches!(
                info.product_string(),
                Some("FM-1 DMA Iso" | "FM-1 DMA Bulk")
            ) || info.interfaces().any(|i| i.class() == 0xff);
            if vendor {
                match read_log(&device, args.timeout)? {
                    Some(history) => history,
                    None => log_ports()?.1,
                }
            } else if info.product_string() == Some("FM-1 Timing") {
                match read_log(&device, args.timeout) {
                    Ok(Some(history)) => history,
                    // Stock timing and MIDI-only installations can still use UsbLog.
                    _ => log_ports()?.1,
                }
            } else {
                let snapshot = log_ports()?.1;
                if snapshot.lines().next() == Some("RXD3 MIDI READY") {
                    read_log(&device, args.timeout)
                        .ok()
                        .flatten()
                        .unwrap_or(snapshot)
                } else {
                    snapshot
                }
            }
        }
        Command::Timing => log_ports()?
            .0
            .connect(|_, _| {})?
            .read_timing(args.timeout)?,
        Command::DmaProbe {
            board_revision,
            out,
        } => {
            return dma_probe::run(
                &device,
                &board_revision,
                out,
                args.port.as_deref(),
                args.timeout,
            );
        }
    };
    io::stdout().lock().write_all(text.as_bytes())
}

fn main() -> ExitCode {
    match run(Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("fm1: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_fractional_timeouts_and_filters_after_subcommand() {
        let args = Args::try_parse_from(["fm1", "log", "--timeout", "0.25", "--port", "Piano"])
            .expect("valid options");
        assert_eq!(args.timeout, Duration::from_millis(250));
        assert_eq!(args.port.as_deref(), Some("Piano"));
        for invalid in ["0", "-1", "nan", "inf", "1e100"] {
            assert!(Args::try_parse_from(["fm1", "timing", "--timeout", invalid]).is_err());
        }
    }
}
