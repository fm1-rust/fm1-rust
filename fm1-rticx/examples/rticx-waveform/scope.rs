//! Redraw only tiles touched by a changed trace segment, including its old pixels.
use super::voice::{HEIGHT, Trace, WIDTH};
use fm1_rticx::bsp::display::{Display, Error};

const TILE_W: usize = 16;
const TILE_H: usize = 8;
const TILES_X: usize = WIDTH / TILE_W;
const TILES_Y: usize = HEIGHT / TILE_H;
const GRAPH_X: u16 = 8;
const GRAPH_Y: u16 = 48;

pub(super) struct Scope {
    last: Option<Trace>,
    pixels: [[u8; 2]; WIDTH * TILE_H],
}

impl Scope {
    pub(super) const fn new() -> Self {
        Self {
            last: None,
            pixels: [[0; 2]; WIDTH * TILE_H],
        }
    }

    pub(super) fn paint(&mut self, display: &mut Display, trace: &Trace) -> Result<(), Error> {
        let mut dirty = [0u16; TILES_Y];
        if let Some(last) = &self.last {
            for x in 0..WIDTH {
                let old = segment(last, x);
                let new = segment(trace, x);
                if old != new {
                    for (top, bottom) in [old, new] {
                        for mask in &mut dirty[top / TILE_H..=bottom / TILE_H] {
                            *mask |= 1 << (x / TILE_W);
                        }
                    }
                }
            }
        } else {
            dirty.fill((1 << TILES_X) - 1);
        }
        for (row, mut mask) in dirty.into_iter().enumerate() {
            while mask != 0 {
                let first = mask.trailing_zeros() as usize;
                let run = (mask >> first).trailing_ones() as usize;
                let x0 = first * TILE_W;
                let y0 = row * TILE_H;
                let width = run * TILE_W;
                let pixels = &mut self.pixels[..width * TILE_H];
                for x in x0..x0 + width {
                    let (top, bottom) = segment(trace, x);
                    for dy in 0..TILE_H {
                        let y = y0 + dy;
                        let color: u16 = if (top..=bottom).contains(&y) {
                            0x5ff5
                        } else if y == HEIGHT / 2 {
                            0x32ca
                        } else if y % 32 == 0 || x % 44 == 0 {
                            0x1965
                        } else {
                            0x0842
                        };
                        pixels[dy * width + x - x0] = color.to_be_bytes();
                    }
                }
                // Rendering the next run can overlap SPI1 reading its private SRAM strip.
                display.write_rect_buffered(
                    GRAPH_X + x0 as u16,
                    GRAPH_Y + y0 as u16,
                    width as u16,
                    TILE_H as u16,
                    pixels.as_flattened(),
                )?;
                mask &= !(((1 << run) - 1) << first);
            }
        }
        display.flush()?;
        self.last = Some(*trace);
        Ok(())
    }
}

fn segment(trace: &Trace, x: usize) -> (usize, usize) {
    let a = usize::from(trace[x.saturating_sub(1)]);
    let b = usize::from(trace[x]);
    (a.min(b), a.max(b))
}
