# Building and running

## Requirements

| Tool | Purpose |
|---|---|
| Rust nightly + `rust-src` | Pinned by [`rust-toolchain.toml`](../rust-toolchain.toml); rustup installs it automatically |
| [`cargo-psp`](https://github.com/ItsNoHax/rust-psp) | Builds the `EBOOT.PBP` / `.prx` |
| PPSSPP (Flatpak) | Interactive runs |
| `PPSSPPHeadless` | Scripted screenshots; see [Diagnostics](diagnostics.md#headless-screenshots) |

```bash
rustup component add rust-src
cargo +nightly-2026-08-26 install --git https://github.com/ItsNoHax/rust-psp --rev a89142b237f3014fc15425d3549e44d3aa07d1c6 cargo-psp --locked
```

The nightly channel is pinned in `rust-toolchain.toml`, and the `psp` crate comes from the
project's rust-psp fork at the same revision. Both match SSB64PSP, so one installed `cargo-psp`
builds either project; change them together.

## Building

```bash
cargo psp                      # debug
cargo psp --release            # use this for hardware: smaller and ~5× faster per frame
cargo psp --features devtools  # on-device diagnostics, see Diagnostics
```

Output in `target/mipsel-sony-psp/<profile>/`:

| File | Used by |
|---|---|
| `angle-zero.EBOOT.PBP` | PSP, PPSSPP GUI |
| `angle-zero.prx` | `PPSSPPHeadless` |

The workspace also contains the host-only car compiler, `tools/anglezero-asset`. It is not a
default member, so `cargo psp` never builds it. Use `-p anglezero-asset` or `--workspace`.

### Features

| Feature | Effect |
|---|---|
| `devtools` | Debug overlay, render-mode overrides, SELECT-to-capture, headless screenshot hook |
| `harness` | Deterministic scripted run for the glitch hunt. Implies `devtools`. Ignores the controller; never ship |

### Linker warnings

The build prints `linking abicalls code with non-abicalls code` and `relocation refers to a
discarded section`. These are a known upstream issue
([rust-psp#203](https://github.com/overdrivenpotato/rust-psp/issues/203)) and harmless. They are
intentionally not silenced so real linker problems stay visible.

## Testing

```bash
cargo test               # game crate
cargo test --workspace   # game crate and asset compiler
```

## Running in PPSSPP

```bash
flatpak override --user --filesystem="$PWD" org.ppsspp.PPSSPP   # once, so the sandbox can read the build
flatpak run org.ppsspp.PPSSPP "$PWD/target/mipsel-sony-psp/debug/angle-zero.EBOOT.PBP"
```

Cars are always read from `ms0:/PSP/GAME/AngleZero/CARS/`, wherever the EBOOT is. Copy them into
the emulator's memory stick directory:

```bash
MS=~/.var/app/org.ppsspp.PPSSPP/config/ppsspp   # Flatpak default; adjust for other installs
mkdir -p "$MS/PSP/GAME/AngleZero/CARS"
cp assets/compiled/*.azcar "$MS/PSP/GAME/AngleZero/CARS/"
```

Without cars the game still runs and says so on the title screen.

## Controls

| Action | Button |
|---|---|
| Throttle | ✕ or D-pad Up |
| Brake | □ or D-pad Down |
| Handbrake | ○ |
| Steer | D-pad Left/Right or analog nub |
| Look at the front of the car | △ (hold) |
| Pause | START |

| Screen | Button | Action |
|---|---|---|
| Title | D-pad Left/Right | Change car |
| Title | ✕ | Start run |
| Pause | D-pad Up/Down, ✕ | Continue / Restart / Select another car |
| Pause | START | Resume |
| Results | ✕ | Run again |
| Results | □ | Back to title |

A newly selected car streams in over a few frames; its silhouette and a progress bar stand in until
it arrives. Pressing ✕ during the load finishes it immediately.

Best time, score and combo are saved to `ms0:/PSP/SAVEDATA/ANGLEZERO/RECORD.BIN`.

## Releasing

```bash
scripts/release.sh          # version from Cargo.toml
scripts/release.sh 0.4.0    # explicit version
```

The script:

1. Builds `--release` without `devtools`.
2. Runs `cargo test`.
3. Refuses to continue if the `.prx` contains devtools-only strings (`ms0:/ANGLEZERO/`, render-mode
   labels, diagnostic filenames).
4. Refuses to continue if `assets/compiled/` has no cars.
5. Writes `dist/AngleZero.<version>.zip`:

```
PSP/GAME/AngleZero/EBOOT.PBP
PSP/GAME/AngleZero/CARS/*.azcar
```
