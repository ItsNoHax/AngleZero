#!/usr/bin/env bash
# Builds a release EBOOT and packages it the way a PSP expects to find it.
#
# Produces dist/AngleZero.<version>.zip containing:
#
#     PSP/GAME/AngleZero/EBOOT.PBP
#
# A PBP has to sit in its own folder under PSP/GAME — the folder name is what
# the XMB lists — so unzipping this at the root of a memory stick puts the game
# straight where it needs to be.
#
# The version comes from Cargo.toml unless one is given:
#
#     scripts/release.sh            # version from Cargo.toml
#     scripts/release.sh 0.2.0      # explicit
set -euo pipefail

cd "$(dirname "$0")/.."

VERSION="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
if [ -z "$VERSION" ]; then
    echo "could not determine a version, and none was given" >&2
    exit 1
fi

NAME="AngleZero"
OUT="dist/${NAME}.${VERSION}.zip"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

echo ">> building ${NAME} ${VERSION}"
# No devtools: the capture tooling, the frame timings and the render-state
# overrides are not something a release should ship with.
cargo psp --release

PBP="target/mipsel-sony-psp/release/angle-zero.EBOOT.PBP"
[ -f "$PBP" ] || { echo "no EBOOT at $PBP" >&2; exit 1; }

echo ">> checking the tests still pass"
cargo test --quiet

echo ">> staging"
mkdir -p "$STAGE/PSP/GAME/${NAME}"
cp "$PBP" "$STAGE/PSP/GAME/${NAME}/EBOOT.PBP"

mkdir -p dist
rm -f "$OUT"
( cd "$STAGE" && zip -q -r - PSP ) > "$OUT"

echo
echo ">> $OUT"
unzip -l "$OUT" | sed 's/^/   /'
echo
echo "   Unzip at the root of the memory stick, or copy the PSP folder over the"
echo "   one already there."
