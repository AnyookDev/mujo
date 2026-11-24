#!/bin/sh
# Build a Linux x86_64 binary into dist/.
set -eu

if [ "$(uname -s)" != "Linux" ]; then
    echo "run this on Linux" >&2
    exit 1
fi

if [ "$(uname -m)" != "x86_64" ]; then
    echo "this script builds x86_64; host is $(uname -m)" >&2
    exit 1
fi

cd "$(dirname "$0")/.."

VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
OUT="dist/mujo-linux-x86_64"
NAME=$(basename "$OUT")

cargo build --release --locked

mkdir -p dist
cp target/release/mujo "$OUT"
chmod 755 "$OUT"

file "$OUT" || true

# The checksum file records the bare file name, so `sha256sum -c` works from
# whatever directory the download landed in.
( cd dist && sha256sum "$NAME" > "$NAME.sha256" )

echo "built $OUT (v$VERSION)"
"$OUT" --version
