#!/bin/sh
# Build a universal (x86_64 + aarch64) macOS binary into dist/.
set -eu

if [ "$(uname -s)" != "Darwin" ]; then
    echo "run this on macOS" >&2
    exit 1
fi

cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
OUT="dist/mujo-v${VERSION}-macos-universal"

rustup target add x86_64-apple-darwin aarch64-apple-darwin

cargo build --release --locked --target x86_64-apple-darwin
cargo build --release --locked --target aarch64-apple-darwin

mkdir -p dist
lipo -create -output "$OUT" \
    target/x86_64-apple-darwin/release/mujo \
    target/aarch64-apple-darwin/release/mujo

# arm64 needs at least an ad-hoc signature, and lipo can leave the merged
# binary needing a fresh one.
codesign --force --sign - "$OUT"

lipo -info "$OUT"
shasum -a 256 "$OUT" > "$OUT.sha256"
echo "built $OUT"
"$OUT" --version
