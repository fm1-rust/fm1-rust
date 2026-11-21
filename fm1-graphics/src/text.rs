use crate::{Rgb565, palette};
use embedded_graphics_core::pixelcolor::{RgbColor, raw::ToBytes};
use fm1::display::{Display, Error, SIZE};

const COLS: usize = TextGrid::COLUMNS;
const ROWS: usize = TextGrid::ROWS;
const CELL_W: u16 = 12;
const CELL_H: u16 = 16;
type CellPixels = [[[u8; 2]; CELL_W as usize]; CELL_H as usize];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cell {
    ch: u8,
    ink: Rgb565,
}
const BLANK: Cell = Cell {
    ch: b' ',
    ink: palette::MUTED,
};

/// A 20 × 15 character grid using the FM-1 dashboards' 5 × 7 font at 2× scale.
/// Each 12 × 16 cell includes a right and bottom margin. Changes stay in RAM
/// until [`paint`](Self::paint) writes their cells to the LCD.
#[derive(Debug)]
pub struct TextGrid {
    cells: [[Cell; COLS]; ROWS],
    dirty: [u32; ROWS],
    background: Rgb565,
}

impl Default for TextGrid {
    fn default() -> Self {
        Self::new()
    }
}

impl TextGrid {
    /// Number of character columns on the display.
    pub const COLUMNS: usize = (SIZE / CELL_W) as usize;
    /// Number of character rows on the display.
    pub const ROWS: usize = (SIZE / CELL_H) as usize;

    /// Start with a blank, clean grid on a black background.
    #[must_use]
    pub const fn new() -> Self {
        Self::with_background(Rgb565::BLACK)
    }

    /// Start with a blank, clean grid over an existing background of this color.
    #[must_use]
    pub const fn with_background(background: Rgb565) -> Self {
        Self {
            cells: [[BLANK; COLS]; ROWS],
            dirty: [0; ROWS],
            background,
        }
    }

    /// Schedule every cell for repaint, for example after another view covered the grid.
    pub fn invalidate(&mut self) {
        self.dirty.fill((1 << COLS) - 1);
    }

    /// Update a row of ASCII text, clipping at the right edge. Coordinates
    /// outside the grid are ignored. The font covers A–Z, digits, space and
    /// `- + # / *`. Lowercase uses uppercase glyphs. Other bytes draw a blank.
    pub fn text(&mut self, x: usize, y: usize, text: impl AsRef<[u8]>, ink: Rgb565) {
        let Some(cells) = self.cells.get_mut(y).and_then(|row| row.get_mut(x..)) else {
            return;
        };
        for (column, (cell, &ch)) in (x..).zip(cells.iter_mut().zip(text.as_ref())) {
            let next = Cell {
                ch: ch.to_ascii_uppercase(),
                ink,
            };
            if *cell != next {
                *cell = next;
                self.dirty[y] |= 1 << column;
            }
        }
    }

    /// Write a right-aligned unsigned decimal field, padding with spaces.
    /// Width is capped at the number of columns. Overflow fills the field with stars.
    pub fn number(&mut self, x: usize, y: usize, width: usize, value: u32, ink: Rgb565) {
        self.decimal(x, y, width, value, None, ink);
    }

    /// Write a right-aligned decimal field with an explicit `+` or `-` sign.
    /// Padding and overflow follow [`Self::number`].
    pub fn signed_number(&mut self, x: usize, y: usize, width: usize, value: i32, ink: Rgb565) {
        self.decimal(
            x,
            y,
            width,
            value.unsigned_abs(),
            Some(if value < 0 { b'-' } else { b'+' }),
            ink,
        );
    }

    #[allow(
        clippy::arithmetic_side_effects,
        reason = "Each ASCII digit is in b'0'..=b'9'."
    )]
    fn decimal(
        &mut self,
        x: usize,
        y: usize,
        width: usize,
        mut n: u32,
        sign: Option<u8>,
        ink: Rgb565,
    ) {
        let mut buf = [b' '; COLS];
        let field = &mut buf[..width.min(COLS)];
        let mut slots = field.iter_mut().rev();
        loop {
            let Some(slot) = slots.next() else {
                field.fill(b'*');
                break;
            };
            *slot = b'0' + (n % 10) as u8;
            n /= 10;
            if n == 0 {
                if let Some(sign) = sign {
                    if let Some(slot) = slots.next() {
                        *slot = sign;
                    } else {
                        field.fill(b'*');
                    }
                }
                break;
            }
        }
        self.text(x, y, field, ink);
    }

    /// Draw each changed cell into one address window, then wait for the last
    /// transfer. An unchanged grid sends nothing.
    ///
    /// # Errors
    /// Returns the BSP transfer error. Dirty cells remain pending for a retry.
    pub fn paint(&mut self, lcd: &mut Display) -> Result<(), Error> {
        if self.dirty == [0; ROWS] {
            return Ok(());
        }
        for (x, y, cell) in self.dirty_cells() {
            let pixels = render(cell, self.background);
            let bytes = pixels.as_flattened().as_flattened();
            lcd.write_rect_buffered(x, y, CELL_W, CELL_H, bytes)?;
        }
        lcd.flush()?;
        self.dirty.fill(0);
        Ok(())
    }

    /// Paint the same dirty cells as [`Self::paint`], yielding while SPI1 sends
    /// their pixels. Bind the BSP's SPI1 handler and supply an async timer.
    /// An error or cancellation leaves all dirty cells pending for the next paint.
    ///
    /// # Errors
    /// Returns the BSP transfer error.
    #[cfg(feature = "async-display")]
    pub async fn paint_async(
        &mut self,
        lcd: &mut Display,
        delay: &mut impl fm1::embedded_hal_async::delay::DelayNs,
    ) -> Result<(), Error> {
        if self.dirty == [0; ROWS] {
            return Ok(());
        }
        let mut lcd = lcd.as_async(delay)?;
        for (x, y, cell) in self.dirty_cells() {
            let pixels = render(cell, self.background);
            let bytes = pixels.as_flattened().as_flattened();
            lcd.write_rect_buffered(x, y, CELL_W, CELL_H, bytes).await?;
        }
        lcd.flush().await?;
        self.dirty.fill(0);
        Ok(())
    }

    #[allow(
        clippy::cast_possible_truncation,
        clippy::arithmetic_side_effects,
        reason = "The 20 by 15 grid fits the 240-pixel panel on both axes."
    )]
    fn dirty_cells(&self) -> impl Iterator<Item = (u16, u16, Cell)> {
        self.dirty.iter().enumerate().flat_map(move |(row, &mask)| {
            let mut pending = mask;
            core::iter::from_fn(move || {
                if pending == 0 {
                    return None;
                }
                let col = pending.trailing_zeros() as usize;
                pending &= !(1 << col);
                Some((
                    col as u16 * CELL_W,
                    row as u16 * CELL_H,
                    self.cells[row][col],
                ))
            })
        })
    }
}

fn render(cell: Cell, background: Rgb565) -> CellPixels {
    let mut pixels = [[background.to_be_bytes(); CELL_W as usize]; CELL_H as usize];
    let ink = cell.ink.to_be_bytes();
    for ([first, second], bits) in pixels.as_chunks_mut::<2>().0.iter_mut().zip(glyph(cell.ch)) {
        for (col, pixels) in first.as_chunks_mut::<2>().0.iter_mut().take(5).enumerate() {
            if bits & (16 >> col) != 0 {
                pixels.fill(ink);
            }
        }
        *second = *first;
    }
    pixels
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
        b'#' => [10, 10, 31, 10, 31, 10, 10],
        b'/' => [1, 1, 2, 4, 8, 16, 16],
        b'*' => [0, 21, 14, 31, 14, 21, 0],
        _ => [0; 7],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_clips_and_only_marks_changed_cells() {
        let mut grid = TextGrid::new();
        grid.text(usize::MAX, 0, "A", palette::CYAN);
        grid.text(0, usize::MAX, "B", palette::CYAN);
        grid.text(19, 14, "a+", palette::CYAN);
        assert_eq!(grid.dirty[14], 1 << 19);
        assert_eq!(grid.cells[14][19].ch, b'A');
        grid.dirty.fill(0);
        grid.text(19, 14, "A", palette::CYAN);
        assert_eq!(grid.dirty, [0; ROWS]);
        grid.text(19, 14, "A", palette::RED);
        assert_eq!(grid.dirty[14], 1 << 19);
        grid.invalidate();
        assert_eq!(grid.dirty, [(1 << COLS) - 1; ROWS]);
    }

    #[test]
    fn partial_traversal_leaves_all_dirty_cells_for_retry() {
        let mut grid = TextGrid::new();
        grid.text(19, 14, "C", palette::RED);
        grid.text(7, 2, "B", palette::WHITE);
        grid.text(1, 2, "A", palette::CYAN);

        let cells = || grid.dirty_cells().map(|(x, y, cell)| (x, y, cell.ch));
        assert_eq!(cells().next(), Some((12, 32, b'A')));
        assert!(cells().eq([(12, 32, b'A'), (84, 32, b'B'), (228, 224, b'C')]));
    }

    #[test]
    fn numeric_fields_pad_clip_and_handle_overflow() {
        let mut grid = TextGrid::new();
        grid.signed_number(0, 0, 3, 0, palette::WHITE);
        grid.signed_number(3, 0, 1, -2, palette::WHITE);
        grid.number(4, 0, 2, 100, palette::WHITE);
        grid.signed_number(6, 0, 11, i32::MIN, palette::WHITE);
        grid.number(17, 0, 2, 9, palette::WHITE);
        grid.number(19, 0, 0, 42, palette::WHITE);
        assert_eq!(grid.cells[0].map(|c| c.ch), *b" +0***-2147483648 9 ");
        grid.number(0, 0, usize::MAX, u32::MAX, palette::CYAN);
        assert_eq!(grid.cells[0].map(|c| c.ch), *b"          4294967295");
    }

    #[test]
    fn bitmap_keeps_doubled_pixels_and_blank_margins() {
        let pixels = render(
            Cell {
                ch: b'A',
                ink: palette::CYAN,
            },
            palette::RED,
        );
        let off = [0xf9, 0xc7];
        let on = [0x5e, 0x9f];
        assert_eq!(
            pixels[0],
            [off, off, on, on, on, on, on, on, off, off, off, off]
        );
        for [top, bottom] in pixels.as_chunks::<2>().0 {
            assert_eq!(top, bottom);
        }
        assert_eq!(pixels[15], [off; CELL_W as usize]);
    }
}
