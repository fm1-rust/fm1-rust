# Verify the DMA diagnostic in fm1-emu

Run from the patched fm1-rust checkout, after building the three diagnostic
images as described in the [physical investigation](../../../docs/usb-dma/README.md#reproduce-on-a-board).
The harness drives the actual firmware through USB enumeration, control
requests, captured SRAM, MIDI and LCD output. Its synthetic writer can
suppress the RX indication after writing SRAM. It does not model the WL82
CRC trailer, the measured clipping or the peripheral write-range monitor.

First build a pristine reference at the patch base and an ordinary patched
timing image. Use the firmware toolchain environment for both builds.

```sh
src="$PWD"
work=$(mktemp -d)
git worktree add --detach "$work/base" 27210227909df5ad0433787895dc1776fe7c2c83
(cd "$work/base" && ./fm1/scripts/build-example.sh rticx-timing)
./fm1/scripts/build-example.sh rticx-timing
```

Extract the application from each flashable container and check it against
the raw linker output. The extractor verifies the FM-1 identity, app entry
and package, flash, app-area and app CRCs.

```sh
extract="$src/tools/usb-dma-probe/emulator/unpack.py"
ref="$work/base/target/firmware/rticx-timing"
python3 "$extract" "$ref.fwsc" "$work/stock.bin"
cmp "$work/stock.bin" "$ref.bin"
for profile in normal midi bulk iso; do
    case "$profile" in
        normal) name=rticx-timing ;;
        *) name="rticx-dma-$profile" ;;
    esac
    image="$src/target/firmware/$name"
    python3 "$extract" "$image.fwsc" "$work/$profile.bin" || exit 1
    cmp "$work/$profile.bin" "$image.bin" || exit 1
done
```

Build the pinned emulator with the host Rust compiler and install its small
test-only USB host extension. It reads emulated device state directly.

```sh
git clone https://github.com/joakimeriksson/fm1-emu "$work/emu"
git -C "$work/emu" checkout --detach 588db9e988ea818951dcf8fda8fdd11d5fcc77eb
git -C "$work/emu" apply "$src/tools/usb-dma-probe/emulator/usb-host.patch"
mkdir -p "$work/emu/crates/fm1-soc/examples"
cp "$src/tools/usb-dma-probe/emulator/check_probe.rs" "$work/emu/crates/fm1-soc/examples/"
(cd "$work/emu" && unset RUSTC RUSTFLAGS CARGO_TARGET_DIR CARGO_BUILD_TARGET && cargo build --locked -p fm1-soc --release --example check_probe)
for profile in stock normal midi bulk iso; do
    "$work/emu/target/release/examples/check_probe" "$work/$profile.bin" "$profile" "$work/stock.config" || exit 1
done
```

The normal image must match the pristine MIDI configuration, use disjoint
EP0 and EP1 staging and survive a modeled 1023-byte write. The diagnostic
checks include a clean no-OUT control, two serviced packets, a 69-byte
crossing, a 1023-byte silent write, local PLAY capture, the MIDI marker, and
the journal and first clean re-arm after an interrupted acquisition. Set
`FM1_PROBE_LCD` to an output BMP path for a diagnostic run to capture the
actual LCD framebuffer.
