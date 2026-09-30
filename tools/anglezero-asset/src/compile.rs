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

/// A wheel's rolling radius: what the config says, or the larger of the measured radius and the
/// hub's height. Why the larger is explained where `WheelDef::radius` is filled in; it is a function
/// so the coarse levels' generated wheels are the same size as the wheel the car rolls on.
fn rolling_radius(config: &CarConfig, w: &wheels::Wheel, scale: f32, hub: [f32; 3]) -> f32 {
    config.wheels.radius.unwrap_or_else(|| (w.radius * scale).max(hub[1]))
}

/// How far away each level takes over, in metres.
///
/// The chase camera sits about 11 m behind the player's car, so LOD0 has to cover everything
/// nearer than that; 18 m is the first distance at which a car is small enough on a 480-wide
/// screen for a halved triangle count not to show. These are starting points — the benchmark
/// modes are how they get checked against something.
const LOD_DISTANCES: [f32; 3] = [0.0, 18.0, 45.0];

/// How much of the car one pixel covers at a distance, in metres: the console's 60° vertical field
/// of view (`camera::RUN_FOV_BASE`) over its 272 lines. 7.6 cm at LOD1's 18 m, 19 cm at LOD2's 45 m.
fn pixel_at(distance: f32) -> f32 {
    2.0 * distance * (30.0f32).to_radians().tan() / 272.0
}

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
    /// With the material, what says two parts may be one surface an exporter split. See
    /// `rejoin_split_parts`.
    parent: String,
    material: usize,
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
    report.note_texture(atlas.textured, model.images.len(), &atlas.resized, atlas.packed.grid);

    let mut buckets: Vec<Bucket> = Vec::new();
    let mut dropped_by_name = 0usize;
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
        // Only wheel geometry is untilted, and only by its own wheel's camber.
        let camber = wheel
            .and_then(|c| found.wheels.iter().find(|w| w.corner == c))
            .map(|w| w.camber)
            .unwrap_or(0.0);
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
        let tile = atlas.working.tiles[part.material];

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
                    // quarter of what a single part of the same importance would get. Body
                    // chrome may be weighted apart from the rims; see `Reduction::trim`.
                    weight: config.reduce.bucket_weight(category, wheel.is_some()),
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
            // Hub-relative, and then untilted: a wheel is stored upright with its axle along X,
            // and `WheelDef::camber` carries the lean for the renderer to put back. Doing it here
            // rather than there is what lets the console spin a wheel with one rotation about X
            // and have it turn in its own plane instead of sweeping a cone.
            let local = [v[0] - origin[0], v[1] - origin[1], v[2] - origin[2]];
            let local = if camber != 0.0 {
                let (s, c) = (-camber).sin_cos();
                [local[0] * c - local[1] * s, local[0] * s + local[1] * c, local[2]]
            } else {
                local
            };
            vertices.push(Vertex::new(local[0], local[1], local[2], colour));
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
        let two_sided = back_only as f32 > part.triangles() as f32 * TWO_SIDED_SHARE
            || config.reduce.two_sided.iter().any(|f| {
                !f.is_empty()
                    && (part.node.to_ascii_lowercase().contains(&f.to_ascii_lowercase())
                        || part.parent.to_ascii_lowercase().contains(&f.to_ascii_lowercase()))
            });

        let weight = config.reduce.part_weight(&part.node, &part.parent);
        let bucket = &mut buckets[slot];
        bucket.pixels += seen.pixels[i] as u64;
        bucket.source_triangles += part.triangles();
        bucket.pieces.push(Piece {
            vertices,
            attrs,
            pixels: seen.pixels[i] as u64,
            indices: part.indices.clone(),
            weight,
            two_sided,
            node: part.node.clone(),
            parent: part.parent.clone(),
            material: part.material,
        });
    }

    // Before welding, so that the border an exporter cut is welded shut like any other edge.
    let rejoined = rejoin_split_parts(&mut buckets);
    if rejoined.0 > 0 {
        report.note_rejoined(rejoined.0, rejoined.1);
    }
    // Counted after rejoining, which can make a surface two-sided that was only partly so.
    let two_sided_triangles: usize = buckets
        .iter()
        .flat_map(|b| &b.pieces)
        .filter(|p| p.two_sided)
        .map(|p| p.indices.len() / 3)
        .sum();

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
    // wing it merely touches, and the boundary between them is better left alone. A part here is
    // what `rejoin_split_parts` left, so a surface an exporter cut in two is welded as one.
    let mut welded_away = 0;
    for b in &mut buckets {
        for p in &mut b.pieces {
            welded_away += simplify::weld(&mut p.vertices, &mut p.attrs, &mut p.indices, atlas.working.span);
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

    spend_and_refill(&mut buckets, &welded, budget, atlas.working.span, Some(&mut report));
    if buckets.is_empty() {
        return Err("the triangle budget left nothing to draw".into());
    }

    // The extra levels, coarsest last. One that collapses to nothing is dropped rather than
    // written as a level with no draw calls in it.
    //
    // Their wheels are built rather than decimated (see `generated_wheels`), out of each corner's
    // size and the colours LOD0 draws its tyre and rim in — measured once, off LOD0's finished
    // buckets, because that is the wheel a coarse level is standing in for.
    let wheel_looks: Vec<WheelLook> = found
        .wheels
        .iter()
        .zip(&hubs)
        .map(|(w, hub)| {
            let radius = rolling_radius(config, w, placement.scale, *hub);
            WheelLook::measure(&welded, &atlas, w.corner, radius, w.width * placement.scale, hub[0])
        })
        .collect();
    let white = white_texel(&atlas);
    let flat = white.map(|white| simplify::FlatTexel {
        pixels: &atlas.working.pixels,
        size: texture::ATLAS,
        white,
    });
    if white.is_none() && !wheel_looks.is_empty() && !config.lods.is_empty() {
        report.warn(
            "the atlas has no white tile to draw generated wheels with, so the lower levels \
             decimate the model's own"
                .into(),
        );
    }
    let mut levels: Vec<Vec<Bucket>> = Vec::new();
    for (level, &lod_budget) in config.lods.iter().enumerate() {
        let mut coarse = welded.clone();
        let distance = LOD_DISTANCES[(level + 1).min(LOD_DISTANCES.len() - 1)];
        let pixel = pixel_at(distance);
        // Fewer sides when the budget is too small to spend a third of it on wheels, and the
        // model's own wheels when even the fewest would be. No car here comes near that; a test
        // car with a 40-triangle level does.
        let far = distance >= FAR_WHEEL_FROM;
        let mut segments = if far { FAR_WHEEL_SEGMENTS } else { NEAR_WHEEL_SEGMENTS };
        // The tread is two triangles a side; the painted face as many as its rings and sectors
        // come to, at most a quarter as many sectors again as sides at LOD1 (see `face_sectors`).
        let wheel_cost = |segments: usize| {
            let face = if far {
                painted_face_cost(FAR_FACE_RINGS.len(), segments)
            } else {
                painted_face_cost(NEAR_FACE_RINGS.len(), segments * 5 / 4)
            };
            (2 * segments + face) * wheel_looks.len()
        };
        while segments > MIN_WHEEL_SEGMENTS && wheel_cost(segments) * 3 > lod_budget {
            segments -= 2;
        }
        let affordable = wheel_cost(segments) * 3 <= lod_budget;
        match white {
            Some(white) if !wheel_looks.is_empty() && affordable => {
                // The wheels' cost comes off the top, so what they no longer spend goes to the
                // bodywork rather than back to the wheels. The whole wheel is built, rim and all, at
                // both levels: the rim's spokes are painted on (see `painted_face`), because a
                // model rim decimated to its share came out as a blob.
                let mut wheels = generated_wheels(&wheel_looks, white, segments, far);
                let cost: usize = wheels.iter().flat_map(|b| &b.pieces).map(|p| p.indices.len() / 3).sum();
                for b in coarse.iter_mut() {
                    if b.wheel.is_some_and(|c| wheel_looks.iter().any(|w| w.corner == c)) {
                        b.pieces.clear();
                    }
                }
                coarse.retain(|b| !b.pieces.is_empty());
                spend_coarse(&mut coarse, lod_budget.saturating_sub(cost), atlas.working.span, pixel, flat.as_ref());
                finish_level(&mut wheels);
                coarse.extend(wheels);
            }
            _ => spend_coarse(&mut coarse, lod_budget, atlas.working.span, pixel, flat.as_ref()),
        }
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
            // Never smaller than the hub is high, which is the measurement that cannot be wrong:
            // the pipeline grounds every car so its wheels touch y = 0, and a wheel resting on the
            // road has its centre at exactly its own radius. So the hub's height *is* the rolling
            // radius, by construction.
            //
            // `measure` derives its figure from the largest part of the wheel by triangle count,
            // on the reasoning that a caliper standing proud of the tread would otherwise inflate
            // it. That guards the right way for a model whose tyre is the biggest part and the
            // wrong way for the many where it is not: a detailed rim easily outweighs the tyre
            // wrapped around it, and a rim is *smaller* in diameter, so the radius came out short.
            // Eleven of twenty-three cars were affected, the E30 worst at 0.177 m against a hub
            // sitting 0.309 m up — a wheel little more than half the size of the one modelled.
            //
            // It showed up as silhouettes with holes where the wheels belong, because those are
            // drawn as cylinders of this radius. The quieter half is worse: this is the rolling
            // radius, so those eleven cars span their wheels far too fast for the road speed.
            //
            // Taking the larger of the two keeps the caliper guard — a caliper cannot raise the
            // hub — while refusing to believe a wheel is smaller than the car standing on it.
            radius: rolling_radius(config, w, placement.scale, *hub),
            // Measured off the source tyre and stored so the renderer can put it back. The wheel's
            // vertices are written upright below, with the tilt taken out of them, because a wheel
            // is spun by rotating about its axle and an axle baked in at an angle turns that spin
            // into a wobble.
            camber: w.camber,
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
    // Built from the welded original rather than handed the coarsest LOD, which is what it used to
    // be and what made it look like a crushed can. Two things were wrong with that, and both come
    // from a level tuned for a car eighteen pixels tall being asked to be one four hundred pixels
    // tall:
    //
    // * **The budget went to parts with no outline in them.** LOD2 shares its triangles across
    //   every category by how many pixels each is worth over the visibility sweep, and the E36's
    //   interior is 4,841 triangles of seats and door cards that are *inside the shell*. Every one
    //   it kept was a triangle the bodywork did not get.
    // * **Decimation shrinks a car into itself.** Collapsing edges pulls a convex surface inward,
    //   which is invisible at 45 m and is the entire subject at 5 m: the arches and sills went
    //   first, and the wheels ended up standing outside a body that had retreated from them.
    //   Spending a budget of its own on the outside of the car alone leaves the shell enough
    //   triangles to keep its own width.
    //
    // Every part goes in except the wheels', which are generated (see `silhouette_wheels`). It used
    // to be `body` and `window` only, on the reasoning that nothing else can be seen past a filled
    // outline, and that was wrong in both directions. Wings, splitters, diffusers and side skirts
    // are carbon or black plastic, and categorise as `chrome`: the NSX's whole aero kit, wing
    // included, was missing from its silhouette. And a cabin whose material carries an alpha
    // channel is `window` by default and `interior` once a config says what it is, so fixing a
    // car's seats took its cabin out of its silhouette — the E30 went from 0.3% missing to 15.8%.
    // What a part is called no longer matters: `simplify::reduce_shell` removes everything that is
    // covered from every direction before it spends anything, so the seats behind the glass cost
    // nothing and the glass in front of them is kept.
    //
    // Dropping whole parts is not the same thing as cutting a mesh up to decimate the pieces,
    // which cracks bodywork and is never done here: the parts go in whole and are simplified as
    // one surface.
    let sil_buckets: Vec<&Bucket> = welded.iter().filter(|b| b.wheel.is_none()).collect();
    let mut silhouette = build_silhouette(&sil_buckets, origin_of);
    simplify::reduce_shell(
        &mut silhouette.0,
        &mut silhouette.1,
        config.silhouette.unwrap_or(SILHOUETTE_TRIANGLES),
    );
    silhouette_wheels(&wheel_defs, &mut silhouette.0, &mut silhouette.1);
    let silhouette = narrow_silhouette(silhouette.0, silhouette.1);
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
        atlas: atlas.packed.pixels,
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
    // Ties go to the lower colour. A HashMap iterates in a different order every run, and a tie
    // decided by that order made the same car compile to different bytes from one build to the next.
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))
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

/// How many border vertices two parts must have in common before they are taken to be one surface
/// an exporter split, rather than two parts that happen to touch at a corner.
///
/// Every pair this has ever been asked about is far above it or at zero: the RAV4's two halves of
/// its paint share 19,263, the Lada's tread patches 2,584 and 7,565, the smallest real case (the
/// Lada's front wing, split in two) 38. It exists so that a stray coincident vertex or two cannot
/// join parts, not to draw a line anywhere interesting.
const REJOIN_SHARED_BORDER: usize = 8;

/// …and how much of the smaller part's border that has to be, for the two to be one surface rather
/// than two objects an exporter happened to write as one primitive.
///
/// The M5 is why this exists. Its cabin is one material in one object, split at 95,265 and 15,556
/// triangles, and the halves meet along 364 border vertices — 15% of the smaller one's edge. So
/// they are mostly separate furniture that touches, and joined they were worse than apart: the
/// larger piece's error went from 9% to 19.5%, pruning at that error removed the near rear door card
/// whole, and the sky showed through the rear side window. Every pair where joining was what fixed
/// the car shares far more — the RAV4's paint 86%, the Lada's tread 80% and 99%, the AE86's paint
/// 94% — and nothing measured falls between 34% and 51%, which is where the line is drawn.
const REJOIN_SHARE: f32 = 0.4;

/// Puts back together the parts an exporter split out of one surface, so each is welded and
/// decimated as the single mesh it was modelled as.
///
/// This is the other side of the rule that a mesh is never divided to decimate it, and it exists
/// because source files break that rule on the compiler's behalf. A glTF primitive is indexed with
/// 16 bits by most exporters, so the RAV4's paint arrived as `Object_32` — stopped at 65,532
/// vertices — and `Object_33` carrying on from exactly where it left off, the two interleaved
/// across every panel of the car. The Lada's tyre node holds two primitives of the same name, the
/// shoulder blocks and the band between them, that fit together along a border thousands of
/// vertices long. Handed to the decimator as two parts, each half is reduced with nothing relating
/// it to the other, the shared border drifts apart from both sides, and what reaches the screen is
/// bodywork covered in see-through slivers and white flecks exactly along the lines the exporter
/// cut. No weight fixes it: both halves stop at the free error with their borders already apart.
///
/// Three things have to agree before two parts are one surface, and each one is there because of
/// what joining the wrong two would cost:
///
/// * **The same bucket.** Already the unit a draw call is made of, so joining across one is not
///   possible in the first place — and it means a wheel's parts only ever join their own wheel's.
/// * **The same material and the same parent node.** The parent is the object a person would name
///   (see `Part::parent`), so this is "one object, one material", which is what an exporter splits
///   and nothing else is. Without it, a bumper and the wing its edge sits on join because they are
///   both paint, and then one part's budget is being spent by the decimator's error metric over
///   both instead of by the visibility sweep over each — the separation the per-part budget exists
///   for. Across the whole fleet the three conditions together join 26 parts into 12 surfaces on
///   five cars, every one of them a split primitive; a shared border alone matched about 3,400
///   pairs, almost all of them panels that touch.
/// * **A shared border, and a large one.** Positions on the edge of both, to the weld grid, making
///   up a good part of the smaller one's edge (`REJOIN_SHARE`). It is what makes the join safe to
///   do: two parts that meet along a border come out of welding as one connected surface, which
///   the decimator then treats as interior, and there is no border left to open.
///
/// What a joined piece carries is decided once, for the whole of it, because the whole of it is
/// one mesh from here on:
///
/// * pixels are **summed**, which is what the sweep would have measured of the surface unsplit;
/// * the config's `[reduce.parts]` weight is the **pixel-weighted mean** of the parts', which is
///   the only value that leaves the bucket's share-out arithmetic (pixels × weight, summed) exactly
///   as it was — a pattern that named one half still moves the budget by as much as it did;
/// * it is **two-sided if any part was**, for the reason whole parts are two-sided at all: the
///   alternative is a hole where the sweep saw one. `[reduce] drop` needs no rule here because it
///   is applied to each part by its own name before anything reaches a bucket.
///
/// Returns how many parts went in and how many surfaces came out, for the report.
fn rejoin_split_parts(buckets: &mut [Bucket]) -> (usize, usize) {
    let (mut parts_in, mut surfaces_out) = (0, 0);
    for b in buckets.iter_mut() {
        // Only parts with a partner of the same object and material are worth the border walk,
        // which on a 100,000-triangle body shell is not free.
        let mut groups: HashMap<(usize, &str), Vec<usize>> = HashMap::new();
        for (i, p) in b.pieces.iter().enumerate() {
            groups.entry((p.material, p.parent.as_str())).or_default().push(i);
        }
        let mut owner: Vec<usize> = (0..b.pieces.len()).collect();
        fn root(owner: &mut [usize], mut i: usize) -> usize {
            while owner[i] != i {
                owner[i] = owner[owner[i]];
                i = owner[i];
            }
            i
        }
        let mut any = false;
        for members in groups.values().filter(|m| m.len() > 1) {
            let mut on_border: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
            let mut border_len: HashMap<usize, usize> = HashMap::new();
            for &i in members {
                let border = border_positions(&b.pieces[i]);
                border_len.insert(i, border.len());
                for k in border {
                    on_border.entry(k).or_default().push(i);
                }
            }
            let mut shared: HashMap<(usize, usize), usize> = HashMap::new();
            for list in on_border.values() {
                for x in 0..list.len() {
                    for y in x + 1..list.len() {
                        *shared.entry((list[x], list[y])).or_default() += 1;
                    }
                }
            }
            for ((x, y), n) in shared {
                let smaller = border_len[&x].min(border_len[&y]).max(1);
                if n >= REJOIN_SHARED_BORDER && n as f32 >= smaller as f32 * REJOIN_SHARE {
                    let (rx, ry) = (root(&mut owner, x), root(&mut owner, y));
                    if rx != ry {
                        owner[rx.max(ry)] = rx.min(ry);
                        any = true;
                    }
                }
            }
        }
        if !any {
            continue;
        }

        // Joined in source order, into the lowest-numbered part of each set, so a bucket with
        // nothing to rejoin is untouched and one with something keeps every other part where it
        // was.
        let pieces = std::mem::take(&mut b.pieces);
        let mut at: HashMap<usize, usize> = HashMap::new();
        let mut members: Vec<usize> = Vec::new();
        // Pixels × weight, summed, for the weighted mean.
        let mut worth: Vec<f64> = Vec::new();
        for (i, p) in pieces.into_iter().enumerate() {
            let r = root(&mut owner, i);
            match at.get(&r) {
                None => {
                    at.insert(r, b.pieces.len());
                    members.push(1);
                    worth.push(p.pixels as f64 * p.weight as f64);
                    b.pieces.push(p);
                }
                Some(&slot) => {
                    members[slot] += 1;
                    worth[slot] += p.pixels as f64 * p.weight as f64;
                    let into = &mut b.pieces[slot];
                    let base = into.vertices.len() as u32;
                    into.vertices.extend_from_slice(&p.vertices);
                    into.attrs.extend_from_slice(&p.attrs);
                    into.indices.extend(p.indices.iter().map(|i| i + base));
                    into.pixels += p.pixels;
                    into.two_sided |= p.two_sided;
                    // A weight is only meaningful against pixels, so with none to go on the
                    // larger of the two is kept rather than an average of nothing.
                    into.weight = into.weight.max(p.weight);
                }
            }
        }
        for (slot, p) in b.pieces.iter_mut().enumerate() {
            if members[slot] > 1 {
                parts_in += members[slot];
                surfaces_out += 1;
                if p.pixels > 0 {
                    p.weight = (worth[slot] / p.pixels as f64) as f32;
                }
            }
        }
    }
    (parts_in, surfaces_out)
}

/// The positions, to the weld grid, of every vertex on an edge of this part — an edge only one of
/// its triangles uses.
///
/// On position alone, not on the welder's full key: a split primitive's two halves duplicate the
/// vertices along the cut, and whether their colours and texture coordinates also agree is for the
/// welder to find out afterwards. What this has to answer is only whether the two parts meet.
fn border_positions(p: &Piece) -> Vec<[i32; 3]> {
    let key: Vec<[i32; 3]> = p
        .vertices
        .iter()
        .map(|v| [simplify::quantise(v.x), simplify::quantise(v.y), simplify::quantise(v.z)])
        .collect();
    let mut edges: HashMap<([i32; 3], [i32; 3]), u32> = HashMap::new();
    for t in p.indices.chunks_exact(3) {
        let c = [key[t[0] as usize], key[t[1] as usize], key[t[2] as usize]];
        if c[0] == c[1] || c[1] == c[2] || c[0] == c[2] {
            continue;
        }
        for (a, z) in [(c[0], c[1]), (c[1], c[2]), (c[2], c[0])] {
            *edges.entry(if a < z { (a, z) } else { (z, a) }).or_default() += 1;
        }
    }
    let mut out: Vec<[i32; 3]> = edges
        .into_iter()
        .filter(|(_, n)| *n == 1)
        .flat_map(|((a, z), _)| [a, z])
        .collect();
    out.sort_unstable();
    out.dedup();
    out
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

/// How many sides a generated wheel has.
///
/// Twelve is round at the size a title screen draws a car, and costs 48 triangles a wheel: 24 for
/// the tread and 24 for the two faces. The faces are not optional — the car is looked at three
/// quarters on, and an open tube shows the scenery through the middle of its own wheel.
const WHEEL_SEGMENTS: usize = 12;

/// How many triangles a silhouette gets, unless a car's config asks for something else, not
/// counting its wheels.
///
/// A thousand. It was six hundred, when the silhouette was clustered onto a grid sized by this
/// number, and at six hundred that grid was coarse enough to round the bottom off a long car: a
/// strip of missing sill along the whole length and a missing front air dam, which is exactly the
/// sort of fault that is invisible in isolation and obvious when the shadow is replaced by the car
/// it stood in for. Edge collapse spends where the outline is rather than evenly, so the number no
/// longer has to be raised for a long car or a tall one; at a thousand the fleet averages about
/// 0.2% of the car missing over the golden views, which is a rim a pixel wide.
///
/// The number is a size as much as a shape: the console reads a car in 32 KB chunks and draws the
/// silhouette out of the first one, so a silhouette that does not fit in a chunk is a silhouette
/// that arrives a frame late. A thousand triangles and the wheels come to 13–17 KB, since a
/// collapsed shell keeps more vertices per triangle than a clustered one did; 1,400 is about 20 KB,
/// and the chunk still has room.
const SILHOUETTE_TRIANGLES: usize = 1000;

/// Flattens buckets into one positions-only array in car space, at full detail.
///
/// Everything that made these buckets separate — category, material, which wheel they belong to,
/// whether they are two-sided — is thrown away here, because a silhouette is drawn in one colour
/// with culling off and none of it would change a pixel. What comes out is the shell as modelled,
/// hundreds of thousands of triangles of it, for `simplify::reduce_shell` to weld and cut down as
/// a single surface.
///
/// Read out of `pieces` rather than out of `vertices`: these are the welded buckets, kept before
/// anything was decimated, and a bucket's merged arrays are not filled in until a budget has been
/// spent on it.
fn build_silhouette(
    group: &[&Bucket],
    origin_of: impl Fn(Option<u8>) -> [f32; 3],
) -> (Vec<[f32; 3]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for b in group {
        let origin = origin_of(b.wheel);
        for p in &b.pieces {
            // A part the visibility sweep never saw a single pixel of cannot be part of an outline:
            // it is a floor pan, an inner wing, the back of a bumper skin. Category does not catch
            // these — they are `body`, the same category as the paint that covers them — and on a
            // shell of 131,424 source triangles they are most of what a budget gets spent on. The
            // sweep has already measured this, so it costs nothing to ask.
            if p.pixels == 0 {
                continue;
            }
            let base = positions.len() as u32;
            positions.extend(
                p.vertices
                    .iter()
                    .map(|v| [v.x + origin[0], v.y + origin[1], v.z + origin[2]]),
            );
            indices.extend(p.indices.iter().map(|i| base + *i));
        }
    }
    (positions, indices)
}

/// Narrows the finished silhouette to the 16-bit indices the format carries.
///
/// A shell that still needs more than 65,536 vertices after reduction produces no silhouette rather
/// than a truncated one: half a car is worse than none. It takes an absurd `silhouette` budget to
/// get there, which is exactly why the case is refused outright rather than handled.
fn narrow_silhouette(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> (Vec<[f32; 3]>, Vec<u16>) {
    if positions.is_empty() || positions.len() > u16::MAX as usize + 1 {
        return (Vec::new(), Vec::new());
    }
    let narrowed = indices.iter().map(|i| *i as u16).collect();
    (positions, narrowed)
}

/// Adds a plain cylinder at each hub, standing in for the wheel.
///
/// Built rather than decimated, which is the one place this pipeline generates geometry instead of
/// simplifying it, and it is worth saying why. A tyre is a tube: decimation at any budget a
/// silhouette can afford turns it into a mangled ring, and the rim that would fill the middle of it
/// is `chrome`, a category with 1,689 triangles a wheel and no business in an outline. The first
/// attempt gave each tyre 23 triangles and the car came out standing on four bent slivers.
///
/// A wheel's outline, though, is not something that has to be discovered: it is a circle of a
/// radius the compiler already measured, on an axle it already located. Forty-eight triangles of
/// cylinder is exactly right from every angle, where two hundred of decimated tyre was wrong from
/// all of them.
fn silhouette_wheels(
    wheels: &[WheelDef],
    positions: &mut Vec<[f32; 3]>,
    indices: &mut Vec<u32>,
) {
    for w in wheels {
        let base = positions.len() as u32;
        let half = w.width * 0.5;
        // The axle is the car's X axis, leaned over by the wheel's camber — the silhouette is a
        // stand-in for the car as it stands, and a stanced car's wheels visibly lean.
        let (cs, cc) = w.camber.sin_cos();
        let place = |x: f32, y: f32, z: f32| {
            [w.hub[0] + x * cc - y * cs, w.hub[1] + x * cs + y * cc, w.hub[2] + z]
        };
        for side in 0..2u32 {
            let x = if side == 0 { -half } else { half };
            for i in 0..WHEEL_SEGMENTS {
                let a = i as f32 / WHEEL_SEGMENTS as f32 * std::f32::consts::TAU;
                positions.push(place(x, w.radius * a.sin(), w.radius * a.cos()));
            }
        }
        // Then the two hub centres, for the faces to fan around.
        positions.push(place(-half, 0.0, 0.0));
        positions.push(place(half, 0.0, 0.0));
        let n = WHEEL_SEGMENTS as u32;
        let centres = [base + 2 * n, base + 2 * n + 1];

        for i in 0..n {
            let (a, b) = (i, (i + 1) % n);
            // The tread, as two triangles across the width.
            indices.extend([base + a, base + b, base + n + b]);
            indices.extend([base + a, base + n + b, base + n + a]);
            // A face at each end. Winding is not worth getting right: a silhouette is drawn with
            // culling off, so both sides of every one of these triangles is the same flat colour.
            indices.extend([centres[0], base + a, base + b]);
            indices.extend([centres[1], base + n + a, base + n + b]);
        }
    }
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
        // Carried from the per-material layout everything upstream was built in to the packed one
        // the texture section holds, here and nowhere earlier: nothing after this line looks at a
        // coordinate to decide anything about the mesh. See `texture::Atlas::build`.
        let uv = atlas.remap(uvs.get(i).copied().unwrap_or([0.0, 0.0]));
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

/// Like `share_budget`, but a part that cannot have the floor is dropped rather than raised to it.
///
/// `share_budget` pins anything that would get less than the floor *at* the floor, which is right
/// when the floors add up to a small part of the budget and wrong when they add up to more than all
/// of it. Every pin then takes budget from the parts still open, which pushes more of them under the
/// floor, and the pass ends with every part at the floor — the M5's 210 body parts at LOD2 each got
/// four triangles, its one-piece paint shell included. So here the least valuable part is left out
/// instead, and the rest shared again, until every part that is kept can have the floor.
fn share_or_drop(claims: &[(f64, usize)], budget: usize, floor: usize) -> Vec<usize> {
    let mut open: Vec<usize> = (0..claims.len()).collect();
    // Most valuable first, so the least valuable is the one at the end to pop.
    open.sort_by(|a, b| claims[*b].0.total_cmp(&claims[*a].0).then(a.cmp(b)));
    loop {
        let kept: Vec<(f64, usize)> = open.iter().map(|&i| claims[i]).collect();
        let shares = share_budget(&kept, budget, 0);
        let short = open
            .iter()
            .zip(&shares)
            .any(|(&i, &s)| s < floor.min(claims[i].1));
        if !short || open.len() <= 1 {
            let mut out = vec![0; claims.len()];
            for (&i, &s) in open.iter().zip(&shares) {
                out[i] = s.max(floor.min(claims[i].1));
            }
            return out;
        }
        open.pop();
    }
}

/// Spends a coarse level's budget, and is why the budgets are now kept.
///
/// LOD1 and LOD2 used to be `spend_budget` again with a smaller number, and three things in it that
/// are harmless at 24,000 triangles broke at 3,000 and 1,200:
///
/// * **The per-part floor overcommitted** (see `share_or_drop`). A bucket of a hundred or two parts
///   at four triangles each is more than the bucket has, so every part was pinned at four, and the
///   level came out at two or three times its budget with nothing in it big enough to be a panel.
///   That is why the M5 wrote 4,106 and 2,976 triangles against 3,000 and 1,200: its wheel hardware
///   alone was 85 to 124 parts a corner at four triangles each, against a share of 33.
/// * **Collapse was allowed any error**, and with pruning an open error is licence to remove large
///   components whole — door skins, bonnets, the roof. See `simplify::reduce_coarse`.
/// * **Clustering smeared the atlas.** A part held at four triangles that collapse cannot take
///   there goes to vertex clustering, which the M5's 62,000-triangle paint shell did, and what came
///   back was shards with the texture dragged across them. Also `simplify::reduce_coarse`.
///
/// What is kept from LOD0 is the share-out between the draw calls, config weights and all, since
/// those say what each category is worth on this car — flattened, and with kept rims capped, see
/// `COARSE_BUCKET_POWER` and `rim_cap`; the parts inside each bucket are shared by what the sweep
/// saw of them, flattened too (`COARSE_PART_POWER`), with the least-seen left out when there is
/// not enough to go round.
/// A part that still had to be clustered is drawn two-sided, because clustering flips triangles and
/// culling turns a flipped triangle into a hole; and one allocated next to nothing that will not
/// come down is dropped, as at LOD0 (`STUCK_TARGET`).
fn spend_coarse(
    buckets: &mut Vec<Bucket>,
    budget: usize,
    tile_span: f32,
    pixel: f32,
    flat: Option<&simplify::FlatTexel>,
) {
    // A level is held to its budget, and a target is not a promise: collapse is accepted a
    // quarter over its target, and a part clustering cannot bring down keeps its smallest answer.
    // Those almost always net out against the parts that come in under, and when they do not the
    // level is built again with the overshoot taken off what it asks for. The M5's LOD2 came out
    // at 1,201 of 1,200 before this.
    let welded = buckets.clone();
    // The whole level as LOD0 draws it, for judging stalled parts in place (see
    // `simplify::Scene`). Wheels are stored about their hubs rather than where they are on the
    // car, and are left out of it; a coarse level builds its own anyway. Glass is left out too,
    // being seen through: in it, a cabin was behind the windows and left out whole for nothing.
    let mut next_id = 0u32;
    let parts: Vec<(u32, &[Vertex], &[u32])> = welded
        .iter()
        .filter(|b| in_scene(b))
        .flat_map(|b| b.pieces.iter())
        .map(|p| {
            next_id += 1;
            (next_id - 1, &p.vertices[..], &p.indices[..])
        })
        .collect();
    let scene = simplify::Scene::build(&parts, pixel * simplify::JUDGE_CELL);
    let mut ask = budget;
    for _ in 0..8 {
        *buckets = welded.clone();
        spend_coarse_once(buckets, ask, tile_span, pixel, flat, &scene);
        let got: usize = buckets.iter().map(|b| b.indices.len() / 3).sum();
        if got <= budget || ask == 0 {
            return;
        }
        ask = ask.saturating_sub(got - budget);
    }
}

fn spend_coarse_once(
    buckets: &mut Vec<Bucket>,
    budget: usize,
    tile_span: f32,
    pixel: f32,
    flat: Option<&simplify::FlatTexel>,
    scene: &simplify::Scene,
) {
    // Each part's index in `scene`, in the order `spend_coarse` built it.
    let mut next_id = 0u32;
    let scene_ids: Vec<Vec<Option<u32>>> = buckets
        .iter()
        .map(|b| {
            b.pieces
                .iter()
                .map(|_| {
                    in_scene(b).then(|| {
                        next_id += 1;
                        next_id - 1
                    })
                })
                .collect()
        })
        .collect();
    // Where a level's wheels are not generated (an atlas with no white tile, or a level too small
    // to afford them), a wheel's draw call is capped at a fortieth of the level (75 triangles at
    // LOD1). It was written for the model rims LOD1 used to keep — enough for five
    // spokes, and a ceiling the config's weights cannot lift. They were set for LOD0, where the
    // E36's `chrome = 12` buys its mesh wheels; at LOD1 the same weight gave its four rims 832 of
    // 3,000 triangles against 820 for the whole body, and the nose shredded. Capping the claim
    // rather than the result hands what the rims cannot take back to everything else.
    let rim_cap = (budget / 40).max(MIN_BUCKET_TRIANGLES);
    let defaults = crate::config::Reduction::default();
    let targets = share_budget(
        &buckets
            .iter()
            .map(|b| {
                let have = b.weights().1;
                // The config's category weight is taken at its square root against the default,
                // not whole: see `COARSE_WEIGHT_POWER`.
                let default = defaults.bucket_weight(b.category, b.wheel.is_some()) as f64;
                let weight = default * (b.weight as f64 / default.max(1.0e-6)).powf(COARSE_WEIGHT_POWER);
                let w = b.pixels as f64 * weight;
                (w.powf(COARSE_BUCKET_POWER), if b.wheel.is_some() { have.min(rim_cap) } else { have })
            })
            .collect::<Vec<_>>(),
        budget,
        MIN_BUCKET_TRIANGLES,
    );
    for ((b, target), ids) in buckets.iter_mut().zip(&targets).zip(&scene_ids) {
        // A part judged out (see `simplify::Coarse::Dropped`) hands its share back, and the draw
        // call is shared again among what is left, from the welded parts. Without it the Abarth's
        // LOD2 came out at 984 triangles of 1,200, the 190E's at 930. At most two share-outs
        // after the first: a part judged out on the second is rare, and on the third it is left
        // out without its share being handed on.
        let welded_pieces = b.pieces.clone();
        let mut judged_out = vec![false; welded_pieces.len()];
        for round in 0..3 {
            b.pieces = welded_pieces.clone();
            let piece_targets = share_or_drop(
                &b.pieces
                    .iter()
                    .zip(&judged_out)
                    .map(|(p, &out)| {
                        if out {
                            (0.0, 0)
                        } else {
                            ((p.pixels as f64 * p.weight as f64).powf(COARSE_PART_POWER), p.indices.len() / 3)
                        }
                    })
                    .collect::<Vec<_>>(),
                *target,
                COARSE_PART_FLOOR,
            );
            let mut more = false;
            for (k, (p, &t)) in b.pieces.iter_mut().zip(&piece_targets).enumerate() {
                let was = p.indices.len() / 3;
                let mut outcome = simplify::Coarse::Collapsed;
                if t == 0 {
                    p.indices.clear();
                } else {
                    outcome = simplify::reduce_coarse(
                        &mut p.vertices,
                        &mut p.attrs,
                        &mut p.indices,
                        t,
                        tile_span,
                        pixel,
                        flat,
                        // A cabin is seen through tinted glass: its colour matters there, its
                        // texture does not.
                        b.category == Category::Interior,
                        ids[k].map(|id| (scene, id)),
                    );
                    if outcome == simplify::Coarse::Clustered {
                        p.two_sided = true;
                    }
                    if outcome == simplify::Coarse::Dropped {
                        judged_out[k] = true;
                        more = true;
                    }
                    if t <= STUCK_TARGET && p.indices.len() / 3 > t * 4 {
                        p.indices.clear();
                    }
                    relight(p, b.category, outcome == simplify::Coarse::Clustered);
                }
                if std::env::var("AZ_PARTS").is_ok() {
                    eprintln!(
                        "COARSE {:>7} -> {:>6} (target {:>6}) {:>8} px {}{}",
                        was,
                        p.indices.len() / 3,
                        t,
                        p.pixels,
                        match outcome {
                            simplify::Coarse::Collapsed => "",
                            simplify::Coarse::Clustered => "clustered ",
                            simplify::Coarse::Dropped => "judged out ",
                        },
                        p.node
                    );
                }
            }
            if !more || round == 2 {
                break;
            }
        }
        b.pieces.retain(|p| p.indices.len() >= 3);
        if b.category == Category::Interior && b.wheel.is_none() {
            lower_cabin_ceiling(b, pixel);
        }
        if std::env::var("AZ_PARTS").is_ok() {
            eprintln!(
                "COARSE BUCKET {:?} {:?}: target {} got {}",
                b.category,
                b.wheel,
                target,
                b.pieces.iter().map(|p| p.indices.len() / 3).sum::<usize>()
            );
        }
    }
    finish_level(buckets);
}

/// Whether a draw call's parts are in the scene stalled coarse parts are judged in.
fn in_scene(b: &Bucket) -> bool {
    b.wheel.is_none() && b.category != Category::Window
}

/// Lights a coarse part from its own faces rather than from the light its vertices were welded
/// with.
///
/// A vertex's light is the mean over every source vertex welded into it, and where a panel turns a
/// corner — a bonnet's leading edge rolling under, a roof meeting its gutter — that is a mean of an
/// upward face and a sideways or downward one. At LOD0 such vertices sit in a strip a few
/// millimetres wide. A coarse level keeps exactly those vertices, because the corners are where
/// the shape is, and spans a whole panel between them, so the grey of the corner was drawn across
/// it: white cars came out grey at LOD1 and LOD2 (the E30's and Abarth's bonnets, the AE86's flank,
/// the E30's roof at LOD2 near black). Relit from the area-weighted normal of the part's own
/// triangles, the panel is lit as the panel it now is.
///
/// Clustering flips triangles, so a clustered part's faces are all turned upward and it keeps the
/// brighter of its old light and the new: its normals are too rough to darken anything with, and
/// the fault being mended is only ever a panel too dark. The underside of a clustered part
/// brightens with it, which no game camera sees.
fn relight(p: &mut Piece, category: Category, clustered: bool) {
    let mut sum = vec![[0.0f32; 3]; p.vertices.len()];
    for t in p.indices.chunks_exact(3) {
        let (a, b, c) = (&p.vertices[t[0] as usize], &p.vertices[t[1] as usize], &p.vertices[t[2] as usize]);
        let e1 = [b.x - a.x, b.y - a.y, b.z - a.z];
        let e2 = [c.x - a.x, c.y - a.y, c.z - a.z];
        let mut n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
        if clustered && n[1] < 0.0 {
            n = [-n[0], -n[1], -n[2]];
        }
        for &i in t {
            for k in 0..3 {
                sum[i as usize][k] += n[k];
            }
        }
    }
    for (a, n) in p.attrs.iter_mut().zip(&sum) {
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if l > 0.0 {
            let lit = light_at([n[0] / l, n[1] / l, n[2] / l], category);
            a.light = if clustered { a.light.max(lit) } else { lit };
        }
    }
}

/// Lowers a coarse level's cabin ceiling by a pixel, leaving everything else of it where it was.
///
/// The headliner sits a few centimetres under the roof, and a coarse level moves every surface by
/// up to a pixel (the collapse limit), or further where clustering has had it. So the headliner
/// came up through the roof: the Lancia Delta's LOD2 roof drew black where the tub won the depth
/// test over it, its LOD1 roof was holed the same way, and so was the E36's. Only the upper half
/// moves, and only down, by up to a pixel at the very top: pulling the whole cabin in towards its
/// middle was tried and opened the E30's doors, whose skins are `interior` in its config.
fn lower_cabin_ceiling(b: &mut Bucket, pixel: f32) {
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for p in &b.pieces {
        for v in &p.vertices {
            lo = lo.min(v.y);
            hi = hi.max(v.y);
        }
    }
    let middle = (lo + hi) * 0.5;
    let half = (hi - lo) * 0.5;
    if half <= pixel * 2.0 {
        return;
    }
    for p in &mut b.pieces {
        for v in &mut p.vertices {
            if v.y > middle {
                v.y -= pixel * (v.y - middle) / half;
            }
        }
    }
}

/// Fewest triangles a part keeps at a coarse level before it is left out instead. Half LOD0's
/// `MIN_PIECE_TRIANGLES`: at LOD2 a two-triangle quad is a number plate, a tail lamp, a grille
/// backing, and four each cost the E30 its lamps and plate and the Lada its grille, because four
/// was more than the least-seen of them could be given and `share_or_drop` left them out.
const COARSE_PART_FLOOR: usize = 2;
/// How flat the coarse share-outs are: parts share their draw call's budget by pixels to this
/// power, and draw calls the level's by their weight to `COARSE_BUCKET_POWER`.
///
/// Straight proportion is right for LOD0, where every part can have enough. At 1,200 triangles it
/// hands the paint shell nearly everything and leaves the parts that close the car — the floor
/// pan, the grille backing, the bumper centre, the headliner behind the glass — a couple of
/// triangles each, which clustering turns into holes: the E39's LOD1 floor went and showed its tan
/// cabin, its LOD2 bumper centre went, the 190E's bumpers took black slashes. Measured pixels are
/// counted from a sweep that mostly sees the top and sides, so the parts that close the car are
/// always the ones it undervalues. A power below one keeps the order and narrows the gap. Tried
/// fleet-wide: 1.0, 0.85, 0.75, 0.6 and 0.5 on parts, where 0.6 left the least uncovered at
/// both levels; 0.8 on draw calls is what gave the AE86 back its floor (a draw call of its own,
/// seen only from below) for no loss elsewhere worth the name.
const COARSE_PART_POWER: f64 = 0.6;
/// How much of a config's category weight a coarse level honours: the default weight times the
/// config's ratio to it, to this power.
///
/// The weights in the configs were set to fix LOD0 faults, and they are large: `light = 25` on the
/// Golf R, `chrome = 60` on the 190E, `interior = 2.0` (five times the default) on ten cars to buy
/// a cabin that reads through the glass at 5 m. Taken whole, even flattened by
/// `COARSE_BUCKET_POWER`, the NSX's and RX-7's cabins took a fifth of LOD2 and their roofs and
/// rear decks went to holes, and the E36's lamps had more of LOD2 than its paint shell. Ignoring
/// them (the default weights alone) mended those and opened the S14's front intake, whose black
/// backing is the `trim` its config weights at 4. The square root keeps which way the config
/// leans and takes most of the size out of it: measured fleet-wide against 0 and 1, it left the
/// least uncovered at LOD2 (2.02% mean, against 2.05% and 2.14%) and kept the S14's intake shut.
const COARSE_WEIGHT_POWER: f64 = 0.5;
const COARSE_BUCKET_POWER: f64 = 0.8;

/// Sides of a generated wheel: sixteen at LOD1, where a wheel is about ten pixels across, and ten
/// from `FAR_WHEEL_FROM` on, where it is four. About 150 and fifty triangles a wheel, most of the
/// first in its painted face.
const NEAR_WHEEL_SEGMENTS: usize = 16;
const FAR_WHEEL_SEGMENTS: usize = 10;
const MIN_WHEEL_SEGMENTS: usize = 6;
const FAR_WHEEL_FROM: f32 = 40.0;
/// A part of a wheel reaching this far out, as a fraction of the rolling radius, is tyre: tread,
/// sidewall, or a one-piece wheel that includes them. Anything short of it is the rim and what is
/// behind it. Measured off the geometry, not the category, because configs file the parts of a
/// wheel wherever LOD0 needed them: the M5's spoke face is `tyre`, the E30's tyre is in with its
/// rim, the Civic EJ's wheel is one part.
const TYRE_REACH: f32 = 0.92;
/// Cells across the view a wheel is measured through, face on. See `WheelLook::measure`.
const WHEEL_VIEW: usize = 96;

/// What one corner's wheel looks like from outside, measured off LOD0's welded parts.
struct WheelLook {
    corner: u8,
    radius: f32,
    width: f32,
    /// +1 when the outside of the wheel faces +X, -1 when it faces -X.
    outside: f32,
    /// Unlit colours, as a vertex carries them before `finish_level` folds the light in.
    tyre: u32,
    rim: u32,
    /// How far out the rim reaches, as a fraction of the radius: where the drawn tyre ends.
    rim_fraction: f32,
    /// The wheel face on from outside, `WHEEL_VIEW` cells square across the rolling diameter: per
    /// cell the unlit colour (texel folded in) and the light of whatever LOD0 draws nearest there,
    /// or nothing where LOD0 draws nothing of the wheel. What `painted_face` paints from.
    face: Vec<Option<([f32; 3], f32)>>,
}

/// Whether a wheel part reaches the tread. See `TYRE_REACH`.
fn reaches_tread(p: &Piece, radius: f32) -> bool {
    p.vertices
        .iter()
        .any(|v| (v.y * v.y + v.z * v.z).sqrt() >= radius * TYRE_REACH)
}

impl WheelLook {
    /// Measures a corner by looking at it: the wheel's parts are rasterised face on, from outside,
    /// into a small grid with a depth test, each cell taking the vertex colour times the texel
    /// under it, unlit. The tyre's colour is the mean of the cells a tread-reaching part won, the
    /// rim's of the cells anything else won, and the rim ends where those stop.
    ///
    /// It replaces a mean over every outward-facing triangle, which got the colours wrong across
    /// the fleet in three different ways. It counted what the eye cannot see — the brake disc
    /// behind the spokes and the barrel inside the lip weigh the same as the spoke face, which
    /// turned the 350Z's gold rims and the E39's silver ones brown. It trusted categories — on the
    /// E30 and E36 the tyre bucket holds the rim, so the tyre came out grey, and on the RAV4, Golf
    /// R32 and W70 the non-tyre parts were the dark hardware, which drew the drum inverted, a pale
    /// ring round a dark face. And it was lit, then lit again when drawn.
    fn measure(welded: &[Bucket], atlas: &texture::Atlas, corner: u8, radius: f32, width: f32, hub_x: f32) -> WheelLook {
        let outside = if hub_x >= 0.0 { 1.0 } else { -1.0 };
        let n = WHEEL_VIEW;
        let cell = 2.0 * radius / n as f32;
        // Per cell: nearest depth so far, colour, and whether a tread-reaching part won it.
        let mut depth = vec![f32::MIN; n * n];
        let mut colour = vec![[0.0f32; 3]; n * n];
        let mut tyre_cell = vec![false; n * n];
        let mut light = vec![0.0f32; n * n];
        let mut any_rim = false;
        for b in welded.iter().filter(|b| b.wheel == Some(corner)) {
            for p in &b.pieces {
                let tread = reaches_tread(p, radius);
                any_rim |= !tread;
                for t in p.indices.chunks_exact(3) {
                    let v = [&p.vertices[t[0] as usize], &p.vertices[t[1] as usize], &p.vertices[t[2] as usize]];
                    let a = [&p.attrs[t[0] as usize], &p.attrs[t[1] as usize], &p.attrs[t[2] as usize]];
                    // Face on from outside: across is z, up is y, nearer is further along `outside`.
                    let q: Vec<[f32; 2]> = v.iter().map(|v| [(v.z + radius) / cell, (v.y + radius) / cell]).collect();
                    let d = [v[0].x * outside, v[1].x * outside, v[2].x * outside];
                    let area = (q[1][0] - q[0][0]) * (q[2][1] - q[0][1]) - (q[2][0] - q[0][0]) * (q[1][1] - q[0][1]);
                    if area.abs() < 1.0e-9 {
                        continue;
                    }
                    let lo = |k: usize| q.iter().map(|p| p[k]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
                    let hi = |k: usize| (q.iter().map(|p| p[k]).fold(f32::MIN, f32::max).ceil() as usize).min(n);
                    for y in lo(1)..hi(1) {
                        for x in lo(0)..hi(0) {
                            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                            let w1 = ((px - q[0][0]) * (q[2][1] - q[0][1]) - (q[2][0] - q[0][0]) * (py - q[0][1])) / area;
                            let w2 = ((q[1][0] - q[0][0]) * (py - q[0][1]) - (px - q[0][0]) * (q[1][1] - q[0][1])) / area;
                            let w0 = 1.0 - w1 - w2;
                            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                                continue;
                            }
                            let z = d[0] * w0 + d[1] * w1 + d[2] * w2;
                            let at = y * n + x;
                            if z <= depth[at] {
                                continue;
                            }
                            depth[at] = z;
                            let uv = [
                                a[0].uv[0] * w0 + a[1].uv[0] * w1 + a[2].uv[0] * w2,
                                a[0].uv[1] * w0 + a[1].uv[1] * w1 + a[2].uv[1] * w2,
                            ];
                            let texel = atlas_texel(atlas, uv);
                            let mut c = [0.0f32; 3];
                            for (vert, w) in v.iter().zip([w0, w1, w2]) {
                                let u = unpack(vert.color);
                                for k in 0..3 {
                                    c[k] += u[k] * w;
                                }
                            }
                            for k in 0..3 {
                                c[k] *= texel[k];
                            }
                            colour[at] = c;
                            light[at] = a[0].light * w0 + a[1].light * w1 + a[2].light * w2;
                            tyre_cell[at] = tread;
                        }
                    }
                }
            }
        }

        // Only the disc: a cell outside the rolling radius is a caliper or a lip standing proud.
        let mut tyre = [0.0f64; 4];
        let mut rim = [0.0f64; 4];
        let mut rim_radii: Vec<f32> = Vec::new();
        for y in 0..n {
            for x in 0..n {
                let at = y * n + x;
                if depth[at] == f32::MIN {
                    continue;
                }
                let r = (((x as f32 + 0.5) * cell - radius).powi(2) + ((y as f32 + 0.5) * cell - radius).powi(2)).sqrt();
                if r > radius {
                    continue;
                }
                let target = if tyre_cell[at] { &mut tyre } else { &mut rim };
                for k in 0..3 {
                    target[k] += colour[at][k] as f64;
                }
                target[3] += 1.0;
                if !tyre_cell[at] {
                    rim_radii.push(r / radius);
                }
            }
        }
        let mean = |a: [f64; 4]| (a[3] > 0.0).then(|| pack([(a[0] / a[3]) as f32, (a[1] / a[3]) as f32, (a[2] / a[3]) as f32, 1.0]));
        let seen = tyre[3] + rim[3];
        let separate_rim = any_rim && rim[3] > seen * 0.15;
        let (tyre, rim, rim_fraction) = if separate_rim {
            rim_radii.sort_by(|a, b| a.total_cmp(b));
            let reach = rim_radii[rim_radii.len() * 19 / 20];
            (mean(tyre).unwrap_or(pack([0.1, 0.1, 0.1, 1.0])), mean(rim).unwrap(), reach.clamp(0.45, 0.9))
        } else {
            // One part, or a rim too small to tell: find where the rim ends by the brightness
            // across the radius instead. A tyre is darker than the lip it meets on every wheel
            // here, so the rim ends at the outermost band still halfway from the tyre's
            // brightness to the rim's brightest; a rim as dark as its tyre (the Murciélago's)
            // leaves nothing to find, and keeps two thirds, which is where most rims end.
            const BANDS: usize = 40;
            let mut bands = vec![([0.0f64; 3], 0.0f64, 0.0f64); BANDS];
            for y in 0..n {
                for x in 0..n {
                    let at = y * n + x;
                    let r = (((x as f32 + 0.5) * cell - radius).powi(2) + ((y as f32 + 0.5) * cell - radius).powi(2)).sqrt() / radius;
                    if depth[at] == f32::MIN || r >= 1.0 {
                        continue;
                    }
                    let b = &mut bands[(r * BANDS as f32) as usize];
                    for k in 0..3 {
                        b.0[k] += colour[at][k] as f64;
                    }
                    let c = colour[at];
                    b.1 += ((0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]) * light[at]) as f64;
                    b.2 += 1.0;
                }
            }
            let brightness = |b: usize| (bands[b].2 > 0.0).then(|| bands[b].1 / bands[b].2);
            let band_of = |f: f32| ((f * BANDS as f32) as usize).min(BANDS - 1);
            let tread: Vec<f64> = (band_of(0.92)..BANDS).filter_map(brightness).collect();
            let tread_level = tread.iter().sum::<f64>() / tread.len().max(1) as f64;
            let peak = (band_of(0.4)..band_of(0.9)).filter_map(brightness).fold(0.0f64, f64::max);
            let fraction = if peak - tread_level < 0.08 {
                0.66
            } else {
                let bar = tread_level + 0.5 * (peak - tread_level);
                (band_of(0.45)..band_of(0.9))
                    .rev()
                    .find(|&b| brightness(b).is_some_and(|v| v > bar))
                    .map_or(0.66, |b| (b + 1) as f32 / BANDS as f32)
            };
            let sum = |from: usize, to: usize| {
                let mut a = [0.0f64; 4];
                for b in &bands[from..to] {
                    for k in 0..3 {
                        a[k] += b.0[k];
                    }
                    a[3] += b.2;
                }
                a
            };
            (
                mean(sum(band_of(fraction + 0.05).max(band_of(0.9)), BANDS)).unwrap_or(pack([0.1, 0.1, 0.1, 1.0])),
                mean(sum(0, band_of(fraction))).unwrap_or(pack([0.5, 0.5, 0.5, 1.0])),
                fraction,
            )
        };
        let face = (0..n * n).map(|at| (depth[at] != f32::MIN).then(|| (colour[at], light[at]))).collect();
        WheelLook { corner, radius, width, outside, tyre, rim, rim_fraction, face }
    }
}

/// The atlas colour at a texture coordinate, nearest texel, as linear 0–1 RGBA.
fn atlas_texel(atlas: &texture::Atlas, uv: [f32; 2]) -> [f32; 4] {
    let n = texture::ATLAS;
    let x = ((uv[0] * n as f32) as usize).min(n - 1);
    let y = ((uv[1] * n as f32) as usize).min(n - 1);
    let at = (y * n + x) * 4;
    let p = &atlas.working.pixels[at..at + 4];
    [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0]
}

/// A texture coordinate that samples pure white with its neighbours white too, so a vertex there
/// draws exactly its own colour. The atlas has one wherever a material with no image got its tile;
/// a car whose every material is textured has none, and keeps its decimated wheels.
fn white_texel(atlas: &texture::Atlas) -> Option<[f32; 2]> {
    let n = texture::ATLAS;
    let white = |x: usize, y: usize| atlas.working.pixels[(y * n + x) * 4..(y * n + x) * 4 + 4] == [255, 255, 255, 255];
    for y in 2..n - 2 {
        for x in 2..n - 2 {
            if (y - 2..=y + 2).all(|yy| (x - 2..=x + 2).all(|xx| white(xx, yy))) {
                return Some([(x as f32 + 0.5) / n as f32, (y as f32 + 0.5) / n as f32]);
            }
        }
    }
    None
}

/// Builds each corner's wheel for a coarse level instead of decimating it.
///
/// The one other place this pipeline makes geometry rather than simplifying it is the silhouette's
/// wheels (see `silhouette_wheels`), and the argument is the same. At forty-five metres a wheel is
/// four pixels across, and what four pixels of wheel have to be is a dark disc with a lighter
/// middle. Decimated to its share of 1,200 triangles it was not: a tyre is a tube of tread blocks
/// that collapse cannot take below a few hundred triangles, so clustering took it the rest of the
/// way and the Civic EJ stood on four black slabs of four to eight triangles, the 190E's alloys
/// came out as spikes, and one corner of a car could lose its rim outright while its mirror image
/// kept one. A drum at the measured radius and width, in the colours LOD0 draws its tread in, is
/// round from every angle — and its cost is fixed, so the rest of the budget goes to the bodywork.
///
/// Its outside face is painted, not modelled: see `painted_face`. At LOD1 that is three rings of
/// sectors on the rim, which is where the spokes are drawn, and one on the sidewall; at LOD2 one
/// on each. Stored the way every wheel is, upright about its own hub with its axle along
/// X, so the renderer steers, cambers and spins it exactly as it does the decimated ones. Drawn
/// two-sided, so the outer face also serves as the inner one.
fn generated_wheels(looks: &[WheelLook], white: [f32; 2], n: usize, far: bool) -> Vec<Bucket> {
    looks
        .iter()
        .map(|w| {
            let mut vertices = Vec::new();
            let mut attrs = Vec::new();
            let mut indices: Vec<u32> = Vec::new();
            let mut push = |x: f32, y: f32, z: f32, colour: u32, light: f32| -> u32 {
                vertices.push(Vertex::new(x, y, z, colour));
                attrs.push(simplify::Attr { light, uv: white });
                (vertices.len() - 1) as u32
            };
            let (outer, inner) = (w.width * 0.5 * w.outside, -w.width * 0.5 * w.outside);
            let angle = |i: usize| i as f32 / n as f32 * std::f32::consts::TAU;
            // Lit as `light_at` lights everything else: the colours were measured unlit.
            let mut tread = Vec::new();
            for i in 0..n {
                let (s, c) = angle(i).sin_cos();
                let light = light_at([0.0, s, c], Category::Tyre);
                tread.push((
                    push(outer, w.radius * s, w.radius * c, w.tyre, light),
                    push(inner, w.radius * s, w.radius * c, w.tyre, light),
                ));
            }
            for i in 0..n {
                let j = (i + 1) % n;
                indices.extend([tread[i].0, tread[j].0, tread[j].1, tread[i].0, tread[j].1, tread[i].1]);
            }
            // The outside face, painted out to the tread: the rim's rings, then the sidewall's.
            let side = light_at([w.outside, 0.0, 0.0], Category::Tyre);
            let rings = face_rings(w, far);
            let (sectors, phase) = if far { (n, 0.0) } else { face_sectors(w, &rings, n * 3 / 4, n * 5 / 4) };
            let painted = painted_face(w, &rings, sectors, phase, side, far);
            let sector_angle = |k: usize| phase + k as f32 / sectors as f32 * std::f32::consts::TAU;
            for (r, row) in painted.iter().enumerate() {
                let (r0, r1) = (if r == 0 { 0.0 } else { rings[r - 1] * w.radius }, rings[r] * w.radius);
                for (k, &(colour, light)) in row.iter().enumerate() {
                    // Each sector has corners of its own, so its colour stops at its edges rather
                    // than blending into the next: a spoke and the gap beside it are one sector
                    // wide at ten pixels, and interpolated they would both be grey.
                    let (s0, c0) = sector_angle(k).sin_cos();
                    let (s1, c1) = sector_angle(k + 1).sin_cos();
                    let a1 = push(outer, r1 * s0, r1 * c0, colour, light);
                    let b1 = push(outer, r1 * s1, r1 * c1, colour, light);
                    if r0 == 0.0 {
                        let centre = push(outer, 0.0, 0.0, colour, light);
                        indices.extend([centre, a1, b1]);
                    } else {
                        let a0 = push(outer, r0 * s0, r0 * c0, colour, light);
                        let b0 = push(outer, r0 * s1, r0 * c1, colour, light);
                        indices.extend([a1, b1, b0, a1, b0, a0]);
                    }
                }
            }
            Bucket {
                category: Category::Tyre,
                wheel: Some(w.corner),
                pieces: vec![Piece {
                    vertices,
                    attrs,
                    indices,
                    pixels: 0,
                    weight: 1.0,
                    two_sided: true,
                    node: "generated wheel".into(),
                    parent: String::new(),
                    material: 0,
                }],
                vertices: Vec::new(),
                uvs: Vec::new(),
                indices: Vec::new(),
                source_triangles: 0,
                pixels: 0,
                weight: 1.0,
                two_sided_from: 0,
            }
        })
        .collect()
}

/// Rings of a painted face at LOD1, each one's outer edge as a fraction of the rim's radius,
/// innermost first: the hub, the spokes, and the lip the spokes run into.
const NEAR_FACE_RINGS: [f32; 3] = [0.3, 0.8, 1.0];
/// At LOD2 the rim is four pixels and one ring says all of it that can be seen.
const FAR_FACE_RINGS: [f32; 1] = [1.0];
/// What shows through a rim where LOD0 draws nothing of the wheel behind it: the dark inside of
/// the arch, unlit colour.
const FACE_GAP: [f32; 3] = [0.06, 0.06, 0.06];

/// A face's rings as fractions of the rolling radius: the rim's, scaled to where the rim was
/// measured to end, then the sidewall out to the tread. The sidewall is painted like the rest
/// rather than drawn in the tyre's colour, so a lip the measurement put on the wrong side of the
/// line, or white lettering, still shows as LOD0 draws it.
fn face_rings(w: &WheelLook, far: bool) -> Vec<f32> {
    let rim: &[f32] = if far { &FAR_FACE_RINGS } else { &NEAR_FACE_RINGS };
    rim.iter().map(|f| f * w.rim_fraction).chain([1.0]).collect()
}

/// Triangles a painted face costs, for a level to budget its wheels before it builds them.
fn painted_face_cost(rim_rings: usize, sectors: usize) -> usize {
    sectors * (2 * (rim_rings + 1) - 1)
}

/// Every cell of a wheel's face-on view that falls inside the tread: ring, angle, unlit colour and
/// light. A cell LOD0 draws nothing in is the gap colour, lit as a face turned sideways is.
fn face_samples(w: &WheelLook, rings: &[f32], gap_light: f32) -> Vec<(usize, f32, [f32; 3], f32)> {
    let n = WHEEL_VIEW;
    let cell = 2.0 * w.radius / n as f32;
    let mut out = Vec::new();
    for y in 0..n {
        for x in 0..n {
            // Across the view is z and up it is y, as `WheelLook::measure` laid it out.
            let (z, yy) = ((x as f32 + 0.5) * cell - w.radius, (y as f32 + 0.5) * cell - w.radius);
            let r = (z * z + yy * yy).sqrt() / w.radius;
            let Some(ring) = rings.iter().position(|&edge| r <= edge) else {
                continue;
            };
            let a = yy.atan2(z).rem_euclid(std::f32::consts::TAU);
            let (colour, light) = w.face[y * n + x].unwrap_or((FACE_GAP, gap_light));
            out.push((ring, a, colour, light));
        }
    }
    out
}

/// How many sectors a LOD1 face is cut into, and where the first one starts.
///
/// A wheel is ten pixels across at LOD1, its spokes a pixel or two wide, and a sector either lands
/// on a spoke or it averages spoke and gap into the grey disc that read as a hubcap. So the count
/// follows the spokes: the strongest harmonic of the brightness round the spoke ring, 3 to 12, is
/// taken as the spoke count and the sectors are its smallest multiple from `min` to `max` (12 to
/// 20 at LOD1) — fifteen for five spokes, twenty for ten, twelve for six, fourteen for seven. The
/// range is what the level can pay for: a painted face is seven triangles a sector, and at 16 to
/// 24 the wheels took some 350 triangles of LOD1 from the bodywork of the cars whose one-piece
/// wheels had cost 80 a corner, and the Murciélago's, RX-7's and Mini's lower noses thinned out.
/// The phase is the one of eight
/// that tells the sectors apart the most. A face with no spokes worth the name (a disc, a
/// fifteen-spoke mesh no grid of ten pixels can draw) gets `min`.
fn face_sectors(w: &WheelLook, rings: &[f32], min: usize, max: usize) -> (usize, f32) {
    let band = if rings.len() >= 4 { 1 } else { 0 };
    let samples: Vec<(f32, f32)> = face_samples(w, rings, AMBIENT)
        .into_iter()
        .filter(|s| s.0 == band)
        .map(|(_, a, c, l)| (a, (0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]) * l))
        .collect();
    if samples.is_empty() {
        return (min, 0.0);
    }
    const BINS: usize = 360;
    let mut profile = vec![(0.0f32, 0usize); BINS];
    for &(a, v) in &samples {
        let b = ((a / std::f32::consts::TAU * BINS as f32) as usize).min(BINS - 1);
        profile[b].0 += v;
        profile[b].1 += 1;
    }
    let mean = samples.iter().map(|s| s.1).sum::<f32>() / samples.len() as f32;
    let values: Vec<f32> = profile.iter().map(|&(s, c)| if c > 0 { s / c as f32 } else { mean }).collect();
    let (mut spokes, mut strongest) = (0usize, 0.0f32);
    for m in 3..=12usize {
        let (mut re, mut im) = (0.0f32, 0.0f32);
        for (b, v) in values.iter().enumerate() {
            let t = (m * b) as f32 / BINS as f32 * std::f32::consts::TAU;
            re += (v - mean) * t.cos();
            im += (v - mean) * t.sin();
        }
        let amplitude = 2.0 * (re * re + im * im).sqrt() / BINS as f32;
        if amplitude > strongest {
            (spokes, strongest) = (m, amplitude);
        }
    }
    // A brightness swing of under 4% round the ring is texture, not spokes.
    let count = if strongest < 0.04 {
        min
    } else {
        (min..=max).find(|s| s % spokes == 0).unwrap_or(min)
    };
    let mut best = (f32::MIN, 0.0f32);
    for p in 0..8 {
        let phase = p as f32 / 8.0 / count as f32 * std::f32::consts::TAU;
        let mut sums = vec![(0.0f32, 0usize); count];
        for &(a, v) in &samples {
            let k = (((a - phase).rem_euclid(std::f32::consts::TAU)) / std::f32::consts::TAU * count as f32) as usize;
            let k = k.min(count - 1);
            sums[k].0 += v;
            sums[k].1 += 1;
        }
        // Between-sector variance, up to a constant: how much of the ring's brightness the sectors
        // explain.
        let spread: f32 = sums.iter().filter(|s| s.1 > 0).map(|&(s, c)| (s / c as f32 - mean).powi(2) * c as f32).sum();
        if spread > best.0 {
            best = (spread, phase);
        }
    }
    (count, best.1)
}

/// The colour and light of each sector of each ring of a wheel's face, painted from LOD0's own
/// face-on view (see `WheelLook::measure`).
///
/// The kept model rim this replaces at LOD1 did not survive its share. A spoked alloy is a few
/// thousand triangles of separate spokes, bolts and lip; collapse stalls at a hundred and seventy
/// or so, and a rim is allowed a fortieth of the level, so it was clustered, and clustering filled
/// the gaps between the spokes — the Golf R's, the 350Z's, the Xsara's and the S15's rims all came
/// out as pale blobs, the E30's as white ones, and a one-piece wheel (the Abarth's, the Lancia's,
/// the Civic's) as a flat disc of its mean colour. What says which wheel it is at ten pixels is
/// the pattern of light and dark round the hub, and that is what a ring of sectors in the colours
/// LOD0 draws there keeps, for about as many triangles as the blob.
///
/// A sector's colour is the light-weighted mean of the unlit colours under it, and its light the
/// mean light, so that `finish_level`'s product of the two is the mean of what LOD0 draws there.
///
/// With `uniform` (LOD2) every sector of a ring takes the ring's mean: at four pixels a wheel has
/// no spokes to show, and ten sectors each in the colour of whatever spoke or gap it happened to
/// land on drew a pinwheel.
fn painted_face(w: &WheelLook, rings: &[f32], sectors: usize, phase: f32, gap_light: f32, uniform: bool) -> Vec<Vec<(u32, f32)>> {
    let mut sums = vec![vec![([0.0f32; 3], 0.0f32, 0usize); sectors]; rings.len()];
    for (ring, a, c, l) in face_samples(w, rings, gap_light) {
        let k = ((((a - phase).rem_euclid(std::f32::consts::TAU)) / std::f32::consts::TAU * sectors as f32) as usize)
            .min(sectors - 1);
        let s = &mut sums[ring][k];
        for i in 0..3 {
            s.0[i] += c[i] * l;
        }
        s.1 += l;
        s.2 += 1;
    }
    let paint = |(cl, l, count): ([f32; 3], f32, usize)| -> Option<(u32, f32)> {
        (count > 0 && l > 1.0e-6).then(|| {
            let c = [cl[0] / l, cl[1] / l, cl[2] / l];
            (pack([c[0].min(1.0), c[1].min(1.0), c[2].min(1.0), 1.0]), l / count as f32)
        })
    };
    sums.iter()
        .map(|row| {
            // A sector too thin to hold a cell (only ever at the very centre) takes its ring's
            // mean, and a ring with none at all the rim colour.
            let mut whole = ([0.0f32; 3], 0.0f32, 0usize);
            for s in row {
                for i in 0..3 {
                    whole.0[i] += s.0[i];
                }
                whole.1 += s.1;
                whole.2 += s.2;
            }
            let fallback = paint(whole).unwrap_or((w.rim, gap_light));
            row.iter().map(|&s| if uniform { fallback } else { paint(s).unwrap_or(fallback) }).collect()
        })
        .collect()
}

/// Folds the light into the colour and flattens each bucket into its draw call.
fn finish_level(buckets: &mut Vec<Bucket>) {
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

/// How close a tyre vertex has to be to another part of its own wheel to be held still at LOD0, m.
///
/// See `bead_locks`. Measured on the Lada, whose rim lip overlaps the tyre bead by about 8 mm: at
/// 3 mm and 5 mm the lip came out clean and the tyre's error rose from 1.1% to 1.4-1.5%; at 7 mm
/// it rose to 5%, and at 10 mm to 44%, because the ring held 864 vertices a corner against a
/// target of about 1,500 triangles and the tread was collapsed into what was left.
const BEAD_LOCK: f32 = 0.005;

/// The bead is held only when the tyre's target is at least this many times its locked vertices.
///
/// Without it five cars whose rims sit close along the whole bead (RS6, E30, EK9, R34, RAV4)
/// locked a third to a half of their tyre and lost it: the RAV4's went from 982 triangles at 0.4%
/// to 483 at 8.6%.
const BEAD_LOCK_SHARE: usize = 4;

/// For every piece of every bucket, which of its vertices LOD0's decimation must not move: the
/// tyre's vertices that lie within `BEAD_LOCK` of another part of the same wheel. Empty vectors
/// for everything else.
///
/// Where a tyre meets its rim the model has two surfaces from two parts overlapping by a few
/// millimetres, and each is decimated on its own. Collapse either edge and the overlap opens: on
/// the Lada the rim lip (r 0.652 source units) runs only about 8 mm past the tyre bead (0.631),
/// so a bead vertex pulled 8 mm inwards is a hairline of light between the two draws, and one
/// pulled outwards is a shard of tyre over the lip. The parts cannot be joined or cut (see
/// **Never split a mesh** in docs/cars.md), but the bead can be held: a locked vertex is never
/// moved and an edge between two of them never collapses, so the bead keeps the circle the rim
/// was modelled against and the rest of the tyre is decimated round it.
///
/// It is held only when the locked vertices run at least `BEAD_RING` of the way round the axle and
/// the tyre's target is at least `BEAD_LOCK_SHARE` times their number (checked where the target is
/// known, in `spend_budget_with`). On the current fleet that is the Lada, 350Z, AE86 and Golf R32.
///
/// Only the tyre is held, not the rim. The bead is a ring of a few hundred vertices on a part that
/// is mostly tread; the rim's lip is a small share of a part that is mostly spokes and is the
/// thing the rim's budget is for.
fn bead_locks(buckets: &[Bucket]) -> Vec<Vec<Vec<bool>>> {
    let mut out: Vec<Vec<Vec<bool>>> =
        buckets.iter().map(|b| vec![Vec::new(); b.pieces.len()]).collect();
    let cell = BEAD_LOCK;
    let key = |v: &Vertex| {
        (
            (v.x / cell).floor() as i32,
            (v.y / cell).floor() as i32,
            (v.z / cell).floor() as i32,
        )
    };
    let corners: Vec<u8> = {
        let mut c: Vec<u8> = buckets.iter().filter_map(|b| b.wheel).collect();
        c.sort_unstable();
        c.dedup();
        c
    };
    for corner in corners {
        // Everything on this corner that is not tyre, hashed by position. All of a wheel's parts
        // are stored about the same hub, so their coordinates compare directly.
        let mut grid: HashMap<(i32, i32, i32), Vec<[f32; 3]>> = HashMap::new();
        for b in buckets.iter().filter(|b| b.wheel == Some(corner) && b.category != Category::Tyre) {
            for p in &b.pieces {
                for v in &p.vertices {
                    grid.entry(key(v)).or_default().push([v.x, v.y, v.z]);
                }
            }
        }
        if grid.is_empty() {
            continue;
        }
        for (bi, b) in buckets.iter().enumerate() {
            if b.wheel != Some(corner) || b.category != Category::Tyre {
                continue;
            }
            for (pi, p) in b.pieces.iter().enumerate() {
                let locks: Vec<bool> = p
                    .vertices
                    .iter()
                    .map(|v| {
                        let (x, y, z) = key(v);
                        (-1..=1).any(|dx| {
                            (-1..=1).any(|dy| {
                                (-1..=1).any(|dz| {
                                    grid.get(&(x + dx, y + dy, z + dz)).is_some_and(|near| {
                                        near.iter().any(|q| {
                                            let d = [q[0] - v.x, q[1] - v.y, q[2] - v.z];
                                            d[0] * d[0] + d[1] * d[1] + d[2] * d[2] <= cell * cell
                                        })
                                    })
                                })
                            })
                        })
                    })
                    .collect();
                let coverage = ring_coverage(&p.vertices, &locks);
                if std::env::var("AZ_PARTS").is_ok() && locks.iter().any(|&l| l) {
                    eprintln!("LOCK {} of {} vertices, {:.0}% round  {}", locks.iter().filter(|&&l| l).count(), locks.len(), coverage * 100.0, p.node);
                }
                if coverage >= BEAD_RING {
                    out[bi][pi] = locks;
                }
            }
        }
    }
    out
}

/// How much of the way round the axle a bead has to run before it is held. See `ring_coverage`.
///
/// A handful of tyre vertices that happen to lie near a spoke or a valve are not a bead, and
/// pinning them does not hold a seam, it only reorders the collapses round them. On the E36, ten
/// such vertices spanning 16% of the circle put new shards of tyre over the lip. Every car whose
/// locked set ran at least half-way round (Lada 59%, 350Z 66%, AE86 50%, R32's rear) rendered alone
/// the same or cleaner; every car below it was left alone.
const BEAD_RING: f32 = 0.5;

/// The fraction of the circle round the wheel's axle that the locked vertices reach, in 64 sectors.
///
/// Wheel geometry is stored upright about its hub, so the axle is X and a vertex's angle round it
/// is its angle in the YZ plane.
fn ring_coverage(vertices: &[Vertex], locks: &[bool]) -> f32 {
    const SECTORS: usize = 64;
    let mut hit = [false; SECTORS];
    for (v, _) in vertices.iter().zip(locks).filter(|(_, &l)| l) {
        let a = v.z.atan2(v.y) / std::f32::consts::TAU + 0.5;
        hit[((a * SECTORS as f32) as usize).min(SECTORS - 1)] = true;
    }
    hit.iter().filter(|&&h| h).count() as f32 / SECTORS as f32
}

/// Decimates each bucket to a target that has already been decided.
fn spend_budget_with(
    buckets: &mut Vec<Bucket>,
    targets: &[usize],
    tile_span: f32,
    mut report: Option<&mut Report>,
) {
    let locks = bead_locks(buckets);
    for ((b, bucket_target), locks) in buckets.iter_mut().zip(targets).zip(&locks) {
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
        for ((p, target), locked) in b.pieces.iter_mut().zip(&piece_targets).zip(locks) {
            let was = p.indices.len() / 3;
            // A held bead is only affordable when it is a small part of what the tyre may keep.
            // Each locked vertex costs about two triangles to stitch to the rest, so a ring of more
            // than a quarter of the target leaves the tread to be collapsed into what is left: on
            // the RAV4 938 locked vertices took the tyre from 982 triangles at 0.4% to 483 at 8.6%.
            let held = locked.iter().filter(|&&l| l).count();
            let locked: &[bool] = if held > 0 && held * BEAD_LOCK_SHARE <= *target { locked } else { &[] };
            let error = simplify::reduce(
                &mut p.vertices,
                &mut p.attrs,
                &mut p.indices,
                *target,
                tile_span,
                locked,
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

    finish_level(buckets);
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
        assert!(atlas.packed.tiles.len() >= 2, "the test car needs several materials");

        let mut used = std::collections::HashSet::new();
        for v in car.vertices() {
            let inside = atlas.packed.tiles.iter().position(|t| {
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

    /// The M5's LOD2 body in miniature: many parts, a floor that adds up to more than the budget.
    /// The share has to stay inside the budget by leaving the least-seen parts out, and the part
    /// that matters most has to keep the most — not every part pinned at the floor.
    #[test]
    fn a_coarse_share_leaves_parts_out_rather_than_overspend() {
        let mut claims: Vec<(f64, usize)> = (0..210).map(|i| (1.0 + i as f64, 300)).collect();
        claims[17] = (120_000.0, 62_000); // the paint shell
        let shares = share_or_drop(&claims, 457, MIN_PIECE_TRIANGLES);
        assert!(shares.iter().sum::<usize>() <= 457, "spent {}", shares.iter().sum::<usize>());
        assert!(shares.iter().all(|s| *s == 0 || *s >= MIN_PIECE_TRIANGLES));
        assert_eq!(shares.iter().max(), Some(&shares[17]));
        assert!(shares[17] > 200, "the paint got {}", shares[17]);
        assert!(shares.iter().any(|s| *s == 0));

        // When every part can have the floor, none is left out.
        let few: Vec<(f64, usize)> = (0..10).map(|i| (1.0 + i as f64, 300)).collect();
        assert!(share_or_drop(&few, 457, MIN_PIECE_TRIANGLES).iter().all(|s| *s >= MIN_PIECE_TRIANGLES));
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

    /// A grid of quads, 12 wide and 10 tall, cut at column `cut` into two parts that share the
    /// column of vertices there — which is how an exporter splitting a primitive leaves it.
    fn split_strip(cut: usize, parent_b: &str) -> Bucket {
        const TALL: u32 = 10;
        let piece = |from: usize, to: usize, parent: &str| {
            let mut vertices = Vec::new();
            let mut indices = Vec::new();
            for x in from..=to {
                for y in 0..=TALL {
                    vertices.push(Vertex::new(x as f32 * 0.1, y as f32 * 0.1, 0.0, 0));
                }
            }
            let at = |x: u32, y: u32| x * (TALL + 1) + y;
            for x in 0..(to - from) as u32 {
                for y in 0..TALL {
                    let (a, b, c, d) = (at(x, y), at(x, y + 1), at(x + 1, y), at(x + 1, y + 1));
                    indices.extend_from_slice(&[a, c, b, b, c, d]);
                }
            }
            Piece {
                attrs: vec![Attr { light: 1.0, uv: [0.0; 2] }; vertices.len()],
                vertices,
                indices,
                pixels: 100,
                weight: 1.0,
                two_sided: false,
                node: "strip".into(),
                parent: parent.into(),
                material: 0,
            }
        };
        let mut a = piece(0, cut, "Body");
        let b = piece(cut, 12, parent_b);
        a.weight = 3.0;
        Bucket {
            category: Category::Body,
            wheel: None,
            pieces: vec![a, b],
            vertices: Vec::new(),
            uvs: Vec::new(),
            indices: Vec::new(),
            source_triangles: 240,
            pixels: 200,
            weight: 1.0,
            two_sided_from: 0,
        }
    }

    #[test]
    fn halves_of_one_object_are_rejoined_before_welding() {
        let mut buckets = vec![split_strip(3, "Body")];
        assert_eq!(rejoin_split_parts(&mut buckets), (2, 1));
        let p = &buckets[0].pieces[0];
        assert_eq!(buckets[0].pieces.len(), 1);
        assert_eq!(p.indices.len() / 3, 12 * 10 * 2);
        assert_eq!(p.pixels, 200);
        // Pixel-weighted: 100 px at 3.0 and 100 px at 1.0 are worth 200 px at 2.0.
        assert!((p.weight - 2.0).abs() < 1e-6);
    }

    #[test]
    fn parts_of_different_objects_are_left_apart_even_where_they_touch() {
        let mut buckets = vec![split_strip(3, "Wing")];
        assert_eq!(rejoin_split_parts(&mut buckets), (0, 0));
        assert_eq!(buckets[0].pieces.len(), 2);
    }
}
