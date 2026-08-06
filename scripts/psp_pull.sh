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

# Per-shot counters are short enough to read in full.
for txt in "$DEST"/SHOT*.txt; do
    [ -e "$txt" ] || continue
    echo "--- $(basename "$txt") ---"
    cat "$txt"
done

# Traces are 600 rows each, so summarise instead: report the range of each geometry counter and
# call out any frame that submitted nothing. A drop to zero is the signature of geometry being
# culled or a draw being skipped; a steady count means the fault is further down the pipeline.
for trace in "$DEST"/TRACE*.txt; do
    [ -e "$trace" ] || continue
    echo
    echo "--- $(basename "$trace") ---"
    awk '
        /^#/ { next }
        {
            n++
            for (i = 2; i <= 7; i++) {
                if (n == 1 || $i < lo[i]) lo[i] = $i
                if (n == 1 || $i > hi[i]) hi[i] = $i
            }
            if ($2 == 0 || $3 == 0) zero[$1] = $0
        }
        END {
            split("road terrain lines rails dashes props", name, " ")
            printf "  %d frames recorded\n", n
            for (i = 2; i <= 7; i++)
                printf "  %-8s %3d .. %-3d%s\n", name[i-1], lo[i], hi[i],
                       (lo[i] == 0 ? "   <-- dropped to zero" : "")
            k = 0
            for (f in zero) k++
            if (k > 0) {
                printf "\n  %d frame(s) submitted no road or no terrain:\n", k
                c = 0
                for (f in zero) { if (c++ < 12) printf "    %s\n", zero[f] }
            } else {
                print "\n  every frame submitted road and terrain geometry"
            }
        }
    ' "$trace"
done
