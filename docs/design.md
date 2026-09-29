# Design

The intent behind the game. Where this and the code disagree, the code is authoritative.

## Concept

One car, one road, one run. Start at the top of a mountain pass at night and drive to the bottom.
No opponents, no traffic, nothing to collect.

**Sekira Pass** is roughly 3.5 km of switchbacks with ~190 m of descent and around ten hairpins.
It is generated at boot from a list of turns, so it costs no storage and is identical every run.

The run starts at the summit, beside a car park looking out over a city in the basin below. The
title screen parks the car there, nose to the rail, and holds one shot from above and behind it:
the first hairpin on the left, 28 m down the steep drop off the summit, and the city on the right.

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

The road is two lanes, and traffic keeps left: a white dashed centre line on the straights
turns to double solid yellow, with raised markers, through every bend. Before each bend come a
painted "40", optical speed bars and 急カーブ ("sharp bend") in the lane. The surface is worn:
darker wheel tracks, patches, sealed cracks, sections resurfaced at different times, drain
grates in the shoulders and other drivers' tyre marks through the apexes. The lanes are paint
only; the car can use the whole road.

The world is vertex-lit and carries low-contrast tiles: asphalt grain on the road, grass on the
hillside, a sprayed-concrete lattice on the cut banks, and alpha-tested pines. Every tile is
generated at boot, and every light is baked into vertices: the moon on each slope, sodium warmth
near each lamp. It renders at the native 480 × 272 with no antialiasing, so anything thinner than
a pixel is either drawn only up close (rail posts) or snapped to whole pixels (stars, valley
lights). Cars are textured models compiled offline from third-party sources (see
[Cars](cars.md)).

Beyond the road: three rings of moonlit ridgeline, and below them a city, a town and the roads
between, under a haze and a band of mist. The city is a street grid of lights round a downtown
of towers with red beacons, a lattice broadcast tower, two elevated expressways and a railway. Hard bends are cut into the hillside on
their inside and marked with chevrons on their outside; the rail carries reflectors. All three
answer the headlights. On the title screen, Up or Down wets the road, which puts each lamp's
reflection in the tarmac. It changes nothing about the handling.

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
