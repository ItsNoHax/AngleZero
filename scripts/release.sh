#!/usr/bin/env bash
# Builds a release EBOOT and packages it the way a PSP expects to find it.
#
# Produces dist/AngleZero.<version>.zip containing:
#
#     PSP/GAME/AngleZero/EBOOT.PBP
#
# and nothing else: every car in assets/compiled/ is packed into the EBOOT's
# DATA.PSAR by `anglezero-asset bundle`. So the game is one folder, and it
# works wherever a player puts it — a category folder, a PSP Go's internal
# storage — with no CARS/ to lose on the way. The cars are not compiled into
# the program: DATA.PSAR is never loaded, and the game reads each car out of
# it a chunk at a time, exactly as it reads a loose file.
#
# Development builds (plain `cargo psp`) carry no cars and read loose ones
# from ms0:/PSP/GAME/AngleZero/CARS/ instead, so a recompiled car is still one
# file copied and no rebuild.
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
for marker in "NO CULL" "NO DEPTH" "ms0:/ANGLEZERO/" "ATRAC.TXT" "TRACE.TXT" "SCRIPT.TXT"; do
    if strings -a "$PRX" | grep -qF "$marker"; then
        LEAKED="$LEAKED $marker"
    fi
done
if [ -n "$LEAKED" ]; then
    echo "REFUSING: the build carries devtools diagnostics:$LEAKED" >&2
    echo "Build without --features devtools." >&2
    exit 1
fi

# The cars. A build without them runs and says so on the title screen, which is
# not something to ship, so this refuses instead.
CARS=(assets/compiled/*.azcar)
if [ ! -e "${CARS[0]}" ]; then
    echo "REFUSING: no compiled cars in assets/compiled/" >&2
    echo "Run: cargo run --release -p anglezero-asset -- convert <model.glb> assets/compiled/<car>.azcar" >&2
    exit 1
fi

# Packing checks every car with the console's own parser and refuses one that
# is stale or oversized, then reads the bundle back before writing it.
echo ">> packing ${#CARS[@]} cars into the EBOOT"
mkdir -p "$STAGE/PSP/GAME/${NAME}"
cargo run --quiet --release -p anglezero-asset --bin anglezero-asset -- \
    bundle "$PBP" "$STAGE/PSP/GAME/${NAME}/EBOOT.PBP" "${CARS[@]}"

mkdir -p dist
rm -f "$OUT"
( cd "$STAGE" && zip -q -r - PSP ) > "$OUT"

echo
echo ">> $OUT"
unzip -l "$OUT" | sed 's/^/   /'
echo
echo "   Unzip at the root of the memory stick, or copy the PSP folder over the"
echo "   one already there. The AngleZero folder can be moved anywhere under"
echo "   PSP/GAME, category folders included: the cars are inside the EBOOT."
