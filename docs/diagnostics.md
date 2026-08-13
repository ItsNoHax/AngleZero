# Diagnostics

Getting evidence off the console, and out of a headless emulator.

## Inspecting it on real hardware

The diagnostics live behind the `devtools` feature, which is **off by default** — a shipping build
carries none of it, and does not sit there issuing an emulator devctl twice a second. Build with it
when you need it:

```bash
cargo psp --release --features devtools
```

A feature rather than `debug_assertions`, because this tooling is most useful *on hardware*, and
hardware builds are release builds: the debug binary is 7.6 MB against 460 KB.

PPSSPP's software rasteriser is far more forgiving than the GE — no cache, no 16-bit depth buffer,
no display list to overrun — so some faults only appear on a PSP. With the feature on, the game
captures its own evidence:

- **START** toggles a counter readout.
- **SELECT** writes the current frame and those counters to `ms0:/ANGLEZERO/`.

Put the PSP into USB mode (Settings → USB Connection) and run:

```bash
scripts/psp_pull.sh
```

It finds the memory stick, converts the frames to PNG in `captures/`, and prints the counters.

`SCF` is the field to watch: refused vertex-arena allocations. Any non-zero value means draws were
silently dropped, and it turns red. `LST` is display-list bytes against the 1 MB buffer. Together
they distinguish the two silent failure modes — an exhausted arena and an overrun list — which both
look like flickering geometry rather than like an error.

## Headless screenshots

`PPSSPPHeadless` boots a `.prx` and renders with a deterministic software rasteriser. No window, no
X server, no GPU.

Capture is **pull-based**: `--screenshot-save` only writes a file when the emulated program asks it
to, by calling `sceIoDevctl("emulator:", 0x20, ...)` — `EMULATOR_DEVCTL__EMIT_SCREENSHOT`. That is
the `emit_screenshot()` helper in [`src/psp/mod.rs`](../src/psp/mod.rs), called every 30 frames. On
real hardware the devctl just fails harmlessly. Headless does **not** capture anything on its
`--timeout` path, so a program that never emits produces no file at all.

### One-time setup

The PPSSPP Flatpak does not ship the headless binary, so build it from source. `PPSSPP_SRC` below is
just where you want the checkout to live — pick anywhere:

```bash
export PPSSPP_SRC="$HOME/.local/src/ppsspp"

sudo apt install -y build-essential cmake ninja-build libgl1-mesa-dev libglu1-mesa-dev \
    libvulkan-dev libsdl3-dev libsdl3-ttf-dev
git clone --recurse-submodules --shallow-submodules --depth 1 \
    https://github.com/hrydgard/ppsspp.git "$PPSSPP_SRC"
cd "$PPSSPP_SRC"
cmake -B build-headless -G Ninja -DCMAKE_BUILD_TYPE=Release -DHEADLESS=ON -Wno-dev
cmake --build build-headless --target PPSSPPHeadless -j"$(nproc)"
```

The checkout is ~2 GB and the build takes roughly 15–30 minutes. Two dependency gotchas: current
PPSSPP requires **SDL3** (`libsdl3-dev`), not SDL2 — older build guides are out of date — and the
bundled GLEW needs `GL/glu.h`, which is in `libglu1-mesa-dev`, *not* in `libgl1-mesa-dev`.

The commands below resolve the binary through `PPSSPP_HEADLESS`, so export it (in your shell profile
if you want it to persist) or accept the default:

```bash
export PPSSPP_HEADLESS="${PPSSPP_SRC:-$HOME/.local/src/ppsspp}/build-headless/PPSSPPHeadless"
```

### Capturing a frame

```bash
"$PPSSPP_HEADLESS" \
    --graphics=software \
    --screenshot-save=/tmp/psp.bmp \
    --timeout=15 \
    target/mipsel-sony-psp/debug/angle-zero.prx
```

`--graphics=software` gives byte-identical output across runs, which makes screenshots suitable for
regression comparison. `--timeout` is required for a program with an infinite main loop, otherwise
headless never returns. Give it enough headroom to reach an emit — the software rasteriser is slow
now that there is a real scene, and a debug build needs ~15 s to render the first 30 frames.

The BMP is 512×272 — the framebuffer stride — with the right-hand 32 px unused. Crop to the visible
480×272 while converting to PNG:

```bash
ffmpeg -y -i /tmp/psp.bmp -vf crop=480:272:0:0 -update 1 /tmp/psp.png
```

### Capturing with a button held

Headless has no input device, but `--debugger=<port>` starts PPSSPP's WebSocket debugger, whose
`input.buttons.send` call routes into the same `__CtrlUpdateButtons` HLE path real input uses. So
the guest genuinely sees the press. [`scripts/psp_input.py`](../scripts/psp_input.py) wraps this and
has no third-party dependencies:

```bash
"$PPSSPP_HEADLESS" \
    --graphics=software --debugger=9333 \
    --screenshot-save=/tmp/cross.bmp --timeout=16 \
    target/mipsel-sony-psp/debug/angle-zero.prx &
python3 scripts/psp_input.py 9333 cross
```

It accepts several buttons at once (`psp_input.py 9333 cross circle left`). The script holds them
until killed, which matters: `--screenshot-save` is overwritten by every emit, so releasing early
lets a later idle frame replace the held one. Let headless reach its own `--timeout` with the
buttons still down. `--debugger` also implies `startBreak`, so the script sends `cpu.resume` before
pressing anything.

Claude Code users: the `/psp-preview` skill wraps this whole flow.

## Hunting flicker automatically

A single screenshot cannot show flicker: the artifact only exists as a difference between consecutive
frames. `--screenshot-save` overwrites one file, so it cannot show it either.

The `harness` feature turns a run into something comparable frame to frame. It fixes the frame delta
at 1/60 s, so nothing depends on the clock or on how long the host took to write the last capture;
replays input from a script keyed to the frame counter rather than to wall-clock seconds; captures
*consecutive* frames rather than every fourth; and exits when its script is done. Nothing else in the
game reads a clock or a random number, so two runs of the same script are byte-identical — which is
what makes a before/after comparison mean anything.

```bash
scripts/psp_glitch.py --node 1200 --burst 60 --frames 40 --label hairpin
```

About ten seconds end to end. It builds, runs headless, harvests the frames the guest wrote to
`ms0:/ANGLEZERO/` (a host directory under headless), and reports two independent things:

- **A pixel comparison.** A tile that differs from the frame before *and* the frame after, while
  those two agree, is a one-frame blink. Two filters keep ordinary motion out of it: a shift search,
  because a lamp post passing close by sweeps wider than itself in a frame and otherwise looks
  exactly like a blink; and a bracket test, because a pixel in a smoothly moving scene stays between
  what it was and what it will be, even when the motion is accelerating. Treat its output as leads,
  not verdicts — it still flags the moment one lamp's pool hands over to the next.
- **The draw tally**, from `trace.rs`. This one is exact, because it records the draw call rather
  than its result: holes in the road and terrain chunk sets, any chunk set that blinked, a refused
  vertex-arena allocation, a display list near its 1 MB buffer.

When the two disagree, the tally wins. Identical masks and vertex counts across the frames either
side of a blink is what tells you nothing was dropped and the fault is downstream of submission.

`--node` drops the car anywhere on the centreline, so a corner two thirds of the way down can be
looked at without driving there and surviving every corner in between. It *replaces* the input
script rather than adding to it, so `--hold` is ignored whenever it is given — a run that has to be
braking when the burst starts needs its `SCRIPT.TXT` written by hand. `--mode N` runs under a
`render::DEBUG_MODES` override, which is how a cause gets narrowed down: run the same frames with one
suspect removed and see whether the artifact survives. Modes 9 to 12 exist for exactly that — they
drop the headlight beams, the car's lamp glows, the effects, and the roadside light pools, which all
land on the road on top of each other.

Removing a pass is also how to find out what it *paints*, which is a different question from whether
it was submitted, and the more useful one. Both halves of the vehicle lighting were being submitted
every frame — `LMP` and `BM` in the overlay, `lamps` and `beams` in the trace, all non-zero — while
between them they lit under eight hundred pixels of a 480x272 screen: the beams were buried under
the road they lay on, and the glows were inside the bodywork they belonged to. Diffing one
deterministic frame at `--mode 9` and `--mode 10` against `--mode 0` said so exactly, one run each,
after a good deal of staring at screenshots had not.

Mode 15 is the odd one out: it adds rather than removes. Every lamp on every car burns at once and
from both sides, whatever the driver is doing, which is how to check that a newly imported car's
lamps came out on the panels they belong to.
