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

# 4. Put it on the stick. The game loads every .azcar it finds, up to four.
cp assets/compiled/*.azcar ~/.ppsspp/PSP/GAME/AngleZero/CARS/     # emulator
```

L and R on the title screen pick between them. That is the architectural test: the AE86 was added
with a thirty-line TOML and no renderer change at all.

## What `inspect` is for

Everything about a source model that decides what the converter has to do is visible before a
vertex is read — the accessors carry their own bounds, so a full report costs 28 ms rather than
the half-minute decoding the embedded PNGs would take. Three things are worth looking for:

* **Units and facing.** The report guesses from the bounding box. A car authored in centimetres
  loads as a hundred-metre building; one facing −Z drives backwards. Both are silent until it is
  on screen, and `scale` in the config is the fix.
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
| `triangles` | The budget for LOD0. Default 10,000. |
| `lods` | Coarser budgets, nearest first, e.g. `[4500, 1800]`. |
| `[wheels] match` | Node-name fragments identifying wheel parts. Almost always needed. |
| `[materials]` | The category guesser does not speak this model's language. |
| `[reduce]` | A category deserves more or less of the budget than its screen area suggests. |
| `[reduce] drop` | Parts to leave out entirely — detail the sweep can see but that is not worth a triangle. |
| `[handling]` | The car should not drive like the one the game was tuned around. |

## Where the budget goes

Not one ratio across the model. The converter sweeps 72 viewpoints around the car, drops what no
viewpoint can see, sorts what is left into six categories, and shares the budget out by how many
pixels each part actually owns — then decimates each part to its own share.

That ordering is the whole trick. The E36's engine is 137,000 triangles behind a closed bonnet: a
decimator handed the `body` category as one lump takes a third of the paint's detail with it. The
report prints where every triangle went, what each category cost in error, and warns when a
category is losing its shape.

### What the sweep does not answer

It measures whether a part is on screen, not whether it is worth drawing, and those come apart in
one specific place: detail behind an opening. Each of the E36's alloys has 4,762 triangles of brake
disc and caliper behind it, visible through the gaps between the spokes — so the sweep gives it a
share of the corner's budget, and the alloy in front is left with about 150 triangles. At 150 a
five-spoke wheel is a disc, because the spoke windows are the first thing an edge collapse closes.

`[reduce] drop` is the answer: name the part and it is left out. The wheel then gets the whole
corner, the gaps read as gaps, and the report says how many triangles were dropped by name so it
cannot happen quietly. Worth checking on any car whose wheels look like blobs — rendering the
compiled wheel side-on next to the source is how this one was found.

The body saturates. Sweeping the E36's budget from 5,000 to 40,000 moves the bodywork from 2,616
to 4,876 triangles and no further — past that the converter can already draw every panel within
five millimetres of the original, which is under a pixel at any distance the car is seen from.
Wheels are the opposite: surfaces of revolution take every triangle offered, and a wheel decimated
to a wedge is the most obvious tell of a cheap model. So the budget above the body's ceiling goes
to the wheels deliberately, and 15,000 is where that stops paying.

## Levels of detail

A car carries three copies of itself: LOD0 for the one being driven, LOD1 beyond 18 m, LOD2 beyond
45 m. Each is decimated from the welded original rather than from the level above, so the coarsest
carries one decimation's error and not three.

With eight cars on screen this halves the vertex load — 260,223 to 138,225 a frame — for 0.05% of
pixels differing at all. It costs file size: of the E36's 479 KB, 346 KB is geometry across three
levels against 208 KB for LOD0 alone, and 129 KB is the texture.

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

| Car | Author | Licence |
|---|---|---|
| BMW 3-Series E36 | [Black Snow](https://sketchfab.com/BlackSnow02) | CC-BY-4.0 |
| Toyota AE86 Trueno | Stanbox | CC-BY-NC-4.0 |

**The AE86 is non-commercial.** Fine for a hobby build, not for a paid release.
