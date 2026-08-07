#!/usr/bin/env bash
# Builds a release EBOOT and packages it the way a PSP expects to find it.
#
# Produces dist/AngleZero.<version>.zip containing:
#
#     PSP/GAME/AngleZero/EBOOT.PBP
#
# Unzipping that at the root of a memory stick puts the game straight where it
# needs to be.
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

# The diagnostics are behind the `devtools` feature, which is off by default — but "off by
# default" is a promise the build makes, not one it checks. If someone adds devtools to a
# default feature set, or leaves it enabled in a config, nothing else here would notice. So
# look in the binary itself for strings that only exist when it is on, and refuse to package
# a build that has them.
echo ">> checking no diagnostics were built in"
PRX="target/mipsel-sony-psp/release/angle-zero.prx"
LEAKED=""
for marker in "NO CULL" "NO DEPTH" "ms0:/ANGLEZERO/" "ATRAC.TXT" "TRACE.TXT"; do
    if strings -a "$PRX" | grep -qF "$marker"; then
        LEAKED="$LEAKED $marker"
    fi
done
if [ -n "$LEAKED" ]; then
    echo "REFUSING: the build carries devtools diagnostics:$LEAKED" >&2
    echo "Build without --features devtools." >&2
    exit 1
fi

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
