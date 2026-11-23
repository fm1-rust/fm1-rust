#!/usr/bin/env bash
# Verify the workspace crate archive and build the BSP or RTICX examples.
set -euo pipefail
package=${1:?usage: build-firmware.sh PACKAGE [ARCHIVE]}
archive=${2:-}

# RTICX path dependencies can be unpublished during pull requests.
if [[ -n "$archive" || "$package" == fm1 ]]; then
    cargo package --locked --package "${archive:-$package}" --all-features \
        --target "$PI32V2_TARGET" -Zjson-target-spec -Zbuild-std=core,compiler_builtins \
        -Zbuild-std-features=compiler-builtins-mem
fi

for example in "$package"/examples/*.rs; do
    ./fm1/scripts/build-example.sh "$(basename "$example" .rs)"
done

if [[ $package == fm1-rticx ]]; then
    for profile in midi bulk iso stress; do
        ./fm1/scripts/build-example.sh "rticx-dma-$profile"
    done
    # Use the same macro, controller and target ABI as the firmware examples.
    layout=(cargo rustc --color never --locked -p rticx-pi32v2 --example shared-layout --profile firmware
        --target "$PI32V2_TARGET" -Zjson-target-spec -Zbuild-std=core,compiler_builtins
        -Zbuild-std-features=compiler-builtins-mem -- --check-cfg 'cfg(packed_shared)')
    "${layout[@]}"
    log=$(mktemp)
    trap 'rm -f "$log"' EXIT
    if "${layout[@]}" --cfg packed_shared >"$log" 2>&1; then
        echo "packed shared fields compiled successfully" >&2
        exit 1
    fi
    cat "$log"
    grep -Fq 'error[E0793]' "$log"

    handoffs=(cargo rustc --color never --locked -p rticx-pi32v2 --example interrupt-handoffs
        --profile firmware --target "$PI32V2_TARGET" -Zjson-target-spec -Zbuild-std=core,compiler_builtins
        -Zbuild-std-features=compiler-builtins-mem --)
    cases=(bad_shared bad_local bad_input)
    for flag in "${cases[@]}"; do handoffs+=(--check-cfg "cfg($flag)"); done
    "${handoffs[@]}"
    for flag in "${cases[@]}"; do
        if "${handoffs[@]}" --cfg "$flag" >"$log" 2>&1; then
            echo "unsafe RTICX call compiled: $flag" >&2
            exit 1
        fi
        grep -F "error[E0277]" "$log" || { cat "$log" >&2; exit 1; }
    done

    dispatch=(cargo rustc --color never --locked -p rticx-pi32v2 --example dispatcher-access
        --profile firmware --target "$PI32V2_TARGET" -Zjson-target-spec -Zbuild-std=core,compiler_builtins
        -Zbuild-std-features=compiler-builtins-mem --)
    cases=(forge_irq forge_idle call_installer replace_slot forge_facade forge_token)
    for flag in "${cases[@]}"; do dispatch+=(--check-cfg "cfg($flag)"); done
    "${dispatch[@]}"
    for flag in "${cases[@]}"; do
        case "$flag" in
            forge_irq|forge_idle) expect=E0423 ;;
            forge_facade) expect=E0308 ;;
            forge_token) expect=E0603 ;;
            *) expect=E0133 ;;
        esac
        if "${dispatch[@]}" --cfg "$flag" >"$log" 2>&1; then
            echo "unsafe RTICX call compiled: $flag" >&2
            exit 1
        fi
        grep -F "error[$expect]" "$log" || { cat "$log" >&2; exit 1; }
    done
fi
