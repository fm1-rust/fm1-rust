//! Host fixture for the chip entry points and registers touched by board startup.
//! Tests drive interrupt deliveries and SPI completions explicitly.
extern crate std;
use self::std::{collections::BTreeMap, vec::Vec};
use super::*;
use core::cell::RefCell;
use embedded_hal::digital::{InputPin, OutputPin, StatefulOutputPin};
use registers::*;

#[derive(Default)]
struct Chip {
    regs: BTreeMap<usize, u32>,
    ticks: u32,
    spi_done: bool,
    dma_stuck: bool,
    transfers: Vec<(bool, Vec<u8>)>,
    shifted: u16,
    latched: Vec<u16>,
}
std::thread_local! { static CHIP: RefCell<Chip> = RefCell::default(); }

pub(crate) fn read(addr: usize) -> u32 {
    CHIP.with_borrow_mut(|chip| match addr {
        addr if addr == PA + IN => {
            // The PLAY switch can only pull PA6 low when its LED drive is blank.
            let raw = chip.regs.get(&addr).copied().unwrap_or(0);
            if chip.regs.get(&(PA + OUT)).copied().unwrap_or(0) & (1 << 10) != 0 {
                raw | (1 << 6)
            } else {
                raw
            }
        }
        0x10804 => {
            chip.ticks = chip.ticks.wrapping_add(24_000);
            chip.ticks
        }
        0x11d00 => chip.regs[&addr] | (u32::from(chip.spi_done) << 15),
        _ => chip.regs.get(&addr).copied().unwrap_or_default(),
    })
}
pub(crate) fn write(addr: usize, mut value: u32) {
    CHIP.with_borrow_mut(|chip| {
        if addr == PA + OUT {
            let last = chip.regs.get(&addr).copied().unwrap_or_default();
            let rising = value & !last;
            if rising & 8 != 0 {
                chip.shifted = chip.shifted << 1 | u16::from(value & 16 != 0);
            }
            if rising & 2 != 0 {
                chip.latched.push(chip.shifted);
            }
        }
        if addr == 0x11d00 {
            if value & (1 << 14) != 0 {
                chip.spi_done = false;
            }
            value &= !((1 << 14) | (1 << 15));
        }
        if addr == 0x11d08 {
            assert_eq!(chip.regs[&(PC + OUT)] & 0x180, 0); // CS and DC low
            chip.transfers.push((
                false,
                std::vec![u8::try_from(value).expect("8-bit SPI command")],
            ));
            chip.spi_done = true;
        }
        chip.regs.insert(addr, value);
    })
}
#[unsafe(no_mangle)]
extern "C" fn fm1_set_icfg(value: u32) {
    assert_eq!(value, 0x300);
}
#[unsafe(no_mangle)]
extern "C" fn fm1_timer5_isr() {
    time::rust_timer5_tick();
}
#[unsafe(no_mangle)]
unsafe extern "C" fn fm1_spi1_dma(p: *const u8, n: u32) {
    // The library owns this static buffer and publishes exactly n initialized bytes.
    let bytes = unsafe { core::slice::from_raw_parts(p, n as usize) }.to_vec();
    CHIP.with_borrow_mut(|chip| {
        assert_eq!(chip.regs[&(PC + OUT)] & 0x180, 0x100); // CS low, DC high
        chip.transfers.push((true, bytes));
        chip.spi_done = !chip.dma_stuck;
    })
}

#[test]
fn blink_and_display_use_the_board_api() {
    // Isolated, single-core register model at boot.
    let mut board = unsafe { Board::from_reset() };
    assert_eq!(read(0x10908), 59_999);
    assert_eq!(read(0x01ee_f100 + 28), 3 << 28);
    CHIP.with_borrow(|chip| assert_eq!(chip.latched, [0xfbff]));
    assert_eq!(board.timers.ticks(), 0);
    for _ in 0..50 {
        fm1_timer5_isr();
    }
    assert_eq!(board.timers.ticks(), 50);

    board.panel.led().set_high().unwrap();
    write(PA + IN, !0);
    assert!(!board.panel.button().is_pressed());
    write(PA + IN, !(1 << 6));
    assert!(board.panel.button().is_low().unwrap());
    assert!(board.panel.led().is_set_high().unwrap());
    board.panel.led().toggle().unwrap();
    assert!(board.panel.led().is_set_low().unwrap());
    write(PB + IN, !0);
    assert!(!board.headphones.is_inserted());
    write(PB + IN, !(1 << 5));
    assert!(board.headphones.is_inserted());

    let mut matrix = board.panel.into_matrix();
    matrix.select(panel::Column::new(3).expect("valid column"));
    CHIP.with_borrow(|chip| assert_eq!(chip.latched.last(), Some(&0xfff7)));
    matrix.set_led(panel::LedRow::Pa8, true);
    assert_eq!(matrix.sample(), 1 << 2);
    assert_ne!(read(PH + OUT) & (1 << 9), 0);

    let mut lcd = board.display.init().expect("working SPI");
    let pixels: Vec<_> = (0..=u8::MAX).cycle().take(4800).collect();
    lcd.write_rect(0, 2, 240, 10, &pixels)
        .expect("valid rectangle");
    CHIP.with_borrow(|chip| {
        let commands: Vec<_> = chip
            .transfers
            .iter()
            .filter(|(data, _)| !data)
            .map(|(_, v)| v[0])
            .collect();
        assert_eq!(
            commands,
            [1, 0x11, 0x3a, 0x36, 0x21, 0x13, 0x2a, 0x2b, 0x2c, 0x29]
        );
        let data: Vec<_> = chip
            .transfers
            .iter()
            .filter(|(data, _)| *data)
            .map(|(_, v)| v.as_slice())
            .collect();
        assert_eq!(
            data,
            [
                &[0x55][..],
                &[0],
                &[0, 0, 0, 239],
                &[0, 2, 0, 11],
                &pixels[..3840],
                &pixels[3840..]
            ]
        );
    });
    CHIP.with_borrow_mut(|chip| chip.dma_stuck = true);
    assert_eq!(
        lcd.write_rect(0, 0, 1, 1, &[0, 0]),
        Err(display::Error::Timeout)
    );
    assert_eq!(read(0x11d00) & 1, 0);
    assert_ne!(read(PC + OUT) & (1 << 7), 0);
    let transfers = CHIP.with_borrow(|chip| chip.transfers.len());
    assert_eq!(
        lcd.write_rect(0, 0, 1, 1, &[0, 0]),
        Err(display::Error::Timeout)
    );
    CHIP.with_borrow(|chip| assert_eq!(chip.transfers.len(), transfers));
}
