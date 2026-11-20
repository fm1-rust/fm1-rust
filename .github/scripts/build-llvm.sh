#!/usr/bin/env bash
# Build the host LLVM library used by the patched rustc.
set -euo pipefail
prefix=$(realpath -m "${1:?usage: build-llvm.sh INSTALL_DIR}")
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
    -DCMAKE_INSTALL_PREFIX="$prefix" \
    -DLLVM_TARGETS_TO_BUILD=X86 -DLLVM_EXPERIMENTAL_TARGETS_TO_BUILD=PI32V2 \
    -DLLVM_LINK_LLVM_DYLIB=ON -DLLVM_PARALLEL_LINK_JOBS=1 \
    -DLLVM_INCLUDE_TESTS=OFF -DLLVM_INCLUDE_BENCHMARKS=OFF -DLLVM_INCLUDE_EXAMPLES=OFF
cmake --build llvm-build --parallel "$jobs" \
    --target install-LLVM install-llvm-config
# Install generated headers after the library has finished building.
cmake --build llvm-build --parallel "$jobs" --target install-llvm-headers
