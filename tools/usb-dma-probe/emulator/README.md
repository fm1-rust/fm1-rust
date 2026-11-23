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
git worktree add --detach "$work/base" 2462b9817678ea9ed5d0763f00024c2d1bd739f8
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
for fault in normal-count normal-silent normal-ep0; do
    "$work/emu/target/release/examples/check_probe" "$work/normal.bin" "$fault" "$work/stock.config" || exit 1
done
```

The normal image must match the pristine MIDI configuration and use disjoint
guarded EP0 and EP1 staging. A legal 64-byte packet and a modeled silent
write of exactly 2048 bytes leave USB online. A 65-byte ready count, a
2049-byte silent EP1 write and a changed last byte of EP0's guard each
detach USB and reach the timing firmware's panic handler in a fresh boot.
The harness checks its retained download-mode mailbox and the disabled USB0
controller. To exercise the same cases in the piano firmware, build
`rticx-piano` and supply its unpacked binary in place of `normal.bin`.
The diagnostic
checks include a clean no-OUT control, two serviced packets, a 69-byte
crossing, a 1023-byte silent write, local PLAY capture, the MIDI marker, and
the journal and first clean re-arm after an interrupted acquisition. Set
`FM1_PROBE_LCD` to an output BMP path for a diagnostic run to capture the
actual LCD framebuffer.

The stress profile adds an 8192-byte capture with the production boundary at
offset 2048. Build `rticx-dma-stress`, unpack its FWSC as above, and run
`check_probe stress.bin stress`. It checks all 128 pages, a silent injected
2048-byte write, a silent injected 3072-byte write, endpoint recovery, the LCD,
and an unplug/reconnect interrupting an acquisition. The modeled writer ignores
RX_MAX and writes the length chosen by the harness.
