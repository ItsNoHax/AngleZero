#!/usr/bin/env bash
# Regenerate assets/SND0.AT3 — the XMB background music — from assets/SND0_source.wav.
#
# ATRAC3 is Sony's codec and nothing packages an encoder for it: ffmpeg knows the format but
# ships decoders only, and the `ffmpeg -c:a atrac3` line that circulates in PSP guides fails with
# "no encoders for it are available". So this builds atracdenc, and libsndfile for it to read WAV
# with, into a scratch directory. Nothing is installed system-wide.
#
# The output is committed, so this only needs running if the source loop changes.
#
# Usage: scripts/encode_music.sh [work-dir]

set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="${1:-${TMPDIR:-/tmp}/anglezero-atrac}"
PREFIX="$WORK/prefix"

SRC="$REPO/assets/SND0_source.wav"
OUT="$REPO/assets/SND0.AT3"

[ -f "$SRC" ] || { echo "missing $SRC" >&2; exit 1; }
for t in git cmake g++ make; do
    command -v "$t" >/dev/null || { echo "need $t" >&2; exit 1; }
done

mkdir -p "$WORK"

# --- libsndfile, static and stripped of codecs atracdenc does not need ---
if [ ! -f "$PREFIX/lib/libsndfile.a" ]; then
    echo ">> building libsndfile"
    [ -d "$WORK/libsndfile" ] || git clone --depth 1 --branch 1.2.2 \
        https://github.com/libsndfile/libsndfile.git "$WORK/libsndfile"
    cmake -B "$WORK/libsndfile/build" -S "$WORK/libsndfile" \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$PREFIX" \
        -DBUILD_SHARED_LIBS=OFF -DBUILD_PROGRAMS=OFF -DBUILD_EXAMPLES=OFF \
        -DBUILD_TESTING=OFF -DENABLE_EXTERNAL_LIBS=OFF -DENABLE_MPEG=OFF \
        -DENABLE_CPACK=OFF -Wno-dev >/dev/null
    cmake --build "$WORK/libsndfile/build" -j"$(nproc)" >/dev/null
    cmake --install "$WORK/libsndfile/build" >/dev/null
fi

# --- atracdenc. Needs its libgha submodule, which a plain shallow clone misses ---
#
# Pinned, because it is patched below and a moving HEAD would break that.
ATRACDENC_REV=2ee62a6c6dfe5f7a0a40fccdc0bf3dd4635d1d4e
if [ ! -x "$WORK/atracdenc/build/atracdenc" ]; then
    echo ">> building atracdenc"
    if [ ! -d "$WORK/atracdenc" ]; then
        git clone https://github.com/dcherednik/atracdenc.git "$WORK/atracdenc"
        git -C "$WORK/atracdenc" checkout -q "$ATRACDENC_REV"
    fi
    git -C "$WORK/atracdenc" submodule update --init --recursive --depth 1 >/dev/null
    # Without this the PSP will not play the result. See the patch header for why.
    git -C "$WORK/atracdenc" apply --check "$REPO/scripts/patches/atracdenc-psp-bands.patch" 2>/dev/null \
        && git -C "$WORK/atracdenc" apply "$REPO/scripts/patches/atracdenc-psp-bands.patch" \
        && echo ">> applied the PSP band-count patch"
    cmake -B "$WORK/atracdenc/build" -S "$WORK/atracdenc/src" \
        -DCMAKE_BUILD_TYPE=Release -DCMAKE_PREFIX_PATH="$PREFIX" \
        -DLIBSNDFILE_INCLUDE_DIR="$PREFIX/include" \
        -DSNDFILE_LIBRARIES="$PREFIX/lib/libsndfile.a" -Wno-dev >/dev/null
    cmake --build "$WORK/atracdenc/build" -j"$(nproc)" >/dev/null
fi

# RIFF is the container a PSP SND0.AT3 uses; `oma` and `rm` will not play in the XMB.
#
# LP4 (66 kbps), not LP2. The XMB would not play a 132 kbps file at all: every working SND0 on a
# real memory stick is 66144 bps with a block align of 192 and the joint-stereo flag set, and an
# LP2 file differs in all three. Verified by extracting SND0 from games whose music does play and
# diffing the format chunk.
#
# The mode is `atrac3_lp4`. atracdenc's own help calls it `atrac3_lp`, which the binary rejects.
#
# The source is low-passed first. The top QMF band starts at 44100/8*3 = 16537 Hz and the patch
# above stops it being coded at all, so anything up there would simply be discarded — better to
# filter it cleanly than to let the encoder spend bits on it.
echo ">> encoding"
BAND_LIMITED="$WORK/source-15k5.wav"
ffmpeg -y -v error -i "$SRC" -af "lowpass=f=15500" -c:a pcm_s16le "$BAND_LIMITED"
"$WORK/atracdenc/build/atracdenc" -e atrac3_lp4 --container riff -i "$BAND_LIMITED" -o "$OUT" 2>/dev/null

# Round-trip through ffmpeg's decoder: a file the PSP cannot play is usually one ffmpeg cannot
# read either, and it is cheap to catch that here rather than on the handheld.
if command -v ffprobe >/dev/null; then
    codec=$(ffprobe -v error -show_entries stream=codec_name -of default=nw=1:nk=1 "$OUT")
    dur=$(ffprobe -v error -show_entries format=duration -of default=nw=1:nk=1 "$OUT")
    rate=$(ffprobe -v error -show_entries stream=bit_rate -of default=nw=1:nk=1 "$OUT")
    [ "$codec" = "atrac3" ] || { echo "encoded to '$codec', expected atrac3" >&2; exit 1; }
    # 66144 is LP4. Anything else will sit silent in the XMB.
    [ "$rate" = "66144" ] || { echo "bitrate $rate, expected 66144 (LP4)" >&2; exit 1; }
    # The check that actually matters. A stream with a fourth coded band decodes fine here and
    # is silently refused by the PSP, so verify it frame by frame rather than trusting the build.
    python3 - "$OUT" <<'ENDPY'
import struct, sys
b = open(sys.argv[1], 'rb').read()
pos = 12
while pos + 8 <= len(b):
    cid = b[pos:pos+4]; size = struct.unpack('<I', b[pos+4:pos+8])[0]
    if cid == b'data':
        d = b[pos+8:pos+8+size]; break
    pos += 8 + size + (size & 1)
else:
    sys.exit("no data chunk")
bad = [i for i in range(len(d)//192) if (d[i*192] & 3) != 2]
if bad:
    sys.exit(f"{len(bad)} frame(s) code four QMF bands; the PSP will not play this")
print(f">> {len(d)//192} frames, all with bands_coded=2")
ENDPY
    echo ">> $OUT — $codec, ${rate}bps, ${dur}s, $(stat -c%s "$OUT") bytes"
else
    echo ">> $OUT — $(stat -c%s "$OUT") bytes (install ffmpeg to verify)"
fi
