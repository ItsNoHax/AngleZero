#!/usr/bin/env bash
# Regenerate assets/SND0.AT3 — the XMB background music — from assets/SND0_source.wav.
#
# ATRAC3 is Sony's codec and nothing packages an encoder for it: ffmpeg knows the format but
# ships decoders only. The encoder here is pspbuild's, a workspace dependency, through
# tools/anglezero-eboot. It writes what the XMB needs: RIFF/WAVE, LP4 (66 kbps) joint stereo,
# three QMF bands per frame (the XMB is silent on a frame that codes four), and `fact` and `smpl`
# chunks so the music loops rather than stopping after one pass. It refuses to write a file that
# its own validator finds anything wrong with, a missing loop point included.
#
# The output is committed, so this only needs running if the source loop changes.
#
# Usage: scripts/encode_music.sh

set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

SRC="assets/SND0_source.wav"
OUT="assets/SND0.AT3"

[ -f "$SRC" ] || { echo "missing $SRC" >&2; exit 1; }

cargo run --release -q -p anglezero-eboot --bin anglezero-snd0 -- "$SRC" "$OUT"
