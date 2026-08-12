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
    self, Category, MaterialDef, Mesh, WheelDef, HEADER_BYTES, MAGIC, MATERIAL_BLEND,
    MATERIAL_TWO_SIDED, NO_TEXTURE, NO_WHEEL, VERSION, VERTEX_COLOR_8888_F32,
};
use angle_zero::mesh::Vertex;

use crate::categorise;
use crate::config::CarConfig;
use crate::mat::Bounds;
use crate::model::SourceModel;
use crate::report::Report;
use crate::simplify;
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

pub struct Compiled {
    pub bytes: Vec<u8>,
    pub report: Report,
}

/// One source part, on its way to becoming part of a draw call.
///
/// Kept separate until decimation is done, because a budget is only meaningful per part: a part is
/// the unit that is either on the outside of the car or not.
struct Piece {
    /// Colours here are the material's base, unlit. The light term lives alongside until welding
    /// and decimation are done with it — see `simplify`.
    vertices: Vec<Vertex>,
    light: Vec<f32>,
    indices: Vec<u32>,
    pixels: u64,
    source_triangles: usize,
}

/// One output draw call: a category, optionally belonging to a wheel.
struct Bucket {
    category: Category,
    /// `None` for the body.
    wheel: Option<u8>,
    pieces: Vec<Piece>,
    /// Merged from the pieces once decimation is finished.
    vertices: Vec<Vertex>,
    indices: Vec<u32>,
    /// Source triangles that went in, before any simplification.
    source_triangles: usize,
    /// Pixels this bucket's parts own across the visibility sweep. The budget follows this.
    pixels: u64,
    weight: f32,
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
    fn flatten(&mut self) {
        for p in &self.pieces {
            let base = self.vertices.len() as u32;
            self.vertices.extend_from_slice(&p.vertices);
            self.indices.extend(p.indices.iter().map(|i| i + base));
        }
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

    let mut buckets: Vec<Bucket> = Vec::new();
    for (i, part) in model.parts.iter().enumerate() {
        if config.reduce.drop_hidden && seen.pixels[i] == 0 {
            continue;
        }
        let category = assignment.categories[i];
        let wheel = found.corner_of(i);
        // Wheel geometry is stored about its own hub, so the runtime can rotate it in place.
        let origin = wheel
            .map(|c| hubs[found.wheels.iter().position(|w| w.corner == c).unwrap()])
            .unwrap_or([0.0; 3]);

        let material = &model.materials[part.material];
        let base = base_colour(material, category);

        let slot = match buckets.iter().position(|b| b.key() == (wheel, category)) {
            Some(at) => at,
            None => {
                buckets.push(Bucket {
                    category,
                    wheel,
                    pieces: Vec::new(),
                    vertices: Vec::new(),
                    indices: Vec::new(),
                    source_triangles: 0,
                    pixels: 0,
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
        let mut piece = Piece {
            vertices: Vec::with_capacity(part.positions.len()),
            light: Vec::with_capacity(part.positions.len()),
            indices: part.indices.clone(),
            pixels: seen.pixels[i] as u64,
            source_triangles: part.triangles(),
        };
        for (v, n) in part.positions.iter().zip(&part.normals) {
            piece.vertices.push(Vertex::new(
                v[0] - origin[0],
                v[1] - origin[1],
                v[2] - origin[2],
                packed,
            ));
            piece.light.push(light_at(*n, category));
        }

        let bucket = &mut buckets[slot];
        bucket.pixels += piece.pixels;
        bucket.source_triangles += piece.source_triangles;
        bucket.pieces.push(piece);
    }

    if buckets.is_empty() {
        return Err("nothing to compile: the model has no drawable parts".into());
    }

    // Weld first, then spend the budget. Welding changes what a triangle costs, so a budget shared
    // out before it would be shared out against the wrong numbers. Per part, because that is the
    // unit a source model splits its seams within — nothing is gained by welding a bumper to the
    // wing it merely touches, and the boundary between them is better left alone.
    let mut welded_away = 0;
    for b in &mut buckets {
        for p in &mut b.pieces {
            welded_away += simplify::weld(&mut p.vertices, &mut p.light, &mut p.indices);
        }
    }
    report.note_welding(welded_away);

    // The budget is shared out twice: between the categories, and then between the parts inside
    // each one. Both steps are needed and the second is the one that matters most on a scanned
    // car. The E36's engine is 137,000 triangles of `body` behind a closed bonnet, visible as a
    // few pixels through the grille; given a share of its category's budget it takes a third of
    // the paint's detail with it, because a decimator handed a whole category has no idea which
    // half of it is on the outside.
    let bucket_targets = share_budget(
        &buckets.iter().map(|b| b.weights()).collect::<Vec<_>>(),
        budget,
        MIN_BUCKET_TRIANGLES,
    );

    for (b, bucket_target) in buckets.iter_mut().zip(&bucket_targets) {
        let piece_targets = share_budget(
            &b.pieces
                .iter()
                .map(|p| (p.pixels as f64, p.indices.len() / 3))
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
            let error = simplify::reduce(&mut p.vertices, &mut p.light, &mut p.indices, *target);

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
                p.light.clear();
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
        if stuck > 0 {
            report.note_stuck(b.category, stuck);
        }
        b.pieces.retain(|p| p.indices.len() >= 3);

        report.note_bucket(
            b.category,
            b.wheel,
            b.source_triangles,
            before,
            b.pieces.iter().map(|p| p.indices.len() / 3).sum(),
            error,
        );
    }

    // Only now is the light folded in: welding averaged it and decimation moved vertices about,
    // and both of those are the reason it was kept out of the colour until here.
    for b in &mut buckets {
        for p in &mut b.pieces {
            for (v, l) in p.vertices.iter_mut().zip(&p.light) {
                v.color = apply_light(v.color, *l);
            }
        }
        b.flatten();
    }

    // Drop anything simplification emptied, so the file has no zero-triangle draw calls in it.
    buckets.retain(|b| b.indices.len() >= 3);
    if buckets.is_empty() {
        return Err("the triangle budget left nothing to draw".into());
    }

    // One material record per category actually used.
    let mut categories: Vec<Category> = Vec::new();
    for b in &buckets {
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
    let mut indices: Vec<u16> = Vec::new();
    let mut meshes: Vec<Mesh> = Vec::new();

    for b in &buckets {
        let base = vertices.len();
        if base + b.vertices.len() > u16::MAX as usize + 1 {
            return Err(format!(
                "the compiled car needs {} vertices for {} triangles, and the format holds {}. \
                 Lower the triangle budget.",
                base + b.vertices.len(),
                (indices.len() + b.indices.len()) / 3,
                u16::MAX as usize + 1
            ));
        }
        let first_index = indices.len() as u32;
        vertices.extend_from_slice(&b.vertices);
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
            Some(c) => strings.push(&format!("{}_{}", CORNER_NAMES[c as usize], b.category.name())),
            None => strings.push(b.category.name()),
        };
        meshes.push(Mesh {
            first_index,
            index_count: (indices.len() as u32) - first_index,
            material: categories.iter().position(|c| *c == b.category).unwrap() as u16,
            wheel: b.wheel.map(u16::from).unwrap_or(NO_WHEEL),
            name,
            flags: 0,
            center: centre,
            radius,
        });
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

    // The car's own bounds, which is not the bounds of the vertex array: wheel vertices are stored
    // about their hubs, so a tyre's lowest vertex is at -0.29 in its own space and on the road in
    // the car's. Adding the hub back is what makes this the box the car actually occupies.
    let mut bounds = Bounds::EMPTY;
    for b in &buckets {
        let origin = b
            .wheel
            .and_then(|c| found.wheels.iter().position(|w| w.corner == c))
            .map(|i| hubs[i])
            .unwrap_or([0.0; 3]);
        for v in &b.vertices {
            bounds.add([v.x + origin[0], v.y + origin[1], v.z + origin[2]]);
        }
    }

    report.note_output(&vertices, &indices, &meshes, &materials, &wheel_defs, bounds);

    let bytes = write(
        &vertices,
        &indices,
        &meshes,
        &materials,
        &wheel_defs,
        &strings.bytes,
        credit_at,
        name_at,
        bounds,
    );
    report.note_size(&bytes);
    Ok(Compiled { bytes, report })
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

fn rotate_y(p: [f32; 3], yaw: f32) -> [f32; 3] {
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
        Category::Light => 0.35,
        Category::Window => 0.04,
        _ => 0.06,
    };
    let mut out = [0.0f32; 4];
    for i in 0..3 {
        out[i] = srgb(material.base_color[i]).max(floor);
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
struct Strings {
    bytes: Vec<u8>,
    seen: HashMap<String, u16>,
}

impl Strings {
    fn push(&mut self, s: &str) -> u16 {
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

/// Lays the sections out with every one of them 16-byte aligned.
fn write(
    vertices: &[Vertex],
    indices: &[u16],
    meshes: &[Mesh],
    materials: &[MaterialDef],
    wheels: &[WheelDef],
    strings: &[u8],
    credit: u32,
    name: u32,
    bounds: Bounds,
) -> Vec<u8> {
    let mut out = vec![0u8; HEADER_BYTES];

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
    let vertices_at = pad(&mut out);
    for v in vertices {
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
    pad(&mut out);

    use azcar::field as f;
    out[f::MAGIC..4].copy_from_slice(&MAGIC);
    put_u16(&mut out, f::VERSION, VERSION);
    put_u32(&mut out, f::VERTEX_FORMAT, VERTEX_COLOR_8888_F32);
    put_u32(&mut out, f::VERTEX_COUNT, vertices.len() as u32);
    put_u32(&mut out, f::INDEX_COUNT, indices.len() as u32);
    put_u16(&mut out, f::MESH_COUNT, meshes.len() as u16);
    put_u16(&mut out, f::MATERIAL_COUNT, materials.len() as u16);
    put_u16(&mut out, f::TEXTURE_COUNT, 0);
    put_u16(&mut out, f::WHEEL_COUNT, wheels.len() as u16);
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
    put_u32(&mut out, f::TEXTURES_AT, 0);
    put_u32(&mut out, f::WHEELS_AT, wheels_at as u32);
    put_u32(&mut out, f::VERTICES_AT, vertices_at as u32);
    put_u32(&mut out, f::INDICES_AT, indices_at as u32);
    put_u32(&mut out, f::STRINGS_AT, strings_at as u32);
    put_u32(&mut out, f::STRINGS_BYTES, strings.len() as u32);
    put_u32(&mut out, f::LODS_AT, 0);
    put_u32(&mut out, f::CREDIT, credit);
    put_u32(&mut out, f::NAME, name);
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

    #[test]
    fn a_compiled_car_passes_the_runtimes_own_reader() {
        let (bytes, report) = compile_test_car(10_000);
        let car = angle_zero::azcar::Car::parse(&bytes).expect("must parse");
        assert_eq!(car.triangle_count(), report.out_triangles);
        assert_eq!(car.vertex_count(), report.out_vertices);
        assert_eq!(car.wheel_count(), 4);
        assert!(car.mesh_count() >= 5, "a body and four wheels at least");
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
