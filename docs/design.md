# Design

The intent behind the game. Where this and the code disagree, the code is authoritative.

## Concept

One car, one road, one run. Start at the top of a mountain pass at night and drive to the bottom.
No opponents, no traffic, nothing to collect.

**Sekira Pass** is roughly 3.5 km of switchbacks with ~170 m of descent and around ten hairpins.
It is generated at boot from a list of turns, so it costs no storage and is identical every run.

## Rules

- **Gravity drives.** The car accelerates downhill without throttle. The challenge is arriving at
  each corner in a controllable state.
- **Sliding scores.** Points accrue while sliding, scaled by angle and speed. Sustained slides
  build a multiplier; touching a guard rail resets it.
- **Recovery is free.** Spinning out, facing the wrong way or getting stuck puts the car back on
  the road facing downhill. The combo is lost, the run is not.

## Look

Late, cold, sparse: headlights, sodium lamps, a moon over a ridge. The palette is near-black —
deep blue sky, dark tarmac, dark green hillside — so warm lamps and red tail lights carry the
scene.

The world is flat-shaded and untextured, rendered at the native 480 × 272 with no antialiasing.
Cars are textured models compiled offline from third-party sources (see [Cars](cars.md)).

## Constraints

- **333 MHz CPU, fixed-function GPU.** World lighting is baked into vertices at boot.
- **480 × 272.** The track mesh is much coarser than the physics representation.
- **Fog at a few hundred metres**, which bounds what is submitted each frame.
- **Fixed 1/120 s physics step**, so handling is independent of frame rate.
- **Testable without the console.** Game logic has no PSP dependency; see
  [Architecture](architecture.md).

## Out of scope

Opponents, traffic, tuning, progression. Car selection exists, but every car runs the same road.

Controls are listed in [Building and running](building.md#controls).
