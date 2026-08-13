# Architecture

How the crate is split, and why almost all of it can be tested without a PSP.

## Layout

The crate is split so that almost all of the game can be tested on a normal machine:

| Path | What it is |
|---|---|
| `src/*.rs` | `no_std`, target-agnostic game core — track, physics, scoring, camera, screen flow, HUD numbers, mesh building. No PSP dependency. |
| `src/psp/` | The PSP shell: GU bring-up, controller, renderer, 2D overlay. Only compiled for `target_os = "psp"`. |
| `tools/anglezero-asset/` | The car asset compiler. A workspace member, and the game is the only default member, so `cargo psp` never tries to build glTF parsing for mipsel. See [Cars](cars.md). |
| `tests/` | Host tests. Everything in `src/*.rs` is exercised here. |
| `scripts/` | Music encoding, the glitch hunt, and pulling captures off a console. |

The `psp` crate is a target-specific dependency, so `cargo test` never builds it, and `src/main.rs`
compiles to an empty `main` off-target. That is what makes `cargo test` work at all.

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

`src/lights.rs` is the clearest example of why the split is drawn where it is. Vehicle lighting looks
like a rendering feature, but almost none of it is: whether the tail lamps are hard on, whether the
reverse lamps are lit, where each lamp has been carried to by a car that is pitched onto a slope —
all of that is arithmetic about the game, and all of it is wrong in ways a screenshot cannot settle.
It lives on the host side and `tests/lights.rs` asks it directly. What is left on the PSP side is two
additive passes that draw what they are handed.
