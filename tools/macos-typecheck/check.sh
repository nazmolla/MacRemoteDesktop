#!/bin/bash
# Type-check (and optionally lint) the macOS build on a Linux machine.
#
# Nothing is compiled to machine code or linked: C and Swift build steps are
# replaced with stand-ins that only create empty output files, and BoringSSL's
# bindgen step parses its headers against a small stub SDK. The Rust compiler
# still checks every `cfg(target_os = "macos")` path, which is the point.
#
# Usage:
#   tools/macos-typecheck/check.sh                 # cargo check
#   tools/macos-typecheck/check.sh --all-targets   # include tests
#   MODE=clippy tools/macos-typecheck/check.sh --all-targets -- -D warnings
#
# Needs: rustup target aarch64-apple-darwin, clang/libclang (for bindgen).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
target=aarch64-apple-darwin

rustup target list --installed | grep -qx "$target" || rustup target add "$target"

export PATH="$here/bin:$PATH"
export CC_aarch64_apple_darwin="$here/cc/cc"
export CXX_aarch64_apple_darwin="$here/cc/c++"
export AR_aarch64_apple_darwin="$here/cc/ar"
if [ -z "${LIBCLANG_PATH:-}" ]; then
    for d in /usr/lib/llvm-*/lib; do [ -e "$d" ] && export LIBCLANG_PATH="$d"; done
fi

# boring-sys: skip its CMake build (prebuilt path), keep bindgen on real headers.
bssl="$repo/target/macos-typecheck/bssl"
mkdir -p "$bssl/lib"
src="$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/boring-sys-4.*/deps/boringssl/src/include 2>/dev/null | head -1)"
if [ -z "$src" ]; then
    # Populate the registry first (downloads crates, builds nothing for mac).
    (cd "$repo" && cargo fetch --locked >/dev/null)
    src="$(ls -d "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/boring-sys-4.*/deps/boringssl/src/include | head -1)"
fi
ln -sfn "$src" "$bssl/include"
export BORING_BSSL_PATH="$bssl"

cd "$repo"
exec cargo "${MODE:-check}" --locked --target "$target" "$@"
