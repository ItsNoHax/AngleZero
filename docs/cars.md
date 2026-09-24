# Cars

Cars are compiled offline from glTF models into `.azcar` files. The host does all the expensive
work (parsing, visibility, decimation, texture packing); the PSP validates the file once and hands
its buffers to the GE. Adding a car requires no Rust changes.

```
assets/source/bmw_3-series_e36.glb   source model, unmodified, not in git
assets/configs/bmw_e36.toml          what the compiler cannot measure
assets/compiled/bmw_e36.azcar        compiled car, committed
```

## Adding a car

```bash
# 1. List compiled cars and source models without a config.
scripts/cars.sh list

# 2. Inspect the model. Reports only; converts nothing.
cargo run --release -p anglezero-asset -- inspect assets/source/your_car.glb
cargo run --release -p anglezero-asset -- inspect --deep assets/source/your_car.glb

# 3. Write assets/configs/your_car.toml (start with `name` and `source`).

# 4. Compile and read the report.
scripts/cars.sh build your_car

# 5. Render the result and its silhouette.
cargo run --release -p anglezero-asset --bin azview -- \
    assets/compiled/your_car.azcar /tmp/car.png --yaw 210 --pitch 14 --dist 6
cargo run --release -p anglezero-asset --bin azview -- \
    assets/compiled/your_car.azcar /tmp/sil.png --silhouette --yaw 270 --pitch 6

# 6. Check lamps and silhouette coverage across the fleet.
scripts/lights_check.py
scripts/silhouette_check.py
```

Steps 2–5 usually take several iterations. The `/add-car` Claude Code skill automates the loop.

Copy the result to `PSP/GAME/AngleZero/CARS/` on the memory stick. The title screen lists every
`.azcar` found there, sorted by filename.

## Limits

| Limit | Value | Enforced by |
|---|---|---|
| File size | 1.25 MB (`azcar::MAX_CAR_BYTES`) | Compiler refuses to write larger files |
| Cars on the stick | 128 (`catalogue::MAX_ENTRIES`) | Catalogue |
| Cars in memory | 1 displayed + 1 loading | Car arena, `src/psp/car.rs` |

Current cars are 0.7–1.0 MB. Load time is the practical cost: a car streams in 32 KB chunks,
one per frame (~29 frames on hardware).

## Inspecting a source model

`inspect` reads accessor bounds only and runs in milliseconds. Check:

| Question | How to tell | Fix |
|---|---|---|
| Units | Report guesses from the bounding box. A 0.04-unit-long car is in centimetres | `scale` |
| Facing | Not derivable from bounds. Headlights should be at +Z | `[spawn] yaw = 180` |
| Wheels | Four similar parts at mirrored X and two Z stations | `[wheels] match` |
| Part structure | One node per part, or one merged mesh per material | Handled automatically; merged wheels are split geometrically |
| Lattices (grilles, meshes) | More vertices than triangles | `[reduce.parts]` |

A car facing backwards looks correct on the title screen and wrong from the chase camera. Wheels
are classified after `[spawn] yaw` is applied, so the steering wheels are always the headlight end.

`inspect --material <name>` lists every part using a material, with node names, bounds and source
UV ranges.

## Configuration

Only configure what the compiler cannot measure. Most cars need `name`, `source`, `scale`,
`triangles`, `lods` and `[wheels] match`.

```toml
name = "Abarth 500"
source = "2014_abarth_500_1.4_16v.glb"
scale = 100.0
triangles = 24000
lods = [3000, 1200]
silhouette = 1400

[wheels]
match = ["Abarth_500_2014_Wheel"]

[reduce]
drop = ["Base_Geo_lodA_Base_Geo_lodA"]   # display plinth
```

### Top level

| Key | Default | Description |
|---|---|---|
| `name` | required | Title-screen name (shown uppercase) |
| `source` | — | Model filename in `assets/source/`. Used by `scripts/cars.sh`; `convert` rejects a mismatch |
| `scale` | 1.0 | Uniform scale to metres |
| `triangles` | 10,000 | LOD0 budget. All current cars use 24,000 (one uses 32,000) |
| `lods` | `[]` | Coarser budgets, nearest first. Current cars use `[3000, 1200]` |
| `silhouette` | 1,000 | Silhouette grid resolution. Raise for long or tall cars |

### `[spawn]`

| Key | Description |
|---|---|
| `yaw` | Degrees about Y for a model not facing +Z |
| `offset_x`, `offset_y`, `offset_z` | Shift after grounding and wheelbase centring. Rarely needed |

### `[wheels]`

| Key | Description |
|---|---|
| `match` | Node-name fragments identifying wheel parts (tyre, rim, disc, caliper) |
| `radius` | Override the rolling radius measured from the hub height |
| `[wheels.front_left] node` (and `front_right`, `rear_left`, `rear_right`) | Name one corner explicitly |

### `[reduce]`

Weights multiply a category's share of the triangle budget.

| Key | Default | Description |
|---|---|---|
| `body`, `window`, `tyre`, `chrome` | 1.0 | Category weights |
| `interior`, `light`, `wheel` | tuned defaults | Category weights; `wheel` stacks on top of the part's category |
| `drop_hidden` | true | Drop parts the visibility sweep never sees |
| `drop` | `[]` | Node-name fragments to remove entirely |

`[reduce.parts]` weights individual parts by node-name fragment (matched against node and parent,
case-insensitive; matches multiply):

```toml
[reduce.parts]
"amdb11_brakedisc" = 0.4
"amdb11_rim" = 6.0
```

### `[materials]`

| Key | Description |
|---|---|
| `body`, `window`, `tyre`, `interior`, `light`, `chrome` | Material-name fragments forcing a category |
| `palette` | Materials whose texture is a swatch palette; sampled at compile time into vertex colours |
| `[[materials.colour]]` | Override a material's colour (see below) |

```toml
[[materials.colour]]
match = ["material"]              # material-name fragments
rgb = [185, 188, 196]             # sRGB 0–255
flat = true                       # optional: discard the texture
inside = { min = [-0.06, 0.57, 2.12], max = [0.06, 0.70, 2.30] }   # optional: car-space box
```

`inside` uses compiled car space (metres, Y up, Z forward, wheels on the ground), the same as
`azview --look`. Vertex colours interpolate across triangles, so make the box tighter than the
feature and constrain it on the axis that separates it from its neighbours.

### `[lights]`

| Key | Default | Description |
|---|---|---|
| `enabled` | true | `false` removes all lamps |
| `steer` | false | Headlights turn with the front wheels |
| `range` | 40 | Headlight throw, m |
| `spread` | 3.8 | Beam half-width where it lands, m |

Per-lamp tables: `headlight_left`, `headlight_right`, `tail_left`, `tail_right`, `brake_left`,
`brake_right`, `reverse_left`, `reverse_right`.

| Key | Description |
|---|---|
| `node` | Node-name fragment for the lens. Position and size are still measured |
| `at` | `[x, y, z]` placement when the model has nothing to point at |
| `radius` | Glow radius, m |
| `color` | `[r, g, b]` |
| `intensity` | Brightness multiplier |

```toml
[lights.headlight_left]
node = "HL_Glass_ONM"
[lights.headlight_right]
node = "HL_Glass_ONM"
```

The same fragment for both sides is normal; parts are split at the centreline first. `node` matches
parts in any category.

### `[handling]`

Omitted keys use the game's reference car.

| Key | Unit |
|---|---|
| `mass` | kg |
| `inertia` | kg·m² (derived from mass if omitted) |
| `front_axle`, `rear_axle` | m from centre of mass |
| `engine` | N at full throttle from rest |
| `top_speed` | m/s where drive force has tailed off |
| `brake` | N |
| `steer_lock` | rad at standstill |
| `grip` | multiplier; < 1 slides earlier |

## Compiler

### Pipeline

1. **Load and weld.** Vertices weld on position, colour and UV. Exact duplicate triangles with the
   same winding are removed (they create non-manifold edges that block decimation). Reversed
   duplicates are kept; they are intentional two-sided sheets.
2. **Categorise** materials into `body`, `window`, `tyre`, `interior`, `light`, `chrome` by
   keyword match on material and node names.
3. **Visibility sweep.** 72 viewpoints. A coarse 128 px pass measures each part's screen share; a
   512 px pass decides whether a part exists at all and whether culling it opens holes.
4. **Allocate** the budget by measured pixels × category weight × part weight.
5. **Decimate** each part to its share with meshoptimizer's attribute-aware simplifier.
6. **Build LODs, silhouette, texture atlas and lamp records**, then write the file.

### Budget allocation

- **Two passes.** Parts that stop short of their share (the simplifier cannot improve them further)
  are pinned at what they used; the surplus goes to parts limited by their target.
- **Mirrored corners are averaged** before allocation, so four wheels get equal shares despite the
  sweep seeing the near side more.
- **5 % tolerance** on "filled its share", because mirrored geometry decimates in slightly different
  orders.
- **Pins are levelled per category** at the best any corner reached, so one wheel stopping early
  does not cap its mirror images.

### Error threshold

The simplifier's "free" error limit is `min(5 mm, 0.2 % of the part's extent)`. A flat absolute
limit erased badges and small trim entirely.

### Decimation fallbacks

meshoptimizer's `Prune` removes whole disconnected components and can empty a part made of many
small shells. When it does, the part is simplified again without pruning. Vertex clustering
(`simplify_sloppy`) is a last resort only.

### Back-face culling

The GE culls back faces and source models' `doubleSided` flags are unreliable. The 512 px sweep
renders each view culled and unculled, and marks triangles whose culling exposes something behind
them. A part is drawn with culling off (`MESH_TWO_SIDED`) when more than `TWO_SIDED_SHARE` (15 %)
of its triangles are marked.

Parts are never split into culled and unculled pieces: decimating the halves independently opens
cracks along the seam.

### Levels of detail

| Level | Used from | Budget (typical) |
|---|---|---|
| LOD0 | 0 m | 24,000 |
| LOD1 | 18 m | 3,000 |
| LOD2 | 45 m | 1,200 |

Each level is decimated from the welded original, not from the level above.

### Wheels and camber

Wheel radius comes from hub height above the ground. Camber is measured from the source tyre (the
axle is the smallest eigenvector of the vertex covariance), wheel vertices are stored upright, and
the angle is stored in `WheelDef::camber`. The renderer applies steer, then camber, then spin.

### Lights

Lamp position and size come from the **mean and spread** of each lens's vertices, not a bounding
box. Each corner is split into connected lenses first; the lamp is the lens with the most geometry.

A lens's kind comes from its name where clear, otherwise from which end of the car it is on, and
the config overrides both.

Detection prefers no lamp to a wrong lamp:

- Lenses on the centreline are ignored.
- Unpaired lamps are ignored.
- A left/right pair must mirror; otherwise both positions are reported and neither is used.
- Brake lamps are never inferred. Unnamed brake lamps fall back to tail lamps lit brighter.

**Reversing lamps** are derived when a car has tail lamps and no named reversing lamps: 0.85 × the
tail lamp's distance from centreline, same height, 4 cm further back, radius 0.16 m. These values
come from the one model (E39) that names its reversing lenses. Config entries override them.

Lit lens colour is scaled so its brightest channel reaches full brightness. Neutral (grey or white)
surfaces keep their authored brightness.

Lamps are transformed by the car's full yaw, pitch and roll. Glows sit 3 cm proud of the lens and
use `sceGuDepthOffset` to stay in front of the body.

Runtime cost: two additive draw calls per frame for all cars. Beams are drawn within
`lights::BEAM_FAR` (70 m), glows within `lights::LAMP_FAR` (330 m). In PPSSPP's software
rasteriser, glows cost ~13 ms and beams ~7 ms per frame for one car, almost entirely fill;
hardware figures may differ.

### Silhouette

A flat stand-in drawn while the car loads. It is stored immediately after the 112-byte header so
the first chunk contains it.

- Built from `body` and `window` parts only, excluding parts the sweep never saw.
- The shell is welded on position (5 mm) and simplified as one mesh by vertex clustering, which
  preserves outline rather than surface.
- Wheels are generated 12-sided cylinders at the measured radius.
- `silhouette` sets the clustering grid, so longer cars need a higher value.
- Thin standoff features (e.g. the R34's wing) are lost at any budget.

Typically 8–13 KB per car. `scripts/silhouette_check.py` measures how much of each car its
silhouette fails to cover. Cars compiled before silhouettes existed load without one.

### Texture atlas

One 256 × 256 texture per car. The grid has one tile per source image plus one shared tile holding
a texel per flat colour, and is the smallest square that fits. Each tile has a one-texel gutter so
the car can be sampled bilinearly.

- Untextured materials sample white, so vertex colour alone determines their appearance.
- UVs outside [0, 1] are clamped.
- `convert --atlas out.png` writes the atlas for inspection.

## `azview`

Renders a compiled car with the console's rules: opaque then blended meshes, per-mesh culling,
16-bit depth over 0.4–2400 m, bilinear textures.

```bash
cargo run --release -p anglezero-asset --bin azview -- <car.azcar> <out.png> [options]
```

| Option | Description |
|---|---|
| `--yaw`, `--pitch`, `--dist`, `--fov` | Camera (defaults 210°, 14°, 7 m, 45°) |
| `--look x,y,z` | Orbit target in car space |
| `--size WxH` | Output size (default 960x640) |
| `--lod N` | Level to draw |
| `--only <name>`, `--hide <name>` | Filter by material or mesh name fragment |
| `--mesh N` | Draw only mesh N (repeatable) |
| `--by-mesh` | Colour by mesh |
| `--no-cull` | Disable culling. Anything that appears is being culled away |
| `--no-tex` | Vertex colour only |
| `--white` | Pale background, to tell black parts from holes |
| `--lamps`, `--lamp <kind>` | Draw lamp glows (all, or `headlight`/`tail`/`brake`/`reverse`) |
| `--silhouette` | Draw the silhouette instead |

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| Car is huge or tiny | Wrong units | `scale` |
| Car drives backwards | Model faces −Z | `[spawn] yaw = 180` |
| No headlights | Lenses categorised as glass, or unpaired | `[lights.headlight_*] node` |
| Lamps on the wrong panel | Detector picked the wrong lens | `[lights]` entries; check with render mode 15 |
| Part missing entirely | Dropped by visibility sweep (see `Visibility:` in the report) | `drop_hidden = false` to confirm; `[reduce.parts]` |
| Grille or mesh is a flat slab | Lattice lost the allocation | `[reduce.parts]` weight |
| Wheel is a disc, no spokes | Brake hardware behind the spokes took the share | Weight the rim up and hardware down, or `drop` it |
| Alloy cuts through tyre | Tyre starved (often by a high `chrome` weight) | Lower `chrome`, raise `tyre`/`wheel` |
| Category warning persists at high triangle counts | Source geometry problem, not budget | Accept or fix the model |
| Wrong colour on one material | Model is wrong | `[[materials.colour]]` |
| Colour flips when atlas changes | Texture is a matcap | `[[materials.colour]]` with `flat = true` |
| Adjacent colours bleed together | Texture is a swatch palette | `[materials] palette` |
| Fine texture detail is blocky | Too few texels | Check the atlas with `--atlas` |
| Car renders as stray pixels | Renderer, not asset (see [PSP notes](psp-notes.md)) | — |

To find which material owns a surface, paint a candidate an unmistakable colour and recompile:

```toml
[[materials.colour]]
match = ["candidate_material"]
rgb = [255, 0, 255]
```

## Benchmarking

Render modes 13 and 14 draw four and eight different cars (see
[Diagnostics](diagnostics.md#render-modes)):

```bash
scripts/psp_glitch.py --node 1200 --burst 60 --frames 12 --mode 14 --label eight-cars
```

Vertex and draw-call counts in the trace are exact. PPSSPP timings are only useful relative to each
other; use the on-device `CAR` and `US` counters for real figures.

## Licences

Source models are not in git. The credit line is read from each model's `asset.extras` and drawn on
the title screen, so attribution ships inside every `.azcar`.

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
| Toyota RAV4 (2010) | [Sketchfab](https://sketchfab.com/3d-models/2010-rav4-limited-edition-450e36f0992c4ae09bd6c8d509a51a77) | [Teddy168](https://sketchfab.com/Teddy168) | CC-BY-4.0 |
| Mini Cooper S (F56) | [Sketchfab](https://sketchfab.com/3d-models/2014-mini-cooper-s-f56-bec89391257c426d824a8b0434bbcf6b) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Exotic Rides W70 (2016) | [Sketchfab](https://sketchfab.com/3d-models/2016-exotic-rides-w70-9c2b97cd6e104671a2f90992c64cc849) | [Ddiaz Design](https://sketchfab.com/ddiaz-design) | CC-BY-NC-SA-4.0 |
| Lada 2109 Sputnik | [Sketchfab](https://sketchfab.com/3d-models/lada-2109-sputnik-instagram-404b8c9d153b4e99a24eb16601ce7397) | [Bogdan Styopin](https://sketchfab.com/boom.dam) | CC-BY-4.0 |

20 of 27 models are non-commercial (19 also share-alike). Only these may ship in a commercial
release: E36, E39, 190E, E30 (Pandem), 720S, RAV4, Lada 2109. Share-alike terms extend to derived
works, which includes the compiled `.azcar`.
