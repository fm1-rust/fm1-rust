//! Read FM-1 diagnostics from the host.

use clap::{Parser, Subcommand};
use core::time::Duration;
use fm1_usb::Device;
use std::{
    io::{self, Write},
    process::ExitCode,
};

#[derive(Parser)]
#[command(name = "fm1", version, about = "Read FM-1 diagnostics over USB-MIDI")]
struct Args {
    /// Match a unique substring in both MIDI port names.
    #[arg(long, global = true)]
    port: Option<String>,

    /// Seconds to wait for each MIDI reply.
    #[arg(long, global = true, default_value = "2", value_parser = parse_timeout, value_name = "SECONDS")]
    timeout: Duration,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print one USB log snapshot, preserving its line endings.
    Log,
    /// Read the 25-page rticx-timing report after the LCD shows DONE.
    Timing,
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
    let (ports, snapshot) = device.log_ports(args.port.as_deref(), args.timeout)?;
    let text = match args.command {
        Command::Log => snapshot,
        Command::Timing => ports.connect(|_, _| {})?.read_timing(args.timeout)?,
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
