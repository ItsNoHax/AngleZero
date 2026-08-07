# Angle Zero — the idea

The starting point. Everything in the repository grew out of this; where the two disagree, the
code is right and this is a record of the intent.

## The game

One car, one road, one run. You start at the top of a mountain pass at night and drive to the
bottom. There is nobody to race and nothing to collect. The whole game is the road and how you
take it.

The road is **Sekira Pass**: about three and a half kilometres of switchbacks dropping some
hundred and seventy metres, with ten or so hairpins tight enough to need a deliberate slide. It is
generated from a short list of turns rather than shipped as a model, so it costs nothing to store
and is identical every run.

## What makes it work

**Gravity drives.** The car accelerates downhill with no throttle at all. Going fast is not the
problem; the problem is arriving at the next corner in a state you can do something about. That
one decision sets the pace of everything else — it is a descent, not a lap.

**Sliding is the point, not a mistake.** Points come from holding a slide: the further sideways
and the faster, the better. A sustained slide builds a multiplier. Brushing a guard rail takes it
away. So the rails are not scenery, they are the thing standing between you and the score, and
the interesting line is the one that runs closest to them.

**Recovery is never punished twice.** Spin, face the wrong way, or grind along a barrier and the
game puts you back on the road facing downhill. You lose the combo, not the run.

## How it should feel

Late, cold and a little lonely. Headlights, sodium lamps, a moon over a ridge, and not much else.
The palette is nearly black — deep blues in the sky, near-black tarmac, dark green hillside — so
the warm lights and the red tail lamps are the only things that carry.

Everything is deliberately low-fidelity: flat-shaded blocks, no textures on the world, hard pixel
edges, no antialiasing, rendered at the console's own 480 × 272 and not a pixel more. The car is
about thirty boxes. It should look like something that could have shipped on the hardware, not
like a modern game running on it.

## The constraints that shaped it

Written for a **PSP**, which means:

- A 333 MHz CPU and a fixed-function GPU, so lighting is baked into the geometry at boot and
  nothing is lit at runtime.
- 480 × 272, so fine detail is wasted effort; the track mesh is far coarser than the physics.
- Fog at a few hundred metres, which lets most of the world go unsubmitted every frame.
- Physics on a fixed 1/120 s step, so the handling does not change with frame rate.

The self-imposed one: **the game must be testable without the console.** Track generation,
physics, scoring, the camera and the screen flow are ordinary Rust with no PSP dependency, and
only the shell that draws and reads the pad needs hardware. See
[architecture.md](architecture.md).

## Controls

| | |
|---|---|
| Throttle | ✕ |
| Brake | ▢ or Down |
| Handbrake | ○ |
| Steer | D-pad, or the nub |
| Back on the road | △ |

## What was left out

No opponents, no traffic, no car selection, no tuning, no progression, no menus beyond a title and
a results screen. Each of those is a good idea for a different game. This one is a road, a car,
and one run down.
