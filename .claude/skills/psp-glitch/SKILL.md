---
name: psp-glitch
description: Hunt rendering glitches in this PSP project automatically — flickering terrain, geometry popping in and out, surfaces fighting over depth — using a deterministic headless run and a frame-to-frame comparison. Use whenever asked to find, diagnose, reproduce, or fix a visual or graphical bug, or to check whether a rendering change made things better or worse.
---

# Hunting rendering glitches

A single screenshot cannot show flicker: the artifact only exists as a difference between consecutive
frames, and `--screenshot-save` overwrites one file. Use `/psp-preview` to look at *a* frame; use this
to find something that only happens on *some* frames.

## The one command

```bash
scripts/psp_glitch.py --node 1200 --burst 60 --frames 40 --label hairpin
```

About ten seconds end to end, build included. It builds with `--features harness`, runs
`PPSSPPHeadless`, harvests the frames the guest wrote to `ms0:/ANGLEZERO/` (which under headless is
`~/.ppsspp/ANGLEZERO/`), and reports findings. Output lands in `captures/glitch/<label>/`, which is
git-ignored. Needs Pillow.

`--scan-only captures/glitch/<label>` re-runs the analysis on frames already harvested. Use it after
changing the detector — there is no reason to re-run the emulator.

The run installs `assets/compiled/*.azcar` onto the emulator's stick first, every time, including
under `--no-build`. The car is an asset rather than part of the binary, and a run that draws last
week's model against this week's code looks exactly like a change that did nothing — which is the
one conclusion this script must never produce by accident. Recompiling a car with
`anglezero-asset convert` is therefore enough; nothing has to be copied by hand.

Cars are offered in filename order and a run starts on the first of them. A burst taken over the
title screen will catch a car still arriving — read a chunk per frame, standing as a flat near-black
**silhouette** until the whole file has landed — which is correct behaviour rather than a car that
failed to draw.

This is also the right tool for looking at that transition, because a single screenshot cannot show
it. `--hold '20 r'` presses right on a known frame and the frames either side say whether the swap
is clean:

```bash
scripts/psp_glitch.py --burst 15 --frames 40 --hold '0 -' --hold '20 r' --label car-swap
```

That is how the one-frame gap in the first version of car streaming was found: on the frame the
button was pressed, the old car had gone and the incoming silhouette had not been read yet, so the
lay-by was empty for a sixtieth of a second. Nobody would have caught it by eye.

## Why runs are comparable

`--features harness` (see `src/psp/harness.rs`) fixes the frame delta at 1/60 s, replays input from a
frame-indexed script, captures consecutive frames, and exits when done. Nothing else in the game reads
a clock or a random number, so **two runs of the same script are byte-identical** — verified. That is
what makes a before/after mean anything, and it is worth re-verifying if you ever touch the timing:

```bash
scripts/psp_glitch.py --node 1200 --burst 60 --frames 12 --label det-a
scripts/psp_glitch.py --no-build --node 1200 --burst 60 --frames 12 --label det-b
cmp captures/glitch/det-a/SHOT000.png captures/glitch/det-b/SHOT000.png
```

Do not use `scripts/psp_input.py` for this. It drives the WebSocket debugger on wall-clock delays,
which bear no fixed relation to emulated frames under fast-forward, so input lands on a different
frame every run. It is still the right tool for a one-off "hold a button and screenshot".

## Reaching the place you care about

`--node N` (0–2620) drops the car straight onto a centreline node at `--kph`. Without it most of the
track is unreachable: driving there means a steering script that survives every corner in between, and
one mistake ends the run against a guard rail.

It **replaces** the input script rather than adding to it, so any `--hold` given alongside it is
silently ignored. A run that has to be braking, or steering, when the burst starts needs its
`SCRIPT.TXT` written by hand and headless run directly — which is worth knowing before spending a run
wondering why the brake lights never came on.

## Reading the output, and not over-trusting half of it

**The draw tally is exact.** It records the draw call, not its result: holes in the road or terrain
chunk sets, a chunk set that blinked A-B-A, a refused vertex-arena allocation, a display list near its
1 MB buffer. When the two halves disagree, this one wins.

**The pixel comparison produces leads, not verdicts.** Two filters keep ordinary motion out — a shift
search, because a lamp post passing a few metres away sweeps wider than itself in one frame and
otherwise looks exactly like a blink; and a bracket test, because a pixel in a smoothly moving scene
stays between what it was and what it will be even when the motion accelerates. It still flags the
moment one light pool hands over to the next. Always look at the `blink-SHOT*.png` sheet it writes
before believing a finding.

Traps worth knowing, all of which have already cost a session:

- **Do not trust `--no-build` unless you know what is on disk.** Any other `cargo psp` invocation
  overwrites the `.prx`. A `devtools` binary sits waiting for a SELECT press that never comes and
  produces no captures at all.
- **A "hole" in the props or glows mask means nothing.** Those chunks are legitimately empty where
  there are no lamps or trees. Only road and terrain are dense enough for the hole test; for the other
  two, only the A-B-A test is trustworthy.
- **Identical masks and vertex counts across a blink** means nothing was dropped and the fault is
  downstream of submission — depth, blending, or winding. Stop looking at culling.

## Narrowing down a cause

This is the part that actually finds bugs. `--mode N` runs under a `render::DEBUG_MODES` override:
re-run the same frames with one suspect removed and see whether the artifact survives.

| Mode | Removes | Mode | Removes |
| --- | --- | --- | --- |
| 1 | back-face culling | 7 | everything but the terrain |
| 2 | the depth test | 8 | the HUD |
| 3 | fog | 9 | the headlight beams |
| 5 | the sky | 10 | the car's lamp glows |
| 6 | the terrain | 11 | skid marks and smoke |
| | | 12 | the roadside light pools |
| | | 15 | *adds*: every lamp on every car, lit from both sides |

Two things make this quantitative rather than a matter of squinting:

**Verify the mode actually changed the picture.** A mode that removes something not on screen
produces a byte-identical frame, and reading that as "not the cause" is wrong.

```bash
python3 -c "
from PIL import Image, ImageChops
a=Image.open('captures/glitch/m0/SHOT015.png').convert('RGB')
b=Image.open('captures/glitch/m6/SHOT015.png').convert('RGB')
print(ImageChops.difference(a,b).getbbox())"
```

**Isolate one pass exactly.** Two runs differing only by a mode are deterministic, so subtracting them
gives that pass's own contribution and nothing else — the fastest way to see what a suspect actually
draws. This is how the headlight beams were caught contributing *zero pixels*, and how the light pools
were caught being cut short.

**Submitted is not painted, and the counters cannot tell you which.** The trace counts draw calls —
`cars`, `lamps`, `beams` among them — and every one of those was non-zero, every frame, while the two
vehicle-lighting passes were lighting under eight hundred pixels between them: the beams buried under
the road they lay on, the glows inside the bodywork they belonged to. A count proves a pass ran. Only
the subtraction above proves anybody can see it. The same subtraction, run either side of a fix, is
also how to tell a change that helped from one that merely moved something: 49 lit pixels became 256,
for the same 13 ms, because a discarded fragment costs what it costs to rasterise.

Then measure the thing itself over frames rather than arguing about the images. A stationary pool being
driven past must extend further down the screen every frame; any step backwards is the artifact.

## Known geometry, so it is not rediscovered

- The road is **crowned**: `ROAD_STATIONS` puts its centre 3 cm above the centreline, and the
  edge-line ribbons sit at 5 cm. The car's `state.y` is the raw centreline height. Anything laid "on
  the road" from the car's `y` must clear ~5 cm or it fights the paint.
- Flat additive overlays on a faceted road lose the depth fight over most of their area. The fix used
  is `sceGuDepthOffset(POOL_DEPTH_BIAS)` around the pass, reset to 0 afterwards because it is context
  state. A bias fixes a *tie*; it will not lift geometry that is genuinely buried, and if eight times
  the bias changes nothing then the problem is not depth.
- **Anything drawn beside the car must use the car's whole transform**, not its heading. `draw_one_car`
  turns the body by yaw, then pitch, then roll; on a pass that falls 7.4 cm a metre that is 4.2° of
  pitch, which moves a point 2 m from the origin by 15 cm — in *opposite* directions at the two ends
  of the car. That is what made the brake lights sit too low and the headlights too high at the same
  time, and two symptoms in opposite directions is the signature of a missing rotation rather than a
  wrong offset. rust-psp builds those matrices in VFPU assembly; read the `vrot.q` columns rather than
  assuming the convention, since this repo has already been caught by two of that crate's matrix
  helpers.
- Every additive ground pass here draws **double-sided**. A flat quad has one winding and the camera
  can be on either side of it; one pass missing `sceGuDisable(GuState::CullFace)` had its headlights
  invisible for the whole history of the file.
- The software rasteriser does not model the data cache or a 16-bit depth buffer, so it is more
  forgiving than the console. A clean headless run is not proof the hardware is clean — check the
  console for anything depth- or cache-shaped (see `psp-deploy`).
