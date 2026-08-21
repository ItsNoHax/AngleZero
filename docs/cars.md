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
# 1. What is already done, and what is sitting in assets/source/ with no config yet.
scripts/cars.sh list

# 2. Look at the model you picked. Nothing is converted; this only reports.
cargo run --release -p anglezero-asset -- inspect assets/source/your_car.glb
cargo run --release -p anglezero-asset -- inspect --deep assets/source/your_car.glb

# 3. Write assets/configs/your_car.toml. Start with a name and the `source` line; add only what
#    the report says you must.
# 4. Compile, and read the report rather than just the file size.
scripts/cars.sh build your_car

# 5. Look at what you got, which is the step nobody should skip.
cargo run --release -p anglezero-asset --bin azview -- \
    assets/compiled/your_car.azcar /tmp/car.png --yaw 210 --pitch 14 --dist 6
cargo run --release -p anglezero-asset --bin azview -- \
    assets/compiled/your_car.azcar /tmp/sil.png --silhouette --yaw 270 --pitch 6

# 6. Put it on the stick. The game offers every .azcar it finds there.
cp assets/compiled/*.azcar ~/.ppsspp/PSP/GAME/AngleZero/CARS/     # emulator
```

`source = "your_car.glb"` in the config is the only record of which model a car came from — the
names do not match, and the models are not in git — and it is what lets step 1 answer its question.
`convert` refuses to run if it disagrees with the file it was handed.

Steps 2 to 5 are a loop rather than a list, because a source model off a scanning site is usually
wrong in at least one way that only a render shows. The **`/add-car`** skill drives that loop: it
picks up the unconverted models, reads the report, writes the smallest config that could work, and
then renders the car and its silhouette and judges them against the failure modes below.

Left and right on the title screen pick between them. That is the architectural test: the AE86 was
added with a thirty-line TOML and no renderer change at all.

**How many cars fit is now a question about the memory stick, not about the game.** Only one car is
in memory at a time — the one on screen — so a directory with fifty in it costs what a directory
with seven costs, and boot does not get slower for finding them. The list is sorted by filename and
holds 128 entries, which is a number chosen to be unreachable rather than to be budgeted against.

The limit that is left is per car: **1.25 MB a file**, which is what one residency slot is. The
compiler refuses to write a car larger than that, so it is caught on a development machine by name
rather than on a title screen by absence. Today's fleet runs from 716 KB to 905 KB.

What that costs is time rather than space. Picking a car reads it, which is most of a megabyte off
a memory stick, so the file arrives over the next few frames rather than instantly — see
[Silhouettes](#silhouettes) for what is on screen while it does.

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
| `source` | Always. The model's filename in `assets/source/`; the only record of which one a car came from, and what `scripts/cars.sh` reads to tell converted models from unconverted. |
| `scale` | The model is not in metres, or not life size. |
| `[spawn] yaw` | Degrees about Y, for a model that does not face +Z. The E39 needs 180. |
| `triangles` | The budget for LOD0. Default 10,000. |
| `lods` | Coarser budgets, nearest first, e.g. `[4500, 1800]`. |
| `silhouette` | The stand-in's budget, default 1,000. Raise it for a long or awkwardly shaped car — the number sizes a *grid*, so a 5 m saloon needs more of it than a hatchback. |
| `[wheels] match` | Node-name fragments identifying wheel parts. Almost always needed. |
| `[wheels] radius` | Override the measured rolling radius. Rarely — it is taken from the hub's height above the road, which cannot be wrong on a car standing on its wheels. |
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

### Why the budget is 24,000 and not 15,000

Because a pixel count is not an importance, and the parts it underrates are the ones a player reads
the car by. Every car here ran at 15,000 for a long time and three faults came out of it, all
looking like different bugs and all being this one:

* **The E36's kidney grille** drew as black shards. Two slatted grilles are a few hundred pixels
  across the sweep, so they were allocated about forty triangles between them, and forty triangles
  of slats is shrapnel. It survives culling perfectly well — turning culling off does not improve
  it, which is what says the fault is the allocation and not the winding.
* **The E36's alloys ate their own tyres.** An alloy's outer lip and a tyre's inner bead are the
  same circle in the model, decimated separately. At 560 triangles the lip is a coarse polygon whose
  edges cut across the bead, so the two interpenetrate and the wheel reads as a ragged chrome dish
  with no sidewall. It is not camber, and the way to tell is to compile the same model at 90,000,
  where the wheel is clean.
* **The Golf's grille bar** left a gap between the headlights.

None of these is a rendering bug and none of them has a fix in the renderer. The reason there was
room to raise the budget is in [Levels of detail](#levels-of-detail) — the coarse levels were 30% of
every file and are worth far less than that.

### The half of it that was not the budget

Raising the budget to 30,000 fixed all three, and then a fourth fault showed that half the problem
had never been the budget at all. **The Golf's VW roundel and the trim around it were missing**, and
raising `chrome` did not put them back — the category sat at 238 triangles at *any* weight.

A part that comes in under its allocation never has the allocation enforced, so no weight in any
config can rescue a part that the free-error pass has already flattened. And that pass was flattening
this one: it asks for the cheapest mesh within an error nobody can see, and that error was 5 mm
absolute — which is the entire relief of a badge. A flat disc is within 5 mm of a VW roundel, so a
flat disc is what came back.

The limit is now the *smaller* of 5 mm and 0.2% of the part's own extent, which leaves a 4.3 m body
shell exactly where it was and gives the badge 0.2 mm. Chrome went 238 → 471 and the wheels' own
brightwork doubled, having been quietly flattened the same way.

That is also why the budget is 24,000 rather than the 30,000 the first three faults needed: with
small parts no longer flattened, 20,000 is within 0.3% of 30,000's pixels on the E36's wheel and
0.6% on its nose. The budget was paying for a bug.

### What a category weight is really for

The two changes above moved enough triangles around to expose which categories had been living off
the slack, and the answer is worth writing down, because all three cases are a category that is
*seen* far more than its pixel count says.

* **Interiors.** The R34's cabin is 47,234 triangles as a single part and was weighted *down* to
  0.4, reaching 606 triangles at 20% error — a cabin that is visible through the glass for the whole
  descent. There is a cliff rather than a curve here: 1.0 is *worse* than 0.4 (21.5% at 1,557
  triangles, because partial pruning takes components whole), and 2.0 lands at 1.81%. The E36 and the
  Golf were the same story.
* **Tyres against brightwork.** These pull harder than any other pair, because an alloy and a bumper
  trim share `chrome` while the tyre has only `tyre`. The E36 at `chrome = 12.0` with nothing
  opposing it put its tyres at 170 triangles a corner, which is a ring coarse enough for the alloy
  inside it to cut across — the same artefact as the original fault, arrived at from the other side.
* **A warning that stays put is geometry, not budget.** The AE86's tyres sit at 31–45% error at
  2,300 triangles a corner and do not improve when given more. That is a model problem, and the tell
  is a high triangle count and a high error at the same time.

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

### Culling, and why the fix is a whole part at a time

The console culls back faces and the visibility sweep does not, so something has to reconcile them.
A source model is no help: both BMWs declare **every** material `doubleSided`, so the glTF flag
carries no information at all. What the sweep does instead is render each view twice, once as the
console will and once with everything drawn, and note each triangle that is the nearest surface,
faces away, and leaves its pixel to something further off when culled. That is the question culling
actually asks, asked once with the answer kept.

**The answer is applied to a whole part, and that is a retreat from something better.** A part is not
the unit a model winds consistently — the E36 mirrors its right-hand side and brings the winding
across with it, so its off-side wheels read entirely as their own back face, while the Golf merges
its whole exterior into one 50,880-triangle primitive with the grille inside it. Per part cannot say
that the grille is a sheet and the wing beside it is not.

So it was per triangle, with each part cut into a culled piece and a two-sided piece. That is the
better answer in principle and it was a worse one on screen. Cutting a mesh means decimating the two
halves with nothing relating one to the other, so both drift and the cut opens into a crack through
the bodywork. Pinning the shared row of vertices — which meshoptimizer supports, and which a test
confirmed held the seam — was not enough, because pinning fixes the seam and leaves the two interiors
free. Measured as background pixels enclosed by the car's own silhouette, the split was worse than
not splitting on four of five cars and worse than the *original* on four of five.

What is left is a fraction: a part whose back-facing share passes `TWO_SIDED_SHARE` is drawn whole
with culling off. The threshold is 15%, chosen because 35% loses the Golf's grille entirely and 5%
measures identically to 15% while unculling more. It costs culling across a whole part where a sheet
inside it was the reason — 16–52% of a car's compiled triangles, the E36 worst — against a class of
crack that cannot happen if no mesh is ever divided.

### What a decimator will not touch, and what happens next

Two things stop meshoptimizer's topological pass dead, and both of them ended up on screen.

**Geometry that was drawn twice.** The E39's front bumper is 15,784 triangles of which 3,201 are a
second copy of a triangle already in it, corner for corner and the same way round. Nothing on screen
says they are there — they rasterise the same pixels the same colour — but every edge they share is
an edge with four faces on it, and there is no collapse a decimator will make across that. Welded
without noticing, the bumper had 6,248 non-manifold edges out of 17,770 and the simplifier could not
reduce it by a single triangle. Welding drops the repeats now, which is what makes the part
simplifiable at all. Only repeats with the *same* winding: the other way round is a sheet a model
deliberately made two-sided by backing it with itself.

**A prune that takes everything.** `Prune` removes disconnected components whole, so a part that is
a hundred small shells and no large one has nothing left once the target is small enough — which is
every part of every car at LOD2. An empty answer used to be read as "the simplifier could not help"
and the original was kept, which is the worst available outcome: the original is then far enough over
the target to trip the sloppy vertex-clustering pass below, and that is how a bumper becomes
shrapnel. Collapse alone always returns a surface, so when pruning empties a part the simplifier is
asked again without it.

Between them these two are what the coarse levels were failing on. LOD2 of both BMWs used to have
the roof and half the greenhouse missing — a car you could see the scenery through from forty-five
metres — and it was neither a budget nor a renderer fault: it was the sloppy pass being handed the
whole part because the two passes before it had each been read as a failure.

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
  headlight       left at ( 0.46, 0.61,  2.04), 0.38 m across, 40 m beam   named in the model
  tail light      left at ( 0.66, 0.75, -2.30), 0.36 m across              named in the model
  reverse light   left at ( 0.56, 0.75, -2.34), 0.36 m across              named in the model
```

Almost none of that is configured. The material sorter has always put lenses in the `light`
category, so the parts are already known; where a lamp is and how big it is are measured off the
lens itself — from the *mean* of its vertices and their spread, not from a bounding box. That is not
a nicety: a box is fixed by its two extreme vertices, so one strip of trim modelled into the same
mesh drags it the whole way. The E39's rear lens mesh runs from 0.78 up to 1.29, which put its tail
lamps a quarter of a metre above the lamps, on the bodywork, where they plainly looked wrong. What a lens is *for* is the part a model does not answer — nothing about a red lens
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

### What a lens is painted

A lit lens is its own colour at full brightness, which means the colour is scaled until its
brightest channel reads as lit rather than each channel being floored. Flooring is the one thing
that must not be done to a coloured lens: a tail lamp's dark red encodes to (0.54, 0.16, 0.16), and
lifting every channel to 0.35 leaves a grey-pink — every rear lamp on every car painted the colour
of a lamp that is switched off and dusty.

**And only to a lens that has a colour.** A lamp cluster is not all lens: the E39's
`tail_light_lod0` is 846 triangles of the dark grey backing the red lens is set into, and scaling a
grey until its brightest channel is lit does not make it a brighter grey, it makes it white. Both of
that car's rear clusters were coming out as white blobs with some red in them. A neutral surface has
no ratio between its channels for the scaling to preserve, so it keeps the brightness the model gave
it — which leaves a white headlight lens white, because it was already there, and a grey backing
grey.

### What is not a lamp

`reflector` reads as the silvered bowl behind a bulb and was in the keyword list for exactly that
reason. On the seven cars here it names that on none of them: the E39 wears it on a 4.5 m
mirror-finish shell running the length of the car and the 190E on `INT_Reflector` in the cabin.
Between them that was 10,712 of the E39's 12,712 `light` triangles — 84% of a category weighted at
8.0, spent on parts that are not lamps — which left the four real lenses a couple of hundred
triangles between them. It is out of the list, and no car lost a lamp.

The general shape of that mistake is worth recognising, because `Light_Map` was the same mistake
one model earlier: a word that names a lamp on the car it was learned from and something else
entirely on the next one. The check is the `Lights:` block in the report, which says where every
lamp landed, and the `light` line in the budget table, which says how much of the category is
actually lens.

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

### Where they are drawn

Wherever the bodywork is, which means the car's whole attitude and not merely its heading. The
renderer turns a car by yaw, then pitch, then roll before drawing a vertex of it, and a lamp placed
by yaw alone stays level while the body noses down the hill. This pass falls 7.4 cm a metre — 4.2
degrees — and at that angle a headlight 1.86 m ahead of the origin belongs 14 cm lower than a level
frame puts it, while a tail lamp 2.06 m behind belongs 15 cm higher. It reads as two separate faults,
the brake lights too low and the headlights too high, and it is one.

The glow itself sits on its lens, three centimetres proud of the glass and no more. Keeping it out of
the panel it is set into is the depth bias's job — a camera-facing disc centred on a lens is very
nearly coplanar with it, exactly like a light pool on the road, and `sceGuDepthOffset` is what both
of them use. Standing the glow off far enough to win on geometry alone takes a hand's breadth, which
is visible as a gap between a car and its own lights.

### What it costs

Two draw calls a frame, whatever the number of cars, because both passes build world-space triangles
into one buffer. A lamp glow is eight triangles and a beam is twelve, and a beam is only resolved
within 70 m — past that a car keeps its lamps and loses its beam, and past the fog at 330 m it has
neither. The trace has a column for each, so a field of cars can be priced:

```
      field   us/frame     verts  car draws  lamps  beams
      1 car      64,601    85,615       13      2      2
     4 cars     124,735   119,194       52      8      8
     8 cars     203,015   160,374      103     16     16
```

Geometry is not where lighting shows up: 8 cars carry 960 vertices of lamps and beams against
160,374 for the frame, six tenths of a per cent. What it costs is fill, and there the measurement
contradicts the assumption it was made under. Turning each pass off for one car:

```
  1 car, everything    65,412 us
  1 car, no beams      58,688 us      the beams cost 6.7 ms
  1 car, no glows      52,253 us      the glows cost 13.2 ms
```

The glows are twice the beams, not a rounding error beside them. Both are additive passes with the
depth test on, and a camera-facing disc rasterises a great many fragments that the bodywork behind it
then discards — a beam lies on open road, where most of what it rasterises survives.

That reading is worth the detail, because it was measured twice either side of a change that took the
glows from 49 lit pixels to 256 — five times the light on screen, for the *same* 13 ms. What a
discarded fragment costs is what it costs to rasterise; the depth test only decides whether anybody
sees it.

Treat the ratio as indicative rather than as the console's: this is PPSSPP's software rasteriser,
which pays per fragment on a CPU, and the GE does not. The levers if hardware disagrees are
`lights::BEAM_FAR` and `LAMP_FAR`, which decide how far away each effect stops existing, and the glow
radius the compiler measures off each lens.

## Levels of detail

A car carries three copies of itself: LOD0 for the one being driven, LOD1 beyond 18 m, LOD2 beyond
45 m. Each is decimated from the welded original rather than from the level above, so the coarsest
carries one decimation's error and not three.

With eight cars on screen this halves the vertex load for 0.05% of pixels differing at all. It costs
file size: the E36 is 775 KB, of which 128 KB is the texture, 5 KB is the silhouette, and the rest
is geometry across three levels.

The coarse levels are 3,000 and 1,200 against LOD0's 24,000, and they were 4,500 and 1,800 against
15,000. Cutting them while raising LOD0 is close to free and was what paid for the raise: at 4,500
and 1,800 the two of them were 30% of every file, spent on a car that is eighteen metres away and a
hundred pixels wide.

That is what used to make the arena the binding limit. Seven cars run from the AE86's 716 KB to the
R34's 905 KB and come to 5.6 MB together, against an arena of 6 MB — so the eighth car did not fit,
and the answer was going to be another megabyte of `.bss` on a machine with 24 MB of it, bought to
hold six cars nobody was looking at.

What was actually wrong is that residency was proportional to how many cars were on the *stick*
when it only ever needed to be proportional to how many were on *screen*, which is one. So the
arena is two slots now, the car being driven and the car arriving, and the fleet can be any size.
The saving is real in both directions: a shipping build reserves 2.5 MB where it used to reserve 6,
and the limit that replaced it — 1.25 MB for one file — is a number the compiler can enforce.

A devtools build keeps five slots, at 6.25 MB, because `--mode 13` and `--mode 14` below exist to
price a field of *different* models and one car drawn eight times prices none of it.

It came down from 5.54 when the atlas grid was sized by the images rather than the materials, which
is not a saving anybody asked for and is worth understanding rather than pocketing: `UV_WEIGHT`
prices a unit of texture slide against a metre of geometry, and a unit of atlas space is now more
texels than it was, so a collapse that drags a decal across a panel costs the simplifier more than
it used to and it stops sooner. That is the right direction — the slide really is more visible now
— but it means the triangle counts moved on every car without any budget changing.

## Reversing lamps

Twenty-two of the twenty-three cars never showed one, and the models all plainly have them. The
reason is that they do not *have* them as geometry: on most of these the whole rear cluster — tail,
indicator and reversing lens — is one painted part, and on the S15 every lamp on the car is a single
mesh with the differences living in a texture. There is nothing to find. The lamp detector reads
names and geometry, and the E39 is the only model in the set that names its lenses, which is exactly
the one car that had them.

The models are not being misread, which is the first thing to rule out. Searching all twenty-three
source files for `reverse`, `reversing`, `backup` or `rueckfahr` finds the word in exactly one:
`bmw_e39_free`, as `reverse_light_lod0`, which is the one car whose lamps come out measured. The
E36 contains `backlight`, and that is a trap rather than a miss — it is
`E36_coupe_backlight_BMWE36_glass_0`, the rear *window*, which is what a backlight is on a car.
Adding it to the word list would turn a rear screen into a reversing lamp.

Colour looked like the answer and is not. A reversing lamp is the white lens in a cluster of red
ones, which is a fact about cars rather than a guess — but these lenses are textured, so their
material's base colour is white whatever the lens looks like. Testing it marked the E36's
uniformly red cluster as having a reversing lamp, and testing it *only* on untextured materials
found nothing at all, on any car.

So they are derived rather than detected. Where a car has tail lamps and no reversing lamps, the
reversing lamps are placed from the tail lamps: at 0.85 of their distance from the centreline,
level with them, 4 cm further back. Those numbers are the E39's own — the one car that names its
lenses, measured — and they generalise where a position would not, because a reversing lamp is
inboard of the tail lamp on every car ever built.

The *size* is not derived, and that is worth saying because the obvious thing does not work. A tail
lamp's radius is the `[0.15, 0.35]` clamp's ceiling on seventeen of the twenty-three cars, because a
measured quadrant holds a whole cluster rather than one lens — so scaling it just returns the
ceiling, and the first attempt produced glows twice the width of the E39's real reversing lamp,
hanging off the corners of the car. A number that is saturated carries no information. The derived
lamps are given 0.16 m outright, which is the E39's measured 0.150 rounded up, and a reversing lens
is small on every car. The report says `derived from
the tail lamp` rather than `named in the model`, and anything in `[lights]` wins outright.

Brake lenses are still never guessed, and the difference is worth stating. A brake lens is red like
the tail lens beside it and sits in the same place; nothing but a name tells them apart, so there is
nothing to derive from. The runtime lights the tail lamps harder under braking instead.

## Camber

Eighteen of the twenty-three cars are stanced, and carry between one and five degrees of camber
modelled into their wheels. The E36 is the extreme at 5.0 degrees on all four corners.

That is a problem for the one thing the renderer does to a wheel, which is spin it: a wheel is
turned by one rotation about the car's X axis, and a wheel whose axle is baked into its vertices at
an angle does not turn under that — it sweeps a cone once per revolution, which reads as a buckled
rim. It is invisible standing still, which is why it survived until somebody drove one.

So the axle is measured off the source tyre, the wheel's vertices are stored **upright**, and the
angle goes in `WheelDef::camber` for the renderer to put back with a rotation about Z, applied
after the steering and before the spin. Wheel, hub and lean each move in the order the real parts
do.

Measuring it is a small piece of linear algebra rather than a guess. A tyre is a disc — about
0.28 m through and 0.58 m across — so the direction it varies least in is its axle, which is the
smallest eigenvector of the vertex covariance, found by inverse iteration. It is taken on the source
part before decimation, because a coarse level makes a lopsided disc and the axis wanders with it:
the same measurement on a 280-triangle LOD gives answers off by tens of degrees. The check that it
worked is that left and right come out mirrored to a tenth of a degree on every car, which a broken
estimator does not do by accident.

`azview` applies it too. It has to: a viewer that drew a stanced car standing straight would stop
agreeing with the console about the one thing it is for.

## Silhouettes

Picking a car reads its file, and a memory stick is not fast. Read whole, that stops the title
screen dead for a fraction of a second on every press of L or R — on the one screen where L and R
are pressed repeatedly. So the file arrives a chunk a frame instead, and what stands in the lay-by
while it does is the car's own shape: a few hundred triangles, flat and near-black, culling off.

That shape is a fourth copy of the car, and it is written **in front of every other section**, at
byte 112, so the load's first chunk already holds it. The read is issued the moment the button is
pressed rather than on the next frame, which is the difference between a car replaced by its shadow
and a car that blinks out of the lay-by on its way to being one — that gap existed, and a scripted
capture is what found it.

The geometry was LOD2's to begin with, reused on the argument that a level built to be recognisable
at 45 m is already a shape. It is not. A silhouette is 22 KB of proof that "coarse" and "coarse in
the right way" are different things, and the E36 was the worst of it: a crushed can with slivers
hanging off it, wheels standing outside a body that had shrunk away from them. Four things were
wrong, and each is worth knowing because each is a different kind of mistake.

**The budget went to parts with no outline in them.** LOD2 shares triangles across every category
by measured pixels, and the E36's interior is 4,841 triangles of seats and door cards *inside the
shell*. So the silhouette is built from `body` and `window` only — the shell, and the glass that
fills the greenhouse, without which the cabin is a hole you can see the sky through. Parts the
visibility sweep never saw a single pixel of are dropped as well: floor pans and inner wings are
`body` too, and category alone does not catch them.

**It was decimated piece by piece.** That is right for a car, where each piece has its own material
and the seams are real, and it is wrong for one flat shape: at a few hundred triangles every piece
shrinks away from its neighbours and the car arrives covered in cracks. `simplify::reduce_shell`
welds the whole shell together on position alone — 5 mm, which would be far too coarse for a car
and is nothing to an outline — and simplifies it as **one mesh**. That is also most of the size
saving: the seams stop carrying two copies of every vertex, and the E36 went from 763 vertices to
197.

**It was decimated the wrong way.** `reduce` collapses edges to minimise error over the surface,
and the cheapest error a car body offers is the concave step where one panel meets another — the
foot of a C-pillar, the shut line behind a bonnet. Those steps are what a three-box saloon *is*.
The E39 came out a smooth wedge with no boot and no bonnet, and needed 2,400 triangles and 31 KB
before the shape came back. Vertex clustering instead — `simplify_sloppy`, which `reduce` keeps
only as a last resort and calls "obviously wrong for a body panel" — preserves extent rather than
smoothness, and extent is the only thing a silhouette has. The same E39 is correct at 597
triangles.

**The wheels were decimated at all.** A tyre is a tube; at 23 triangles it is a bent sliver, and the
rim that would fill it is `chrome` and excluded. But a wheel's outline does not have to be
discovered — it is a circle of a radius the compiler already measured, on an axle it already
located. Four generated twelve-sided cylinders, 48 triangles each, are exactly right from every
angle.

The result is **8–13 KB a car**, around 1% of the file, against 22 KB before. `--silhouette` on
`azview` draws one, which is how all of the above was found:

```bash
cargo run --release -p anglezero-asset --bin azview -- \
    assets/compiled/bmw_e39.azcar out.png --silhouette --yaw 270 --pitch 6 --size 640x300
```

The budget is a **grid** resolution, not a triangle count, and that is the thing to remember when
one looks wrong. Clustering snaps vertices to a lattice sized by the target, so the same number
buys less on a longer car: at 600 the M5 was missing 3.9% of itself — a strip of sill down its
whole length and its front air dam — while the median car was at 0.6%. The default is 1,000 now,
and the M5 and the 500 ask for 1,400 in their own configs. `scripts/silhouette_check.py` is what
measures this; it renders each car against its silhouette and reports what is not covered.

One thing is knowingly given up. Clustering swallows anything thinner than a grid cell standing off
the body, so the R34 loses its GT-R wing, and no budget buys it back — the wing's standoff shrinks
with the grid about as fast as the grid does. A missing wing is a detail off a car that still reads
as the right car; the E39 under edge collapse read as a different kind of car. `silhouette` in a
car's config raises the budget for a car whose shape needs it.

The header had no room left for the offset — `LIGHT_COUNT` ends at byte 110 of a 112-byte header —
so it is stored in the two bytes that remain, **in 16-byte units**. Growing the header instead
would have made every car already on a memory stick unreadable, which is the same reason the
version has never been bumped for a new section. A car compiled before this reads as a car with no
silhouette and pops in rather than fading in.

## Measuring

`--mode 13` and `--mode 14` draw four and eight cars, alternating every car resident, so that
switching vertex buffers and material state between different cars is part of what is timed.
Entering either mode reads cars into the spare slots to have something to alternate between, which
is a stall of a second or so on the button press and is why it is a devtools build that does it:

```bash
scripts/psp_glitch.py --node 1200 --burst 60 --frames 12 --mode 14 --label eight-cars
```

The vertex and draw-call counts in the trace are exact. The microseconds are PPSSPP's software
rasteriser and are only good for comparing one run against another — it is fill-bound where the
console is not, so it under-reports what vertices cost. Real numbers come off the hardware, from
the `CAR` and `US` fields in the on-device overlay.

## Which way round a surface is

The console culls back faces and a source model cannot be trusted to have wound anything. Every
material on both BMWs and on the Golf is declared `doubleSided`, which is the exporter's way of
saying it never checked — so the flag says nothing, and honouring it would mean drawing whole cars
with culling off.

What can be measured is the thing that actually matters, which is not "is this wound inwards" but
**"does culling this cost the picture anything"**. The visibility sweep therefore draws each of its
72 views twice, once with everything and once culled the way the GE culls, and compares them: a pixel
whose nearest surface faces away, and which the culled pass leaves to something further off or to
nothing at all, is a pixel where culling opens a hole. The triangle that was closing it is marked.

Three things about that are worth knowing.

* **It is per triangle, not per part.** The E36 could have got away with per part — its mirrored
  instances came out of the exporter with the whole right-hand side wound inwards, headlight,
  alloys and all, so the parts are uniform. The Golf merges its entire exterior into one
  50,880-triangle primitive with the radiator grille inside it, and no part-wide answer can say
  that the grille is a sheet while the wing beside it is not.
* **The comparison sweep runs at 512 pixels, not 128.** The coarse buffer is right for asking how
  much of the car a part is and useless for asking about one slat of a grille: at 3.3 cm a pixel the
  Golf's grille came back with 209 triangles flagged out of some thousands, and drew as a hole with
  a few slats left in it.
* **A part is split rather than flagged.** The triangles that need it become a piece of their own,
  which welds, is allocated and decimates separately, and is emitted as a second mesh over the same
  vertices with `MESH_TWO_SIDED` set — one more draw call, no more triangles, and the seam between
  the two is one the simplifier will not collapse across, which is what stops a grille being smeared
  into the bumper it sits in.

It comes to 0.5% of the Golf's triangles and 3% of the E36's, and it is the difference between a
grille and a hole you can see the scenery through. Cars go from 13 draw calls to between 16 and 25.

## When a part is missing rather than wrong

Two different mechanisms delete a part, and they want opposite fixes.

**The visibility sweep can drop it outright.** A part owning no pixels from any of 72 viewpoints is
removed before the budget is shared. That sweep runs at 128 px a side — 3.3 cm a pixel on a 4.2 m
car — which resolves a door handle and does not resolve a foglight behind a bumper aperture, a mesh
in a bumper opening, or the panel behind a kidney grille. The E36 was losing 98 parts and 21,294
triangles that way.

Existence is therefore settled by the finer sweep that already runs for culling, at 512 px a side
or 8 mm a pixel; the coarse sweep still decides the *share*, which is all it was ever good for. The
report's `Visibility:` line is the one to read — if it says a suspicious number of parts, they are
gone before any weight can help them.

**Or decimation can flatten it.** A part that survives the sweep still competes for triangles on
measured pixels, and a lattice always loses: a grille slat or a mesh square is a pixel from where
the camera stands and a wing is a thousand. The tell is a part that is *present but flat* — a black
slab across a bumper opening rather than a hole in one. That is what `[reduce.parts]` is for, and
the E36's front needed three of them.

A useful signature for a lattice: **more vertices than triangles.** `BMWE36_black.008` is 27,360
vertices against 18,084 triangles, which is hundreds of disconnected little squares and could not
be anything else. `inspect` prints both.

## When a part looks wrong

A wheel is about twenty pixels on a 480-wide screen, which is too few to tell a bad mesh from a
small one. Render the part out of the compiled asset instead — on its own, large. `azview` does
exactly that, with the console's own rules: opaque meshes then blended ones, culling except where the
mesh or material says otherwise, and the same 16-bit depth buffer over the same 0.4 m to 2400 m
frustum, which is about 2.4 mm at eight metres and is why shells millimetres apart fight.

```bash
cargo run --release -p anglezero-asset --bin azview -- \
    assets/compiled/bmw_e39.azcar /tmp/nose.png --yaw 30 --pitch 10 --dist 3 --look 0,0.5,2
```

`--only body`, `--hide interior` and `--lod 2` narrow it down; `--white` puts a pale background
behind the car, which is how you tell a black part from a hole. **`--no-cull` is the diagnostic**:
run it with and without, and anything that appears is something culling is throwing away.
`--silhouette` draws the stand-in instead of the car, with the console's rules for that — flat,
culling off — and prints what it costs.

Rendering at float depth, or without culling, shows a clean car and hides the fault being looked
for. That is not hypothetical — a throwaway rasteriser that drew every triangle showed the far side
of a tyre through the hole in the near side, made every wheel look like a featureless blob, and cost
a wrong diagnosis before it was noticed.

### When the model is wrong about a material

Some faults are not a budget, a category or a UV — the model simply says the wrong thing, and no
amount of triangles will fix it. `[[materials.colour]]` says otherwise, keyed by the same name
fragments the category rules use, and applied before the category touches the material so a lens
named there is still lit and a window still gets its alpha.

```toml
[[materials.colour]]
match = ["BMWE36_paint.004"]   # 3,860 triangles the model calls rgba(0.41, 0.41, 0.41)
rgb = [28, 28, 30]             # sRGB, 0-255, which is what a colour picker gives you
```

`flat = true` on the same rule throws the material's *image* away as well, which is for a texture
that is not a picture of the surface. The Golf's `material` carries a matcap — three strips of sky,
horizon and sunset, meant to be indexed by the surface normal — and sampled as a flat texture it is
a gradient that the grille bars and bumper strakes sit a texel from the orange end of. Every change
to how the atlas resamples moved them across it, and the whole front of the car came out
olive-yellow. There is no tile size that settles that, because a matcap has no stable answer to
sample.

`inside = { min = [...], max = [...] }` narrows a rule to a box in compiled car space — metres, Y
up, Z forward, wheels on the ground and wheelbase centred, which is the same coordinates
`azview --look` takes. It is there for the grain below a material, and below a *part*: the Golf's
grille bars, bumper strakes, mirrors, window surrounds and the ring round its badge are all one
50,880-triangle part of one material, so the badge cannot be named. It can only be located.

```toml
[[materials.colour]]
match = ["material"]
rgb = [26, 26, 28]                # black plastic, everywhere else on the car
flat = true

[[materials.colour]]
match = ["material"]
rgb = [185, 188, 196]             # …except the badge, which is silver
inside = { min = [-0.06, 0.57, 2.12], max = [0.06, 0.70, 2.30] }
```

**Make the box tighter than the thing looks.** A vertex colour is interpolated across the triangle
it belongs to, so a box that catches one vertex of a long triangle bleeds along the whole of it: at
±0.08 in Z this one caught the inboard end of the grille bar and the badge's colour smeared halfway
to each headlight. Find the axis that actually separates them — here Z, because the badge stands
proud of the bar it sits on — rather than making the box small in all three.

`anglezero-asset inspect --material <name> <model.glb>` lists every part wearing a material, with
node names and bounds. That is how you find out whether a part rule would have worked before
reaching for a box.

### When the image is not a picture

Three of these models carry an image that is not a photograph of the surface it is on, and each one
breaks the atlas in its own way. They are worth recognising, because all three look like a texturing
bug and none of them is one.

| What it is | How it looks | What to do |
|---|---|---|
| A **matcap** — strips of sky, horizon and sunset, indexed by the normal | A gradient that flips colour whenever the tile size moves | `flat = true` and name a colour |
| A **palette** — a strip of colour swatches the UVs point at | Neighbouring swatches blend into each other once filtered | `palette = [...]` |
| A real picture | Fine detail goes blocky when there are too few texels | more texels, then the gutter |

The Golf has both of the first two. Its interior is palette-mapped: `Int_Plas_SH`, `Leather_Int`
and `GolfCarpet_Int` share a sheet of dashboard symbols with about forty-eight colour swatches
across its top sixteen rows, and every part wearing them has V in [0.001, 0.068]. They address the
swatches and nothing else.

An atlas cannot carry that, at any tile size this packer produces: two thirds of the image reduced
to sixty-two texels is roughly one texel a swatch, so nearest sampling picks an arbitrary
neighbour and linear sampling blends two. That is what smeared the Golf's door cards yellow the
moment the car was filtered. `palette = ["Int_Plas", "Leather_Int", "Carpet_Int"]` does the lookup
in the compiler instead, at the source's full 512 px, and multiplies the result into the vertex
colour exactly where the hardware's `Modulate` would have. It is exact rather than approximate, it
costs no tile, and nothing the atlas does later can disturb it.

**`inspect --material <name>` prints each part's source UV range**, which is how a palette gives
itself away: an island a few hundredths tall is not reading a picture.

**The way to find which material owns a surface is the rule itself.** Paint a candidate a colour
nothing else on the car wears, compile, and look at where it lands:

```toml
[[materials.colour]]
match = ["material"]
rgb = [255, 0, 255]
```

That is how all of these were settled, and it is much faster than reasoning about names. Two guesses
about the E36's white lower trim — `etki_modparts.001` and `BMWE36_chrom` — were both wrong, and
`BMWE36_chrom` turned out to be the headlight surrounds.

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

One per car, 256×256, and every source material samples somewhere inside it. The runtime merges
parts into six materials, so a draw call covers the paint and the badges on it at once — a texture
per material could not be bound. Packing means the console binds once per car and never switches.

Materials with no image sample white. That is what keeps the whole thing additive: glTF's base
colour is `texture × factor`, the factor is already baked into the vertex alongside the light term,
so white multiplies to exactly the colour that vertex had before textures existed. Anything
untextured looks the same as it always did.

### Why the grid is sized by the images, not the materials

A material with no image needs one texel. Counting it into the grid anyway is what held every car
at a 32 px tile, and 32 px is where both of the texture faults came from: the S15's tyre tread is
about a texel tall there and came out blocky, and the Golf's grille sits one texel from the orange
tail-light strip in the same image, so a one-texel gutter round each tile — which fixes the tread
outright — moved the grille's band into the orange and turned it olive-yellow. The gutter was never
wrong. There was no room for it.

So the grid holds one tile per *image*, plus one shared tile in which every flat colour gets a
single texel, and it is the smallest square that fits — not the smallest power of two, which was
costing a factor of two per axis for nothing. Nothing needs a tile to be a power of two; the atlas
is, and that is the only size the hardware has an opinion about.

| Car | materials | textured | tile before | tile now |
|---|---|---|---|---|
| VW Golf R | 21 | 15 | 32 px | 64 px |
| BMW E39 | 61 | 6 | 32 px | 85 px |
| Toyota AE86 | 20 | 6 | 32 px | 85 px |
| Nissan R34 | 26 | 17 | 32 px | 51 px |
| Nissan S15 | 27 | 16 | 32 px | 51 px |
| BMW E36 | 57 | 17 | 32 px | 51 px |
| Mercedes 190E | 62 | 60 | 32 px | 32 px |

The 190E is the case that gains nothing, and it is honest that it does not: it really does bring
sixty images, so it really does need a 8×8 grid. Everything else was paying for tiles that held one
colour.

The one thing to look at after changing this is any material whose image is a gradient rather than
a picture. `material` on the Golf is a matcap — three strips of sky, horizon and sunset — and its
interior panels are a lookup into that ramp, so which band a panel lands in moves when the tile is
resampled. Point-sampled at 32 px it read grey; at 64 it reads yellow in places. Both are honest
samples of the same source and the finer one is closer to it, but it is a visible change with no
fault behind it, and it is the kind of thing to recognise rather than chase.

### The gutter, and why it needed the tiles first

Each tile carries a one-texel ring of its own edge colour, and the UVs address the content inside
it. That is what lets the car be sampled `Linear` — bilinear filtering near a tile's edge reaches a
texel outside it, and in an atlas that texel belongs to a different material, so without a ring the
only safe filter is `Nearest`. Nearest is what made the S15's tread a handful of blocks: it is a
fine pattern crushed into a tile, and point-sampling gives one texel per pixel with nothing in
between.

The gutter was built and reverted twice before the tiles were resized, and the reason is
arithmetic. The ring costs two texels of the tile in each axis — 6% at 32 px, 3% at 64 — and the
Golf's grille samples a band about a texel tall in `Light_Map`, whose image holds the dark
headlight photo directly above the orange tail-light strip. Rescaling the content by 6% moved that
band across the boundary and the grille came out olive-yellow, twice, and the second time with
sampling still on `Nearest` — which is what proved the fault was in the atlas rather than in the
filter. Nothing about the ring was wrong. There was no room for it.

`azview` samples bilinear too, and has to keep matching. A gutter judged against a point-sampling
viewer is judged by a renderer that cannot show either what the ring smooths or what it prevents.

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
packed next door, so a tiling pattern shows its edge rather than somebody else's paint. What is
inside the square is mapped onto the tile's content, gutter excluded, so UV 0 and 1 are the outer
edges of the first and last content texel: the image is addressable end to end, and what a filter
reaches for past either end is the ring, which is a copy of the texel it is already standing on.

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
| Dodge Charger R/T 1969 | [Sketchfab](https://sketchfab.com/3d-models/1969-dodge-charger-rt-261de8013c4e4fb0884a8106bd3212a7) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Honda Civic Type R EK9 | [Sketchfab](https://sketchfab.com/3d-models/2000-honda-civic-type-r-ek9-4012706636b843d49ad3974af8fba593) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Abarth 500 | [Sketchfab](https://sketchfab.com/search?q=abarth+500+ddiaz) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| BMW E30 (Pandem) | [Sketchfab](https://sketchfab.com/search?q=pandem+e30+ddiaz) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-4.0 |
| Nissan 350Z (Rachel's) | [Sketchfab](https://sketchfab.com/search?q=rachel+350z+ddiaz) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Mazda RX-7 (VeilSide C-II) | [Sketchfab](https://sketchfab.com/3d-models/1993-veilside-c-ii-mazda-rx-7-fast-furious-e145e48775b8460da9de844769cc407b) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Honda Civic EJ (ViS Racing) | [Sketchfab](https://sketchfab.com/3d-models/1993-honda-civic-coupe-vis-racing-fast-furious-c4c2acfb6fe9444c85364ddd8c8454b0) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Lancia Delta HF Integrale Evo 2 | [Sketchfab](https://sketchfab.com/3d-models/1994-lancia-delta-hf-integrale-evo-2-227eb681355d468d96726b419adaf519) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Volkswagen Golf R32 Mk4 | [Sketchfab](https://sketchfab.com/3d-models/2002-volkswagen-golf-r32-mk4-57fa4f06ba3e4eb09572796a4d76f546) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Citroen Xsara WRC | [Sketchfab](https://sketchfab.com/3d-models/2005-citroen-xsara-wrc-sebastien-loeb-edd3efd463b24035aa231de33ed59aaa) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Honda NSX (Rocket Bunny) | [Sketchfab](https://sketchfab.com/3d-models/2015-rocket-bunny-racing-honda-nsx-e30ef65ebbcc4526bc727edcce93c9b3) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| McLaren 720S (LB-Works) | [Sketchfab](https://sketchfab.com/3d-models/2017-mclaren-720s-lbworks-97ea6542578c4ae1a0045c80551bd903) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-4.0 |
| Nissan Silvia S14 (Vertex Ridge) | [Sketchfab](https://sketchfab.com/3d-models/2018-vertex-ridge-s14-silvia-kouki-c5958cec759f41bbb770fca138bdb6ce) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Audi RS6 Avant | [Sketchfab](https://sketchfab.com/3d-models/2020-audi-rs6-avant-980dbda2cbbb4bae8decaed2fa80aa0c) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Lamborghini Murcielago (LB Silhouette Works) | [Sketchfab](https://sketchfab.com/3d-models/2024-lbsilhouette-works-murcielago-gt-evo-64f859d01c9345429874513cf561ec12) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| BMW M5 Sedan | [Sketchfab](https://sketchfab.com/3d-models/2025-bmw-m5-sedan-4ab34c43bada482b8e467be97ebaeba3) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |

**Nineteen of the twenty-three are non-commercial, and eighteen of those are also share-alike.** Fine for a hobby
build; a paid release could ship the E36, the E39 and the 190E and nothing else. `ShareAlike` is the
stricter half of that — it reaches the derivative, which is the compiled `.azcar` and arguably the
screenshots of it, not merely the sale.

The credit line is read out of each model's `asset.extras` by the converter and drawn on the title
screen, so this table is a convenience: the attribution ships whether or not anybody updates it.
