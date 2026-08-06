#!/usr/bin/env bash
# Pull on-device captures off a PSP mounted as USB mass storage.
#
# The game writes frames and their counters to ms0:/ANGLEZERO/ when SELECT is pressed. Put the
# PSP into USB mode (XMB → Settings → USB Connection), then run this. BMPs are converted to PNG
# and the counter dumps are printed, because the counters are usually what explains the picture.
#
# Usage: scripts/psp_pull.sh [destination]   (default: captures/)

set -euo pipefail

DEST="${1:-captures}"

find_stick() {
    # A PSP memory stick is identifiable by having a PSP/ directory at its root. Look wherever
    # this distribution happens to automount removable media.
    local root
    for root in /run/media/"$USER"/* /media/"$USER"/* /media/* /mnt/*; do
        [ -d "$root" ] || continue
        if [ -d "$root/PSP" ] || [ -d "$root/ANGLEZERO" ]; then
            printf '%s\n' "$root"
            return 0
        fi
    done
    return 1
}

STICK="${PSP_MOUNT:-$(find_stick || true)}"
if [ -z "$STICK" ]; then
    echo "No PSP memory stick found." >&2
    echo >&2
    echo "On the PSP: Settings -> USB Connection, with the cable plugged in." >&2
    echo "If it is mounted somewhere unusual, point at it directly:" >&2
    echo "    PSP_MOUNT=/path/to/stick $0" >&2
    exit 1
fi

SRC="$STICK/ANGLEZERO"
echo "memory stick: $STICK"
if [ ! -d "$SRC" ]; then
    echo "No captures yet — press SELECT in-game to save a frame." >&2
    exit 1
fi

mkdir -p "$DEST"
shopt -s nullglob nocaseglob

count=0
for bmp in "$SRC"/*.bmp; do
    base="$(basename "${bmp%.*}")"
    if command -v ffmpeg >/dev/null 2>&1; then
        ffmpeg -y -loglevel error -i "$bmp" -update 1 "$DEST/$base.png"
    else
        cp "$bmp" "$DEST/"
    fi
    count=$((count + 1))
done

for txt in "$SRC"/*.txt; do
    cp "$txt" "$DEST/"
done

echo "pulled $count frame(s) into $DEST/"
echo

for txt in "$DEST"/*.txt; do
    echo "--- $(basename "$txt") ---"
    cat "$txt"
done
