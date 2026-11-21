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
