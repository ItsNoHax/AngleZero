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
