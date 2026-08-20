---
name: add-car
description: Turn a source car model in assets/source/ into a compiled .azcar the game can drive, and check by rendering that it actually looks right. Use whenever asked to add, convert, compile or import a car, to work through the unconverted models, to fix a car that came out wrong — hovering, backwards, inside-out, miscoloured, too heavy — or to judge whether a compiled car and its silhouette are good enough to ship.
---

# Adding a car

A car is a file, not code. `tools/anglezero-asset` turns a `.glb` into a `.azcar` on this machine,
the console opens that file and draws it, and adding a car changes no Rust at all. What it does
take is judgement, because a source model off a scanning site is authored for a renderer with no
budget and no opinions, and almost every one of them is wrong in at least one way that only shows
up when you look at it.

So the shape of this job is: **decide as little as possible up front, convert, then look at what
you got and fix what you can see.** The looking is the part that cannot be skipped. Numbers in a
conversion report will not tell you the car is facing backwards.

## 1. Find what needs doing

```bash
scripts/cars.sh list
```

Configured cars with their sizes, then every model in `assets/source/` that no config claims.
Those are the ones waiting. If the user did not name a car, show them the list and ask which —
each one is a few minutes of conversion and a judgement call about whether it came out well, so
working through several unattended is rarely what somebody wants.

## 2. Look at the model before converting it

```bash
cargo run --release -p anglezero-asset --bin anglezero-asset -- inspect assets/source/<model>.glb
```

Costs about 28 ms, because the accessors carry their own bounds and nothing has to be decoded. Four
things in the report decide the config, and getting them from here is much cheaper than getting
them from a render:

- **Units.** The bounding box's longest axis is the car's length. A real car is 4.0–5.3 m; a model
  reading 0.053 units wants `scale = 100`. Guess from the box and confirm in the render.
- **Facing.** The convention is headlights at **+Z**. The report cannot guess this and does not try
  — a bounding box is the same box either way round — so read it off the part positions: a mesh
  named `BumperF` or `Grille` at positive z means the model already agrees, and one at negative z
  means `[spawn] yaw = 180`. Get this wrong and the car drives the entire descent backwards, which
  is invisible on the title screen and obvious from the chase camera.
- **Attribution.** `Extras:` carries the title, author and licence, and the converter lifts them
  straight out of there into the compiled car — there is no `credit` key to write, and the title
  screen draws whatever the model came with. Four of the seven cars here are under non-commercial
  licences that *require* the credit, so check it actually appeared (it is printed in the
  conversion report and drawn on the title screen), and if the model carries none, find the
  attribution before going further. Add the model to the licence table at the bottom of
  `docs/cars.md` either way.
- **What the car is made of.** The material list, largest first, is what the category rules have to
  sort into body, window, tyre, interior, light and chrome. Names like `_Glass_`, `_Light_`,
  `_Rim_`, `_Interior_` mostly sort themselves; the ones to look at are the big ones with vague
  names.

`--deep` reads every vertex and adds what only the data can say. `--material <name>` lists every
part wearing a material, which is how a config tells them apart by node.

## 3. Write the smallest config that could work

`assets/configs/<car>.toml`. Start with almost nothing:

```toml
# One line on what this car is and anything odd about it.
name = "Dodge Charger"
source = "1969_dodge_charger_rt.glb"

scale = 100.0
triangles = 24000
lods = [3000, 1200]
```

`source` is not decoration: it is the only record of which model a car came from — the names never
match, and the models are not in git — and `convert` refuses to run if it disagrees with the file
it was handed.

Add nothing else yet. Every other key exists to fix a specific fault, and adding them speculatively
means you cannot tell which one did what. `docs/cars.md` under **The config** has the full set, and
the sections after it are organised by symptom.

## 4. Convert

```bash
scripts/cars.sh build <car>
```

Roughly a minute. Read the report rather than skipping to the file size — it says what the budget
bought, what it had to drop, and what it could not simplify at any budget. Warnings here are often
the whole diagnosis.

Two hard limits: the file must come in under **1.25 MB**, which is one residency slot on the
console and which the compiler refuses to exceed, and the compiled car must have found its
**wheels** — a car with none gets default proportions and skates.

Four wheels is not the same as four *whole* wheels. A wheel is about a fourteenth of the car's
length, and the report warns when it is much less, because that means the patterns caught the rim
and missed the tyre. It is a quiet fault — the car compiles, has four wheels, and then drives with
them turning too fast for the road while sitting a tyre's thickness too low.

Be careful about what you drop, too. Names lie in both directions: the Golf R32's tyres live in a
part called `TARMAC_TYRE_WALL`, which reads like scenery and is a sidewall, and dropping it put the
car out on bare rims. Before dropping anything, render it — `--only` it, or drop it and compare —
rather than deciding from the name.

## 5. Look at it, and keep looking until it is right

This is the loop, and it is the reason this skill exists. `azview` renders a compiled car offline
in a couple of seconds with the console's own rules — the same draw order, the same culling, the
same 16-bit depth buffer over the same frustum — so a fault you can see here is a fault the console
has, and one you cannot is usually not there.

```bash
AZ="cargo run --release -q -p anglezero-asset --bin azview --"
$AZ assets/compiled/<car>.azcar /tmp/car_34.png  --yaw 210 --pitch 14 --dist 6 --size 900x600
$AZ assets/compiled/<car>.azcar /tmp/car_side.png --yaw 270 --pitch 6  --dist 6.5 --size 900x600
$AZ assets/compiled/<car>.azcar /tmp/sil.png --silhouette --yaw 270 --pitch 6 --dist 6.5 --size 640x300
```

Then **read the PNGs** and judge them. You know what a 1969 Dodge Charger looks like; that
knowledge is the instrument here. Ask, in this order, because each question is cheap to answer and
the early ones invalidate the later ones:

| Look for | If it is wrong |
| --- | --- |
| Is it that car at all — right silhouette, right proportions? | `scale`, or the wrong model |
| Is the nose where the nose should be? | `[spawn] yaw = 180` |
| Is it standing on its wheels, not hovering or sunk? | `[spawn]` offset, `[wheels] radius` |
| Are there four wheels, round, in the arches, **with rubber on them**? | `[wheels] match` patterns — see **When the car steers with the wrong wheels** |
| Any holes into the cabin, or surfaces lit from inside? | run again with `--no-cull`; what appears is what culling throws away |
| Is the glass glass, are the lamps lamps? | `[[materials.category]]`, but see below |
| Is anything a colour it should not be? | `[[materials.colour]]` |
| Does the silhouette read as this car — wheels, roofline, no wedge? | `silhouette` budget; see **Silhouettes** in `docs/cars.md` |

For that last row there is a measurement rather than a judgement, and it is worth running on every
car because the ways a silhouette goes wrong are not the ways you notice by looking at one:

```bash
scripts/silhouette_check.py                    # every car
scripts/silhouette_check.py --overlay bmw_m5   # and see *where* it is wrong
```

It renders each car and its silhouette from the same camera and reports how much of the car the
silhouette fails to cover, having first eroded the car's outline by a few pixels so that ordinary
decimation shrinkage does not drown the real faults. Under 2% is fine. Above it, the overlay paints
missing geometry red, and red is the whole diagnosis: a red panel is a categorisation fault (the
part is in a category silhouettes are not built from), a red ring at a wheel is a wheel-pattern
fault, and a red strip along the sills means the budget is too coarse for the car's length — raise
`silhouette` in its config.

Two rules that were learned the expensive way and are worth obeying without re-deriving:

- **Render the part on its own and large.** A wheel is twenty pixels on a whole-car shot, which is
  far too few to tell a bad mesh from a small one. `--only <name>`, `--hide <name>`, `--mesh <n>`
  and `--look x,y,z` narrow it down; `--white` puts a pale background behind the car, which is how
  you tell a black part from a hole. A five-metre contact sheet once hid four inside-out wheels.
- **Never recategorise a part you have not looked at alone.** Moving a part between categories
  changes whether it blends, whether it is culled and how it is lit, and doing it on a guess
  produces a different fault that looks like progress.

### Converging on a fault

Change **one thing**, rebuild that car, re-render, compare. A config change is cheap and a wrong
diagnosis is not: two changes at once and you have learned nothing about either.

**Every pass must narrow the fault, not try another idea.** That distinction is the whole skill.
Guessing at node names and rebuilding to see what happens can run all day without converging,
because the search space is thousands of parts; narrowing halves it every time. When a car is
wrong, work down this ladder — each rung is a measurement, and each one tells you which rung to
stand on next:

1. **Which category is it in?** `--only body`, `--only window`, `--only light`, and so on. One of
   them contains the offending geometry. This is one command per category and it always works.
2. **Where is it, and how big?** `azview` prints every compiled mesh with its centre and radius
   before it draws. A part that does not belong to the car announces itself as a radius much
   larger than the body's, or a centre far off the origin.
3. **Which source part is that?** `inspect --deep` prints every part with its triangle count and
   its bounding extent. Match on the numbers — the count and the extent are a fingerprint, and a
   part whose extent is near the model's whole bounding box when no body panel is, is the thing
   you are looking for. `inspect --material <name>` goes the other way, from a material to the
   parts wearing it.
4. **Confirm before fixing.** `--hide <fragment>` removes candidates from the render without a
   rebuild. When the fault disappears, you have its name.

The numbers in the conversion report are evidence, not decoration. A car reporting a length that is
not the real car's length has something in it that is not the car. A category "losing its shape" is
a budget problem. Wheels at an implausible radius mean the wheel patterns caught the wrong parts.
Read them before rendering anything.

Keep going until the car is right. A fault that resists three attempts is not a reason to stop; it
is a sign that the attempts were guesses and the ladder above has not been climbed. Work it out.

The only honest reason to stop short is a fault that **cannot be fixed from the config at all** —
the model itself is broken or needs geometry edited, which `docs/cars.md` covers under **When the
model is wrong about a material**. If that happens, say exactly which part, what is wrong with it,
what you measured, and which rung of the ladder you got to. "I tried three things" is not that.

Stopping *polishing*, though, is different and is worth doing early. Once the car reads correctly
from both angles and the silhouette reads as the same car, it is done. These are 24,000 triangles
seen from ten metres at night on a 480×272 screen, and the budget spent making a badge crisp is
budget taken off the bodywork.

## 6. Land it

- `scripts/lights_check.py` and `scripts/silhouette_check.py` — both check every car, both are
  quick, and both catch things that are invisible in a still render. The lamp one exists because
  the Golf R32 shipped with its tail lights under the driver's elbow: the detector falls back to
  position when a model does not name its lenses, and what it finds can be the dashboard lighting
  or one merged mesh holding every lamp on the car. That is fixed with `[lights.tail_left]` and
  friends — `node` to point at the real lens, `at` to place it by hand when there is no separate
  lens to point at.
- `cargo test --workspace` — the asset tests run the compiler over its own output.
- Add the model to the licence table at the bottom of `docs/cars.md`. The credit is a licence
  condition, not a courtesy.
- Commit the `.azcar` **with** its config. The compiled car is a build artifact that is deliberately
  in git, and a config without the car it produced leaves the next person unable to tell whether the
  car on disk came from it.
- To see it in the game rather than in a renderer, `/psp-preview` captures the title screen and
  `/psp-deploy` puts it on the console. Cars are offered in filename order, so a new car appears
  where its name sorts.
