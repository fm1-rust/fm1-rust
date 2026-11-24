# FM-1 Graphics

[![crates.io][crate-badge]][crate]
[![docs.rs][docs-badge]][docs]
[![Build status][ci-badge]][ci]

Extended high-performance graphics support for the M-VAVE FM-1.

## About

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

## Drawing with embedded-graphics

Enable the `embedded-graphics` feature on `fm1-graphics` to use the upstream
fonts, text styles and primitives through `fm1_graphics::embedded_graphics`.
Enable `bsp-display` to also use the BSP's dirty-row `Framebuffer` and
its ST7789 `MipidsiDisplay` draw target. See the [BSP drawing guide][bsp-graphics] for
initialization and DMA transfers.

```rust
# #[cfg(feature = "bsp-display")]
# {
use fm1_graphics::{Framebuffer, palette, embedded_graphics::{
    mono_font::{ascii::FONT_6X10, MonoTextStyle},
    prelude::*,
    text::{Baseline, Text},
}};

let mut strip = Framebuffer::<96, 12>::new();
Text::with_baseline("CUTOFF", Point::zero(),
    MonoTextStyle::new(&FONT_6X10, palette::CYAN), Baseline::Top)
    .draw(&mut strip).unwrap();
assert!(strip.take_dirty().is_some());
# }
```

`TextGrid::draw` (via the `Drawable` trait) renders the full 240 × 240 grid
to any RGB565 target. Its dirty cells remain available for the next
`paint` or `paint_async` on the LCD.

## License

Licensed under the [MIT License][mit].

### Contribution

Unless you explicitly state otherwise, you license contributions you
intentionally submit to this crate under the [MIT License][mit].

[crate-badge]: https://img.shields.io/crates/v/fm1-graphics?logo=rust
[crate]: https://crates.io/crates/fm1-graphics
[docs-badge]: https://docs.rs/fm1-graphics/badge.svg
[docs]: https://docs.rs/fm1-graphics/
[ci-badge]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-graphics.yml/badge.svg?branch=main&event=push
[ci]: https://github.com/fm1-rust/fm1-rust/actions/workflows/fm1-graphics.yml
[bsp-graphics]: https://docs.rs/fm1/latest/fm1/#drawing-with-embedded-graphics
[mit]: https://github.com/fm1-rust/fm1-rust/blob/main/fm1-graphics/LICENSE
