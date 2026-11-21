use crate::Rgb565;
use embedded_graphics_core::pixelcolor::raw::ToBytes;
use fm1::display::{Display, Error, SIZE};

const TILE_W: usize = 16;
const TILE_H: usize = 8;
const MAX_BANDS: usize = (SIZE as usize).div_ceil(TILE_H);

/// Colors and grid spacing for an oscilloscope trace.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct WaveformStyle {
    /// Color underneath the trace and grid.
    pub background: Rgb565,
    /// Grid line color.
    pub grid: Rgb565,
    /// Center line color.
    pub center: Rgb565,
    /// Trace color.
    pub trace: Rgb565,
    /// Distance in pixels between vertical grid lines. Zero disables them.
    pub grid_x: u8,
    /// Distance in pixels between horizontal grid lines. Zero disables them.
    pub grid_y: u8,
}

impl WaveformStyle {
    /// The palette and grid from the RTICX waveform demo.
    pub const DEFAULT: Self = Self {
        background: Rgb565::new(1, 2, 2),
        grid: Rgb565::new(3, 11, 5),
        center: Rgb565::new(6, 22, 10),
        trace: Rgb565::new(11, 63, 21),
        grid_x: 44,
        grid_y: 32,
    };
}

impl Default for WaveformStyle {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// An oscilloscope plot with one y coordinate per pixel column. The renderer
/// retains the previous trace and eight RGB565 rows of scratch space. Adjacent
/// dirty tiles share one ST7789 window. Drawing can overlap SPI1 pixel DMA.
#[derive(Debug)]
pub struct Waveform<const W: usize, const H: usize> {
    x: u16,
    y: u16,
    style: WaveformStyle,
    last: Option<[u8; W]>,
    pixels: [[[u8; 2]; W]; TILE_H],
}

#[allow(
    clippy::arithmetic_side_effects,
    clippy::cast_possible_truncation,
    reason = "Construction bounds W, H and the plot origin to the 240-pixel display. Bands have at most 15 tiles."
)]
impl<const W: usize, const H: usize> Waveform<W, H> {
    /// Position a plot on the LCD. Returns `None` for empty or out-of-bounds plots.
    #[must_use]
    pub const fn new(x: u16, y: u16) -> Option<Self> {
        if W == 0
            || H == 0
            || x >= SIZE
            || y >= SIZE
            || W > (SIZE - x) as usize
            || H > (SIZE - y) as usize
        {
            return None;
        }
        Some(Self {
            x,
            y,
            style: WaveformStyle::DEFAULT,
            last: None,
            pixels: [[[0; 2]; W]; TILE_H],
        })
    }

    /// Apply a new palette or grid spacing on the next paint.
    pub fn set_style(&mut self, style: WaveformStyle) {
        if self.style != style {
            self.style = style;
            self.invalidate();
        }
    }

    /// Repaint the whole plot on the next call, for example after a page change.
    pub fn invalidate(&mut self) {
        self.last = None;
    }

    /// Render connected sample positions. Zero is the top row. Values below
    /// the bottom edge are clamped to the last row. An identical trace sends
    /// nothing. On return all pixels in this update have reached the LCD.
    ///
    /// # Errors
    /// Returns the BSP transfer error. A failed update invalidates the old trace.
    pub fn paint(&mut self, lcd: &mut Display, trace: &[u8; W]) -> Result<(), Error> {
        if self.last.as_ref() == Some(trace) {
            return Ok(());
        }
        let result = self
            .paint_tiles(trace, |x, y, w, h, pixels| {
                lcd.write_rect_buffered(x, y, w, h, pixels)
            })
            .and_then(|()| lcd.flush());
        self.last = result.is_ok().then_some(*trace);
        result
    }

    fn paint_tiles<E>(
        &mut self,
        trace: &[u8; W],
        mut write: impl FnMut(u16, u16, u16, u16, &[u8]) -> Result<(), E>,
    ) -> Result<(), E> {
        let mut dirty = [0u16; MAX_BANDS];
        let bands = &mut dirty[..H.div_ceil(TILE_H)];
        if let Some(last) = &self.last {
            for x in 0..W {
                let old = Self::segment(last, x);
                let new = Self::segment(trace, x);
                if old != new {
                    for (top, bottom) in [old, new] {
                        for mask in &mut bands[top / TILE_H..=bottom / TILE_H] {
                            *mask |= 1 << (x / TILE_W);
                        }
                    }
                }
            }
        } else {
            bands.fill((1 << W.div_ceil(TILE_W)) - 1);
        }

        let style = self.style;
        for (row, &dirty) in bands.iter().enumerate() {
            if dirty == 0 {
                continue;
            }
            let mut mask = dirty;
            let y0 = row * TILE_H;
            let height = TILE_H.min(H - y0);
            // Compute the grid once per band. The inner pixel loop only selects colors.
            let backgrounds: [[Rgb565; 2]; TILE_H] = core::array::from_fn(|dy| {
                let y = y0 + dy;
                if y == H / 2 {
                    [style.center; 2]
                } else if style.grid_y != 0 && y.is_multiple_of(usize::from(style.grid_y)) {
                    [style.grid; 2]
                } else {
                    [style.background, style.grid]
                }
            });
            while mask != 0 {
                let first = mask.trailing_zeros() as usize;
                let run = (mask >> first).trailing_ones() as usize;
                let x0 = first * TILE_W;
                let width = (run * TILE_W).min(W - x0);
                let pixels = &mut self.pixels.as_flattened_mut()[..width * height];
                for dx in 0..width {
                    let x = x0 + dx;
                    let (top, bottom) = Self::segment(trace, x);
                    let vertical_grid =
                        style.grid_x != 0 && x.is_multiple_of(usize::from(style.grid_x));
                    for dy in 0..height {
                        let y = y0 + dy;
                        let color = if (top..=bottom).contains(&y) {
                            style.trace
                        } else {
                            backgrounds[dy][usize::from(vertical_grid)]
                        };
                        pixels[dy * width + dx] = color.to_be_bytes();
                    }
                }
                write(
                    self.x + x0 as u16,
                    self.y + y0 as u16,
                    width as u16,
                    height as u16,
                    pixels.as_flattened(),
                )?;
                mask &= !(((1 << run) - 1) << first);
            }
        }
        Ok(())
    }

    fn segment(trace: &[u8; W], x: usize) -> (usize, usize) {
        let a = usize::from(trace[x.saturating_sub(1)]).min(H - 1);
        let b = usize::from(trace[x]).min(H - 1);
        (a.min(b), a.max(b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::convert::Infallible;

    #[test]
    fn bounds_include_partial_tiles_and_the_panel_edge() {
        assert!(Waveform::<0, 16>::new(0, 0).is_none());
        assert!(Waveform::<16, 0>::new(0, 0).is_none());
        assert!(Waveform::<241, 1>::new(0, 0).is_none());
        assert!(Waveform::<1, 1>::new(u16::MAX, 0).is_none());
        assert!(Waveform::<35, 13>::new(206, 227).is_none());
        assert!(Waveform::<1, 1>::new(239, 239).is_some());
    }

    #[test]
    fn changing_one_endpoint_repaints_and_erases_only_its_edge_tiles() {
        const W: usize = 35;
        const H: usize = 13;
        let mut scope = Waveform::<W, H>::new(205, 227).expect("bottom-right plot");
        let base = [2; W];
        let mut edge = base;
        edge[W - 1] = u8::MAX;
        let mut frame = [[[0xaa, 0x55]; W]; H];
        for (pass, trace) in [&base, &edge, &base, &base].into_iter().enumerate() {
            let mut windows = 0;
            scope
                .paint_tiles(trace, |x, y, w, h, bytes| {
                    assert_eq!(usize::from(y - 227), windows * 8);
                    if pass == 0 {
                        assert_eq!((x, w), (205, 35));
                    } else {
                        assert_eq!((x, w), (237, 3));
                    }
                    assert_eq!(h, if windows == 0 { 8 } else { 5 });
                    for (i, pixel) in bytes.as_chunks::<2>().0.iter().enumerate() {
                        frame[usize::from(y - 227) + i / usize::from(w)]
                            [usize::from(x - 205) + i % usize::from(w)] = *pixel;
                    }
                    windows += 1;
                    Ok::<_, Infallible>(())
                })
                .expect("infallible display");
            scope.last = Some(*trace);
            assert_eq!(windows, if pass == 3 { 0 } else { 2 });
            for (y, row) in frame.iter().enumerate() {
                for (x, &pixel) in row.iter().enumerate() {
                    let color = if y == 2 || (pass == 1 && x == W - 1 && y > 2) {
                        0x5ff5u16
                    } else if y == H / 2 {
                        0x32ca
                    } else if x == 0 || y == 0 {
                        0x1965
                    } else {
                        0x0842
                    };
                    assert_eq!(pixel, color.to_be_bytes(), "({x}, {y}) on pass {pass}");
                }
            }
        }
        scope.set_style(WaveformStyle {
            grid_x: 0,
            grid_y: 0,
            ..WaveformStyle::default()
        });
        assert!(scope.last.is_none());
    }
}
