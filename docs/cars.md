# Cars

How a 400,000-triangle model off a scanning site becomes a car the console draws at 60 Hz, and how
to add another one.

The whole point is the split. A development machine does everything expensive — parsing glTF,
deciding what nobody can see, decimating, sorting materials — and writes a `.azcar`. The PSP opens
that file, checks it once, and hands the bytes inside it to the hardware. **Nothing on the console
parses a model format, and adding a car changes no Rust.**

```
assets/source/bmw_3-series_e36.glb     the model, never modified, not in git (28 MB)
assets/configs/bmw_e36.toml            what the converter cannot work out for itself
assets/compiled/bmw_e36.azcar          the build artifact, committed, copied to the stick
```

## Adding a car

```bash
# 1. Look at what you have. Nothing is converted; this only reports.
cargo run --release -p anglezero-asset -- inspect assets/source/your_car.glb
cargo run --release -p anglezero-asset -- inspect --deep assets/source/your_car.glb

# 2. Write assets/configs/your_car.toml. Start with a name; add only what the report says you must.
# 3. Compile.
cargo run --release -p anglezero-asset -- convert \
    assets/source/your_car.glb assets/compiled/your_car.azcar \
    --config assets/configs/your_car.toml

# 4. Put it on the stick. The game loads every .azcar it finds that fits in the arena.
cp assets/compiled/*.azcar ~/.ppsspp/PSP/GAME/AngleZero/CARS/     # emulator
```

Left and right on the title screen pick between them. That is the architectural test: the AE86 was
added with a thirty-line TOML and no renderer change at all.

There are twelve slots and a 6 MB arena behind them, so the limit anybody reaches is the arena —
which is a number of bytes that can be measured, rather than a number of cars somebody picked. Seven
cars at the budgets in this repo come to 3.7 MB, leaving room for about four more. A car that does
not fit is refused by name on the title screen and the rest still load.

## What `inspect` is for

Everything about a source model that decides what the converter has to do is visible before a
vertex is read — the accessors carry their own bounds, so a full report costs 28 ms rather than
the half-minute decoding the embedded PNGs would take. Three things are worth looking for:

* **Units.** The report guesses from the bounding box. A car authored in centimetres loads as a
  hundred-metre building. `scale` is the fix, and the report says enough to work it out: three of
  the seven cars here are 0.04 units long and want multiplying by 100.
* **Facing.** A bounding box cannot answer this — it is the same box either way round — so the
  report cannot guess and does not try. Look at which end the lamps are on. Every car here bar one
  puts its headlights at **+Z** and its tail lights at −Z; the E39 is the exception, with headlights
  at z = −2.14, and `[spawn] yaw = 180` is the fix. Get it wrong and the car drives the whole
  descent backwards, which is invisible on the title screen — a saloon three-quarters on is a
  saloon either way round — and obvious from the chase camera, which spends the run looking at the
  car's nose.
* **Which parts are wheels.** This is the one question no model answers reliably. `inspect` prints
  each part's centre, and four similar parts at mirrored X and two Z stations are the wheels.
* **How the parts are split.** The E36 is one node per part with a child per material; the AE86 is
  one node with everything merged by material, so all four tyres are a single mesh. The converter
  handles both, but only the second needs its wheels cut apart geometrically.

## The config

Only what cannot be measured. The E36's file is a name, a triangle budget, three node-name
fragments and a comment explaining each; everything else — wheel radius, ride height, wheelbase
centring, which of 57 materials is glass — the converter works out.

| Key | When you need it |
|---|---|
| `name` | Always. Shown on the title screen, folded to uppercase because the font has no lowercase. |
| `scale` | The model is not in metres, or not life size. |
| `[spawn] yaw` | Degrees about Y, for a model that does not face +Z. The E39 needs 180. |
| `triangles` | The budget for LOD0. Default 10,000. |
| `lods` | Coarser budgets, nearest first, e.g. `[4500, 1800]`. |
| `[wheels] match` | Node-name fragments identifying wheel parts. Almost always needed. |
| `[materials]` | The category guesser does not speak this model's language. |
| `[reduce]` | A category deserves more or less of the budget than its screen area suggests. |
| `[reduce.parts]` | One *part* deserves more or less than the others in its category. |
| `[reduce] drop` | Parts to leave out entirely — detail the sweep can see but that is not worth a triangle. |
| `[handling]` | The car should not drive like the one the game was tuned around. |
| `[lights]` | The lamp detector refused, or the car's beams should be a different length. |

## Where the budget goes

Not one ratio across the model. The converter sweeps 72 viewpoints around the car, drops what no
viewpoint can see, sorts what is left into six categories, and shares the budget out by how many
pixels each part actually owns — then decimates each part to its own share.

That ordering is the whole trick. The E36's engine is 137,000 triangles behind a closed bonnet: a
decimator handed the `body` category as one lump takes a third of the paint's detail with it. The
report prints where every triangle went, what each category cost in error, and warns when a
category is losing its shape.

### It shares out twice

A bucket cannot always use its share. The E36's bodywork is within five millimetres of the original
at about 3,600 triangles, and past that the simplifier refuses to spend more on something it cannot
improve — five millimetres is under a pixel at any distance the car is seen from. Sweeping the
budget from 5,000 to 40,000 moves the bodywork from 2,616 to 4,876 and no further.

That shortfall does not exist until the simplifier has run, which is after the sharing is done, so
a 15,000-triangle budget quietly produced an 11,375-triangle car. The allocator therefore runs
twice: the second pass pins every bucket that came in under its share at what it actually used, and
shares the difference among the ones that were stopped by their target rather than by their own
geometry. Wheels and glass take it, because a surface of revolution accepts every triangle offered.

Three things had to be true for that to be safe. The four corners are averaged before the split —
the sweep sees the near side of a car far more than the off side, and following that literally gave
one tyre 2,648 triangles and the one across from it 647. And "filled its share" has a five per cent
tolerance, because mirrored geometry collapses in slightly different orders and an exact test
called two of four identical tyres full and handed the surplus to the other two.

The third is that the pin itself is levelled across a category's corners, at the best any of them
reached. Pinning a bucket at what it used says the simplifier was stopped by the geometry rather
than by the target, which is true of a body panel and not of a wheel: four corners are the same
wheel four times, mirrored, and on geometry the simplifier finds hard it can stop far short on one
and not on its reflection. The 190E's alloys — identical 5,565-triangle corners on identical shares
— came out at 2,217, 2,143, 1,049 and 821, and raising the budget never moved the last of them,
because it had been declared full at 821 on the first pass and held there while its own reflection
was refilled to nearly three times as much. Judging all four at the best of them fixes it: they now
land within 7% of each other instead of a factor of nearly three. A corner that still cannot reach
the levelled target simply returns less, which costs nothing — a target was never a promise.

### What the sweep does not answer

It measures whether a part is on screen, not whether it is worth drawing, and those come apart in
one place: detail behind an opening. Each of the E36's alloys has 4,762 triangles of brake disc and
caliper behind it, visible through the gaps between the spokes — so the sweep gives the hardware a
share of the corner, and the alloy in front is left with about 150 triangles. At 150 a five-spoke
wheel is a disc, because the spoke windows are the first thing an edge collapse closes.

`[reduce.parts]` is the answer, and `[reduce] drop` is the blunt version of it. Weighting the alloy
at 6.0 against the hardware's 0.4 keeps both — the hardware is what makes the gaps read as gaps,
and the caliper is the only colour in the wheel — where dropping it outright throws it away. Use
`drop` when a part is worth nothing at all, and `[reduce.parts]` when it is worth something but not
what its pixel count claims.

Category weights and part weights are not interchangeable, and one pair pulls against the other:
the alloys and the bumper trim are both bright metal and share the `chrome` category, so raising it
feeds the wheels and starves the trim. Moving `chrome` up and `wheel` down separates them — on the
E36 that takes the trim from 42 triangles at 18.2% error to 221 at 5.3% while the wheels keep
theirs. Past `chrome = 12` the tyres collapse to 69 triangles each.

## Lights

A car's lamps come out of its asset, the same way its wheels do. The renderer has no idea which car
it is lighting: it reads the lamp records, asks what each one should be burning at, and draws two
additive passes for every car on screen.

```
Lights:
  Headlights:      2
  Tail lights:     2
  Brake lights:    0
  Reverse:         2
  headlight       left at ( 0.50, 0.61,  2.04), 0.44 m across, 40 m beam   named in the model
  tail light      left at ( 0.38, 0.91, -2.09), 1.10 m across              named in the model
  reverse light   left at ( 0.57, 0.75, -2.34), 0.36 m across              named in the model
```

Almost none of that is configured. The material sorter has always put lenses in the `light`
category, so the parts are already known; where a lamp is and how big it is are measured off the
lens itself. What a lens is *for* is the part a model does not answer — nothing about a red lens
says whether it comes on under braking — so the kind comes from the part's name where the name is
clear, from which end of the car it is on where it is not, and from the config where neither will
do.

Six of the seven cars here need no `[lights]` table at all.

### What it refuses, and why

Detection is conservative in the same way wheel identification is: **no lamp rather than the wrong
lamp.** Three rules do the refusing, and each of them fires on a real car in this repo.

* **A lens on the centreline belongs to no side.** The E36 has two — the strip across the boot lid
  and the high-level lamp in the back window — and handing one of them to whichever side the
  wheelbase centring left it a millimetre on is exactly the "random mesh as a brake light" this is
  meant to avoid. Both are named in a warning and left off; the pair in the rear cluster does the
  braking.
* **Lamps come in pairs.** One of a pair is nearly always a lens the sorter missed on the other
  side, not a car with one headlight.
* **A pair has to be a mirror image.** The 190E's largest front-left lens is a repeater on the wing
  at z = 0.30, against a right-hand headlight at z = 1.35. Which of the two was right cannot be
  known here, so neither is used and the report prints both positions.

A brake or reverse lens is never inferred from position at all. It has to be named — in the model or
in the config — because nothing about where a lens sits says what switches it on.

### When a car comes out with no headlights

Name the lens. The fragment picks *which* part; where the lamp is and how big it is are still
measured off the model.

```toml
[lights.headlight_left]
node = "HL_Glass_ONM"
[lights.headlight_right]
node = "HL_Glass_ONM"
```

The same fragment for both sides is normal: parts are cut at the centreline first — most models put
a whole pair, or every lamp on the car, in one mesh — so a fragment plus a side is enough to name
one lamp. `node` reaches parts of any category, which is what makes it an override rather than a
filter: the 190E's lenses are transparent, so the sorter calls them glass, which is a perfectly good
answer that leaves the car with no headlights.

The other keys are for the cases where a model has nothing to point at (`at` places a lamp
outright), or where the light should look different (`color`, `intensity`, `radius`, `range`,
`spread`, `steer`). `enabled = false` gives a car no lamps at all.

### What it costs

Two draw calls a frame, whatever the number of cars, because both passes build world-space
triangles into one buffer. A lamp glow is eight triangles and a beam is twelve, and a beam is only
resolved within 70 m — beyond that a car keeps its lamps and loses its beam, and past the fog at
330 m it has neither. The trace has a column for each, so a field of cars can be priced:

```
# frame road terrain lines rails dashes props cars lamps beams verts ...
```

## Levels of detail

A car carries three copies of itself: LOD0 for the one being driven, LOD1 beyond 18 m, LOD2 beyond
45 m. Each is decimated from the welded original rather than from the level above, so the coarsest
carries one decimation's error and not three.

With eight cars on screen this halves the vertex load — 260,223 to 138,225 a frame — for 0.05% of
pixels differing at all. It costs file size: the E36 is 580 KB, of which 129 KB is the texture and
the rest is geometry across three levels.

That is what made the arena the binding limit. Seven cars run from the 190E's 450 KB to the R34's
627 KB and come to 3.7 MB together — against an arena that was 1.5 MB, which is why the four slots
next to it were never once reached: the third car was refused for want of bytes long before a
fourth was asked for. The arena is 6 MB now and the slots are twelve, deliberately more than it can
hold, so that the failure names the resource that actually ran out.

## Measuring

`--mode 13` and `--mode 14` draw four and eight cars, alternating every asset on the stick, so that
switching vertex buffers and material state between different cars is part of what is timed:

```bash
scripts/psp_glitch.py --node 1200 --burst 60 --frames 12 --mode 14 --label eight-cars
```

The vertex and draw-call counts in the trace are exact. The microseconds are PPSSPP's software
rasteriser and are only good for comparing one run against another — it is fill-bound where the
console is not, so it under-reports what vertices cost. Real numbers come off the hardware, from
the `CAR` and `US` fields in the on-device overlay.

## When a part looks wrong

A wheel is about twenty pixels on a 480-wide screen, which is too few to tell a bad mesh from a
small one. Render the part out of the compiled asset instead — on its own, large — next to the same
part read out of the source `.glb`. One is what the converter produced and the other is what it was
given, and the difference is the answer. That is how the alloys were sorted out: the source was a
clean five-spoke wheel with a caliper behind it and the compiled one was not.

**Cull backfaces when you do.** A throwaway rasteriser that draws every triangle shows the far side
of the tyre through the hole in the near side, which covers the alloy completely and makes every
wheel look like a featureless blob however good it is. The GE culls; a debugging tool that does not
will lie to you, and it cost a wrong diagnosis here before it was noticed.

## When the car is the right way round and steers with the wrong wheels

`[spawn] yaw` turns the geometry, and until the E39 needed it, it turned nothing else. Corners are
worked out from where the wheels sit — `front` is simply the greater Z — and that ran on the model
as authored, before the rotation. So the option advertised above as the fix for a backwards car
would have turned the body to face front and left the corner labels behind it, and the labels are
not cosmetic: `steers` comes straight from them, and the tyre marks come out from under whichever
pair is called the rear.

The result would have been a car that looks completely correct and turns its back wheels. `wheels`
classifies in the rotated frame now, which is a no-op at a yaw of zero, so the six cars that do not
use the option are unaffected. Worth knowing because the failure has no visual signature at all —
the check is that the two wheels that turn are the two at the end with the headlights.

## When the whole car looks wrong

Check the order of its meshes before suspecting anything else. The S15 arrived drawing as a handful
of pixels on the road, and every offline measurement said the asset was perfect — 4.47 m of vertex
data at all three levels, four wheels at the right hubs, sane materials, and thirteen draw calls
issued on the console. What was different about it was that the compiler emitted its wheel meshes
before its body meshes, and it is the only car in the set that does.

That mattered because `sceGumPushMatrix` and `sceGumPopMatrix` do not address the same stack slot in
rust-psp 0.3.13 — push advances the pointer then saves, pop retreats the pointer then loads, so what
is popped is never what was pushed. It only appears to work once a draw has synced the right matrix
into the slot underneath. Every car whose body comes first does exactly that on its first mesh; the
S15 pushed and popped before drawing anything, restored leftovers, and rendered its whole body
through them. `draw_one_car` now rebuilds the car's transform before each mesh and uses no stack at
all, so mesh order means nothing again — but the failure is worth recognising, because "the asset is
fine and the car is missing" points at the renderer and not at the pipeline.

## The texture

One per car, 256×256, and every source material has a tile in it. The runtime merges parts into six
materials, so a draw call covers the paint and the badges on it at once — a texture per material
could not be bound. Packing means the console binds once per car and never switches.

Materials with no image get a white tile. That is what keeps the whole thing additive: glTF's base
colour is `texture × factor`, the factor is already baked into the vertex alongside the light term,
so a white tile multiplies to exactly the colour that vertex had before textures existed. Anything
untextured looks the same as it always did.

```bash
anglezero-asset convert ... --atlas /tmp/atlas.png    # look at what was packed
```

Worth doing on a new car. The atlas says at a glance whether the material sort found the right
images, and a texture that is wrong is far quicker to recognise by looking at it than by reading a
report about it.

Two stages had to be taught that UVs exist. Welding keys on the texture coordinate as well as the
position and colour, because two vertices at a point with different UVs are a seam somebody put
there deliberately. Decimation uses meshoptimizer's attribute-aware simplifier, because the
collapse it likes best on a flat panel is the one that drags a decal across it — the shape does not
change and the texture slides.

UVs outside the unit square are clamped. In an atlas a repeated texture would sample the material
packed next door, so a tiling pattern shows its edge rather than somebody else's paint.

## Licences

The models are other people's work, under licences that require attribution. The credit line comes
out of the asset — the converter reads it from the glTF's `asset.extras` — so the title screen
shows it without anybody having to remember to.

The source `.glb` are tens of megabytes each and are not in git. These are where they came from; the
file name each one is expected to have is in `assets/configs/`, beside the config that compiles it.

| Car | Model | Author | Licence |
|---|---|---|---|
| BMW 3-Series E36 | [Sketchfab](https://sketchfab.com/3d-models/bmw-3-series-e36-street-13d0a12ecda04317b96ce1e618300412) | [Black Snow](https://sketchfab.com/BlackSnow02) | CC-BY-4.0 |
| BMW 5-Series E39 | [Sketchfab](https://sketchfab.com/3d-models/bmw-e39-free-531a5a93da5d493d9918eb36f011c20d) | [Black Snow](https://sketchfab.com/BlackSnow02) | CC-BY-4.0 |
| Mercedes-Benz 190E (W201) | [Sketchfab](https://sketchfab.com/3d-models/1982-mercedes-w201-9b2ea34482654173a7f421aab8f1b287) | [Dave Love](https://sketchfab.com/Tyler_Dave) | CC-BY-4.0 |
| Toyota AE86 Trueno | [Sketchfab](https://sketchfab.com/3d-models/toyota-ae86-trueno-da86f9fb5e6149878b433f8a2b81443a) | [StanBox](https://sketchfab.com/StanBox) | CC-BY-NC-4.0 |
| Nissan Skyline R34 GT-R | [Sketchfab](https://sketchfab.com/3d-models/2002-nissan-skyline-gt-r-v-spec-ii-nur-r34-778a13fa476c4806b9df75a87a6ecf7c) | [OUTPISTON](https://sketchfab.com/outpiston) | CC-BY-NC-SA-4.0 |
| Nissan Silvia S15 (Vertex Edge) | [Sketchfab](https://sketchfab.com/3d-models/2010-vertex-edge-nissan-s15-silvia-1edf4f37e6284bdaa6df0f9572389875) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Volkswagen Golf R Mk7.5 | [Sketchfab](https://sketchfab.com/3d-models/2019-volkswagen-golf-r-ae63f9a1b236480588bd2e8dcce7b7b2) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |

**Four of the seven are non-commercial, and three of those are also share-alike.** Fine for a hobby
build; a paid release could ship the E36, the E39 and the 190E and nothing else. `ShareAlike` is the
stricter half of that — it reaches the derivative, which is the compiled `.azcar` and arguably the
screenshots of it, not merely the sale.

The credit line is read out of each model's `asset.extras` by the converter and drawn on the title
screen, so this table is a convenience: the attribution ships whether or not anybody updates it.
