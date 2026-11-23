//! Paint from CPU-owned summaries. The LCD task never selects USB registers.
use crate::measure::Line;
use core::fmt::{self, Write};
use fm1_graphics::{Rgb565, TextGrid, palette::*};
use fm1_rticx::bsp::usb_midi::ProbeDisplay;

struct Number(Option<u16>);
impl fmt::Display for Number {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Some(n) => n.fmt(f),
            None => f.write_str("-"),
        }
    }
}

fn row(grid: &mut TextGrid, y: usize, color: Rgb565, args: fmt::Arguments<'_>) {
    let mut line = Line::new();
    line.write_fmt(args).expect("ASCII LCD row");
    let mut cells = [b' '; TextGrid::COLUMNS];
    let bytes = line.as_str().as_bytes();
    let n = bytes.len().min(cells.len());
    cells[..n].copy_from_slice(&bytes[..n]);
    grid.text(0, y, cells, color);
}

pub(super) fn draw(grid: &mut TextGrid, profile: &str, state: ProbeDisplay) {
    row(grid, 0, CYAN, format_args!("EP1 DMA  {profile}"));
    row(
        grid,
        1,
        WHITE,
        format_args!("ENUM {} RETRY {}", state.stage, state.retries),
    );
    let bus = if state.online { "USB UP" } else { "USB DOWN" };
    if state.aborted != 0 {
        row(grid, 2, AMBER, format_args!("{bus} AB#{}", state.aborted));
    } else {
        row(grid, 2, MUTED, format_args!("{bus}"));
    }
    row(grid, 14, MUTED, format_args!("OCT-/+ 2S TO FLASH"));
    if let Some(r) = state.reading {
        let label = if state.active { "ARM" } else { "LAST" };
        let pipe = match r.alt {
            1 => "BULK",
            2 => "ISO",
            _ => "MIDI",
        };
        row(grid, 3, CYAN, format_args!("{label} #{} {pipe}", r.token));
        let service = if r.delayed { "DLY50" } else { "NOW" };
        if let Some(n) = r.requested {
            row(grid, 4, WHITE, format_args!("REQ {n}  {service}"));
        } else {
            row(grid, 4, WHITE, format_args!("PLAY CAPTURE 3S"));
        }
        row(
            grid,
            5,
            WHITE,
            format_args!(
                "MAX {:02X}/{:02X} FILL {:02X}",
                r.rx_max[0], r.rx_max[1], r.fill
            ),
        );
        row(
            grid,
            6,
            WHITE,
            format_args!("READY {} IRQ {}", r.packets, u8::from(r.irq)),
        );
        row(
            grid,
            7,
            WHITE,
            format_args!(
                "CNT {} / {}",
                Number((r.packets > 0).then_some(r.counts[0])),
                Number((r.packets > 1).then_some(r.counts[1]))
            ),
        );
        if state.active {
            row(grid, 8, MUTED, format_args!("WAITING FOR FREEZE"));
            for y in 9..=11 {
                row(grid, y, MUTED, format_args!(""));
            }
        } else {
            row(
                grid,
                8,
                WHITE,
                format_args!("GATE {} CSR {:02X}", r.gate_count, r.gate_csr),
            );
            row(grid, 9, CYAN, format_args!("HIGH {}", Number(r.highest)));
            if let Some(i) = r.first_guard {
                row(
                    grid,
                    10,
                    RED,
                    format_args!("GUARD {i} BYTE {:02X}", r.guard_byte),
                );
            } else {
                row(grid, 10, GREEN, format_args!("GUARD INTACT"));
            }
            let (label, color) =
                if r.requested == Some(0) && (r.highest.is_some() || r.packets != 0) {
                    ("QUIET CHANGED", AMBER)
                } else if r.first_guard.is_some() {
                    (
                        if profile == "stress" {
                            "PAST BYTE 2047"
                        } else {
                            "PAST BYTE 67"
                        },
                        RED,
                    )
                } else if r.highest.is_none() {
                    ("NO SRAM CHANGE", MUTED)
                } else {
                    ("CAPTURED", CYAN)
                };
            row(grid, 11, color, format_args!("{label}"));
        }
    } else {
        for y in [4, 7, 9, 10, 11] {
            row(grid, y, MUTED, format_args!(""));
        }
        row(grid, 3, WHITE, format_args!("WAITING FOR ARM"));
        row(grid, 5, MUTED, format_args!("1 ATTACH  2 SETUP"));
        row(grid, 6, MUTED, format_args!("3 ADDR  4 CONFIG"));
        if profile == "midi" {
            row(grid, 8, CYAN, format_args!("PLAY  CAPTURE 3 SEC"));
        } else {
            row(
                grid,
                8,
                CYAN,
                format_args!(
                    "RUN FM1 {}",
                    if profile == "stress" {
                        "DMA-STRESS"
                    } else {
                        "DMA-PROBE"
                    }
                ),
            );
        }
    }
    if let Some(hit) = state.first_hit {
        row(
            grid,
            12,
            RED,
            format_args!("HIT #{} MAX {:02X}", hit.token, hit.rx_max[0]),
        );
        row(
            grid,
            13,
            RED,
            format_args!(
                "AT {} REQ {}",
                Number(hit.first_guard),
                Number(hit.requested)
            ),
        );
    } else {
        row(grid, 12, MUTED, format_args!("NO GUARD HIT YET"));
    }
}
