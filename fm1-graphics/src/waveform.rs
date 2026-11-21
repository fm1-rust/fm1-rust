use crate::Rgb565;
use embedded_graphics_core::pixelcolor::raw::ToBytes;
use fm1::display::{Display, Error, SIZE};

const TILE_W: usize = 16;
const TILE_H: usize = 8;
const MAX_BANDS: usize = (SIZE as usize).div_ceil(TILE_H);

struct TileRun {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    background: [[Rgb565; 2]; TILE_H],
}

struct Strip<'a> {
    x: u16,
    y: u16,
    width: u16,
    height: u16,
    pixels: &'a [u8],
}

impl Strip<'_> {
    fn write(&self, lcd: &mut Display) -> Result<(), Error> {
        lcd.write_rect_buffered(self.x, self.y, self.width, self.height, self.pixels)
    }
}

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
        for run in self.dirty_runs(trace) {
            self.render(trace, run).write(lcd)?;
        }
        lcd.flush()?;
        self.last = Some(*trace);
        Ok(())
    }

    /// Paint the same dirty tiles as [`Self::paint`], yielding while SPI1 sends
    /// the previous strip. Bind the BSP's SPI1 handler and supply an async timer.
    /// A cancelled paint causes the next paint to redraw the whole plot.
    ///
    /// # Errors
    /// Returns the BSP transfer error.
    #[cfg(feature = "async-display")]
    pub async fn paint_async(
        &mut self,
        lcd: &mut Display,
        trace: &[u8; W],
        delay: &mut impl fm1::embedded_hal_async::delay::DelayNs,
    ) -> Result<(), Error> {
        if self.last.as_ref() == Some(trace) {
            return Ok(());
        }
        let mut lcd = lcd.as_async(delay)?;
        for run in self.dirty_runs(trace) {
            let strip = self.render(trace, run);
            lcd.write_rect_buffered(strip.x, strip.y, strip.width, strip.height, strip.pixels)
                .await?;
        }
        lcd.flush().await?;
        self.last = Some(*trace);
        Ok(())
    }

    fn dirty_runs(&mut self, trace: &[u8; W]) -> impl Iterator<Item = TileRun> + use<W, H> {
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

        // Commit the new trace only after all strips have reached the LCD.
        self.last = None;
        let style = self.style;
        dirty
            .into_iter()
            .enumerate()
            .filter(|(_, mask)| *mask != 0)
            .flat_map(move |(row, mut mask)| {
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
                core::iter::from_fn(move || {
                    if mask == 0 {
                        return None;
                    }
                    let first = mask.trailing_zeros() as usize;
                    let run = (mask >> first).trailing_ones() as usize;
                    let x0 = first * TILE_W;
                    let width = (run * TILE_W).min(W - x0);
                    mask &= !(((1 << run) - 1) << first);
                    Some(TileRun {
                        x: x0,
                        y: y0,
                        width,
                        height,
                        background: backgrounds,
                    })
                })
            })
    }

    fn render(&mut self, trace: &[u8; W], run: TileRun) -> Strip<'_> {
        let pixels = &mut self.pixels.as_flattened_mut()[..run.width * run.height];
        for dx in 0..run.width {
            let x = run.x + dx;
            let (top, bottom) = Self::segment(trace, x);
            let vertical_grid =
                self.style.grid_x != 0 && x.is_multiple_of(usize::from(self.style.grid_x));
            for dy in 0..run.height {
                let color = if (top..=bottom).contains(&(run.y + dy)) {
                    self.style.trace
                } else {
                    run.background[dy][usize::from(vertical_grid)]
                };
                pixels[dy * run.width + dx] = color.to_be_bytes();
            }
        }
        Strip {
            x: self.x + run.x as u16,
            y: self.y + run.y as u16,
            width: run.width as u16,
            height: run.height as u16,
            pixels: pixels.as_flattened(),
        }
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
    fn interrupted_update_repaints_the_entire_previous_trace() {
        let mut scope = Waveform::<32, 16>::new(0, 0).expect("plot fits");
        let previous = [0; 32];
        scope.last = Some(previous);
        let mut next = previous;
        next[31] = 15;
        assert_eq!(
            scope.dirty_runs(&next).next().map(|run| run.width),
            Some(16)
        );
        let mut retry = scope.dirty_runs(&previous);
        assert_eq!(retry.next().map(|run| run.width), Some(32));
        assert_eq!(retry.next().map(|run| run.width), Some(32));
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
            for run in scope.dirty_runs(trace) {
                let Strip {
                    x,
                    y,
                    width: w,
                    height: h,
                    pixels: bytes,
                } = scope.render(trace, run);
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
            }
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
