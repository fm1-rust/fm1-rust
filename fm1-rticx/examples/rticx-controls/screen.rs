//! 20 × 15 text grid on the 240 × 240 display. Only changed glyphs are transferred.

use fm1_rticx::bsp::display::{Display, Error};

#[derive(Copy, Clone, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Ink {
    Muted,
    White,
    Cyan,
    Green,
    Amber,
    Red,
}

impl Ink {
    fn rgb(self) -> [u8; 2] {
        let color: u16 = match self {
            Self::Muted => 0x9cd3,
            Self::White => 0xffff,
            Self::Cyan => 0x5e9f,
            Self::Green => 0x5f6c,
            Self::Amber => 0xfe28,
            Self::Red => 0xf9c7,
        };
        color.to_be_bytes()
    }
}

#[derive(Copy, Clone, PartialEq, Eq)]
struct Cell {
    ch: u8,
    ink: Ink,
}
const BLANK: Cell = Cell {
    ch: b' ',
    ink: Ink::Muted,
};

pub(super) struct Screen {
    cells: [[Cell; 20]; 15],
    dirty: [u32; 15],
}

#[allow(
    clippy::arithmetic_side_effects,
    reason = "The fixed grid has 20 columns, 15 rows and eight bands per glyph."
)]
impl Screen {
    pub(super) fn new() -> Self {
        Self {
            cells: [[BLANK; 20]; 15],
            dirty: [0; 15],
        }
    }

    pub(super) fn clear(lcd: &mut Display) -> Result<(), Error> {
        static BLACK: [u8; 240 * 4 * 2] = [0; 240 * 4 * 2];
        for y in (0..240).step_by(4) {
            lcd.write_rect(0, y, 240, 4, &BLACK)?;
        }
        Ok(())
    }

    pub(super) fn text(&mut self, x: usize, y: usize, bytes: &[u8], ink: Ink) {
        for (offset, &ch) in bytes.iter().enumerate() {
            let col = x + offset;
            let value = Cell { ch, ink };
            if self.cells[y][col] != value {
                self.cells[y][col] = value;
                self.dirty[y] |= 1 << col;
            }
        }
    }

    pub(super) fn number(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        mut n: u32,
        sign: Option<u8>,
        ink: Ink,
    ) {
        let mut bytes = [b' '; 4];
        let mut i = width;
        loop {
            i -= 1;
            bytes[i] = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 || i == usize::from(sign.is_some()) {
                break;
            }
        }
        if n != 0 {
            bytes[..width].fill(b'*');
        } else if let Some(sign) = sign {
            bytes[i - 1] = sign;
        }
        self.text(x, y, &bytes[..width], ink);
    }

    pub(super) fn paint(&mut self, lcd: &mut Display) -> Result<(), Error> {
        for (y, dirty) in self.dirty.iter_mut().enumerate() {
            while *dirty != 0 {
                let x = dirty.trailing_zeros() as usize;
                let cell = self.cells[y][x];
                let rgb = cell.ink.rgb();
                let mut pixels = [0; 12 * 16 * 2];
                for (row, bits) in glyph(cell.ch).into_iter().enumerate() {
                    let line = &mut pixels[row * 48..][..48];
                    for col in 0..5 {
                        if bits & (16 >> col) != 0 {
                            line[col * 4..][..4].copy_from_slice(&[rgb[0], rgb[1], rgb[0], rgb[1]]);
                        }
                    }
                    line.copy_within(0..24, 24);
                }
                #[allow(
                    clippy::cast_possible_truncation,
                    reason = "Cell origins fit within 240 pixels."
                )]
                lcd.write_rect((12 * x) as u16, (16 * y) as u16, 12, 16, &pixels)?;
                *dirty &= !(1 << x);
            }
        }
        Ok(())
    }
}

fn glyph(ch: u8) -> [u8; 7] {
    match ch {
        b'A' => [14, 17, 17, 31, 17, 17, 17],
        b'B' => [30, 17, 17, 30, 17, 17, 30],
        b'C' => [14, 17, 16, 16, 16, 17, 14],
        b'D' => [28, 18, 17, 17, 17, 18, 28],
        b'E' => [31, 16, 16, 30, 16, 16, 31],
        b'F' => [31, 16, 16, 30, 16, 16, 16],
        b'G' => [14, 17, 16, 23, 17, 17, 15],
        b'H' => [17, 17, 17, 31, 17, 17, 17],
        b'I' => [14, 4, 4, 4, 4, 4, 14],
        b'J' => [7, 2, 2, 2, 2, 18, 12],
        b'K' => [17, 18, 20, 24, 20, 18, 17],
        b'L' => [16, 16, 16, 16, 16, 16, 31],
        b'M' => [17, 27, 21, 21, 17, 17, 17],
        b'N' => [17, 25, 25, 21, 19, 19, 17],
        b'O' => [14, 17, 17, 17, 17, 17, 14],
        b'P' => [30, 17, 17, 30, 16, 16, 16],
        b'Q' => [14, 17, 17, 17, 21, 18, 13],
        b'R' => [30, 17, 17, 30, 20, 18, 17],
        b'S' => [15, 16, 16, 14, 1, 1, 30],
        b'T' => [31, 4, 4, 4, 4, 4, 4],
        b'U' => [17, 17, 17, 17, 17, 17, 14],
        b'V' => [17, 17, 17, 17, 17, 10, 4],
        b'W' => [17, 17, 17, 21, 21, 21, 10],
        b'X' => [17, 17, 10, 4, 10, 17, 17],
        b'Y' => [17, 17, 10, 4, 4, 4, 4],
        b'Z' => [31, 1, 2, 4, 8, 16, 31],
        b'0' => [14, 17, 19, 21, 25, 17, 14],
        b'1' => [4, 12, 4, 4, 4, 4, 14],
        b'2' => [14, 17, 1, 2, 4, 8, 31],
        b'3' => [30, 1, 1, 14, 1, 1, 30],
        b'4' => [2, 6, 10, 18, 31, 2, 2],
        b'5' => [31, 16, 16, 30, 1, 1, 30],
        b'6' => [6, 8, 16, 30, 17, 17, 14],
        b'7' => [31, 1, 2, 4, 8, 8, 8],
        b'8' => [14, 17, 17, 14, 17, 17, 14],
        b'9' => [14, 17, 17, 15, 1, 2, 12],
        b'-' => [0, 0, 0, 14, 0, 0, 0],
        b'+' => [0, 4, 4, 31, 4, 4, 0],
        b'/' => [1, 1, 2, 4, 8, 16, 16],
        b'*' => [0, 21, 14, 31, 14, 21, 0],
        _ => [0; 7],
    }
}
