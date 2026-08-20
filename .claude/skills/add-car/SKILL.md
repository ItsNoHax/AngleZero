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
| Are there four wheels, round, in the arches? | `[wheels] match` patterns — see **When the car steers with the wrong wheels** |
| Any holes into the cabin, or surfaces lit from inside? | run again with `--no-cull`; what appears is what culling throws away |
| Is the glass glass, are the lamps lamps? | `[[materials.category]]`, but see below |
| Is anything a colour it should not be? | `[[materials.colour]]` |
| Does the silhouette read as this car — wheels, roofline, no wedge? | `silhouette` budget; see **Silhouettes** in `docs/cars.md` |

Two rules that were learned the expensive way and are worth obeying without re-deriving:

- **Render the part on its own and large.** A wheel is twenty pixels on a whole-car shot, which is
  far too few to tell a bad mesh from a small one. `--only <name>`, `--hide <name>`, `--mesh <n>`
  and `--look x,y,z` narrow it down; `--white` puts a pale background behind the car, which is how
  you tell a black part from a hole. A five-metre contact sheet once hid four inside-out wheels.
- **Never recategorise a part you have not looked at alone.** Moving a part between categories
  changes whether it blends, whether it is culled and how it is lit, and doing it on a guess
  produces a different fault that looks like progress.

### Iterating

Change **one thing**, rebuild that car, re-render, compare. A config change is cheap and a wrong
diagnosis is not: two changes at once and you have learned nothing about either.

Stop when the car reads correctly from both angles and the silhouette reads as the same car — not
when it is perfect. These are 24,000 triangles seen from ten metres at night on a 480×272 screen,
and there is a real cost to gold-plating: the budget spent making a badge crisp is budget taken off
the bodywork.

If **three passes** have not fixed something, stop and say so plainly, with the render and what you
tried. Some faults are in the model rather than the config — `docs/cars.md` has a whole section on
**When the model is wrong about a material** — and a car that needs source edits is a different job
from a car that needs a config line.

## 6. Land it

- `cargo test --workspace` — the asset tests run the compiler over its own output.
- Add the model to the licence table at the bottom of `docs/cars.md`. The credit is a licence
  condition, not a courtesy.
- Commit the `.azcar` **with** its config. The compiled car is a build artifact that is deliberately
  in git, and a config without the car it produced leaves the next person unable to tell whether the
  car on disk came from it.
- To see it in the game rather than in a renderer, `/psp-preview` captures the title screen and
  `/psp-deploy` puts it on the console. Cars are offered in filename order, so a new car appears
  where its name sorts.
