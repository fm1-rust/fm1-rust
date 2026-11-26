//! Host fixture for the chip entry points and registers touched by board startup.
//! Tests drive interrupt deliveries and SPI completions explicitly.
extern crate std;
use self::std::{
    collections::{BTreeMap, VecDeque},
    vec::Vec,
};
use super::*;
use core::cell::RefCell;
use embedded_hal::digital::{InputPin, OutputPin, StatefulOutputPin};
use registers::*;

#[derive(Default)]
struct Chip {
    regs: BTreeMap<usize, u32>,
    ticks: u32,
    usb_reply: Option<u8>,
    p33_output: Vec<u8>,
    p33_input: VecDeque<u8>,
    p33_stall_at: Option<usize>,
    spi_done: bool,
    dma_stuck: bool,
    preempt_until_after_spi_timeout: bool,
    #[cfg(feature = "async-display")]
    hold_irq_dma: bool,
    dma: Option<(*const u8, usize)>,
    transfers: Vec<(bool, Vec<u8>)>,
    shifted: u16,
    latched: Vec<u16>,
    last_key_sample: Option<(u16, u32)>,
}
impl Chip {
    fn complete_spi_dma(&mut self) {
        if let Some((ptr, len)) = self.dma.take() {
            // SAFETY: the SPI stub records the driver's permanent SRAM strip and
            // transfer length. Display tests hold DISPLAY while the strip is live.
            let bytes = unsafe { core::slice::from_raw_parts(ptr, len) }.to_vec();
            self.transfers.push((true, bytes));
            self.spi_done = true;
        }
    }
}
std::thread_local! { static CHIP: RefCell<Chip> = RefCell::default(); }
// The registers are thread-local, but the real driver's DMA strip is shared.
static DISPLAY: std::sync::Mutex<()> = std::sync::Mutex::new(());
static POWER: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(feature = "async-display")]
mod async_display;

#[cfg(feature = "embedded-graphics")]
mod graphics;

pub(crate) fn complete_usb_on_timer_read(value: u8) {
    CHIP.with_borrow_mut(|chip| chip.usb_reply = Some(value));
}

pub(crate) fn read(addr: usize) -> u32 {
    CHIP.with_borrow_mut(|chip| match addr {
        addr if addr == PA + IN => {
            // The PLAY switch can only pull PA6 low when its LED drive is blank.
            let raw = chip.regs.get(&addr).copied().unwrap_or(0);
            let drive = chip.regs.get(&(PA + OUT)).copied().unwrap_or(0);
            chip.last_key_sample = chip.latched.last().map(|&column| (column, drive));
            if drive & (1 << 10) != 0 {
                raw | (1 << 6)
            } else {
                raw
            }
        }
        0x10804 => {
            if core::mem::take(&mut chip.preempt_until_after_spi_timeout) {
                // The transfer completes while the CPU is preempted.
                chip.complete_spi_dma();
                chip.dma_stuck = false;
                // A higher-priority interrupt prevents the CPU from resuming for 51 ms.
                chip.ticks = chip.ticks.wrapping_add(51 * 24_000);
            }
            chip.ticks = chip.ticks.wrapping_add(24_000);
            // Complete an outstanding SIE read while a millisecond elapses in an ISR.
            if chip.regs.get(&0x11804).is_some_and(|v| v & 0x4000 != 0)
                && let Some(value) = chip.usb_reply.take()
            {
                chip.regs.insert(0x11804, 0x8000 | u32::from(value));
            }
            chip.ticks
        }
        0x11d00 => {
            if !chip.dma_stuck {
                chip.complete_spi_dma();
            }
            chip.regs[&addr] | (u32::from(chip.spi_done) << 15)
        }
        _ => chip.regs.get(&addr).copied().unwrap_or_default(),
    })
}
pub(crate) fn write(addr: usize, mut value: u32) {
    CHIP.with_borrow_mut(|chip| {
        if addr == 0x13e08 && value & 0x10 != 0 {
            assert_eq!(value & 0x101, 1, "P33 selected outside the RTC domain");
            let byte = chip.regs[&0x13e0c].to_le_bytes()[0];
            chip.p33_output.push(byte);
            value &= !0x10;
            if chip.p33_stall_at == Some(chip.p33_output.len()) {
                value |= 2;
            }
            if let Some(reply) = chip.p33_input.pop_front() {
                chip.regs.insert(0x13e0c, u32::from(reply));
            }
        }
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
        if addr == 0x10600 {
            let pending = chip.regs.get(&addr).copied().unwrap_or_default() & (1 << 15);
            let clear = value & (1 << 14) != 0;
            value &= !0xc000;
            if !clear {
                value |= pending;
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

#[test]
fn p33_reads_only_status_registers_and_releases_the_bridge_after_timeout() {
    use crate::power::Error;
    const CON: usize = 0x13e08;
    const IDLE: u32 = 0x108; // An unrelated control bit and the original RTC bank.
    let _lock = POWER.lock().expect("P33 fixture");
    let board = Board::init();
    let mut power = board.take_power().expect("exclusive P33 owner");
    assert!(board.take_power().is_none());
    write(CON, IDLE);

    for bits in 0..8u8 {
        let (supply, enabled, full) = (bits & 4 != 0, bits & 2 != 0, bits & 1 != 0);
        CHIP.with_borrow_mut(|chip| {
            chip.p33_output.clear();
            chip.p33_input.extend([
                0,
                0,
                0xdf | (u8::from(supply) << 5),
                0,
                0,
                0xfe | u8::from(enabled),
                0,
                0,
                0xfe | u8::from(full),
            ]);
        });
        let status = power.supply_status().expect("P33 read");
        assert_eq!(
            (
                status.external_power,
                status.charger_enabled,
                status.charge_full
            ),
            (supply, enabled, full)
        );
        assert_eq!(status.is_charging(), supply && enabled && !full);
        CHIP.with_borrow(|chip| {
            assert_eq!(
                chip.p33_output,
                [0x80, 0xa1, 0x5e, 0x80, 0x1a, 0x5e, 0x80, 0x19, 0x5e]
            )
        });
        assert_eq!(read(CON), IDLE);
    }
    for byte in 1..=9 {
        CHIP.with_borrow_mut(|chip| {
            chip.p33_output.clear();
            chip.p33_stall_at = Some(byte);
        });
        assert_eq!(power.supply_status(), Err(Error::Timeout));
        CHIP.with_borrow(|chip| assert_eq!(chip.p33_output.len(), byte));
        assert_eq!(read(CON), IDLE);
    }
    for occupied in [IDLE | 1, IDLE | 2] {
        write(CON, occupied);
        assert_eq!(power.supply_status(), Err(Error::Busy));
        assert_eq!(read(CON), occupied);
    }
    write(CON, IDLE);
    CHIP.with_borrow_mut(|chip| chip.p33_stall_at = None);
    assert!(power.supply_status().is_ok());
    drop(power);
    assert!(board.take_power().is_some());
}

#[test]
fn watchdog_commands_and_boot_reset_flags() {
    use crate::power::{Error, WatchdogTimeout};
    let _lock = POWER.lock().expect("P33 fixture");
    let board = Board::init();
    let mut power = board.take_power().expect("P33");
    const CON: usize = 0x13e08;
    const IDLE: u32 = 0x108;
    write(CON, IDLE);
    write(0x100c0, 0x8000_0000);
    CHIP.with_borrow_mut(|chip| chip.p33_input.extend([0, 0, 0x85]));
    power::snapshot_reset_cause();
    let cause = power.reset_cause();
    assert_eq!(cause.p33, Some(0x85));
    assert_eq!(cause.system, 0x8000_0000);
    assert_eq!(cause.power_on(), Some(true));
    assert_eq!(cause.watchdog(), Some(true));
    assert_eq!(cause.software(), Some(false));
    // A later peripheral write or a new handle cannot change the boot snapshot.
    write(0x100c0, 1 << 5);
    drop(power);
    power = board.take_power().expect("reclaim P33");
    assert_eq!(power.reset_cause(), cause);
    for (timeout, selector) in [
        (WatchdogTimeout::Sec1, 0xa),
        (WatchdogTimeout::Sec2, 0xb),
        (WatchdogTimeout::Sec4, 0xc),
        (WatchdogTimeout::Sec8, 0xd),
        (WatchdogTimeout::Sec16, 0xe),
        (WatchdogTimeout::Sec32, 0xf),
    ] {
        CHIP.with_borrow_mut(|chip| chip.p33_output.clear());
        power.arm_watchdog(timeout).expect("arm");
        power.feed_watchdog().expect("feed");
        power.disarm_watchdog().expect("disarm");
        power::request_chip_reset().expect("chip reset command");
        CHIP.with_borrow(|chip| {
            assert_eq!(
                chip.p33_output,
                [
                    0,
                    0x80,
                    0,
                    0x40,
                    0x17,
                    0xbf,
                    0,
                    0x80,
                    selector | 0x40,
                    0x20,
                    0x80,
                    0x10,
                    0x20,
                    0x80,
                    0x40,
                    0,
                    0x80,
                    0,
                    0x20,
                    0xa0,
                    0x10,
                ]
            )
        });
        assert_eq!(read(CON), IDLE);
    }
    for byte in 1..=12 {
        CHIP.with_borrow_mut(|chip| {
            chip.p33_output.clear();
            chip.p33_stall_at = Some(byte);
        });
        assert_eq!(
            power.arm_watchdog(WatchdogTimeout::Sec1),
            Err(Error::Timeout)
        );
        CHIP.with_borrow(|chip| assert_eq!(chip.p33_output.len(), byte));
        assert_eq!(read(CON), IDLE);
    }
    write(CON, IDLE | 1);
    assert_eq!(power.feed_watchdog(), Err(Error::Busy));
    power::snapshot_reset_cause();
    let cause = power.reset_cause();
    assert_eq!(cause.p33, None);
    assert_eq!(cause.power_on(), None);
    assert_eq!(cause.watchdog(), None);
    assert_eq!(cause.software(), Some(true));
    write(0x100c0, 0);
    power::snapshot_reset_cause();
    assert_eq!(power.reset_cause().software(), None);
    write(CON, IDLE);
    CHIP.with_borrow_mut(|chip| {
        chip.p33_stall_at = None;
        chip.p33_input.extend([0, 0, 0x44]);
    });
    power::snapshot_reset_cause();
    let cause = power.reset_cause();
    assert_eq!(
        (cause.software(), cause.watchdog()),
        (Some(true), Some(true))
    );
}
#[unsafe(no_mangle)]
extern "C" fn fm1_set_icfg(value: u32) {
    assert_eq!(value, 0x300);
}
#[unsafe(no_mangle)]
extern "C" fn fm1_timer5_isr() {
    time::rust_timer5_tick();
}
/// # Safety
/// `p` must remain readable for `n` initialized bytes until the mock completes
/// the transfer or discards it. The caller must hold the display test lock.
#[unsafe(no_mangle)]
unsafe extern "C" fn fm1_spi1_dma(p: *const u8, n: u32) {
    CHIP.with_borrow_mut(|chip| {
        assert_eq!(chip.regs[&(PC + OUT)] & 0x180, 0x100); // CS low, DC high
        assert_ne!(n, 0);
        assert!(chip.dma.replace((p, n as usize)).is_none());
        chip.spi_done = false;
        #[cfg(feature = "async-display")]
        if chip.hold_irq_dma && chip.regs[&0x11d00] & (1 << 13) != 0 {
            chip.dma_stuck = true;
        }
    })
}

#[test]
fn timer2_alarm_arms_acknowledges_and_leaves_other_timers_alone() {
    let mut timers = time::Timers::start();
    let clocks = (read(0x10800), read(0x10900));
    let mut alarm = timers.take_alarm().expect("first claim");
    assert!(timers.take_alarm().is_none());
    alarm.arm_after(500_000);
    assert_eq!(
        (read(0x10600), read(0x10604), read(0x10608)),
        (9, 0, 500_000)
    );
    alarm.arm_after(0);
    assert_eq!(read(0x10608), 1);
    CHIP.with_borrow_mut(|chip| {
        chip.regs.insert(0x10600, 9 | (1 << 15));
    });
    drop(alarm);
    assert_eq!(read(0x10600), 0);
    assert_eq!((read(0x10800), read(0x10900)), clocks);
}

#[test]
fn green_play_indicator_restores_its_column_after_sampling() {
    use panel::PlayColor::{Amber, Green};

    const LED_LINES: u32 = (1 << 9) | (1 << 10);
    let mut panel = panel::Play::init();
    panel.led().set_color(Green);
    assert!(panel.led().is_set_low().unwrap());
    panel.led().set_high().unwrap();
    write(PA + IN, !(1 << 6));
    assert!(panel.button().is_pressed());
    CHIP.with_borrow(|chip| {
        let (column, drive) = chip.last_key_sample.expect("PLAY was sampled");
        assert_eq!(column, 0xfbff); // switch on column 10
        assert_eq!(drive & LED_LINES, 0);
        assert_eq!(chip.latched.last(), Some(&0xfeff)); // green on column 8
    });
    assert_eq!(read(PA + OUT) & LED_LINES, 1 << 9);

    panel.led().set_color(Amber);
    assert_eq!(read(PA + OUT) & LED_LINES, 1 << 10);
    CHIP.with_borrow(|chip| assert_eq!(chip.latched.last(), Some(&0xfbff)));
    panel.led().set_color(Green);
    let _matrix = panel.into_matrix();
    assert_eq!(read(PA + OUT) & LED_LINES, 0);
    CHIP.with_borrow(|chip| assert_eq!(chip.latched.last(), Some(&0xfbff)));
}

#[test]
fn blink_and_display_use_the_board_api() {
    let _display = DISPLAY.lock().expect("exclusive display");
    // SAFETY: the thread-local chip starts at reset and only this test delivers
    // its interrupts. DISPLAY guards the shared SPI staging buffer.
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
    let before = CHIP.with_borrow(|c| c.transfers.len());
    assert_eq!(
        lcd.write_rect_buffered(0, 0, 1, 1, &[0]),
        Err(display::Error::InvalidRegion)
    );
    lcd.fill_rect(240, 240, 0, 0, 0).expect("empty fill");
    CHIP.with_borrow(|c| assert_eq!(c.transfers.len(), before));

    let mut pixel = [0x12, 0x34];
    lcd.write_rect_buffered(1, 1, 1, 1, &pixel)
        .expect("stage the caller's pixel");
    pixel.fill(0);
    CHIP.with_borrow(|c| assert!(c.dma.is_some()));
    lcd.fill_rect(0, 0, 239, 9, 0xabcd)
        .expect("finish old DMA and repeat a strip");
    CHIP.with_borrow(|c| {
        let data: Vec<_> = c.transfers[before..]
            .iter()
            .filter(|(d, _)| *d)
            .map(|(_, v)| v.as_slice())
            .collect();
        assert_eq!(data[2], [0x12, 0x34]);
        assert_eq!(data[5].len(), 3840);
        assert_eq!(data[6].len(), 239 * 9 * 2 - 3840);
        for chunk in &data[5..] {
            assert!(chunk.as_chunks::<2>().0.iter().all(|p| *p == [0xab, 0xcd]));
        }
        assert_eq!(
            c.transfers
                .iter()
                .filter(|(d, v)| !d && v == &[0x29])
                .count(),
            1
        );
    });

    lcd.write_rect_buffered(0, 0, 1, 1, &pixel)
        .expect("launch without waiting");
    CHIP.with_borrow_mut(|chip| chip.dma_stuck = true);
    assert_eq!(lcd.flush(), Err(display::Error::Timeout));
    assert_eq!(read(0x11d00) & 1, 0);
    assert_ne!(read(PC + OUT) & (1 << 7), 0);
    let transfers = CHIP.with_borrow(|chip| chip.transfers.len());
    assert_eq!(
        lcd.write_rect(0, 0, 1, 1, &[0, 0]),
        Err(display::Error::Timeout)
    );
    CHIP.with_borrow(|chip| assert_eq!(chip.transfers.len(), transfers));
}

#[test]
fn completion_before_resuming_after_preemption_wins() {
    let _display = DISPLAY.lock().expect("exclusive display");
    let mut lcd = display::DisplayPort(Token::new()).init().expect("SPI init");
    lcd.write_rect_buffered(0, 0, 1, 1, &[0x12, 0x34])
        .expect("submit");
    CHIP.with_borrow_mut(|chip| {
        chip.dma_stuck = true;
        chip.preempt_until_after_spi_timeout = true;
    });
    let result = lcd.flush();
    CHIP.with_borrow(|chip| {
        assert!(chip.transfers.contains(&(true, std::vec![0x12, 0x34])));
    });
    assert_eq!(result, Ok(()));
}
