//! Latched audio result and live USB observations on the probe's LCD.
use super::protocol::Report;
use core::fmt::Write;
use fm1_graphics::{Rgb565, TextGrid, palette};
use fm1_rticx::bsp::{
    display::{Display, Error, SIZE},
    time::Timers,
    usb_midi::{ControlStall, EnumerationStage, RegisterTimeout, UsbStatus},
};
use heapless::String;

pub(super) struct Dashboard {
    grid: TextGrid,
    shown: Option<UsbStatus>,
}

impl Dashboard {
    pub(super) fn new() -> Self {
        Self {
            grid: TextGrid::new(),
            shown: None,
        }
    }

    pub(super) fn paint(
        &mut self,
        lcd: &mut Display,
        report: Report,
        usb: UsbStatus,
    ) -> Result<(), Error> {
        if self.shown.is_none() {
            let (raw, background) = if report.status == "PASS" {
                (0x0400, Rgb565::new(0, 32, 0))
            } else {
                (0x8000, Rgb565::new(16, 0, 0))
            };
            lcd.fill_rect(0, 0, SIZE, SIZE, raw)?;
            self.grid = TextGrid::with_background(background);
            self.grid.text(1, 1, "AUDIO", palette::WHITE);
            self.grid.text(8, 1, report.status, palette::WHITE);
            self.field(3, "BLOCKS", report.accepted);
            let ticks_per_us = Timers::CRYSTAL_HZ / 1_000_000;
            self.field(4, "COMMIT US", report.commit.div_ceil(ticks_per_us));
            self.field(5, "GAP US", report.gap.div_ceil(ticks_per_us));
            self.field(6, "WORKER US", report.worker.div_ceil(ticks_per_us));
        }
        if self.shown != Some(usb) {
            self.usb(usb);
            self.grid.paint(lcd)?;
            self.shown = Some(usb);
        }
        Ok(())
    }

    fn field(&mut self, row: usize, label: &str, value: u32) {
        self.grid.text(1, row, label, palette::WHITE);
        self.grid.number(11, row, 8, value, palette::WHITE);
    }

    fn usb(&mut self, usb: UsbStatus) {
        let label = match usb.stage {
            EnumerationStage::Starting => "STARTING",
            EnumerationStage::Attached => "WAIT RESET",
            EnumerationStage::Reset => "WAIT SETUP",
            EnumerationStage::Setup => "WAIT ADDRESS",
            EnumerationStage::Addressed => "WAIT CONFIG",
            EnumerationStage::Configured => "ONLINE",
            EnumerationStage::Suspended => "SUSPENDED",
            EnumerationStage::Disconnected => "DISCONNECTED",
            EnumerationStage::Retry => "SIE TIMEOUT",
        };
        let ink = if usb.stage == EnumerationStage::Configured {
            palette::WHITE
        } else {
            palette::AMBER
        };
        let mut line = String::<19>::new();
        write!(line, "USB {label:<14}").expect("USB stage fits");
        self.grid.text(1, 8, &line, ink);
        self.field(9, "RESETS", u32::from(usb.resets));
        self.field(10, "RETRIES", u32::from(usb.retries));
        if let Some(setup @ [_, _, _, _, il, ih, ll, lh]) = usb.last_setup {
            let req = request("REQ", setup);
            self.grid.text(1, 11, &req, palette::WHITE);
            line.clear();
            write!(line, "IDX {ih:02X}{il:02X} LEN {lh:02X}{ll:02X}").expect("setup fields fit");
            self.grid.text(1, 12, &line, palette::WHITE);
        } else {
            self.grid.text(1, 11, "REQ NONE", palette::WHITE);
        }
        line.clear();
        match usb.last_stall {
            Some(ControlStall::Request(setup)) => line = request("STALL", setup),
            Some(ControlStall::ByteCount(count)) => {
                write!(line, "STALL COUNT {count:02X}  ").expect("stall count fits");
            }
            None => line.push_str("STALL NONE").expect("stall label fits"),
        }
        self.grid.text(1, 14, &line, palette::WHITE);
        if let Some(timeout) = usb.last_timeout {
            let (op, register) = match timeout {
                RegisterTimeout::Read(reg) => ("RD", reg),
                RegisterTimeout::Write(reg) => ("WR", reg),
            };
            line.clear();
            write!(line, "TIMEOUT {op} {register:02X}").expect("timeout fits");
            self.grid.text(1, 13, &line, palette::AMBER);
        } else {
            self.grid.text(1, 13, "TIMEOUT NONE", palette::WHITE);
        }
    }
}

fn request(label: &str, [ty, req, vl, vh, ..]: [u8; 8]) -> String<19> {
    let mut line = String::new();
    write!(line, "{label} {ty:02X} {req:02X} {vh:02X}{vl:02X}").expect("setup fields fit");
    line
}
