# Architecture

Game logic is target-agnostic and tested on the host. Only a thin shell touches the PSP.

## Layout

| Path | Contents |
|---|---|
| `src/*.rs` | `no_std` game core: track, vehicle physics, scoring, camera, screen flow, HUD values, mesh building, lighting, `.azcar` format, car catalogue, load streaming, save format |
| `src/psp/` | PSP shell: GU setup, renderer, controller, audio, save I/O, car loading, diagnostics. Compiled only for `target_os = "psp"` |
| `tools/anglezero-asset/` | Host-only car compiler and `azview` renderer. See [Cars](cars.md) |
| `tests/` | Host tests for everything in `src/*.rs` |
| `scripts/` | Release, car builds, music encoding, glitch hunt, capture retrieval, asset checks |
| `assets/` | XMB assets, car configs and compiled cars. See [Assets](assets.md) |
| `.claude/skills/` | Agent workflows: `add-car`, `psp-preview`, `psp-glitch`, `psp-deploy` |

The `psp` crate is a target-specific dependency and `src/main.rs` compiles to an empty `main`
off-target, so `cargo test` never builds PSP code.

## Boundary

The shell holds no game logic. Anything that can be expressed as arithmetic lives in the core,
even when it looks like rendering or I/O:

| Concern | Core (tested) | Shell |
|---|---|---|
| Car selection | `catalogue.rs`: which cars exist, sort order, naming | Directory scan |
| Car loading | `stream.rs`: chunk counts, progress | File handle, arena |
| Vehicle lights | `lights.rs`: which lamps are lit, intensity, world-space placement | Two additive draw passes |
| Asset format | `azcar.rs`: parsing and validation | Hands buffers to the GE |

## Tests

```bash
cargo test               # game crate
cargo test --workspace   # plus the asset compiler
```

Notable suites:

| Suite | Covers |
|---|---|
| `track_query.rs` | Nearest-node queries used by containment, gravity and scoring |
| `stability.rs` | Hard driving that would expose a numerically unstable model |
| `matrix.rs` | View matrix construction (replaces a broken SDK helper, see [PSP notes](psp-notes.md)) |
| `lights.rs` | Lamp state and placement on pitched and rolled cars |
| `catalogue.rs`, `stream.rs` | Car ordering and load arithmetic |
