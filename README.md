# Angle Zero

A night-time downhill drift game for the Sony PSP, written in Rust.

One car, one road: 3.5 km of switchbacks down Sekira Pass, generated at boot rather than shipped as
a mesh. Gravity does the driving — the car builds speed downhill with no throttle at all — and
points come from holding a slide, with the guard rails there to take them away again.

![The road down Sekira Pass](docs/screenshot.png)

## Quick start

```bash
cargo psp --release
```

Copy `target/mipsel-sony-psp/release/angle-zero.EBOOT.PBP` to `PSP/GAME/AngleZero/EBOOT.PBP` on a
memory stick, renaming it to exactly `EBOOT.PBP`. It needs custom firmware; stock firmware will not
launch unsigned homebrew.

To run the whole test suite, which needs no PSP and no emulator:

```bash
cargo test
```

## Documentation

| | |
|---|---|
| [Building and running](docs/building.md) | Toolchain, build flags, controls, running under PPSSPP |
| [Architecture](docs/architecture.md) | The crate split, and how the game is tested on the host |
| [PSP hardware notes](docs/psp-notes.md) | Traps the hardware sets that emulators do not, and where the frame budget goes |
| [Diagnostics](docs/diagnostics.md) | Capturing frames and traces from the console, and headless screenshots |
| [Assets](docs/assets.md) | The XMB icon, background and music, and how the ATRAC3 is encoded |

## Layout

```
src/            game core — no PSP dependency, runs and is tested on the host
src/psp/        the shell: GU setup, rendering, audio, save data, diagnostics
tests/          158 tests, all host-side
scripts/        music encoding, pulling captures off a PSP
assets/         XMB icon, background, music, and the music's source
docs/           everything above
```

The split is the point: `src/` knows nothing about the PSP, so track generation, physics, scoring,
the camera and the screen flow are all ordinary Rust that `cargo test` exercises directly. Only
`src/psp/` needs hardware, and it holds no game logic. See
[Architecture](docs/architecture.md).
