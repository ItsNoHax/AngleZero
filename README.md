# Angle Zero

A car drifting game for the PSP, written in Rust with the [`psp`](https://crates.io/crates/psp)
crate.

Early days — there is no game yet. What exists is the render + input scaffolding: the screen clears
to a colour that depends on which face button is held (idle = navy, ❌ = red, ⭕ = green), which
exercises the GU setup, the controller read loop, and the headless capture pipeline below.

Note that PSP clear colours are **ABGR**, not ARGB — `0xff4040e0` is R=0xe0, so it renders red.

## Requirements

| Tool | Purpose |
|---|---|
| Rust nightly + `rust-src` | Pinned by [`rust-toolchain.toml`](rust-toolchain.toml); rustup installs it automatically |
| [`cargo-psp`](https://github.com/overdrivenpotato/rust-psp) | Builds the `EBOOT.PBP` / `.prx` |
| PPSSPP (Flatpak) | Interactive runs |
| `PPSSPPHeadless` | Scripted screenshot capture with no window (see below) |

### Toolchain and build tool

```bash
rustup component add rust-src
cargo install cargo-psp
```

The nightly channel itself is pinned in `rust-toolchain.toml`, so rustup fetches the right one on
first build. The `psp` crate requires nightly ≥ 2026-05-30.

## Building

```bash
cargo psp
```

Artifacts land in `target/mipsel-sony-psp/debug/`:

- `EBOOT.PBP` — what a real PSP or the PPSSPP GUI runs
- `angle-zero.prx` — what `PPSSPPHeadless` runs

The build prints `rust-lld: ... linking abicalls code with non-abicalls code` and
`relocation refers to a discarded section` warnings. These are a known upstream issue
([rust-psp#203](https://github.com/overdrivenpotato/rust-psp/issues/203)) — recent Rust nightlies
stopped suppressing pre-existing linker noise on this target. The output runs correctly. They are
deliberately *not* silenced with `#![allow(linker_messages)]`, so that genuine linker problems stay
visible.

## Running interactively

```bash
flatpak run org.ppsspp.PPSSPP "$PWD/target/mipsel-sony-psp/debug/EBOOT.PBP"
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
the `emit_screenshot()` helper in [`src/main.rs`](src/main.rs), called every 60 frames. On real
hardware the devctl just fails harmlessly. Headless does **not** capture anything on its
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
    --timeout=3 \
    target/mipsel-sony-psp/debug/angle-zero.prx
```

`--graphics=software` gives byte-identical output across runs, which makes screenshots suitable for
regression comparison. `--timeout` is required for a program with an infinite main loop, otherwise
headless never returns.

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
    --screenshot-save=/tmp/cross.bmp --timeout=10 \
    target/mipsel-sony-psp/debug/angle-zero.prx &
python3 scripts/psp_input.py 9333 cross
```

The script holds the button until killed, which matters: `--screenshot-save` is overwritten by every
emit, so releasing early lets a later idle frame replace the held one. Let headless reach its own
`--timeout` with the button still down. `--debugger` also implies `startBreak`, so the script sends
`cpu.resume` before pressing anything.

Claude Code users: the `/psp-preview` skill wraps this whole flow.
