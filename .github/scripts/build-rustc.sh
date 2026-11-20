#!/usr/bin/env bash
# Build the x86-64 Linux rustc used by the firmware job on a cache miss.
set -euo pipefail
prefix=$(realpath -m "${1:?usage: build-rustc.sh INSTALL_DIR [LLVM_DIR]}")
version=1.100.0
host=x86_64-unknown-linux-gnu
patch_file=$(realpath "${PI32V2_ISA:?}/examples/fm1-blink-rust/rustc/rustc-$version-pi32v2.patch")
jobs=${CMAKE_BUILD_PARALLEL_LEVEL:-$(nproc)}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
if [[ -n ${2:-} ]]; then
    llvm=$(realpath -e "$2")
else
    llvm="$work/llvm"
    "$(dirname "$0")/build-llvm.sh" "$llvm"
fi
cd "$work"

curl -fsSL --retry 3 --output rust.tar.xz \
    "https://static.rust-lang.org/dist/rustc-$version-src.tar.xz"
echo '90dab90b8084eb57c35ae2b1d0b70c8e5138ace6023b22c60a5a400f586f63b2  rust.tar.xz' | sha256sum --check
mkdir rust
tar -xf rust.tar.xz -C rust --strip-components=1
cd rust
patch -p1 < "$patch_file"
cat > bootstrap.toml <<EOF
change-id = "ignore"
[build]
docs = false
vendor = true
[llvm]
download-ci-llvm = false
link-shared = true
[rust]
optimize = 1
download-rustc = false
channel = "dev"
lld = false
llvm-tools = false
llvm-bitcode-linker = false
[target.$host]
llvm-config = "$llvm/bin/llvm-config"
EOF
export LD_LIBRARY_PATH="$llvm/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
python3 x.py build --stage 1 -j "$jobs" library

# Cache only the compiler, its host libraries and the sources for -Zbuild-std.
# The stage1 rust-src symlink points back into this temporary build tree.
stage=build/host/stage1
mkdir -p "$prefix/bin" "$prefix/lib/rustlib/src/rust"
cp "$stage/bin/rustc" "$prefix/bin/"
cp -a "$stage/lib/"*.so "$llvm/lib/"libLLVM*.so* "$prefix/lib/"
cp -a "$stage/lib/rustlib/$host" "$prefix/lib/rustlib/"
cp -a library "$prefix/lib/rustlib/src/rust/"
