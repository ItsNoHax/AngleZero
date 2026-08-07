# Building and running

Everything needed to get a build onto a PSP or into an emulator.

## Requirements

| Tool | Purpose |
|---|---|
| Rust nightly + `rust-src` | Pinned by [`rust-toolchain.toml`](../rust-toolchain.toml); rustup installs it automatically |
| [`cargo-psp`](https://github.com/overdrivenpotato/rust-psp) | Builds the `EBOOT.PBP` / `.prx` |
| PPSSPP (Flatpak) | Interactive runs |
| `PPSSPPHeadless` | Scripted screenshot capture with no window (see below) |

```bash
rustup component add rust-src
cargo install cargo-psp
```

The nightly channel is pinned in `rust-toolchain.toml`. The `psp` crate requires nightly ≥ 2026-05-30.

## Building

```bash
cargo psp
```

Artifacts land in `target/mipsel-sony-psp/debug/`:

- `angle-zero.EBOOT.PBP` — what a real PSP or the PPSSPP GUI runs
- `angle-zero.prx` — what `PPSSPPHeadless` runs

Use `cargo psp --release` for hardware: it is a fraction of the size, since the debug artifact is
mostly debug info, and about five times quicker per frame.

Add `--features devtools` for the on-device diagnostics and the headless screenshot hook. It is off
by default, so a shipping build carries neither — see
[Inspecting it on real hardware](#inspecting-it-on-real-hardware).

The build prints `rust-lld: ... linking abicalls code with non-abicalls code` and
`relocation refers to a discarded section` warnings. These are a known upstream issue
([rust-psp#203](https://github.com/overdrivenpotato/rust-psp/issues/203)) — recent Rust nightlies
stopped suppressing pre-existing linker noise on this target. The output runs correctly. They are
deliberately *not* silenced with `#![allow(linker_messages)]`, so that genuine linker problems stay
visible.

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

## Cutting a release

```bash
scripts/release.sh
```

Builds without `devtools`, runs the tests, and writes `dist/AngleZero.<version>.zip`. The version
comes from `Cargo.toml` unless you pass one (`scripts/release.sh 0.2.0`).

Before it packages anything it greps the built `.prx` for strings that only exist when `devtools`
is on — the render-mode labels, `ms0:/ANGLEZERO/`, the diagnostic filenames — and refuses to
continue if it finds any. "Off by default" is a promise the build makes, not one it checks, and a
release carrying the capture tooling and debug overlay would be easy to produce by accident.

The archive holds a single file:

```
PSP/GAME/AngleZero/EBOOT.PBP
```

Unzip it at the root of a memory stick and the game is where the XMB looks for it.
