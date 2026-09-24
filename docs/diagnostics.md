# Diagnostics

Tooling for faults that only appear on hardware, and for reproducible captures under emulation.
All of it is behind the `devtools` feature, which is off by default and rejected by
`scripts/release.sh`.

## On-device

```bash
cargo psp --release --features devtools
```

Use a release build: hardware faults are what this is for, and the debug binary is ~7.6 MB.

### Controls

| Button | Action |
|---|---|
| R | Toggle the counter overlay |
| L | Cycle render mode (see [Render modes](#render-modes)) |
| SELECT | Capture frame + counters to `ms0:/ANGLEZERO/`. Hold for a burst (every 4th frame) |

### Retrieving captures

Put the PSP in USB mode (**Settings → USB Connection**), then:

```bash
scripts/psp_pull.sh [dest]   # default: captures/
```

Frames are converted to PNG and the counters are printed.

### Counters

| Field | Meaning |
|---|---|
| `US`, `AVG`, `PK` | Frame time in µs: current, rolling average, peak (after the first 90 frames) |
| `LST` | Display-list bytes used, of 1 MB |
| `SCR` | Peak frame-arena use |
| `SCF` | Refused frame-arena allocations. Non-zero (shown red) means draws were dropped |
| `CAR` | Car draw calls |
| `LMP`, `BM` | Lamp glows and headlight beams submitted |
| `SK`, `SM` | Live skid marks and smoke puffs |
| `SHOT` | Frames captured this session |

The title screen adds a line top-left:

| Field | Meaning |
|---|---|
| `RD PK` | Longest single chunk read while loading a car, in µs |
| `ARENA` | Car arena in use / total, in KB |

An exhausted arena (`SCF`) and an overrun list (`LST`) both present as flickering geometry, not as
an error.

`RD PK` can only be measured on hardware (the emulator reads from the host filesystem). It is the basis for `CHUNK_BYTES` in `src/psp/car.rs` (32 KB).
Reference: **4,438 µs** per 32 KB chunk, consistently (~7.4 MB/s, 27 % of a 60 Hz frame, a full car
in ~29 frames). Values above ~16,000 µs mean a chunk costs a whole frame; unstable values suggest
seeks. Re-measure on a different stick or after changing `CHUNK_BYTES`. `ARENA` must stay at one car
while cycling cars; growth means residency is leaking.

### Render modes

| Mode | Effect |
|---|---|
| 0 | Normal |
| 1 | No culling |
| 2 | No depth test |
| 3 | No fog |
| 4 | No culling, depth or fog |
| 5 | No sky |
| 6 | Road only |
| 7 | Terrain only |
| 8 | No HUD |
| 9 | No headlight beams |
| 10 | No lamp glows |
| 11 | No effects |
| 12 | No roadside light pools |
| 13 | Four different cars (benchmark) |
| 14 | Eight different cars (benchmark) |
| 15 | Every lamp on every car lit |

Modes 1–12 each remove one suspect: if the fault disappears, that is the cause. Removing a pass and
diffing against mode 0 also shows exactly what that pass paints. Modes 13–14 load extra cars into
spare arena slots (a stall of about a second) so that switching between models is part of the
measurement. Mode 15 checks that a new car's lamps sit on the right panels.

## Headless screenshots

`PPSSPPHeadless` runs a `.prx` with a deterministic software rasteriser: no window, X server or GPU.

Capture is pull-based. `--screenshot-save` writes only when the guest calls
`sceIoDevctl("emulator:", 0x20, …)`. A `devtools` build does this every 30 frames
(`emit_screenshot()` in `src/psp/mod.rs`); on hardware the call fails harmlessly. Nothing is
captured on `--timeout`.

### Setup

The Flatpak does not include the headless binary. Build it (~2 GB checkout, 15–30 min):

```bash
export PPSSPP_SRC="$HOME/.local/src/ppsspp"

sudo apt install -y build-essential cmake ninja-build libgl1-mesa-dev libglu1-mesa-dev \
    libvulkan-dev libsdl3-dev libsdl3-ttf-dev
git clone --recurse-submodules --shallow-submodules --depth 1 \
    https://github.com/hrydgard/ppsspp.git "$PPSSPP_SRC"
cd "$PPSSPP_SRC"
cmake -B build-headless -G Ninja -DCMAKE_BUILD_TYPE=Release -DHEADLESS=ON -Wno-dev
cmake --build build-headless --target PPSSPPHeadless -j"$(nproc)"

export PPSSPP_HEADLESS="$PPSSPP_SRC/build-headless/PPSSPPHeadless"
```

Current PPSSPP requires SDL3, not SDL2. The bundled GLEW needs `GL/glu.h` from `libglu1-mesa-dev`.

### Capture a frame

```bash
cargo psp --features devtools
"$PPSSPP_HEADLESS" --graphics=software --screenshot-save=/tmp/psp.bmp --timeout=15 \
    target/mipsel-sony-psp/debug/angle-zero.prx
ffmpeg -y -i /tmp/psp.bmp -vf crop=480:272:0:0 -update 1 /tmp/psp.png
```

- `--graphics=software` gives byte-identical output across runs.
- `--timeout` is required (the main loop never exits). A debug build needs ~15 s to reach its first
  emit.
- The BMP is 512 × 272 (framebuffer stride); crop to 480 × 272.

### Capture with buttons held

`--debugger=<port>` enables PPSSPP's WebSocket debugger. `scripts/psp_input.py` (no dependencies)
sends `input.buttons.send`, which the guest sees as real input:

```bash
"$PPSSPP_HEADLESS" --graphics=software --debugger=9333 \
    --screenshot-save=/tmp/cross.bmp --timeout=16 \
    target/mipsel-sony-psp/debug/angle-zero.prx &
python3 scripts/psp_input.py 9333 cross          # several allowed: cross circle left
```

The script resumes the CPU (`--debugger` starts paused) and holds the buttons until killed. Let
headless hit its timeout with the buttons still held, or a later idle frame overwrites the capture.

The `/psp-preview` Claude Code skill wraps this flow.

## Glitch hunting

Flicker exists only between consecutive frames, which a single screenshot cannot show. The
`harness` feature makes a run deterministic:

- fixed 1/60 s frame delta;
- input replayed from a script keyed to frame number;
- consecutive frames captured to `ms0:/ANGLEZERO/`;
- exits when the script ends.

Two runs of the same script are byte-identical.

```bash
scripts/psp_glitch.py --node 1200 --burst 60 --frames 40 --label hairpin
```

| Option | Effect |
|---|---|
| `--burst N` | Frame at which capture starts (default 400) |
| `--frames N` | Consecutive frames to capture (default 40) |
| `--hold 'F BUTTONS'` | Script line, repeatable, e.g. `--hold '90 x' --hold '400 xl'` |
| `--node N` | Place the car at centreline node N (0–2620). Replaces the script, so `--hold` is ignored |
| `--kph N` | Speed when placed with `--node` (default 90) |
| `--mode N` | Run under a [render mode](#render-modes) |
| `--label S` | Output directory under `captures/glitch/` |
| `--no-build` | Reuse the existing build |
| `--scan-only DIR` | Re-analyse an already harvested run |

Runs in about ten seconds and reports:

- **Pixel comparison.** Flags tiles that differ from both neighbouring frames while those agree.
  Shift search and bracket tests filter out ordinary motion. Treat results as leads.
- **Draw tally** (from `src/psp/trace.rs`). Exact: missing road or terrain chunks, blinking chunk
  sets, refused arena allocations, display list near capacity.

When they disagree, trust the tally. Identical masks and vertex counts on both sides of a blink mean
nothing was dropped and the fault is downstream of submission.

The `/psp-glitch` Claude Code skill wraps this flow.
