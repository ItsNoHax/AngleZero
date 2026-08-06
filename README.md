# Angle Zero

A night downhill drift game for the PSP, written in Rust with the [`psp`](https://crates.io/crates/psp)
a mesh. Gravity does the driving — the car builds speed downhill with no throttle at all — and
points come from holding a slide, with the guard rails there to take them away again.

One car, one 3.5 km switchback descent of Sekira Pass, eleven hairpins, 177 m of drop. Gravity does
the driving; you score points for holding slides. Title → run → results.

Note that PSP colours are **ABGR**, not ARGB — `0xff4040e0` is R=0xe0, so it renders red.

## Layout

The crate is split so that almost all of the game can be tested on a normal machine:

| Path | What it is |
|---|---|
| `src/*.rs` | `no_std`, target-agnostic game core — track, physics, scoring, camera, screen flow, HUD numbers, mesh building. No PSP dependency. |
| `src/psp/` | The PSP shell: GU bring-up, controller, renderer, 2D overlay. Only compiled for `target_os = "psp"`. |
| `tests/` | Host tests. Everything in `src/*.rs` is exercised here. |
| `scripts/` | Music encoding, and pulling captures off a console. |

The `psp` crate is a target-specific dependency, so `cargo test` never builds it, and `src/main.rs`
compiles to an empty `main` off-target. That is what makes `cargo test` work at all.

## Requirements

| Tool | Purpose |
|---|---|
| Rust nightly + `rust-src` | Pinned by [`rust-toolchain.toml`](rust-toolchain.toml); rustup installs it automatically |
| [`cargo-psp`](https://github.com/overdrivenpotato/rust-psp) | Builds the `EBOOT.PBP` / `.prx` |
| PPSSPP (Flatpak) | Interactive runs |
| `PPSSPPHeadless` | Scripted screenshot capture with no window (see below) |

```bash
rustup component add rust-src
cargo install cargo-psp
```

The nightly channel is pinned in `rust-toolchain.toml`. The `psp` crate requires nightly ≥ 2026-05-30.

## Testing

```bash
cargo test
```

Runs on the host, no emulator and no network involved.

The track, the vehicle model, scoring, the camera, the screen flow, the mesh builder and the save
format all have their own suite. The two worth knowing about are `tests/track_query.rs`, which
covers the nearest-node queries that containment, gravity and scoring all go through, and
`tests/stability.rs`, which drives the car hard enough to catch a model that blows up rather than
one that merely handles badly.

## Building

```bash
cargo psp
```

Artifacts land in `target/mipsel-sony-psp/debug/`:

- `angle-zero.EBOOT.PBP` — what a real PSP or the PPSSPP GUI runs
- `angle-zero.prx` — what `PPSSPPHeadless` runs

Use `cargo psp --release` for hardware: it is a fraction of the size, since the debug artifact is
mostly debug info, and about five times quicker per frame.

The build prints `rust-lld: ... linking abicalls code with non-abicalls code` and
`relocation refers to a discarded section` warnings. These are a known upstream issue
([rust-psp#203](https://github.com/overdrivenpotato/rust-psp/issues/203)) — recent Rust nightlies
stopped suppressing pre-existing linker noise on this target. The output runs correctly. They are
deliberately *not* silenced with `#![allow(linker_messages)]`, so that genuine linker problems stay
visible.

## Controls

| Action | Button |
|---|---|
| Throttle | ✕ (or D-pad Up) |
| Brake | □ (or D-pad Down) |
| Handbrake | ○ |
| Steer | D-pad Left/Right, or the analog nub |
| Reset to road | △ |
| Start run / restart from results | ✕ / △ |

Best time, score and combo persist to `ms0:/PSP/SAVEDATA/ANGLEZERO/RECORD.BIN` and show on the
title and results screens.

## Two traps worth knowing about

Both of these cost real debugging time, and neither fails loudly.

**`sceGumLookAt` does nothing in rust-psp 0.3.13.** Its helper `gum_look_at` shadows its own
`&mut` output parameter with a local:

```rust
let mut mat = gum_mult_matrix(mat, &t);   // new local, not the caller's matrix
gum_translate(&mut mat, &ieye);
```

so the caller's matrix is never written and the view matrix stays identity. The world still draws —
it is rendered with an identity model matrix — but everything is positioned as though the camera
sat at the world origin, and anything with a model transform (the car) lands somewhere else
entirely or off-screen. `src/math.rs` builds the view matrix instead, checked by `tests/matrix.rs`,
and uploads it with `sceGumLoadMatrix`. That matrix must be 16-byte aligned or the VFPU's `lv.q`
faults.

Related: rust-psp creates its VFPU matrix context lazily, but only inside `sceGumLoadIdentity` and
`sceGumLoadMatrix`. Every other `sceGum*` entry point calls `get_context_unchecked`, which hits an
`unreachable` — surfacing as a bare break instruction, not a panic message. `psp_main` touches
`sceGumLoadIdentity` once during setup so later code can start with `sceGumMatrixMode`.

**The GE reads vertex data asynchronously.** `sceGumDrawArray` only queues a pointer; the hardware
does not read it until the list is kicked at `sceGuFinish`/`sceGuSync`. Building vertices in a
stack local, or reusing one static buffer for several draws in a frame, is therefore a
use-after-free that PPSSPP often survives and hardware will not. Everything dynamic goes through
the bump arena in `src/psp/scratch.rs`, which lives for the whole frame and is flushed out of the
data cache before the list runs.

## Running interactively

```bash
flatpak run org.ppsspp.PPSSPP "$PWD/target/mipsel-sony-psp/debug/angle-zero.EBOOT.PBP"
```

The Flatpak sandbox cannot read arbitrary paths by default. Grant it access to this project once:

```bash
flatpak override --user --filesystem="$PWD" org.ppsspp.PPSSPP
```

Without this, PPSSPP fails to load the EBOOT and you would have to copy it into the emulator's
memory-stick directory instead.

## Headless screenshots

`PPSSPPHeadless` boots a `.prx` and renders with a deterministic software rasteriser. No window, no
X server, no GPU.

Capture is **pull-based**: `--screenshot-save` only writes a file when the emulated program asks it
to, by calling `sceIoDevctl("emulator:", 0x20, ...)` — `EMULATOR_DEVCTL__EMIT_SCREENSHOT`. That is
the `emit_screenshot()` helper in [`src/psp/mod.rs`](src/psp/mod.rs), called every 30 frames. On
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
the guest genuinely sees the press. [`scripts/psp_input.py`](scripts/psp_input.py) wraps this and
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

## Where this departs from the design

The original is the source of truth where the two disagree, and in three places they do.

- **Top speed.** The design asks for ~200 km/h on the straights. The force constants
  cannot produce it: engine force floors at `0.08 × 8200 N` by 53 m/s while aero drag alone is past
  3000 N, giving a terminal velocity of ~168 km/h even on this track's steepest sustained slope —
  and the longest straight here is 60 m. The original peaks at 128 km/h on a clean full-throttle
  run, and so does this port.
- **Hairpin count.** The design says ten. The section plan it publishes has eleven sections at
  `|curvature| ≥ 8.6°` over 13 steps, each sweeping 112–138°.
- **"Left normal".** The design calls `(−dz, dx)` the left normal. In the original's right-handed, +Y-up
  frame it points to the car's *right*. The formula is kept exactly as the original has it, since
  every downstream sign convention is built on it; only the name is corrected.

## Where it stands against the design

Everything in the design is implemented: track, physics, scoring, screens, HUD and minimap, the
night look, skid marks and tyre smoke, the roadside and pull-off props, engine and tyre audio, and
saved records.

Two things are approximations rather than omissions, both noted where they are done:

- The sky is a vertical gradient plus geometry — a camera-following starfield, a moon and a
  mountain ring — rather than a single 2048×1024 painted texture. It costs a few thousand
  vertices instead of a texture upload, and the stars do not slide when the camera turns.
- Tyre stacks on corners are thinned from every third node to every fifteenth, and use six-sided
  cylinders. At 2620 nodes the literal reading is several thousand cylinders; the wall still reads
  as continuous.

Still missing: the additive ground pools under the street lamps and in the pull-off, and rail-impact
audio (the design lists that one as optional).

## Performance

Measured in PPSSPP with the emulated microsecond clock, over a full-throttle descent. This covers
the CPU side — simulation plus building the display list — and not GE rasterisation, which is a
separate unit and the thing this cannot measure from here.

| Build | Peak frame cost | Budget at 30 fps |
|---|---|---|
| debug | 7.7 ms | 33 ms |
| release | 1.4 ms | 33 ms |

Static allocation is ~3.4 MB of `.bss`, against the PSP's 24 MB. Nothing is allocated per frame:
the effect pools are fixed-size ring buffers and every dynamic vertex comes from a frame-lived
arena with a known ceiling.
