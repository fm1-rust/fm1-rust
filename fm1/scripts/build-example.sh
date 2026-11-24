#!/bin/sh
# Build and package a BSP example without connecting to a device.
set -eu
example=${1:-blink}
package=fm1
image=$example
unset FM1_DMA_PROFILE
case "$example" in
    blink|display) set -- ;;
    cpu1-probe) set -- --features cpu1-probe ;;
    graphics) set -- --features embedded-graphics ;;
    tone) set -- --features audio ;;
    usb-window) set -- --features usb-window-probe,embedded-graphics ;;
    usb-log) package=fm1-rticx; set -- --features usb-log,rtt ;;
    rticx-blink|rticx-controls) package=fm1-rticx; set -- ;;
    rticx-cpu1-audio) package=fm1-rticx; set -- --features audio,cpu1-probe,async-usb,usb-log ;;
    rticx-piano) package=fm1-rticx; set -- --features audio,async-usb ;;
    rticx-tone|rticx-waveform) package=fm1-rticx; set -- --features audio ;;
    rticx-dma-stress)
        export FM1_DMA_PROFILE=stress
        example=rticx-timing
        package=fm1-rticx
        set -- --features audio,async-usb,usb-log,usb-dma-stress ;;
    rticx-dma-midi|rticx-dma-bulk|rticx-dma-iso)
        export FM1_DMA_PROFILE=${example#rticx-dma-}
        example=rticx-timing
        package=fm1-rticx
        set -- --features audio,async-usb,usb-log,usb-dma-probe ;;
    rticx-timing) package=fm1-rticx; set -- --features audio,async-usb,usb-log ;;
    *) echo "usage: $0 [blink|cpu1-probe|display|graphics|tone|usb-window|usb-log|rticx-blink|rticx-controls|rticx-cpu1-audio|rticx-piano|rticx-tone|rticx-waveform|rticx-timing|rticx-dma-{midi,bulk,iso,stress}]" >&2; exit 2 ;;
esac
: "${RUSTC:?set RUSTC to the patched pi32v2 rustc}"
: "${PI32V2_BIN:?set PI32V2_BIN to the pi32v2 LLVM bin directory}"
: "${PI32V2_ISA:?set PI32V2_ISA to the pi32v2-isa checkout}"
root=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
target="$root/../target"
out=${FM1_OUT:-"$target/firmware"}
# All three diagnostics compile the timing example with different descriptors.
# Independent invocations must never package another profile's ELF.
if [ "$image" != "$example" ]; then target="$target/$image"; fi
mkdir -p "$out"
out=$(cd "$out" && pwd)
PI32V2_BIN=$(cd "$PI32V2_BIN" && pwd)
packager=$(cd "$PI32V2_ISA" && pwd)/tools/hwtest/fm1app.py
test -f "$packager" || { echo "Cannot find $packager" >&2; exit 1; }
export PI32V2_BIN RUSTC_BOOTSTRAP=1
export CARGO_TARGET_PI32V2_NONE_LINKER="$PI32V2_BIN/ld.lld"
export RUSTFLAGS="-C link-arg=-Tfm1.ld"
cargo build --locked -p "$package" --manifest-path "$root/../Cargo.toml" --profile firmware --example "$example" \
    --target "$root/targets/pi32v2-none.json" --target-dir "$target" \
    -Zbuild-std=core,compiler_builtins -Zbuild-std-features=compiler-builtins-mem -Zjson-target-spec "$@"
elf="$target/pi32v2-none/firmware/examples/$example"
python3 "$packager" "$out/$image.fwsc" --from-elf "$elf"
cp "$elf" "$out/$image.elf"
