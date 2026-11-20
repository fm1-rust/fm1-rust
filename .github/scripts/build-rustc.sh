#!/usr/bin/env bash
# Build the x86-64 Linux rustc used by the firmware job on a cache miss.
set -euo pipefail
prefix=$(realpath -m "${1:?usage: build-rustc.sh INSTALL_DIR}")
version=1.100.0
host=x86_64-unknown-linux-gnu
patch_file=$(realpath "${PI32V2_ISA:?}/examples/fm1-blink-rust/rustc/rustc-$version-pi32v2.patch")
jobs=${CMAKE_BUILD_PARALLEL_LEVEL:-$(nproc)}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
cd "$work"

# The released clang and LLD target the FM-1. rustc also needs the X86 backend
# for host build scripts and a linkable LLVM library.
curl -fsSL --retry 3 --output llvm.tar.gz \
    "https://codeload.github.com/jvitkauskas/pi32v2-llvm/tar.gz/refs/tags/${LLVM_TAG:?}"
echo '4881df6c25ae1892e0be36c73e8f4077e0f58a5f078f320800bc0eee5e1d74b6  llvm.tar.gz' | sha256sum --check
mkdir llvm-src
# LLVMSupport includes headers from the sibling libc project.
tar -xzf llvm.tar.gz -C llvm-src --strip-components=1
cmake -S llvm-src/llvm -B llvm-build -G Ninja \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_CXX_FLAGS_RELEASE="-O1 -DNDEBUG" \
    -DCMAKE_INSTALL_PREFIX="$work/llvm" \
    -DLLVM_TARGETS_TO_BUILD=X86 -DLLVM_EXPERIMENTAL_TARGETS_TO_BUILD=PI32V2 \
    -DLLVM_LINK_LLVM_DYLIB=ON -DLLVM_PARALLEL_LINK_JOBS=1 \
    -DLLVM_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF -DLLVM_INCLUDE_EXAMPLES=OFF
cmake --build llvm-build --parallel "$jobs" \
    --target install-LLVM install-llvm-config
# Install generated headers after the library has finished building.
cmake --build llvm-build --parallel "$jobs" --target install-llvm-headers
rm -rf llvm-build llvm-src llvm.tar.gz

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
llvm-config = "$work/llvm/bin/llvm-config"
EOF
export LD_LIBRARY_PATH="$work/llvm/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
python3 x.py build --stage 1 -j "$jobs" library

# Cache only the compiler, its host libraries and the sources for -Zbuild-std.
# The stage1 rust-src symlink points back into this temporary build tree.
stage=build/host/stage1
mkdir -p "$prefix/bin" "$prefix/lib/rustlib/src/rust"
cp "$stage/bin/rustc" "$prefix/bin/"
cp -a "$stage/lib/"*.so "$work/llvm/lib/"libLLVM*.so* "$prefix/lib/"
cp -a "$stage/lib/rustlib/$host" "$prefix/lib/rustlib/"
cp -a library "$prefix/lib/rustlib/src/rust/"
