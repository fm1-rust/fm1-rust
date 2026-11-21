# FM-1 graphics

[![Build status][ci-badge]][ci]

`fm1-graphics` provides a text grid for instrument readouts and an oscilloscope
plot for the M-VAVE FM-1. Both render into small RGB565 rectangles and send
them through the BSP's buffered SPI1 driver. The crate is allocation-free,
works with the native `fm1::display::Display`, and can be used from any runtime.
The buffered rectangle API is available in `fm1` 0.1.2.

```rust,no_run
use fm1::display::{Display, Error};
use fm1_graphics::{TextGrid, Waveform, palette};

fn draw(lcd: &mut Display, labels: &mut TextGrid, scope: &mut Waveform<224, 128>,
        trace: &[u8; 224], velocity: u8) -> Result<(), Error> {
    labels.text(0, 0, "VELOCITY", palette::MUTED);
    labels.number(9, 0, 3, u32::from(velocity), palette::CYAN);
    labels.paint(lcd)?;
    scope.paint(lcd, trace)
}

let mut labels = TextGrid::new();
let mut scope = Waveform::<224, 128>::new(8, 48).expect("plot fits");
```

Keep both objects between updates. Changing one character sends a 12 × 16
cell (384 bytes of pixel data). The plot takes one vertical pixel position
per column and repaints the tiles touched by the old and new traces. The
application chooses how it samples or triggers its audio. Drawing can run
at a lower priority than audio.

With the `async-display` feature and the BSP's SPI1 handler bound, call
`scope.paint_async(lcd, trace, delay).await?` to yield while a strip is in
flight. The next strip is still prepared while the previous one is on the wire.

The text grid begins clean on an assumed black screen. Clear the LCD once at
startup or call `invalidate()` to paint the entire grid. Invalidate a widget
after another view draws over its area. The waveform's first paint covers
its entire rectangle. Place the text you update outside that rectangle.

For proportional layouts, other fonts and general drawing primitives, the
BSP also exposes `embedded-graphics` through its optional feature.

[ci-badge]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-graphics.yml/badge.svg?branch=main&event=push
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-graphics.yml
