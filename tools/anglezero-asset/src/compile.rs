//! Source model to `.azcar` bytes.
//!
//! The order of the stages here is not arbitrary — each one only works because the one before it
//! ran:
//!
//! 1. **Find the wheels.** Everything after this needs to know which parts turn.
//! 2. **Place the car.** Wheels on the ground, wheelbase centred on the origin. The measurements
//!    that make this possible come from the wheels, so it cannot happen first.
//! 3. **Sort materials into categories.** Six bins instead of fifty-seven materials.
//! 4. **Bake colour into the vertices.** Base colour and the light term, together, because the
//!    renderer keeps GU lighting off.
//! 5. **Weld.** Only now, because the weld key includes the baked base colour, which stops a
//!    black trim strip from bleeding into the paint it touches.
//! 6. **Simplify.** Only now, because before welding the E36's 366,209 vertices are 219,966
//!    positions split at seams, and a simplifier cannot collapse an edge that is really six edges.
//!
//! What comes out is one draw call per category per wheel, and wheel geometry stored about its own
//! hub so the runtime can steer and spin it.

use std::collections::HashMap;

use angle_zero::azcar::{
    self, Category, LightDef, MaterialDef, Mesh, WheelDef, HEADER_BYTES, MAGIC, MATERIAL_BLEND,
    MATERIAL_TWO_SIDED, NO_TEXTURE, NO_WHEEL, TEXTURE_5650, TEXTURE_HEADER_BYTES, VERSION,
    VERTEX_TEX_F32_COLOR_8888_F32,
};
use angle_zero::mesh::Vertex;

use crate::categorise;
use crate::config::CarConfig;
use crate::lamps;
use crate::mat::Bounds;
use crate::model::SourceModel;
use crate::report::Report;
use crate::simplify::{self, Attr};
use crate::texture;
use crate::visibility;
use crate::wheels::{self, CORNER_NAMES};
use crate::Result;

/// How much light a surface facing straight up gets over one facing the horizon.
///
/// The key is purely vertical, and that is a decision rather than a simplification: the mesh is
/// stored in body space and the car spends most of its life yawing, so a key with any sideways
/// component would swing around the bodywork as the car turns. A vertical one is stable through
/// any amount of steering, which is what this game does.
const AMBIENT: f32 = 0.45;
const DIFFUSE: f32 = 0.55;
/// Lights read as lit rather than shaded: a lamp lens with a shadow on it looks broken.
const LIGHT_FLOOR: f32 = 0.92;
/// What fraction of a part has to read as its own back face before the whole part is drawn with
/// culling off. See where it is used.
const TWO_SIDED_SHARE: f32 = 0.15;
/// How bright a lamp lens's brightest channel is made, so the glass looks lit rather than merely
/// pale. Not 1.0: the additive glow the renderer puts over the lens has to have somewhere to go.
const LENS_LIT: f32 = 0.88;
/// How far from grey a lens has to be before being lit is worth doing to it, as a fraction of its
/// own brightest channel. Below this there is no hue to preserve and scaling only whitens it.
const LENS_HUE: f32 = 0.2;

/// How far away each level takes over, in metres.
///
/// The chase camera sits about 11 m behind the player's car, so LOD0 has to cover everything
/// nearer than that; 18 m is the first distance at which a car is small enough on a 480-wide
/// screen for a halved triangle count not to show. These are starting points — the benchmark
/// modes are how they get checked against something.
const LOD_DISTANCES: [f32; 3] = [0.0, 18.0, 45.0];

pub struct Compiled {
    pub bytes: Vec<u8>,
    pub report: Report,
    /// The packed texture as RGBA, kept only so `--atlas` can write it out to be looked at.
    pub atlas: Vec<u8>,
}

/// One source part, on its way to becoming part of a draw call.
///
/// Kept separate until decimation is done, because a budget is only meaningful per part: a part is
/// the unit that is either on the outside of the car or not.
#[derive(Clone)]
struct Piece {
    /// Colours here are the material's base, unlit. The light term and the texture coordinate live
    /// alongside until welding and decimation are done with them — see `simplify`.
    vertices: Vec<Vertex>,
    attrs: Vec<Attr>,
    indices: Vec<u32>,
    pixels: u64,
    /// What the config says this part is worth, over and above its category. Multiplies its
    /// measured pixels when the bucket's budget is shared out between its parts.
    weight: f32,
    /// Whether the console's culling would turn this part into a hole, so it is drawn with culling
    /// off. A whole part at a time — see where it is decided for why it is never a part of one.
    two_sided: bool,
    node: String,
}

/// One output draw call: a category, optionally belonging to a wheel.
#[derive(Clone)]
struct Bucket {
    category: Category,
    /// `None` for the body.
    wheel: Option<u8>,
    pieces: Vec<Piece>,
    /// Merged from the pieces once decimation is finished.
    vertices: Vec<Vertex>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    /// Source triangles that went in, before any simplification.
    source_triangles: usize,
    /// Pixels this bucket's parts own across the visibility sweep. The budget follows this.
    pixels: u64,
    weight: f32,
    /// Where in `indices` the two-sided pieces begin, once flattened. Everything before it is
    /// culled and everything from it on is not, which is what lets one bucket be drawn as two
    /// meshes over one vertex array.
    two_sided_from: usize,
}

impl Bucket {
    fn key(&self) -> (Option<u8>, Category) {
        (self.wheel, self.category)
    }

    /// What this bucket is worth, and what it already costs.
    fn weights(&self) -> (f64, usize) {
        (
            self.pixels as f64 * self.weight as f64,
            self.pieces.iter().map(|p| p.indices.len() / 3).sum(),
        )
    }

    /// Concatenates the surviving pieces into the one array the bucket is drawn from.
    ///
    /// Culled pieces first, then the ones that have to be drawn two-sided, so the boundary between
    /// them is a single index and the bucket can be issued as two draws over one vertex array
    /// rather than as two buckets that would each have wanted their own share of the budget.
    fn flatten(&mut self) {
        // `false` sorts before `true`, and the sort is stable, so this only moves the two-sided
        // pieces to the end and leaves every other piece where it was.
        self.pieces.sort_by_key(|p| p.two_sided);
        let mut boundary = 0;
        for p in &self.pieces {
            let base = self.vertices.len() as u32;
            self.vertices.extend_from_slice(&p.vertices);
            self.uvs.extend(p.attrs.iter().map(|a| a.uv));
            self.indices.extend(p.indices.iter().map(|i| i + base));
            if !p.two_sided {
                boundary = self.indices.len();
            }
        }
        self.two_sided_from = boundary;
        self.pieces.clear();
    }
}

pub fn compile(model: &mut SourceModel, config: &CarConfig, budget: usize) -> Result<Compiled> {
    let mut report = Report::new(&config.name, model);

    let found = wheels::identify(model, config);
    for w in &found.warnings {
        report.warn(w.clone());
    }

    let placement = Placement::of(model, &found, config);
    placement.apply(model);
    let hubs: Vec<[f32; 3]> = found
        .wheels
        .iter()
        .map(|w| placement.point(w.hub))
        .collect();

    let assignment = categorise::assign(model, config, &found);
    report.note_categories(model, &assignment);

    // What the player can see, measured rather than assumed. This is what makes the budget
    // meaningful: without it a third of the E36's goes on an engine behind a closed bonnet.
    let transparent: Vec<bool> = assignment
        .categories
        .iter()
        .map(|c| *c == Category::Window)
        .collect();
    let seen = visibility::measure(model, &transparent);
    let hidden_triangles: usize = model
        .parts
        .iter()
        .zip(&seen.pixels)
        .filter(|(_, px)| **px == 0)
        .map(|(p, _)| p.triangles())
        .sum();
    report.note_visibility(&seen, hidden_triangles);

    // Everything the runtime needs to name a mesh, a material or a wheel — and the attribution
    // line, which is written first so that a car whose credit matters has it near the front of the
    // table whatever else is in there.
    // Refused here rather than on the console: a car with a zero mass puts the vehicle at infinity
    // on its first substep, and the runtime's own check would silently fall back to the default,
    // which looks like a config file that is being ignored.
    let handling = config.handling.resolve();
    report.handling = handling;
    if !handling.is_sane() {
        return Err(format!(
            "invalid handling: {handling:?}. Mass, inertia, axle distances, top speed, steering \
             lock and grip must all be above zero."
        ));
    }

    let mut strings = Strings::default();
    // Folded to uppercase for the same reason as the credit: the console's font has no lowercase
    // and draws what it lacks as blanks.
    let name_at = strings.push(&config.name.to_uppercase()) as u32;
    let credit = credit_line(model);
    let credit_at = credit
        .as_deref()
        .map(|c| strings.push(c) as u32)
        .unwrap_or(azcar::NO_CREDIT);
    if credit.is_none() {
        report.warn(
            "the source model records no author or licence, so the car carries no credit".into(),
        );
    }

    // The lamps, before the parts are walked and long before anything is decimated: a lamp is
    // measured off the lens the model arrived with, not off whatever the budget left of it. A car
    // whose headlights fall to eight triangles still has its headlights exactly where they were.
    let lamps = lamps::identify(model, config, &assignment, &found, &mut strings);
    for w in &lamps.warnings {
        report.warn(w.clone());
    }
    report.note_lights(&lamps);

    // One texture for the whole car, with every source material packed into a tile of it. Built
    // before the parts are walked because each part's UVs have to be rewritten into its material's
    // tile on the way in — after that, nothing downstream has to know an atlas was involved.
    let atlas = texture::Atlas::build(model, &config.materials);
    for w in &atlas.warnings {
        report.warn(w.clone());
    }
    report.note_texture(atlas.textured, model.images.len(), &atlas.resized);

    let mut buckets: Vec<Bucket> = Vec::new();
    let mut dropped_by_name = 0usize;
    let mut two_sided_triangles = 0usize;
    for (i, part) in model.parts.iter().enumerate() {
        if config.reduce.drop_hidden && seen.pixels[i] == 0 {
            continue;
        }
        // Named in the config as not worth drawing at all. Counted so the report can say how much
        // was left out on purpose rather than lost.
        if config.reduce.drop.iter().any(|f| {
            !f.is_empty()
                && (part.node.to_ascii_lowercase().contains(&f.to_ascii_lowercase())
                    || part.parent.to_ascii_lowercase().contains(&f.to_ascii_lowercase()))
        }) {
            dropped_by_name += part.triangles();
            continue;
        }
        let category = assignment.categories[i];
        let wheel = found.corner_of(i);
        // Wheel geometry is stored about its own hub, so the runtime can rotate it in place.
        let origin = wheel
            .map(|c| hubs[found.wheels.iter().position(|w| w.corner == c).unwrap()])
            .unwrap_or([0.0; 3]);

        let material = &model.materials[part.material];
        // A config may say the model is wrong about a material's colour outright. Applied before
        // everything the category does to it, so a lens named here is still lit and a window named
        // here still gets its alpha.
        let mut material = material.clone();
        if let Some(rgb) = config.materials.colour_for(&material.name) {
            material.base_color = [rgb[0], rgb[1], rgb[2], material.base_color[3]];
        }
        let material = &material;
        let base = base_colour(material, category);
        // …and it may say that one region of the material is a different colour again, for the
        // surface an exporter merged into something it is not. Both colours go through
        // `base_colour` and the category, because whichever a vertex ends up with has to have been
        // treated the same way; only the choice between them is per vertex.
        let region = config.materials.region_for(&material.name).map(|(r, rgb)| {
            let mut inside = material.clone();
            inside.base_color = [rgb[0], rgb[1], rgb[2], material.base_color[3]];
            (r, pack(base_colour(&inside, category)))
        });
        // A material whose image is a palette is sampled here rather than packed, at the source's
        // own resolution, and multiplied into the vertex exactly as the hardware would have
        // multiplied a tile. See `MaterialRules::palette`: an atlas cannot hold a swatch one texel
        // wide, and every attempt to make it either picked a neighbour or blended two.
        let palette = atlas.palettes.get(&part.material);
        let tile = atlas.tiles[part.material];

        let slot = match buckets.iter().position(|b| b.key() == (wheel, category)) {
            Some(at) => at,
            None => {
                buckets.push(Bucket {
                    category,
                    wheel,
                    pieces: Vec::new(),
                    vertices: Vec::new(),
                    uvs: Vec::new(),
                    indices: Vec::new(),
                    source_triangles: 0,
                    pixels: 0,
                    two_sided_from: 0,
                    // Wheels are weighted up on top of their category. They are small on screen
                    // and, with the lights, most of what says which car this is — and there are
                    // four of them sharing one allocation, so an unweighted split gives each a
                    // quarter of what a single part of the same importance would get.
                    weight: config.reduce.weight(category)
                        * if wheel.is_some() {
                            config.reduce.wheel
                        } else {
                            1.0
                        },
                });
                buckets.len() - 1
            }
        };
        let packed = pack(base);
        // Where this part's texture coordinates sit relative to the unit square, before the tile
        // clamps them into it. See `texture::unit_shift`.
        let shift = crate::texture::unit_shift(&part.uvs);
        let mut vertices = Vec::with_capacity(part.positions.len());
        let mut attrs = Vec::with_capacity(part.positions.len());
        for (j, (v, n)) in part.positions.iter().zip(&part.normals).enumerate() {
            // Against the position before the hub is subtracted, because a region is written in
            // car space and a wheel's vertices are stored about their own centre.
            let mut colour = match region {
                Some((r, inside)) if r.contains(*v) => inside,
                _ => packed,
            };
            // A part with no texture coordinates at all still gets a valid one: its tile is a flat
            // colour, so any point inside it is the same answer.
            let uv = part.uvs.get(j).copied().unwrap_or([0.0, 0.0]);
            let shifted = [uv[0] + shift[0], uv[1] + shift[1]];
            if let Some(image) = palette {
                // Multiplied after `base_colour`, because that is where the texture stage sits: it
                // is `Modulate` on the console, over a vertex that already carries the material's
                // colour and the category's treatment of it. Doing it here rather than there is
                // the only difference, and the palette's tile is white so the console's multiply
                // is now by one.
                let texel = image.sample(shifted);
                let mut c = unpack(colour);
                for k in 0..3 {
                    c[k] *= texel[k];
                }
                colour = pack(c);
            }
            vertices.push(Vertex::new(
                v[0] - origin[0],
                v[1] - origin[1],
                v[2] - origin[2],
                colour,
            ));
            attrs.push(Attr {
                light: light_at(*n, category),
                uv: tile.map(shifted),
            });
        }

        // A part is drawn two-sided, whole, if the sweep found any triangle in it that culling
        // would turn into a hole. Whole is the operative word, and it was learned the hard way: an
        // earlier version cut the part into a culled half and a two-sided half so that a grille
        // sheet could keep its culling while the bumper around it kept none, which is a better
        // answer in principle and was a worse one in practice.
        //
        // Cutting a mesh means decimating the two halves against each other with nothing relating
        // them, and both drift. Pinning the row of vertices along the cut was not enough — pinning
        // fixes the seam and leaves the interiors free, and what came out was worse tearing than
        // before on three of the five cars it was meant to help. Not cutting at all beat it on four
        // of five and beat the *original* on four of five too.
        //
        // What it costs is culling on a whole part where a sheet inside it was the reason. That is
        // a fill cost on parts a car has a few dozen of, against a class of crack that cannot
        // happen if no mesh is ever divided.
        let back_only = seen
            .two_sided_triangles(i, part.triangles())
            .filter(|b| *b)
            .count();
        let two_sided = back_only as f32 > part.triangles() as f32 * TWO_SIDED_SHARE;

        let weight = config.reduce.part_weight(&part.node, &part.parent);
        let bucket = &mut buckets[slot];
        bucket.pixels += seen.pixels[i] as u64;
        bucket.source_triangles += part.triangles();
        if two_sided {
            two_sided_triangles += part.triangles();
        }
        bucket.pieces.push(Piece {
            vertices,
            attrs,
            pixels: seen.pixels[i] as u64,
            indices: part.indices.clone(),
            weight,
            two_sided,
            node: part.node.clone(),
        });
    }

    // The four corners get the same budget, whatever the sweep happened to see of each.
    //
    // A car's wheels are the same wheel four times, but the viewpoints are not symmetric about it
    // — the near side is seen more than the off side, and the fronts more than the rears — so the
    // measured pixels differ by a factor of four between corners. Left alone that is what the
    // budget follows, and the refill pass below produced tyres of 2,648 and 647 triangles on the
    // same car: one wheel visibly rounder than the one across from it, which reads as a fault
    // rather than as detail. Averaging says the asymmetry is in the sampling, not in the car.
    for category in [
        Category::Tyre,
        Category::Chrome,
        Category::Body,
        Category::Interior,
        Category::Light,
        Category::Window,
    ] {
        let corners: Vec<usize> = (0..buckets.len())
            .filter(|&i| buckets[i].wheel.is_some() && buckets[i].category == category)
            .collect();
        if corners.len() < 2 {
            continue;
        }
        let mean = corners.iter().map(|&i| buckets[i].pixels).sum::<u64>() / corners.len() as u64;
        for &i in &corners {
            buckets[i].pixels = mean;
        }
    }

    if buckets.is_empty() {
        return Err("nothing to compile: the model has no drawable parts".into());
    }
    if dropped_by_name > 0 {
        report.note_dropped_by_name(dropped_by_name, config.reduce.drop.len());
    }
    if two_sided_triangles > 0 {
        report.note_two_sided(two_sided_triangles);
    }

    // Weld first, then spend the budget. Welding changes what a triangle costs, so a budget shared
    // out before it would be shared out against the wrong numbers. Per part, because that is the
    // unit a source model splits its seams within — nothing is gained by welding a bumper to the
    // wing it merely touches, and the boundary between them is better left alone.
    let mut welded_away = 0;
    for b in &mut buckets {
        for p in &mut b.pieces {
            welded_away += simplify::weld(&mut p.vertices, &mut p.attrs, &mut p.indices, atlas.span);
        }
    }
    report.note_welding(welded_away);

    // The budget is shared out twice: between the categories, and then between the parts inside
    // each one. Both steps are needed and the second is the one that matters most on a scanned
    // car. The E36's engine is 137,000 triangles of `body` behind a closed bonnet, visible as a
    // few pixels through the grille; given a share of its category's budget it takes a third of
    // the paint's detail with it, because a decimator handed a whole category has no idea which
    // half of it is on the outside.
    //
    // Kept before anything is decimated, so that each extra level is built from the welded
    // original. Building LOD2 out of LOD1 would carry three decimations' worth of error into the
    // level with the fewest triangles to hide it in.
    // Kept always now rather than only for the levels: the refill pass re-simplifies from it too.
    let welded = buckets.clone();

    spend_and_refill(&mut buckets, &welded, budget, atlas.span, Some(&mut report));
    if buckets.is_empty() {
        return Err("the triangle budget left nothing to draw".into());
    }

    // The extra levels, coarsest last. One that collapses to nothing is dropped rather than
    // written as a level with no draw calls in it.
    let mut levels: Vec<Vec<Bucket>> = Vec::new();
    for &lod_budget in &config.lods {
        let mut coarse = welded.clone();
        spend_budget(&mut coarse, lod_budget, atlas.span, None);
        if coarse.is_empty() {
            report.warn(format!(
                "LOD at {lod_budget} triangles collapsed to nothing and was dropped"
            ));
        } else {
            levels.push(coarse);
        }
    }

    // One material record per category actually used, across every level. A coarser level can only
    // ever be a subset of LOD0, but the mesh writer looks its material up in this list and would
    // panic rather than mis-draw if that ever stopped being true, so it is built from all of them.
    let mut categories: Vec<Category> = Vec::new();
    for b in buckets.iter().chain(levels.iter().flatten()) {
        if !categories.contains(&b.category) {
            categories.push(b.category);
        }
    }


    let materials: Vec<MaterialDef> = categories
        .iter()
        .map(|c| MaterialDef {
            color: representative_colour(&buckets, *c),
            texture: NO_TEXTURE,
            name: strings.push(c.name()),
            category: *c,
            flags: flags_for(*c),
        })
        .collect();

    let mut vertices: Vec<Vertex> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u16> = Vec::new();
    let mut meshes: Vec<Mesh> = Vec::new();
    // Where each level's meshes begin. LOD0's are first, so a reader that knows nothing about
    // levels draws exactly the car it always drew.
    let mut level_ranges: Vec<(u32, u16, usize)> = Vec::new();
    // Where LOD0 ends, so the report can say what the car the player sees costs rather than what
    // every level of it costs added together.
    let (mut lod0_vertices, mut lod0_indices) = (0usize, 0usize);

    for (level, group) in core::iter::once(&buckets).chain(levels.iter()).enumerate() {
        let first_mesh = meshes.len() as u32;
        let mut level_triangles = 0usize;

        for b in group {
            let base = vertices.len();
            if base + b.vertices.len() > u16::MAX as usize + 1 {
                return Err(format!(
                    "the compiled car needs {} vertices for {} triangles across {} levels, and \
                     the format holds {}. Lower the triangle budget, or ask for fewer LODs.",
                    base + b.vertices.len(),
                    (indices.len() + b.indices.len()) / 3,
                    level + 1,
                    u16::MAX as usize + 1
                ));
            }
            let first_index = indices.len() as u32;
            vertices.extend_from_slice(&b.vertices);
            uvs.extend_from_slice(&b.uvs);
            indices.extend(b.indices.iter().map(|i| (*i as usize + base) as u16));

            let mut bounds = Bounds::EMPTY;
            for v in &b.vertices {
                bounds.add([v.x, v.y, v.z]);
            }
            let centre = [
                (bounds.min[0] + bounds.max[0]) * 0.5,
                (bounds.min[1] + bounds.max[1]) * 0.5,
                (bounds.min[2] + bounds.max[2]) * 0.5,
            ];
            let radius = b
                .vertices
                .iter()
                .map(|v| {
                    let d = [v.x - centre[0], v.y - centre[1], v.z - centre[2]];
                    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
                })
                .fold(0.0f32, f32::max);

            let name = match b.wheel {
                Some(c) => {
                    strings.push(&format!("{}_{}", CORNER_NAMES[c as usize], b.category.name()))
                }
                None => strings.push(b.category.name()),
            };
            let index_count = (indices.len() as u32) - first_index;
            level_triangles += index_count as usize / 3;
            // One run, or two where the bucket holds sheets as well as solids: same material, same
            // vertices, one extra draw call, and the second is issued with culling off.
            let split = b.two_sided_from as u32;
            for (at, count, flags) in [
                (first_index, split, 0),
                (
                    first_index + split,
                    index_count - split,
                    azcar::MESH_TWO_SIDED,
                ),
            ] {
                if count == 0 {
                    continue;
                }
                meshes.push(Mesh {
                    first_index: at,
                    index_count: count,
                    material: categories.iter().position(|c| *c == b.category).unwrap() as u16,
                    wheel: b.wheel.map(u16::from).unwrap_or(NO_WHEEL),
                    name,
                    flags,
                    center: centre,
                    radius,
                });
            }
        }

        level_ranges.push((
            first_mesh,
            (meshes.len() as u32 - first_mesh) as u16,
            level_triangles,
        ));
        if level == 0 {
            lod0_vertices = vertices.len();
            lod0_indices = indices.len();
        }
    }

    let wheel_defs: Vec<WheelDef> = found
        .wheels
        .iter()
        .zip(&hubs)
        .map(|(w, hub)| WheelDef {
            corner: w.corner,
            steers: w.corner == azcar::WHEEL_FRONT_LEFT || w.corner == azcar::WHEEL_FRONT_RIGHT,
            name: strings.push(CORNER_NAMES[w.corner as usize]),
            hub: *hub,
            radius: config.wheels.radius.unwrap_or(w.radius * placement.scale),
            width: w.width * placement.scale,
        })
        .collect();

    // Wheel vertices are stored about their hubs, so a tyre's lowest vertex is at -0.29 in its own
    // space and on the road in the car's. Adding the hub back is what puts a bucket where the car
    // actually is, which both the bounds and the silhouette below need.
    let origin_of = |wheel: Option<u8>| -> [f32; 3] {
        wheel
            .and_then(|c| found.wheels.iter().position(|w| w.corner == c))
            .map(|i| hubs[i])
            .unwrap_or([0.0; 3])
    };

    // The car's own bounds, which is not the bounds of the vertex array.
    let mut bounds = Bounds::EMPTY;
    for b in &buckets {
        let origin = origin_of(b.wheel);
        for v in &b.vertices {
            bounds.add([v.x + origin[0], v.y + origin[1], v.z + origin[2]]);
        }
    }

    // The stand-in the console draws while the rest of this file is still being read.
    //
    // It is the coarsest level's geometry rather than a decimation of its own, and that is a
    // deliberate refusal to add a fourth budget: a level built to be recognisable at eighteen
    // metres is already a shape, it has been through the same whole-car simplification every other
    // level has, and reusing it means no new way for the bodywork to crack. Wheels are baked in at
    // their hubs, standing straight, so the whole thing draws under one transform — a stand-in has
    // no steering to do.
    let silhouette = build_silhouette(levels.last().unwrap_or(&buckets), origin_of);
    if silhouette.1.is_empty() {
        report.warn("no silhouette could be built; the car will pop in rather than fade in".into());
    }

    // LOD0's slice, not the whole array: "Compiled: 9,540 triangles" has to mean the car that is
    // drawn, or the number cannot be compared against the budget that produced it.
    report.note_output(
        &vertices[..lod0_vertices],
        &indices[..lod0_indices],
        &meshes[..level_ranges[0].1 as usize],
        &materials,
        &wheel_defs,
        bounds,
    );
    report.note_levels(
        level_ranges.iter().map(|r| r.2).collect(),
        vertices.len(),
        indices.len(),
    );

    let bytes = write(
        &vertices,
        &uvs,
        &atlas,
        &indices,
        &meshes,
        &materials,
        &wheel_defs,
        &lamps.lights,
        &strings.bytes,
        credit_at,
        name_at,
        handling,
        &level_ranges,
        bounds,
        &silhouette,
    );
    report.note_size(&bytes);

    // The console reads a car into a fixed slot, so this is a wall rather than a warning. Refused
    // here, on a development machine, naming the file and the number to lower — the alternative is
    // discovering it on a title screen as a car that declines to appear.
    if bytes.len() > azcar::MAX_CAR_BYTES {
        return Err(format!(
            "the compiled car is {} KB and the console reads one into a {} KB slot. Lower \
             `triangles`, or the `lods` after it, and compile again.",
            bytes.len() / 1024,
            azcar::MAX_CAR_BYTES / 1024
        )
        .into());
    }

    Ok(Compiled {
        bytes,
        report,
        atlas: atlas.pixels,
    })
}

/// Where the model has to move to sit where the game expects a car.
///
/// The game drives a point on the ground between the axles. A source model is wherever its author
/// left it: the E36 stands 9 cm above its own origin with its wheelbase centred 6 cm behind it.
/// Neither offset is visible in a modelling package and both are obvious in game, as a car that
/// hovers or that pivots about its back seat.
struct Placement {
    scale: f32,
    yaw: f32,
    /// Applied after scale and rotation.
    offset: [f32; 3],
}

impl Placement {
    fn of(model: &SourceModel, found: &wheels::Found, config: &CarConfig) -> Placement {
        let s = config.scale;
        let yaw = config.spawn.yaw.to_radians();

        // Ground and centre come from the wheels when there are any: a wheel touches the road by
        // definition, where a bounding box includes the wing mirrors and whatever is under the
        // sills.
        let (ground, centre_x, centre_z) = if found.wheels.is_empty() {
            let b = model.bounds();
            (
                b.min[1],
                (b.min[0] + b.max[0]) * 0.5,
                (b.min[2] + b.max[2]) * 0.5,
            )
        } else {
            let mut b = Bounds::EMPTY;
            for w in &found.wheels {
                for &p in &w.parts {
                    let pb = model.parts[p].bounds();
                    b.add(pb.min);
                    b.add(pb.max);
                }
            }
            let hub_x: f32 =
                found.wheels.iter().map(|w| w.hub[0]).sum::<f32>() / found.wheels.len() as f32;
            let hub_z: f32 =
                found.wheels.iter().map(|w| w.hub[2]).sum::<f32>() / found.wheels.len() as f32;
            (b.min[1], hub_x, hub_z)
        };

        // The offset is expressed in the space after scale and rotation, so it is computed from
        // the rotated centre rather than the raw one.
        let rotated = rotate_y([centre_x * s, ground * s, centre_z * s], yaw);
        Placement {
            scale: s,
            yaw,
            offset: [
                -rotated[0] + config.spawn.offset_x,
                -rotated[1] + config.spawn.offset_y,
                -rotated[2] + config.spawn.offset_z,
            ],
        }
    }

    fn point(&self, p: [f32; 3]) -> [f32; 3] {
        let s = [p[0] * self.scale, p[1] * self.scale, p[2] * self.scale];
        let r = rotate_y(s, self.yaw);
        [
            r[0] + self.offset[0],
            r[1] + self.offset[1],
            r[2] + self.offset[2],
        ]
    }

    fn apply(&self, model: &mut SourceModel) {
        for part in &mut model.parts {
            for p in &mut part.positions {
                *p = self.point(*p);
            }
            if self.yaw != 0.0 {
                for n in &mut part.normals {
                    *n = rotate_y(*n, self.yaw);
                }
            }
        }
    }
}

/// Shared with `wheels`, which has to classify corners in the orientation the car ends up in
/// rather than the one it was authored in.
pub(crate) fn rotate_y(p: [f32; 3], yaw: f32) -> [f32; 3] {
    if yaw == 0.0 {
        return p;
    }
    let (s, c) = yaw.sin_cos();
    [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c]
}

/// The material's colour, converted for display and clamped away from pure black.
///
/// glTF base colours are linear; the renderer writes straight to an 8-bit framebuffer, so they
/// have to be encoded to sRGB or every panel comes out far darker than the model looks in any
/// viewer. A car in this game is also lit by street lamps at night, and a panel at 0.01 linear is
/// pure black on screen — legible as a hole rather than as bodywork — so there is a floor.
fn base_colour(material: &crate::model::Material, category: Category) -> [f32; 4] {
    let floor = match category {
        // A lens is lifted rather than floored — see below — so all it needs of its own is to stay
        // out of the framebuffer's basement.
        Category::Light => 0.05,
        Category::Window => 0.04,
        _ => 0.06,
    };
    let mut out = [0.0f32; 4];
    for i in 0..3 {
        out[i] = srgb(material.base_color[i]).max(floor);
    }
    // A lamp lens is lit, and a lit lens is its own colour at full brightness.
    //
    // Flooring each channel at 0.35, which is what this used to do, is the one thing that must not
    // be done to a coloured lens: a tail lamp's dark red encodes to (0.54, 0.16, 0.16), and lifting
    // every channel to 0.35 leaves (0.54, 0.35, 0.35) — a grey-pink. Every rear lamp on every car
    // was being painted the colour of a lamp that is switched off and dusty.
    //
    // Scaling the whole colour until its brightest channel is lit keeps the ratios between them, so
    // red stays red and simply gets brighter. This is the emissive material the lighting wants, and
    // there is nowhere else to put one: the renderer's entire material system is two flags and a
    // vertex colour, and the vertex colour is this.
    //
    // Only for a lens that has a colour, though, and that is the other half of the same argument. A
    // lamp cluster is not all lens: the E39's `tail_light_lod0` is 846 triangles of the dark grey
    // backing the red lens is set into, and scaling a grey until its brightest channel reads as lit
    // does not make it a brighter grey, it makes it white. Both of the car's rear clusters were
    // coming out as white blobs with some red in them for exactly this reason. A neutral surface has
    // no ratio between its channels to keep, so there is nothing for the scaling to preserve and it
    // keeps the brightness the model gave it — which leaves a white headlight lens white, because it
    // was already at 1.0, and a grey backing grey.
    if category == Category::Light {
        let brightest = out[0].max(out[1]).max(out[2]);
        let darkest = out[0].min(out[1]).min(out[2]);
        if brightest > 0.02 && brightest - darkest > LENS_HUE * brightest {
            let lift = (LENS_LIT / brightest).max(1.0);
            for c in out.iter_mut().take(3) {
                *c = (*c * lift).min(1.0);
            }
        }
    }
    out[3] = match category {
        // Glass has to be see-through, but not so thin that the roofline disappears with it.
        Category::Window => material.base_color[3].clamp(0.35, 0.75),
        _ => 1.0,
    };
    out
}

fn srgb(linear: f32) -> f32 {
    let l = linear.clamp(0.0, 1.0);
    if l <= 0.003_130_8 {
        12.92 * l
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

/// Packs a colour in the GU's `0xAABBGGRR` order — alpha, then blue, green, red.
fn pack(c: [f32; 4]) -> u32 {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    (byte(c[3]) << 24) | (byte(c[2]) << 16) | (byte(c[1]) << 8) | byte(c[0])
}

/// The inverse of `pack`, for the one stage that has to reach back into a packed colour: a palette
/// lookup multiplies into a vertex that has already been through `base_colour` and packed.
fn unpack(c: u32) -> [f32; 4] {
    let chan = |shift: u32| ((c >> shift) & 0xFF) as f32 / 255.0;
    [chan(0), chan(8), chan(16), chan(24)]
}

/// How much light a surface with this normal gets.
fn light_at(normal: [f32; 3], category: Category) -> f32 {
    let light = AMBIENT + DIFFUSE * normal[1].max(0.0);
    if category == Category::Light {
        light.max(LIGHT_FLOOR)
    } else {
        light
    }
}

/// Multiplies a packed colour by a light term, leaving alpha alone.
fn apply_light(color: u32, light: f32) -> u32 {
    let channel = |shift: u32| {
        let v = ((color >> shift) & 0xFF) as f32 * light;
        (v.clamp(0.0, 255.0) + 0.5) as u32
    };
    (color & 0xFF00_0000) | (channel(16) << 16) | (channel(8) << 8) | channel(0)
}

fn flags_for(category: Category) -> u8 {
    match category {
        // Glass is a single sheet with an inside and an outside, and it has to blend.
        Category::Window => MATERIAL_BLEND | MATERIAL_TWO_SIDED,
        // Seats, carpets and door cards are modelled as sheets; culled, they show their backs as
        // holes into the cabin.
        Category::Interior => MATERIAL_TWO_SIDED,
        _ => 0,
    }
}

/// A colour to describe the whole category by, for the report. The most common one, not the mean:
/// averaging a red car's paint with its black trim describes neither.
fn representative_colour(buckets: &[Bucket], category: Category) -> u32 {
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for b in buckets.iter().filter(|b| b.category == category) {
        for v in &b.vertices {
            *counts.entry(v.color).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(c, _)| c)
        .unwrap_or(0xFFFF_FFFF)
}

/// Shares the triangle budget between the buckets.
///
/// Not proportionally: proportional sharing spends the budget on whatever the model happens to
/// have the most triangles of, which on a scanned car is the engine and the seats. It goes by how
/// many pixels each bucket owns across the visibility sweep, times the category's weight from the
/// config — so the split follows what the player looks at, adjusted for the cases where screen
/// area and importance disagree. A headlight is a handful of pixels and half of what makes a car
/// recognisable; a door card is a lot of pixels nobody has ever looked at.
///
/// Two rules stop the arithmetic from doing something stupid:
///
/// * No bucket is given more than it arrived with. A wheel that is 300 triangles does not become
///   better for being allocated 900, and the surplus is wanted elsewhere.
/// * No bucket is reduced below a floor. A mesh that vanishes is a missing headlight or a missing
///   window, and a coarse one reads far better than an absent one.
///
/// Anything left over after the caps is handed round again, so a budget is spent rather than
/// merely divided.
fn share_budget(claims: &[(f64, usize)], budget: usize, floor: usize) -> Vec<usize> {
    let have: Vec<usize> = claims.iter().map(|c| c.1).collect();
    let total: usize = have.iter().sum();
    if total <= budget {
        return have;
    }

    let weights: Vec<f64> = claims.iter().map(|c| c.0).collect();
    let n = claims.len();

    let mut share = vec![0usize; n];
    let mut fixed = vec![false; n];

    // Water-filling. Each pass shares what is unspent between whatever is still open, in
    // proportion to weight; anything that would go over its own size or under the floor is pinned
    // there and taken out of the running, and the next pass shares out what it did not take.
    //
    // What is left is recomputed from the pinned shares at the top of each pass rather than
    // decremented as the pass runs. Subtracting inside the loop makes each item's share depend on
    // where it sits in the list, and the arithmetic stops adding up to the budget.
    loop {
        let spent: usize = share
            .iter()
            .zip(&fixed)
            .filter(|(_, f)| **f)
            .map(|(s, _)| *s)
            .sum();
        let left = budget.saturating_sub(spent);
        let open: Vec<usize> = (0..n).filter(|i| !fixed[*i]).collect();
        if open.is_empty() {
            break;
        }
        let open_weight: f64 = open.iter().map(|i| weights[*i]).sum();

        let want_of = |i: usize| -> usize {
            if open_weight > 0.0 {
                (left as f64 * weights[i] / open_weight) as usize
            } else {
                // Nothing here was seen at all. Split evenly rather than give it all to the first.
                left / open.len()
            }
        };

        let mut pinned = false;
        for &i in &open {
            let want = want_of(i);
            if want >= have[i] {
                // Asking for more than there is. Give it what it has; the surplus goes back.
                share[i] = have[i];
                fixed[i] = true;
                pinned = true;
            } else if want <= floor {
                // A bucket nothing much saw still gets the floor, so an unlucky measurement costs
                // detail rather than the whole part.
                share[i] = floor.min(have[i]);
                fixed[i] = true;
                pinned = true;
            }
        }
        if !pinned {
            for &i in &open {
                share[i] = want_of(i);
                fixed[i] = true;
            }
            break;
        }
    }
    share
}

/// Fewest triangles any one draw call is reduced to.
///
/// A tyre at 24 triangles is a hexagonal prism and reads as a wheel; at 8 it is a wedge. This is
/// the line under which a mesh stops being the thing it was and starts being an artifact — and a
/// category that disappears is much worse than a coarse one, because a car with no windows reads
/// as broken rather than as low-detail.
const MIN_BUCKET_TRIANGLES: usize = 24;

/// Fewest triangles a single part is reduced to before it is dropped instead.
///
/// Far lower than the per-draw-call floor, and deliberately so: a part is a bolt or a badge or a
/// wiper as often as it is a wing, and a bolt rendered as four triangles is a bolt. Below this
/// there is nothing left to be a shape, and the triangles are better spent on the part next to it.
const MIN_PIECE_TRIANGLES: usize = 4;

/// A part allocated no more than this is one the visibility sweep barely saw, and one that may be
/// dropped outright if it refuses to simplify. Above it, an unsimplifiable part is kept and
/// warned about instead: losing a wing to save a budget is worse than going over.
const STUCK_TARGET: usize = 64;

/// The attribution line the game will display, out of whatever the source model recorded.
///
/// Kept short and folded to uppercase because the console's font has no lowercase and draws
/// anything it lacks as a blank — a credit that renders as gaps is not a credit. The URLs the
/// exporter wraps around the author's name go too: they do not fit on a 480-pixel screen, and the
/// licence asks for the name, not the link.
fn credit_line(model: &SourceModel) -> Option<String> {
    let c = &model.credit;
    let author = c.author.as_deref().map(strip_url)?;
    let mut line = format!("MODEL BY {author}");
    if let Some(license) = c.license.as_deref().map(strip_url) {
        line.push_str(&format!(", {license}"));
    }
    Some(line.to_uppercase())
}

/// `Black Snow (https://sketchfab.com/BlackSnow02)` becomes `Black Snow`.
fn strip_url(s: &str) -> &str {
    s.split(" (").next().unwrap_or(s).trim()
}

/// The string table, and the offsets into it.
#[derive(Default)]
pub struct Strings {
    pub bytes: Vec<u8>,
    seen: HashMap<String, u16>,
}

impl Strings {
    pub fn push(&mut self, s: &str) -> u16 {
        if let Some(at) = self.seen.get(s) {
            return *at;
        }
        let at = self.bytes.len() as u16;
        self.bytes.extend_from_slice(s.as_bytes());
        self.bytes.push(0);
        self.seen.insert(s.to_string(), at);
        at
    }
}

/// Flattens a level's buckets into one positions-only array in car space.
///
/// Everything that made these buckets separate — category, material, which wheel they belong to,
/// whether they are two-sided — is thrown away here, because a silhouette is drawn in one colour
/// with culling off and none of it would change a pixel.
///
/// A level that needs more than 65,536 vertices produces no silhouette rather than a truncated one:
/// indices are 16-bit, and half a car is worse than none. No level this pipeline builds comes near
/// it, which is exactly why the case is worth refusing outright rather than handling.
fn build_silhouette(
    group: &[Bucket],
    origin_of: impl Fn(Option<u8>) -> [f32; 3],
) -> (Vec<[f32; 3]>, Vec<u16>) {
    let total: usize = group.iter().map(|b| b.vertices.len()).sum();
    if total == 0 || total > u16::MAX as usize + 1 {
        return (Vec::new(), Vec::new());
    }

    let mut positions = Vec::with_capacity(total);
    let mut indices = Vec::new();
    for b in group {
        let base = positions.len() as u16;
        let origin = origin_of(b.wheel);
        positions.extend(
            b.vertices
                .iter()
                .map(|v| [v.x + origin[0], v.y + origin[1], v.z + origin[2]]),
        );
        indices.extend(b.indices.iter().map(|i| base + *i as u16));
    }
    (positions, indices)
}

/// Lays the sections out with every one of them 16-byte aligned.
fn write(
    vertices: &[Vertex],
    uvs: &[[f32; 2]],
    atlas: &texture::Atlas,
    indices: &[u16],
    meshes: &[Mesh],
    materials: &[MaterialDef],
    wheels: &[WheelDef],
    lights: &[LightDef],
    strings: &[u8],
    credit: u32,
    name: u32,
    handling: angle_zero::vehicle::CarHandling,
    levels: &[(u32, u16, usize)],
    bounds: Bounds,
    silhouette: &(Vec<[f32; 3]>, Vec<u16>),
) -> Vec<u8> {
    let mut out = vec![0u8; HEADER_BYTES];

    // First of every section, and that position is the whole point of it. The console reads a car
    // in chunks and draws this one as soon as the first chunk lands, so anything in front of it is
    // a delay before the player sees the shape they asked for. Written at 112, which is where the
    // header ends — there is nothing to put in front of it.
    let silhouette_at = if silhouette.1.is_empty() {
        0
    } else {
        let at = pad(&mut out);
        let (positions, indices) = silhouette;
        out.extend_from_slice(&(positions.len() as u32).to_le_bytes());
        out.extend_from_slice(&(indices.len() as u32).to_le_bytes());
        // The two arrays' offsets are written after they are laid out, since where they land
        // depends on padding this has not done yet.
        let arrays_at = out.len();
        out.extend_from_slice(&[0u8; 8]);
        let positions_at = pad(&mut out);
        for p in positions {
            for v in p {
                out.extend_from_slice(&v.to_le_bytes());
            }
        }
        let indices_at = pad(&mut out);
        for i in indices {
            out.extend_from_slice(&i.to_le_bytes());
        }
        put_u32(&mut out, arrays_at, (positions_at - at) as u32);
        put_u32(&mut out, arrays_at + 4, (indices_at - at) as u32);
        at
    };

    let meshes_at = pad(&mut out);
    for m in meshes {
        out.extend_from_slice(&m.encode());
    }
    let materials_at = pad(&mut out);
    for m in materials {
        out.extend_from_slice(&m.encode());
    }
    let wheels_at = pad(&mut out);
    for w in wheels {
        out.extend_from_slice(&w.encode());
    }
    // Zero when the car has none, which is what tells a reader there are no lamps rather than a
    // section of length zero to walk.
    let lights_at = if lights.is_empty() {
        0
    } else {
        let at = pad(&mut out);
        for l in lights {
            out.extend_from_slice(&l.encode());
        }
        at
    };
    let vertices_at = pad(&mut out);
    for (i, v) in vertices.iter().enumerate() {
        // Texture, then colour, then position: the order the GE reads a vertex in, not a choice.
        let uv = uvs.get(i).copied().unwrap_or([0.0, 0.0]);
        out.extend_from_slice(&uv[0].to_le_bytes());
        out.extend_from_slice(&uv[1].to_le_bytes());
        out.extend_from_slice(&v.color.to_le_bytes());
        out.extend_from_slice(&v.x.to_le_bytes());
        out.extend_from_slice(&v.y.to_le_bytes());
        out.extend_from_slice(&v.z.to_le_bytes());
    }
    let indices_at = pad(&mut out);
    for i in indices {
        out.extend_from_slice(&i.to_le_bytes());
    }
    let strings_at = pad(&mut out);
    out.extend_from_slice(strings);
    // The level table, written only when there is more than one level. LOD0's meshes are first in
    // the mesh array and `MESH_COUNT` covers only them, so a reader that ignores this section
    // draws the full-detail car and nothing else — which is what makes the section additive.
    let lods_at = if levels.len() > 1 {
        let at = pad(&mut out);
        out.extend_from_slice(&(levels.len() as u16).to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&0u64.to_le_bytes());
        for (i, (first_mesh, mesh_count, triangles)) in levels.iter().enumerate() {
            out.extend_from_slice(&first_mesh.to_le_bytes());
            out.extend_from_slice(&mesh_count.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            // Beyond this many metres, this level is good enough. LOD0 starts at zero and each
            // level after it takes over further away; the runtime picks the last one whose
            // distance it is past, so the order here is the only thing that matters.
            out.extend_from_slice(&LOD_DISTANCES[i.min(LOD_DISTANCES.len() - 1)].to_le_bytes());
            out.extend_from_slice(&(*triangles as u32).to_le_bytes());
        }
        at
    } else {
        0
    };
    let texture_at = pad(&mut out);
    out.extend_from_slice(&(texture::ATLAS as u16).to_le_bytes());
    out.extend_from_slice(&(texture::ATLAS as u16).to_le_bytes());
    out.extend_from_slice(&TEXTURE_5650.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&[0u8; TEXTURE_HEADER_BYTES - 8]);
    out.extend_from_slice(&atlas.to_5650());

    let handling_at = pad(&mut out);
    for v in [
        handling.mass,
        handling.inertia,
        handling.front_axle,
        handling.rear_axle,
        handling.engine,
        handling.top_speed,
        handling.brake,
        handling.steer_lock,
        handling.grip,
    ] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    pad(&mut out);

    use azcar::field as f;
    out[f::MAGIC..4].copy_from_slice(&MAGIC);
    put_u16(&mut out, f::VERSION, VERSION);
    put_u32(&mut out, f::VERTEX_FORMAT, VERTEX_TEX_F32_COLOR_8888_F32);
    put_u32(&mut out, f::VERTEX_COUNT, vertices.len() as u32);
    put_u32(&mut out, f::INDEX_COUNT, indices.len() as u32);
    put_u16(&mut out, f::MESH_COUNT, meshes.len() as u16);
    put_u16(&mut out, f::MATERIAL_COUNT, materials.len() as u16);
    put_u16(&mut out, f::TEXTURE_COUNT, 1);
    put_u16(&mut out, f::WHEEL_COUNT, wheels.len() as u16);
    put_u32(&mut out, f::LIGHTS_AT, lights_at as u32);
    put_u16(&mut out, f::LIGHT_COUNT, lights.len() as u16);
    for (i, v) in [
        bounds.min[0],
        bounds.min[1],
        bounds.min[2],
        bounds.max[0],
        bounds.max[1],
        bounds.max[2],
    ]
    .iter()
    .enumerate()
    {
        put_f32(&mut out, f::BOUNDS + i * 4, *v);
    }
    put_u32(&mut out, f::MESHES_AT, meshes_at as u32);
    put_u32(&mut out, f::MATERIALS_AT, materials_at as u32);
    put_u32(&mut out, f::TEXTURES_AT, texture_at as u32);
    put_u32(&mut out, f::WHEELS_AT, wheels_at as u32);
    put_u32(&mut out, f::VERTICES_AT, vertices_at as u32);
    put_u32(&mut out, f::INDICES_AT, indices_at as u32);
    put_u32(&mut out, f::STRINGS_AT, strings_at as u32);
    put_u32(&mut out, f::STRINGS_BYTES, strings.len() as u32);
    put_u32(&mut out, f::LODS_AT, lods_at as u32);
    put_u32(&mut out, f::CREDIT, credit);
    put_u32(&mut out, f::NAME, name);
    put_u32(&mut out, f::HANDLING_AT, handling_at as u32);
    // In 16-byte units, because the two bytes left in the header cannot hold an offset. See
    // `field::SILHOUETTE_AT_16` — and note that `pad` has already made this a multiple of 16, so
    // nothing is being rounded away here.
    put_u16(
        &mut out,
        f::SILHOUETTE_AT_16,
        (silhouette_at / 16).try_into().unwrap_or(0),
    );
    let total = out.len() as u32;
    put_u32(&mut out, f::LENGTH, total);
    out
}

fn pad(out: &mut Vec<u8>) -> usize {
    while out.len() % 16 != 0 {
        out.push(0);
    }
    out.len()
}

fn put_u16(out: &mut [u8], at: usize, v: u16) {
    out[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn put_u32(out: &mut [u8], at: usize, v: u32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_f32(out: &mut [u8], at: usize, v: f32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Shares a budget out across welded buckets and decimates each bucket to its share.
///
/// Taken out of `compile` so it can be run more than once over the same welded geometry: an LOD is
/// this again with a smaller number.
///
/// The report is only filled in for the level written as LOD0. The others would double every line
/// in it, and it is the car the player is looking at whose error is worth warning about.
/// Spends a budget, and then spends what the first attempt handed back.
///
/// One pass leaves a lot on the table. The allocator shares the budget out by measured importance,
/// but a bucket cannot always use its share: the E36's bodywork is within five millimetres of the
/// original at about 4,000 triangles and the simplifier refuses to spend more on something it
/// cannot improve, so a 15,000-triangle budget produced an 11,000-triangle car. Nothing was wrong
/// with the allocation — the shortfall only becomes visible after the simplifier has run, which is
/// after the sharing is done.
///
/// So it runs twice. The second pass pins every bucket that came in under its share at what it
/// actually used, and shares the difference among the ones that were stopped by their target
/// rather than by their own geometry — which on a car means the wheels, because a surface of
/// revolution takes every triangle it is offered. It is one extra simplification pass over the
/// welded geometry, and it is what turns "the body cannot use this" into "so the wheels will".
/// Levels what one category's four corners are counted as having used, at the best of them.
///
/// The pin below is an inference: a bucket that came in under its share is taken to have been
/// stopped by its own geometry rather than by its target, so it is held at what it managed and the
/// difference is given to buckets that can spend it. That inference is sound for a body panel and
/// wrong for a wheel, because a car's four corners are the same wheel four times and the simplifier
/// does not treat them as such. They are mirrored, so the greedy collapse order differs, and on
/// geometry it finds hard it can stop far short on one corner and not on its reflection.
///
/// Left alone the pin then makes that permanent. The 190E's alloys — 5,565 triangles a corner of a
/// fifteen-hole disc, identical geometry and identical shares — came out at 2,217, 2,143, 1,049 and
/// 821 triangles, and raising the budget never moved the last of them: it had been declared full at
/// 821 on the first pass and pinned there, while its own reflection was declared hungry and refilled
/// to nearly three times as much. One wheel visibly coarser than the one across from it reads as a
/// fault rather than as detail, which is the same argument that already averages the corners'
/// measured pixels before the split.
///
/// So the best any corner achieved is taken as what that geometry can do, and every corner in the
/// group is judged and pinned at it. A corner that still cannot reach it simply returns less; a
/// target is not a promise. The alternative — believing each corner's own number — is believing the
/// collapse order, and that is the thing that is arbitrary here.
fn level_corners(buckets: &[Bucket], used: &[usize]) -> Vec<usize> {
    let mut levelled = used.to_vec();
    for category in [
        Category::Tyre,
        Category::Chrome,
        Category::Body,
        Category::Interior,
        Category::Light,
        Category::Window,
    ] {
        let corners: Vec<usize> = (0..buckets.len())
            .filter(|&i| buckets[i].wheel.is_some() && buckets[i].category == category)
            .collect();
        if corners.len() < 2 {
            continue;
        }
        let best = corners.iter().map(|&i| used[i]).max().unwrap_or(0);
        for i in corners {
            levelled[i] = best;
        }
    }
    levelled
}

fn spend_and_refill(
    buckets: &mut Vec<Bucket>,
    welded: &[Bucket],
    budget: usize,
    tile_span: f32,
    report: Option<&mut Report>,
) {
    let first = share_budget(
        &buckets.iter().map(|b| b.weights()).collect::<Vec<_>>(),
        budget,
        MIN_BUCKET_TRIANGLES,
    );
    spend_budget_with(buckets, &first, tile_span, None);

    let used: Vec<usize> = buckets.iter().map(|b| b.indices.len() / 3).collect();
    // What the four corners of a category are judged to have used, which is not always what each
    // one of them actually did. See `level_corners`.
    let used = level_corners(buckets, &used);
    let spent: usize = used.iter().sum();
    // Nothing meaningful came back, so the first pass was already the answer.
    if spent + spent / 20 >= budget {
        *buckets = welded.to_vec();
        spend_budget_with(buckets, &first, tile_span, report);
        return;
    }

    // What each bucket gets on the second pass: what it used, if it could not fill its share, and
    // a share of everything handed back if it could.
    let surplus: usize = first
        .iter()
        .zip(&used)
        .map(|(t, u)| t.saturating_sub(*u))
        .sum();
    // "Filled its share" has to have a tolerance in it. The simplifier lands a few triangles
    // either side of a target, and the four corners of a car are mirrored geometry that collapses
    // in slightly different orders — so an exact test called two of the four tyres full and two
    // hungry, handed the whole surplus to the two, and gave one wheel 2,500 triangles against 724
    // for the one across from it.
    let hungry: Vec<usize> = (0..buckets.len())
        .filter(|&i| used[i] * 20 >= first[i] * 19)
        .collect();
    let mut second = first.clone();
    for (i, u) in used.iter().enumerate() {
        if !hungry.contains(&i) {
            second[i] = *u;
        }
    }
    if !hungry.is_empty() {
        // A bucket can never use more than the welded geometry it started with, which is both the
        // honest ceiling and small enough to add up — a stand-in "unlimited" here overflowed the
        // sum inside `share_budget`.
        let claims: Vec<(f64, usize)> = hungry
            .iter()
            .map(|&i| {
                let ceiling: usize = welded[i].pieces.iter().map(|p| p.indices.len() / 3).sum();
                (buckets[i].weights().0, ceiling)
            })
            .collect();
        let extra = share_budget(&claims, surplus, 0);
        for (slot, &i) in hungry.iter().enumerate() {
            second[i] += extra[slot];
        }
    }

    *buckets = welded.to_vec();
    spend_budget_with(buckets, &second, tile_span, report);
}

/// Shares a budget out across welded buckets and decimates each bucket to its share.
///
/// Taken out of `compile` so it can be run more than once over the same welded geometry: an LOD is
/// this again with a smaller number, and the refill pass is this again with corrected targets.
///
/// The report is only filled in for the level written as LOD0. The others would double every line
/// in it, and it is the car the player is looking at whose error is worth warning about.
fn spend_budget(
    buckets: &mut Vec<Bucket>,
    budget: usize,
    tile_span: f32,
    report: Option<&mut Report>,
) {
    let targets = share_budget(
        &buckets.iter().map(|b| b.weights()).collect::<Vec<_>>(),
        budget,
        MIN_BUCKET_TRIANGLES,
    );
    spend_budget_with(buckets, &targets, tile_span, report);
}

/// Decimates each bucket to a target that has already been decided.
fn spend_budget_with(
    buckets: &mut Vec<Bucket>,
    targets: &[usize],
    tile_span: f32,
    mut report: Option<&mut Report>,
) {
    for (b, bucket_target) in buckets.iter_mut().zip(targets) {
        let piece_targets = share_budget(
            &b.pieces
                .iter()
                .map(|p| (p.pixels as f64 * p.weight as f64, p.indices.len() / 3))
                .collect::<Vec<_>>(),
            *bucket_target,
            MIN_PIECE_TRIANGLES,
        );

        let before: usize = b.pieces.iter().map(|p| p.indices.len() / 3).sum();
        let mut stuck = 0;
        // Weighted by how many triangles ended up carrying it, not the worst of them. A bolt taken
        // from 200 triangles to 4 has moved half its own width and is still a bolt; the number
        // that matters is what happened to the panel it is screwed to.
        let mut error_sum = 0.0f64;
        let mut error_weight = 0.0f64;
        for (p, target) in b.pieces.iter_mut().zip(&piece_targets) {
            let was = p.indices.len() / 3;
            let error = simplify::reduce(
                &mut p.vertices,
                &mut p.attrs,
                &mut p.indices,
                *target,
                tile_span,
            );
            if std::env::var("AZ_PARTS").is_ok() {
                eprintln!(
                    "PART {:>7} -> {:>6} (target {:>6}) {:5.2}%  {}",
                    was,
                    p.indices.len() / 3,
                    target,
                    error * 100.0,
                    p.node
                );
            }

            // Some geometry cannot be simplified at all. The E36's engine block is 80,869
            // triangles that both simplifiers hand back untouched: it is a mass of hoses, fins and
            // fasteners with enough non-manifold junk in it that there is no collapse left to make
            // and no cluster the sloppy pass will accept. Keeping it means writing a car twenty
            // times its budget for a part the budget valued at four triangles, so it goes.
            //
            // Only ever applied to parts that were allocated next to nothing, which by
            // construction means the visibility sweep barely saw them. A part worth thousands of
            // triangles that will not simplify is kept, and warned about, because dropping a wing
            // to save a budget is the wrong trade in the other direction.
            if *target <= STUCK_TARGET && p.indices.len() / 3 > target * 4 {
                stuck += p.indices.len() / 3;
                p.indices.clear();
                p.vertices.clear();
                p.attrs.clear();
                continue;
            }
            // Only what survives counts towards the bucket's error. A part that was dropped for
            // refusing to simplify would otherwise report its 92% against the panel next to it,
            // and the warning that reads would be about the wrong thing entirely.
            let weight = (p.indices.len() / 3) as f64;
            error_sum += error as f64 * weight;
            error_weight += weight;
        }
        let error = if error_weight > 0.0 {
            (error_sum / error_weight) as f32
        } else {
            0.0
        };
        b.pieces.retain(|p| p.indices.len() >= 3);

        if let Some(report) = report.as_deref_mut() {
            if stuck > 0 {
                report.note_stuck(b.category, stuck);
            }
            report.note_bucket(
                b.category,
                b.wheel,
                b.source_triangles,
                before,
                b.pieces.iter().map(|p| p.indices.len() / 3).sum(),
                error,
            );
        }
    }

    // Only now is the light folded in: welding averaged it and decimation moved vertices about,
    // and both of those are the reason it was kept out of the colour until here.
    for b in buckets.iter_mut() {
        for p in &mut b.pieces {
            for (v, a) in p.vertices.iter_mut().zip(&p.attrs) {
                v.color = apply_light(v.color, a.light);
            }
        }
        b.flatten();
    }

    // Drop anything simplification emptied, so the file has no zero-triangle draw calls in it.
    buckets.retain(|b| b.indices.len() >= 3);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wheels::tests::{config_matching, four_wheeled_model};

    /// Compiles the test car and reads it back through the runtime's reader, which is the only
    /// check that matters: the writer and the console agree, or they do not.
    fn compile_test_car(budget: usize) -> (Vec<u8>, Report) {
        let mut model = four_wheeled_model();
        // The model is built a metre in the air and half a metre off centre, so that placement has
        // something to correct rather than a zero to pass through.
        for part in &mut model.parts {
            for p in &mut part.positions {
                p[1] += 1.0;
                p[2] += 0.5;
            }
        }
        let config = config_matching(&["tyre_", "rim_"]);
        let compiled = compile(&mut model, &config, budget).expect("the test car must compile");
        (compiled.bytes, compiled.report)
    }

    /// A car whose lamps are placed by its config, compiled and read back by the console's own
    /// reader. The four-wheeled test model has no lens geometry at all, which is the case the
    /// config path exists for.
    #[test]
    fn a_cars_lamps_survive_the_round_trip_into_the_file() {
        use angle_zero::azcar::LightKind;
        use crate::config::Anchor;

        let mut model = four_wheeled_model();
        let mut config = config_matching(&["tyre_", "rim_"]);
        config.lights.headlight_left = Some(Anchor {
            at: Some([0.7, 0.68, 2.0]),
            ..Anchor::default()
        });
        config.lights.headlight_right = Some(Anchor {
            at: Some([-0.7, 0.68, 2.0]),
            ..Anchor::default()
        });
        let compiled = compile(&mut model, &config, 10_000).expect("must compile");

        let car = angle_zero::azcar::Car::parse(&compiled.bytes).expect("must parse");
        assert_eq!(car.light_count(), 2);
        let lights: Vec<_> = car.lights().collect();
        assert!(lights.iter().all(|l| l.kind == LightKind::Head));
        assert_eq!(lights[0].at, [0.7, 0.68, 2.0]);
        assert!(lights[0].range > 0.0, "a headlight throws a beam");
        // And the lamps are named in the string table, for the diagnostics that read them back.
        assert_eq!(car.name(lights[0].name), b"headlight_left");
    }

    /// A car with nothing said about lamps and no lenses in it carries none — and is still a car.
    #[test]
    fn a_car_with_no_lamps_is_written_without_a_lights_section() {
        let (bytes, _) = compile_test_car(10_000);
        let car = angle_zero::azcar::Car::parse(&bytes).expect("must parse");
        assert_eq!(car.light_count(), 0);
        assert_eq!(car.lights().count(), 0);
    }

    #[test]
    fn a_compiled_car_passes_the_runtimes_own_reader() {
        let (bytes, report) = compile_test_car(10_000);
        let car = angle_zero::azcar::Car::parse(&bytes).expect("must parse");
        assert_eq!(car.triangle_count(), report.out_triangles);
        assert_eq!(car.vertex_count(), report.out_vertices);
        assert_eq!(car.wheel_count(), 4);
        assert!(car.mesh_count() >= 5, "a body and four wheels at least");
    }

    /// Handling survives the trip out to a file and back, and a car that says nothing gets the
    /// numbers the game was tuned with rather than zeroes.
    #[test]
    fn what_a_car_drives_like_is_carried_by_the_asset() {
        use angle_zero::vehicle::CarHandling;

        let (bytes, _) = compile_test_car(10_000);
        let car = angle_zero::azcar::Car::parse(&bytes).unwrap();
        assert_eq!(car.handling(), CarHandling::DEFAULT);

        let mut model = four_wheeled_model();
        let mut config = config_matching(&["tyre_", "rim_"]);
        config.handling.mass = 940.0;
        config.handling.engine = 4900.0;
        config.handling.grip = 0.94;
        let compiled = compile(&mut model, &config, 10_000).unwrap();
        let car = angle_zero::azcar::Car::parse(&compiled.bytes).unwrap();
        let h = car.handling();
        assert_eq!(h.mass, 940.0);
        assert_eq!(h.engine, 4900.0);
        assert_eq!(h.grip, 0.94);
        // Left out of the config, so derived from the mass rather than left at the saloon's.
        assert!(
            (h.inertia - 940.0 * (CarHandling::DEFAULT.inertia / CarHandling::DEFAULT.mass)).abs()
                < 1e-3,
            "inertia was {}",
            h.inertia
        );
    }

    fn clone_material(m: &crate::model::Material) -> crate::model::Material {
        crate::model::Material {
            name: m.name.clone(),
            base_color: m.base_color,
            metallic: m.metallic,
            roughness: m.roughness,
            emissive: m.emissive,
            image: m.image,
            double_sided: m.double_sided,
            transparent: m.transparent,
        }
    }

    /// Every compiled vertex points inside a tile that belongs to some material, and different
    /// materials end up in different tiles.
    ///
    /// This is the check that the atlas is actually wired up rather than merely built. A UV of
    /// (0, 0) everywhere would look identical in game — most tiles are white, so a car sampling
    /// one corner forever is a car that looks exactly like it did before textures existed — and
    /// the only thing that distinguishes the two is where the coordinates point.
    #[test]
    fn every_vertex_samples_its_own_materials_tile() {
        let mut model = four_wheeled_model();
        // A second material, so there is a second tile for a vertex to land in wrongly, and UVs
        // that span the unit square rather than sitting at one corner.
        let mut second = crate::model::Material {
            name: "trim".into(),
            ..clone_material(&model.materials[0])
        };
        second.base_color = [0.1, 0.1, 0.1, 1.0];
        model.materials.push(second);
        for (i, part) in model.parts.iter_mut().enumerate() {
            part.material = i % 2;
            part.uvs = part
                .positions
                .iter()
                .map(|p| [p[0].rem_euclid(1.0), p[2].rem_euclid(1.0)])
                .collect();
        }

        let config = config_matching(&["tyre_", "rim_"]);
        let compiled = compile(&mut model, &config, 10_000).unwrap();
        let car = angle_zero::azcar::Car::parse(&compiled.bytes).unwrap();

        // Rebuilt from the same materials, so the same tiles: `compile` moves geometry about but
        // never touches the material list.
        let atlas = crate::texture::Atlas::build(&model, &config.materials);
        assert!(atlas.tiles.len() >= 2, "the test car needs several materials");

        let mut used = std::collections::HashSet::new();
        for v in car.vertices() {
            let inside = atlas.tiles.iter().position(|t| {
                v.u >= t.u0 - 1e-6
                    && v.u <= t.u1 + 1e-6
                    && v.v >= t.v0 - 1e-6
                    && v.v <= t.v1 + 1e-6
            });
            let Some(tile) = inside else {
                panic!("({}, {}) is not inside any material's tile", v.u, v.v);
            };
            used.insert(tile);
        }
        assert!(
            used.len() >= 2,
            "every vertex landed in the same tile ({used:?}), so the UVs are not per-material"
        );
    }

    /// Coarse levels are written, are actually coarser, and are picked by distance — with the
    /// full-detail car still first in the mesh array, so a reader that ignores the level table
    /// draws exactly what it drew before there was one.
    #[test]
    fn coarse_levels_are_written_and_chosen_by_distance() {
        let mut model = four_wheeled_model();
        let mut config = config_matching(&["tyre_", "rim_"]);
        config.lods = vec![120, 40];
        let compiled = compile(&mut model, &config, 400).unwrap();
        let car = angle_zero::azcar::Car::parse(&compiled.bytes).unwrap();

        assert_eq!(car.lod_count(), 3);
        let lods: Vec<_> = (0..3).map(|i| car.lod(i)).collect();
        assert_eq!(lods[0].first_mesh, 0, "LOD0 has to come first");
        // Each level is its own run of meshes, and never fewer triangles than the one after it.
        // Not strictly fewer: this test car is nine boxes, already at the floor a decimator can
        // take a closed shell to, so no budget makes it smaller. What the coarse levels are worth
        // is measured on the real cars — the E36 goes 9,540 / 3,634 / 1,600 — and what is checked
        // here is that they are written, found and chosen, which is the part written by hand.
        assert!(lods[1].first_mesh >= lods[0].mesh_count as u32);
        assert!(lods[2].first_mesh >= lods[1].first_mesh + lods[1].mesh_count as u32);
        assert!(lods[0].triangles >= lods[1].triangles && lods[1].triangles >= lods[2].triangles);
        // `triangle_count` is the car as drawn up close, not every level added together.
        assert_eq!(car.triangle_count(), lods[0].triangles as usize);
        assert!(car.total_triangle_count() > car.triangle_count());

        // The chase camera sits about 11 m back, which has to still be the full-detail car.
        assert_eq!(car.lod_for_distance(0.0).first_mesh, lods[0].first_mesh);
        assert_eq!(car.lod_for_distance(11.5).first_mesh, lods[0].first_mesh);
        assert_eq!(car.lod_for_distance(25.0).first_mesh, lods[1].first_mesh);
        assert_eq!(car.lod_for_distance(200.0).first_mesh, lods[2].first_mesh);
    }

    /// A car with no level table answers as one level, so nothing has to special-case it.
    #[test]
    fn a_car_without_levels_is_a_car_with_one() {
        let (bytes, _) = compile_test_car(10_000);
        let car = angle_zero::azcar::Car::parse(&bytes).unwrap();
        assert_eq!(car.lod_count(), 1);
        assert_eq!(car.lod(0).mesh_count as usize, car.mesh_count());
        assert_eq!(car.lod_for_distance(500.0).first_mesh, 0);
    }

    /// Parts named in `[reduce] drop` are left out, and their budget goes to what is left.
    ///
    /// The case this exists for: the E36's alloys have 4,762 triangles of brake hardware behind
    /// each one, visible through the spokes, so the visibility sweep gives it a share — and every
    /// triangle it takes comes out of the wheel in front of it. The alloy fell to about 150
    /// triangles, at which a five-spoke wheel decimates into a featureless disc.
    #[test]
    fn parts_named_in_the_drop_list_are_left_out() {
        let mut model = four_wheeled_model();
        let mut config = config_matching(&["tyre_", "rim_"]);
        let whole = compile(&mut model, &config, 2_000).unwrap();

        let mut model = four_wheeled_model();
        config.reduce.drop = vec!["rim_".into()];
        let without = compile(&mut model, &config, 2_000).unwrap();

        assert!(without.report.dropped_by_name.0 > 0, "nothing was dropped");
        let car = angle_zero::azcar::Car::parse(&without.bytes).unwrap();
        for i in 0..car.mesh_count() {
            let name = car.name(car.mesh(i).name);
            assert!(
                !name.windows(3).any(|w| w == b"rim"),
                "a dropped part was compiled in: {}",
                core::str::from_utf8(name).unwrap_or("?")
            );
        }
        // The wheels are still found and still turn: dropping a part is not dropping a corner.
        assert_eq!(car.wheel_count(), whole.report.out_wheels);
    }

    /// A car whose numbers would break the simulation is refused, rather than written out and
    /// found on a handheld.
    #[test]
    fn handling_that_would_break_the_simulation_is_refused() {
        let mut model = four_wheeled_model();
        let mut config = config_matching(&["tyre_", "rim_"]);
        config.handling.mass = 0.0;
        let Err(err) = compile(&mut model, &config, 10_000) else {
            panic!("a zero mass must be refused");
        };
        assert!(err.contains("handling"), "unhelpful message: {err}");
    }

    /// The car has to arrive where the game drives it from: wheels on the road, wheelbase centred.
    /// Both offsets are invisible in a modelling package and obvious in game.
    #[test]
    fn the_car_is_grounded_and_centred_wherever_the_model_left_it() {
        let (bytes, _) = compile_test_car(10_000);
        let car = angle_zero::azcar::Car::parse(&bytes).unwrap();
        let b = car.bounds();
        assert!(b[1].abs() < 1e-3, "the car sits at y = {}, not on the road", b[1]);
        let mid_x = (b[0] + b[3]) * 0.5;
        let mid_z = (b[2] + b[5]) * 0.5;
        assert!(mid_x.abs() < 0.05, "off centre by {mid_x} m across");
        assert!(mid_z.abs() < 0.05, "off centre by {mid_z} m along");
    }

    /// A region rule paints part of a material and leaves the rest of it alone.
    ///
    /// This is the grain a material rule cannot reach and a part rule would not either: the Golf's
    /// grille bars, bumper strakes and the ring round its badge are one part of one material, and
    /// the only thing that tells the badge apart is where it is. The check is that both colours
    /// come out — a box that painted everything, or nothing, would be the two ways this fails.
    #[test]
    fn a_region_paints_part_of_a_material_and_leaves_the_rest() {
        let mut model = four_wheeled_model();
        let mut config = config_matching(&["tyre_", "rim_"]);
        // The shell is 1.8 x 1.2 x 4.2 about (0, 0.8, 0); this is its nose and nothing else.
        config.materials.colour = vec![
            crate::config::ColourRule {
                match_: vec!["paint".into()],
                rgb: [200, 30, 30],
                flat: false,
                inside: None,
            },
            crate::config::ColourRule {
                match_: vec!["paint".into()],
                rgb: [20, 200, 40],
                flat: false,
                inside: Some(crate::config::Region {
                    min: [-1.0, 0.0, 1.9],
                    max: [1.0, 2.0, 3.0],
                }),
            },
        ];

        let compiled = compile(&mut model, &config, 10_000).unwrap();
        let car = angle_zero::azcar::Car::parse(&compiled.bytes).unwrap();

        // The light term multiplies the colour per vertex, so what survives the round trip is
        // which channel dominates, not the byte.
        let (mut reddish, mut greenish) = (0, 0);
        for v in car.vertices() {
            let (r, g) = ((v.color & 0xFF) as u32, ((v.color >> 8) & 0xFF) as u32);
            if r > g + 8 {
                reddish += 1;
            } else if g > r + 8 {
                greenish += 1;
            }
        }
        assert!(reddish > 0, "nothing kept the material's own colour");
        assert!(greenish > 0, "nothing was painted by the region");
    }

    /// Wheel geometry is stored about its own hub, which is what lets the runtime steer and spin
    /// it. If it were stored in car space, every wheel would orbit the origin instead.
    #[test]
    fn wheel_meshes_are_stored_about_their_hubs() {
        let (bytes, _) = compile_test_car(10_000);
        let car = angle_zero::azcar::Car::parse(&bytes).unwrap();

        for i in 0..car.wheel_count() {
            let w = car.wheel(i);
            assert!(w.hub[0].abs() > 0.3, "hub {i} is not out at a corner: {:?}", w.hub);
            assert!((w.hub[1] - w.radius).abs() < 0.05, "hub {i} is not at wheel height");
        }
        // The front wheels steer and the rear ones do not.
        let steering: Vec<bool> = (0..car.wheel_count()).map(|i| car.wheel(i).steers).collect();
        assert_eq!(steering.iter().filter(|s| **s).count(), 2);

        for m in (0..car.mesh_count()).map(|i| car.mesh(i)) {
            if m.wheel == NO_WHEEL {
                continue;
            }
            // Centred on its own origin, to within its own radius.
            let d = (m.center[0].powi(2) + m.center[1].powi(2) + m.center[2].powi(2)).sqrt();
            assert!(d < m.radius, "wheel mesh sits {d} m from its own hub");
        }
    }

    /// Every mesh belongs to a category, and the flags follow from the category rather than from
    /// the source material — that is the whole point of merging fifty-seven of them into six.
    #[test]
    fn materials_are_categories_and_carry_the_state_the_renderer_needs() {
        let (bytes, _) = compile_test_car(10_000);
        let car = angle_zero::azcar::Car::parse(&bytes).unwrap();
        assert!(car.material_count() <= 6);
        for i in 0..car.material_count() {
            let m = car.material(i);
            assert_eq!(m.blended(), m.category == Category::Window);
        }
    }

    /// A budget is honoured when it can be, and the shortfall is reported when it cannot — never
    /// silently ignored.
    #[test]
    fn the_budget_is_spent_and_reported() {
        let (_, generous) = compile_test_car(100_000);
        let (_, tight) = compile_test_car(200);
        assert!(
            tight.out_triangles < generous.out_triangles,
            "a tighter budget produced {} triangles against {}",
            tight.out_triangles,
            generous.out_triangles
        );
        let accounted: usize = generous.lines.iter().map(|l| l.source).sum();
        assert_eq!(
            generous.source_triangles, accounted,
            "every source triangle must be accounted for somewhere in the report"
        );
    }

    /// The budget split, on its own. It is a handful of arithmetic that decides what the whole
    /// pipeline produces, and getting it wrong does not look like a bug: it looks like a car that
    /// is coarse in the wrong places, or one that quietly comes out ten times the size asked for.
    mod budget {
        use super::super::share_budget;

        /// Sum, plus what the floors are allowed to add on top.
        fn total(share: &[usize]) -> usize {
            share.iter().sum()
        }

        #[test]
        fn a_model_that_already_fits_is_left_alone() {
            let share = share_budget(&[(1.0, 100), (1.0, 200)], 10_000, 4);
            assert_eq!(share, vec![100, 200]);
        }

        #[test]
        fn equal_claims_get_equal_shares() {
            let share = share_budget(&[(1.0, 1000), (1.0, 1000), (1.0, 1000)], 300, 4);
            assert_eq!(share, vec![100, 100, 100]);
        }

        /// The whole point: what the player looks at gets the budget.
        #[test]
        fn the_share_follows_the_weight() {
            let share = share_budget(&[(9.0, 10_000), (1.0, 10_000)], 1000, 4);
            assert_eq!(share, vec![900, 100]);
        }

        /// The engine: a third of the model, seen through a grille, worth almost nothing.
        #[test]
        fn a_part_nobody_looks_at_does_not_get_a_share_of_its_size() {
            let share = share_budget(
                &[
                    (5000.0, 36_000), // the body shell
                    (10.0, 137_000),  // the engine
                ],
                10_000,
                4,
            );
            assert!(share[1] < 100, "the engine took {} triangles", share[1]);
            assert!(share[0] > 9_000, "the body only got {}", share[0]);
        }

        /// Surplus from a part that cannot use its share goes back into the pot rather than being
        /// lost, or a budget would only ever be partly spent.
        #[test]
        fn what_a_small_part_cannot_use_is_handed_back() {
            // The first would be entitled to half of 1000 but only has 50 to give.
            let share = share_budget(&[(1.0, 50), (1.0, 10_000)], 1000, 4);
            assert_eq!(share[0], 50);
            assert_eq!(share[1], 950, "the surplus was not redistributed");
        }

        /// Overshooting the budget is what this used to do, by hundreds of per cent, and nothing
        /// about the resulting car said so.
        #[test]
        fn the_budget_is_never_blown() {
            // A hard case: many tiny claims, a few huge ones, and weights spanning four orders.
            let mut claims: Vec<(f64, usize)> = Vec::new();
            for i in 0..300 {
                claims.push((i as f64 * i as f64, 100 + i * 37));
            }
            claims.push((1e6, 200_000));
            claims.push((0.0, 5_000));

            for budget in [500usize, 3_000, 10_000, 50_000] {
                let share = share_budget(&claims, budget, 4);
                // The floors are the one thing allowed to push past the budget, and only by the
                // floor times the number of claims that hit it.
                let ceiling = budget + 4 * claims.len();
                assert!(
                    total(&share) <= ceiling,
                    "budget {budget} produced {} triangles",
                    total(&share)
                );
                for (i, s) in share.iter().enumerate() {
                    assert!(*s <= claims[i].1, "claim {i} was given more than it has");
                }
            }
        }

        /// A car whose visibility sweep saw nothing at all — every weight zero — still has to
        /// compile into something rather than dividing by zero or handing it all to the first.
        #[test]
        fn claims_with_no_weight_at_all_split_evenly() {
            let share = share_budget(&[(0.0, 1000), (0.0, 1000)], 400, 4);
            assert_eq!(share, vec![200, 200]);
        }
    }

    #[test]
    fn colours_are_packed_the_way_the_hardware_reads_them() {
        // 0xAABBGGRR: red in the low byte, alpha in the high one.
        assert_eq!(pack([1.0, 0.0, 0.0, 1.0]), 0xFF00_00FF);
        assert_eq!(pack([0.0, 1.0, 0.0, 1.0]), 0xFF00_FF00);
        assert_eq!(pack([0.0, 0.0, 1.0, 0.5]), 0x80FF_0000);
    }

    /// Light multiplies the colour and leaves alpha where it was, or every window would fade as
    /// the glass turned away from the sky.
    #[test]
    fn light_does_not_touch_alpha() {
        let lit = apply_light(0x8020_4060, 0.5);
        assert_eq!(lit >> 24, 0x80);
        assert_eq!(lit & 0xFF, 0x30);
        assert_eq!((lit >> 8) & 0xFF, 0x20);
        assert_eq!((lit >> 16) & 0xFF, 0x10);
    }

    /// Surfaces facing the sky are brighter than surfaces facing the horizon. That gradient is the
    /// only thing separating a roof from a door on an unlit, untextured car.
    #[test]
    fn upward_faces_are_brighter_than_vertical_ones() {
        let up = light_at([0.0, 1.0, 0.0], Category::Body);
        let side = light_at([1.0, 0.0, 0.0], Category::Body);
        let down = light_at([0.0, -1.0, 0.0], Category::Body);
        assert!(up > side && side >= down);
        // A lamp lens is lit whichever way it faces: a headlight with a shadow across it reads as
        // switched off.
        let lens_down = light_at([0.0, -1.0, 0.0], Category::Light);
        assert!(lens_down > down * 2.0, "a lamp facing away is barely brighter than paint");
        assert!(lens_down > side);
    }
}
