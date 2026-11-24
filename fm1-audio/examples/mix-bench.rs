//! Compare 128 frames of scalar and FM-1 mixing. PLAY returns to download mode.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

use core::hint::black_box;
use fm1::{Board, display::SpiClock, enter_download_mode, entry, time::cycles};
use fm1_audio::{I24, OUTPUT_FRAMES, OutputBlock, mix, mix::Gain, mix_stereo};
use fm1_graphics::{TextGrid, palette};

entry!(bench);

#[derive(Clone, Copy)]
struct Reading {
    scalar: u32,
    mixer: u32,
    matched: bool,
}

fn timed(out: &mut OutputBlock, mut run: impl FnMut(&mut OutputBlock)) -> u32 {
    let mut best = u32::MAX;
    for _ in 0..16 {
        for (i, frame) in out.iter_mut().enumerate() {
            *frame = match i % 4 {
                0 => [i32::MIN + 7, i32::MAX - 9],
                1 => [i32::MAX, i32::MIN],
                _ => [-123, 456],
            };
        }
        black_box(&mut *out);
        let start = cycles();
        run(black_box(&mut *out));
        black_box(&*out);
        best = best.min(cycles().wrapping_sub(start));
    }
    best
}

#[inline(never)]
fn compare(scalar: impl FnMut(&mut OutputBlock), mixer: impl FnMut(&mut OutputBlock)) -> Reading {
    let mut a = [[0; 2]; OUTPUT_FRAMES];
    let mut b = a;
    let scalar = timed(&mut a, scalar);
    let mixer = timed(&mut b, mixer);
    Reading {
        scalar,
        mixer,
        matched: a == b,
    }
}

const LEVELS: [Gain; 2] = [Gain::from_bits(12_345), Gain::from_bits(-9_876)];

fn scalar_pcm(dst: &mut [i32; 2], frame: [i16; 2], gains: [Gain; 2]) {
    for ((out, sample), gain) in dst.iter_mut().zip(frame).zip(gains) {
        let product = i32::from(sample).wrapping_mul(i32::from(gain.to_bits()));
        *out = out.saturating_add(product >> 6);
    }
}

#[inline(never)]
fn measurements() -> [Reading; 6] {
    // Deliberately place the first stereo frame two bytes past a word boundary.
    #[repr(align(4))]
    struct Samples([i16; OUTPUT_FRAMES * 2 + 1]);
    let mut pcm = Samples([0; OUTPUT_FRAMES * 2 + 1]);
    for (i, s) in pcm.0.iter_mut().enumerate() {
        *s = i16::try_from(i).expect("small block").wrapping_mul(17_197);
    }
    let (stereo, _) = black_box(&pcm.0[1..]).as_chunks::<2>();
    let mono = black_box(&pcm.0[..OUTPUT_FRAMES]);
    let gains = black_box(LEVELS);
    let mono_time = compare(
        |out| {
            for (dst, &s) in out.iter_mut().zip(mono) {
                scalar_pcm(dst, [s; 2], gains);
            }
        },
        |out| {
            mix::mono_i16(out, mono, gains);
        },
    );
    let time_stereo = |stereo: &[[i16; 2]]| {
        compare(
            |out| {
                for (dst, &s) in out.iter_mut().zip(stereo) {
                    scalar_pcm(dst, s, gains);
                }
            },
            |out| {
                mix::stereo_i16(out, stereo, gains);
            },
        )
    };
    let halfword_time = time_stereo(stereo);
    let (aligned, _) = black_box(&pcm.0[..OUTPUT_FRAMES * 2]).as_chunks::<2>();
    let word_time = time_stereo(aligned);
    let bus: OutputBlock = core::array::from_fn(|i| stereo[i].map(|s| i32::from(s) << 8));
    let bus = black_box(&bus);
    let bus_time = compare(
        |out| {
            for (dst, src) in out.iter_mut().zip(bus) {
                dst[0] = dst[0].saturating_add(src[0]);
                dst[1] = dst[1].saturating_add(src[1]);
            }
        },
        |out| {
            mix::stereo_q23(out, bus);
        },
    );
    let left = core::array::from_fn::<_, OUTPUT_FRAMES, _>(|i| bus[i][0]);
    let right = core::array::from_fn::<_, OUTPUT_FRAMES, _>(|i| bus[i][1]);
    let planar_time = compare(
        |out| {
            for ((dst, &l), &r) in out.iter_mut().zip(&left).zip(&right) {
                dst[0] = dst[0].saturating_add(l);
                dst[1] = dst[1].saturating_add(r);
            }
        },
        |out| {
            mix::planar_q23(out, &left, &right);
        },
    );
    let left = left.map(I24::new_unchecked);
    let right = right.map(I24::new_unchecked);
    let legacy_time = compare(
        |out| {
            for ((dst, &l), &r) in out.iter_mut().zip(&left).zip(&right) {
                dst[0] = dst[0].saturating_add(l.inner());
                dst[1] = dst[1].saturating_add(r.inner());
            }
        },
        |out| mix_stereo(out, &left, &right),
    );
    [
        mono_time,
        word_time,
        halfword_time,
        bus_time,
        planar_time,
        legacy_time,
    ]
}

fn bench(board: Board) -> ! {
    show(board, measurements())
}

// Keep the LCD grid and paint scratch off the stack while the two mix blocks are live.
#[inline(never)]
fn show(mut board: Board, results: [Reading; 6]) -> ! {
    let mut lcd = board.display.init_with_clock(SpiClock::Div1).expect("LCD");
    let mut grid = TextGrid::new();
    grid.invalidate();
    grid.text(0, 0, "128 FRAMES / 24 MHZ", palette::WHITE);
    grid.text(0, 2, "TICKS    RUST  MIXER", palette::CYAN);
    for ((label, r), row) in ["MONO16", "S16 WORD", "S16 HALF", "BUS23", "PLANES23", "I24"]
        .into_iter()
        .zip(results)
        .zip(4..)
    {
        grid.text(0, row, label, palette::WHITE);
        grid.number(8, row, 5, r.scalar, palette::MUTED);
        if r.matched {
            grid.number(14, row, 6, r.mixer, palette::GREEN);
        } else {
            grid.text(15, row, "FAIL", palette::RED);
        }
    }
    grid.text(0, 11, "BEST OF 16", palette::MUTED);
    grid.text(0, 13, "PLAY TO EXIT", palette::MUTED);
    grid.paint(&mut lcd).expect("write timings");
    lcd.flush().expect("finish display transfer");
    while !board.panel.button().is_pressed() {}
    enter_download_mode()
}

#[cfg(target_os = "none")]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    enter_download_mode()
}

#[cfg(test)]
#[test]
fn all_backends_match_the_reference() {
    assert!(measurements().iter().all(|r| r.matched));
}
