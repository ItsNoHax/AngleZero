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

# Traces are 600 rows each, so summarise instead. Two things are worth calling out: a counter
# falling to zero, and a *gap* — a chunk skipped while chunks on both sides of it were drawn.
# The second matters because a band missing from the middle of the view only dips a count by one,
# which is indistinguishable from ordinary culling at the edges unless you look at which chunks
# were submitted rather than how many.
for trace in "$DEST"/TRACE*.txt; do
    [ -e "$trace" ] || continue
    echo
    echo "--- $(basename "$trace") ---"
    awk '
        function holes(mask,   i, seen, gap, bad) {
            seen = 0; gap = 0; bad = 0
            for (i = 0; i < 32; i++) {
                if (int(mask / 2^i) % 2 == 1) {
                    if (seen && gap) bad++
                    seen = 1; gap = 0
                } else if (seen) {
                    gap = 1
                }
            }
            return bad
        }
        /^#/ { next }
        NF < 11 { next }
        {
            n++
            # Traces from before the chunk masks were added have 11 columns, not 13.
            haveMask = (NF >= 13)
            for (i = 2; i <= 7; i++) {
                if (n == 1 || $i < lo[i]) lo[i] = $i
                if (n == 1 || $i > hi[i]) hi[i] = $i
            }
            if ($2 == 0 || $3 == 0) zero[$1] = $0
            if (haveMask) {
                masked++
                if (holes($12) > 0) roadhole[$1] = $0
                if (holes($13) > 0) terrhole[$1] = $0
            }
        }
        END {
            if (n == 0) { print "  (no rows)"; exit }
            split("road terrain lines rails dashes props", name, " ")
            printf "  %d frames recorded\n", n
            for (i = 2; i <= 7; i++)
                printf "  %-8s %3d .. %-3d%s\n", name[i-1], lo[i], hi[i],
                       (lo[i] == 0 ? "   <-- dropped to zero" : "")
            k = 0
            for (f in zero) k++
            if (k > 0) {
                printf "\n  %d frame(s) submitted no road or no terrain\n", k
            } else {
                print "\n  every frame submitted road and terrain geometry"
            }
            if (masked == 0) {
                print "  (this trace predates the chunk masks, so gaps cannot be checked)"
                exit
            }
            rh = 0; th = 0
            for (f in roadhole) rh++
            for (f in terrhole) th++
            if (rh || th) {
                printf "  GAPS: %d frame(s) skipped a road chunk mid-run, %d for terrain\n", rh, th
                c = 0
                for (f in roadhole) { if (c++ < 6) printf "    road %s\n", roadhole[f] }
                c = 0
                for (f in terrhole) { if (c++ < 6) printf "    terr %s\n", terrhole[f] }
            } else {
                print "  no gaps: every drawn run of chunks was contiguous"
            }
        }
    ' "$trace"
done
