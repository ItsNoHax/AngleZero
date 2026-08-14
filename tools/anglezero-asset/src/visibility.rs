//! Measuring how much of each part the player can actually see.
//!
//! A third of the E36 is an engine behind a closed bonnet. Another large slice is inner door
//! skins, floor pans, subframes and the backs of seats. Shared out proportionally, a triangle
//! budget spends a third of itself on the engine and produces a car that is coarse everywhere the
//! player is looking to pay for detail nobody will ever see.
//!
//! The obvious fix is a list of part names to drop, and it does not survive one car. The E36 uses
//! one material for the engine block and the window rubbers, calls its rims `felgen` and its
//! headlights `fara`, and names four of its wheels `Object_4.001` through `.004`. The next model
//! will be wrong in a different way.
//!
//! So this measures instead of guessing: the car is rendered from a ring of viewpoints into a
//! small software depth buffer, and each part is scored by how many pixels it owns. A part that
//! owns none is not visible from anywhere the player's camera goes, and can be dropped outright.
//! A part that owns few is one the budget should not be spent on. It is the same question the
//! console's own renderer will ask sixty times a second, asked once, offline.
//!
//! Windows are the wrinkle, and they are why this is two passes. Glass is see-through, so it must
//! not hide the cabin behind it — but it is also visible itself, and a windscreen scored as
//! invisible would be deleted. So opaque parts are rendered first and own the depth buffer, and
//! glass is then tested against it without writing, scoring only where it is in front.

use crate::mat::Bounds;
use crate::model::{Part, SourceModel};

/// Resolution of the depth buffer, per side. A 4.2 m car across 128 pixels is 3.3 cm a pixel,
/// which resolves a door handle and does not resolve a badge screw — about the right line, since
/// a badge screw is exactly what this is meant to find and delete.
const RESOLUTION: usize = 128;

/// Resolution of the second sweep, the one that decides what culling would cost.
///
/// Four times finer, and for a different job: the first sweep asks how much of the car a part is,
/// which a coarse buffer answers well, and this one asks whether a *particular triangle* is the
/// only thing standing between the camera and a hole. A grille is a lattice of slats a few
/// millimetres across, and at 3.3 cm a pixel it owns almost nothing — the Golf's came back with 209
/// triangles flagged out of a grille of some thousands, which drew as a hole with a few slats left
/// in it. At 8 mm a pixel the slats are resolved and the answer is about the grille rather than
/// about the sampling.
///
/// It costs a second sweep at sixteen times the pixels, which is seconds on a development machine
/// and nothing on the console, where none of this runs.
const CULL_RESOLUTION: usize = 512;

/// Views around the car: azimuths at each elevation.
///
/// The elevations are what the game's cameras actually use — the chase camera sits about 2 m up
/// and 7 m back, which is 15°, and the title screen orbits a little higher. Nothing looks at the
/// car from below, which is how the exhaust, the subframes and the floor pan come to be worth
/// nothing, and nothing looks straight down at it either.
const ELEVATIONS: [f32; 3] = [4.0, 16.0, 34.0];
const AZIMUTHS: usize = 24;

pub struct Visibility {
    /// Pixels each part owns, summed over every view.
    pub pixels: Vec<u32>,
    /// How many views were taken, for the report.
    pub views: usize,
    /// Per triangle of the whole model, whether culling it away would open a hole.
    ///
    /// This is the question back-face culling asks, asked once with the answer kept. The console
    /// culls and this sweep does not, so without it the two disagree about the same car: a grille
    /// mesh, a bumper's inner skin or a black trim panel wound away from the camera is measured
    /// here as perfectly visible and then thrown away by the hardware, and what the player sees is
    /// a hole through the car.
    ///
    /// Per triangle rather than per part, because a part is not the unit a model winds
    /// consistently. The E36 splits every panel out and its whole *right-hand side* is inward — the
    /// mirrored instances came across with their winding flipped — so a part-wide answer would have
    /// done there. The Golf merges its entire exterior into one 50,880-triangle primitive with the
    /// grille inside it, and no part-wide answer can say that the grille is a sheet and the wing
    /// beside it is not.
    needed: Vec<bool>,
    /// Per triangle, whether it is wound the wrong way round rather than genuinely two-sided.
    backwards: Vec<bool>,
    /// Where each part's triangles start in the arrays above.
    triangle_at: Vec<usize>,
}

impl Visibility {
    pub fn total(&self) -> u64 {
        self.pixels.iter().map(|p| *p as u64).sum()
    }

    pub fn hidden_parts(&self) -> usize {
        self.pixels.iter().filter(|p| **p == 0).count()
    }

    /// Which of a part's triangles have to be drawn with culling off to be drawn at all.
    ///
    /// The test is not "is this wound inwards", which no amount of winding on its own settles, but
    /// "does culling this cost the picture anything": the sweep renders each view twice, once as
    /// the console will and once with everything drawn, and a triangle qualifies where it is the
    /// nearest surface at a pixel, faces away, and the culled pass leaves that pixel to something
    /// further off or to nothing. A closed shell never qualifies, because the pixel its inside
    /// would have won is already owned by its outside at the same distance or nearer.
    pub fn two_sided_triangles(
        &self,
        part: usize,
        triangles: usize,
    ) -> impl Iterator<Item = bool> + '_ {
        let at = self.triangle_at.get(part).copied().unwrap_or(0);
        (0..triangles).map(move |i| self.needed.get(at + i).copied().unwrap_or(false))
    }

    /// Which of a part's triangles the model wound the wrong way round.
    ///
    /// The narrower half of the same question, and the distinction is what makes it safe to act
    /// on. `two_sided_triangles` says culling costs the picture here, which is true of two quite
    /// different things: a sheet the player sees from both sides, and a triangle somebody mirrored
    /// without reversing. Flipping the first moves the hole to the other side of the sheet — which
    /// is what happened when an earlier attempt flipped everything the sweep flagged and took the
    /// E39's flank with it. Flipping the second fixes it outright and costs nothing to draw.
    ///
    /// They are told apart by whether any view ever saw the triangle's *front*. A sheet is seen
    /// from both sides by definition, so it has been the nearest surface facing towards the camera
    /// at some pixel. A triangle that is only ever seen from behind, and whose removal opens a
    /// hole, is a triangle that is meant to be facing the other way.
    pub fn wound_backwards(
        &self,
        part: usize,
        triangles: usize,
    ) -> impl Iterator<Item = bool> + '_ {
        let at = self.triangle_at.get(part).copied().unwrap_or(0);
        (0..triangles).map(move |i| self.backwards.get(at + i).copied().unwrap_or(false))
    }
}

/// One camera: a direction to look from, in car space.
struct View {
    /// Unit vector from the car towards the camera.
    eye: [f32; 3],
    right: [f32; 3],
    up: [f32; 3],
}

impl View {
    fn new(azimuth_deg: f32, elevation_deg: f32) -> View {
        let (a, e) = (azimuth_deg.to_radians(), elevation_deg.to_radians());
        let eye = [
            a.sin() * e.cos(),
            e.sin(),
            a.cos() * e.cos(),
        ];
        // Any pair perpendicular to the eye will do; the world's up is never parallel to it here
        // because the elevations stop well short of vertical.
        let right = normalize(cross([0.0, 1.0, 0.0], eye));
        let up = cross(eye, right);
        View { eye, right, up }
    }

    /// Screen position in [-1, 1] and depth, which grows towards the camera.
    fn project(&self, p: [f32; 3], centre: [f32; 3], radius: f32) -> [f32; 3] {
        let d = [p[0] - centre[0], p[1] - centre[1], p[2] - centre[2]];
        [
            dot(d, self.right) / radius,
            dot(d, self.up) / radius,
            dot(d, self.eye),
        ]
    }
}

/// Renders the car from every view and counts what each part owns.
pub fn measure(model: &SourceModel, transparent: &[bool]) -> Visibility {
    let bounds = model.bounds();
    let centre = [
        (bounds.min[0] + bounds.max[0]) * 0.5,
        (bounds.min[1] + bounds.max[1]) * 0.5,
        (bounds.min[2] + bounds.max[2]) * 0.5,
    ];
    let radius = radius_of(&bounds).max(1e-3);

    // Where each part's triangles begin in one numbering across the whole model. The depth buffer
    // records which *triangle* won a pixel rather than which part, so that a sheet inside a part
    // that is otherwise a solid can still be told apart from it; the part is a lookup away.
    let mut triangle_at = Vec::with_capacity(model.parts.len() + 1);
    let mut total = 0;
    for part in &model.parts {
        triangle_at.push(total);
        total += part.triangles();
    }
    triangle_at.push(total);

    let mut pixels = vec![0u32; model.parts.len()];
    let mut depth = vec![f32::NEG_INFINITY; RESOLUTION * RESOLUTION];
    let mut owner = vec![u32::MAX; RESOLUTION * RESOLUTION];
    let mut facing = vec![false; RESOLUTION * RESOLUTION];
    let mut glass_depth = vec![f32::NEG_INFINITY; RESOLUTION * RESOLUTION];
    let mut glass_owner = vec![u32::MAX; RESOLUTION * RESOLUTION];
    let mut glass_facing = vec![false; RESOLUTION * RESOLUTION];
    let mut views = 0;

    // Which part a triangle belongs to. `triangle_at` is sorted, so this is a search rather than
    // another array the size of the model.
    let part_of = |triangle: usize| -> usize {
        match triangle_at.binary_search(&triangle) {
            Ok(exact) => {
                // Empty parts share a boundary, so land on the first one that actually holds it.
                let mut at = exact;
                while at + 1 < triangle_at.len() && triangle_at[at + 1] == triangle_at[at] {
                    at += 1;
                }
                at
            }
            Err(after) => after - 1,
        }
    };

    for elevation in ELEVATIONS {
        for i in 0..AZIMUTHS {
            let view = View::new(360.0 * i as f32 / AZIMUTHS as f32, elevation);
            views += 1;

            depth.fill(f32::NEG_INFINITY);
            owner.fill(u32::MAX);

            // Opaque first, owning the buffer.
            for (index, part) in model.parts.iter().enumerate() {
                if transparent.get(index).copied().unwrap_or(false) {
                    continue;
                }
                raster(
                    part,
                    triangle_at[index],
                    &view,
                    centre,
                    radius,
                    &mut depth,
                    &mut owner,
                    &mut facing,
                    true,
                );
            }
            for slot in owner.iter() {
                if *slot != u32::MAX {
                    pixels[part_of(*slot as usize)] += 1;
                }
            }

            // Then the glass, against a copy of that depth. It can hide other glass, so it writes
            // into the copy — but the copy is thrown away, so it never takes the cabin behind it
            // off the board.
            glass_depth.copy_from_slice(&depth);
            glass_owner.fill(u32::MAX);
            for (index, part) in model.parts.iter().enumerate() {
                if !transparent.get(index).copied().unwrap_or(false) {
                    continue;
                }
                raster(
                    part,
                    triangle_at[index],
                    &view,
                    centre,
                    radius,
                    &mut glass_depth,
                    &mut glass_owner,
                    &mut glass_facing,
                    true,
                );
            }
            for (slot, back) in glass_owner.iter().zip(&glass_facing) {
                if *slot != u32::MAX {
                    pixels[part_of(*slot as usize)] += 1;
                    // Glass is drawn two-sided already, so all this can say is that the lens or
                    // window in question is one nobody would see culled — which is true of every
                    // one of them and is why the window category carries the flag outright.
                    let _ = back;
                }
            }
        }
    }

    let mut fine = vec![0u32; model.parts.len()];
    let mut front = vec![false; total];
    let needed = what_culling_would_cost(
        model,
        transparent,
        &triangle_at,
        total,
        centre,
        radius,
        &part_of,
        &mut fine,
        &mut front,
    );
    // A triangle nobody ever saw the front of, that culling would turn into a hole, is one the
    // model wound the wrong way round. See `wound_backwards`.
    let backwards: Vec<bool> = needed
        .iter()
        .zip(&front)
        .map(|(n, f)| *n && !*f)
        .collect();

    // The coarse sweep decides how much of the budget a part is worth. It does not get to decide
    // whether the part exists.
    //
    // At 3.3 cm a pixel, a foglight behind a bumper aperture, a mesh grille and the radiator
    // behind a kidney grille all own nothing — not because nobody can see them but because the
    // sampling cannot resolve them. The E36 was dropping 98 parts and 21,294 triangles that way,
    // and what reached the screen was a bumper with gaps where its lamps and its mesh belong. The
    // finer sweep runs anyway for culling and asks the same question at 8 mm a pixel, so existence
    // is settled there while the share is still settled here.
    //
    // Rescued at the fine count scaled back into coarse units — 512 against 128 is sixteen times
    // the pixels — because it is the same measurement better sampled, not a different one. It
    // comes to a handful of pixels either way, which is the point: these are small parts and they
    // should get a small share of the budget, not nothing and not a special case.
    for (i, p) in pixels.iter_mut().enumerate() {
        if *p == 0 && fine[i] > 0 {
            *p = (fine[i] / 16).max(1);
        }
    }

    triangle_at.pop();
    Visibility {
        pixels,
        views,
        needed,
        backwards,
        triangle_at,
    }
}

/// Which triangles the hardware's back-face culling would take away, leaving a hole behind.
///
/// Each view is drawn twice into a buffer of its own: once with everything, as the sweep above
/// does, and once culled the way the console culls. A pixel whose nearest surface faces away, and
/// which the culled pass leaves to something further off or to nothing at all, is a pixel where
/// culling opens a hole — and the triangle that was closing it has to be drawn with culling off.
///
/// Drawn at [`CULL_RESOLUTION`] rather than [`RESOLUTION`] because the question is about individual
/// triangles rather than about whole parts; see the constant.
///
/// It also counts what each part owns at this resolution into `fine`, which costs one add per
/// pixel over a buffer that is being walked anyway. That is what settles whether a part small
/// enough to fall through the coarse sweep is really invisible or merely under-sampled.
#[allow(clippy::too_many_arguments)]
fn what_culling_would_cost(
    model: &SourceModel,
    transparent: &[bool],
    triangle_at: &[usize],
    total: usize,
    centre: [f32; 3],
    radius: f32,
    part_of: &impl Fn(usize) -> usize,
    fine: &mut [u32],
    front: &mut [bool],
) -> Vec<bool> {
    const N: usize = CULL_RESOLUTION * CULL_RESOLUTION;
    let mut needed = vec![false; total];
    let mut depth = vec![f32::NEG_INFINITY; N];
    let mut owner = vec![u32::MAX; N];
    let mut facing = vec![false; N];
    let mut culled_depth = vec![f32::NEG_INFINITY; N];
    let mut culled_owner = vec![u32::MAX; N];
    let mut culled_facing = vec![false; N];

    for elevation in ELEVATIONS {
        for i in 0..AZIMUTHS {
            let view = View::new(360.0 * i as f32 / AZIMUTHS as f32, elevation);
            depth.fill(f32::NEG_INFINITY);
            owner.fill(u32::MAX);
            culled_depth.fill(f32::NEG_INFINITY);
            culled_owner.fill(u32::MAX);

            for (index, part) in model.parts.iter().enumerate() {
                if transparent.get(index).copied().unwrap_or(false) {
                    continue;
                }
                for (d, o, f, cull) in [
                    (&mut depth, &mut owner, &mut facing, false),
                    (
                        &mut culled_depth,
                        &mut culled_owner,
                        &mut culled_facing,
                        true,
                    ),
                ] {
                    raster_with(
                        part,
                        triangle_at[index],
                        &view,
                        centre,
                        radius,
                        CULL_RESOLUTION,
                        d,
                        o,
                        f,
                        true,
                        cull,
                    );
                }
            }

            for at in 0..N {
                let slot = owner[at];
                if slot == u32::MAX {
                    continue;
                }
                fine[part_of(slot as usize)] += 1;
                if !facing[at] {
                    // Nearest surface here and showing its front, which is the fact that separates
                    // a sheet from a mistake. See `Visibility::wound_backwards`.
                    front[slot as usize] = true;
                    continue;
                }
                if culled_owner[at] == u32::MAX || culled_depth[at] < depth[at] {
                    needed[slot as usize] = true;
                }
            }
        }
    }
    needed
}

/// Rasterises one part into the depth buffer, owning pixels by triangle.
#[allow(clippy::too_many_arguments)]
fn raster(
    part: &Part,
    // Where this part's triangles begin in the model-wide numbering.
    first_triangle: usize,
    view: &View,
    centre: [f32; 3],
    radius: f32,
    depth: &mut [f32],
    owner: &mut [u32],
    // Whether the fragment that won each pixel was a back face.
    facing: &mut [bool],
    write_depth: bool,
) {
    raster_with(
        part,
        first_triangle,
        view,
        centre,
        radius,
        RESOLUTION,
        depth,
        owner,
        facing,
        write_depth,
        false,
    );
}

#[allow(clippy::too_many_arguments)]
fn raster_with(
    part: &Part,
    first_triangle: usize,
    view: &View,
    centre: [f32; 3],
    radius: f32,
    resolution: usize,
    depth: &mut [f32],
    owner: &mut [u32],
    facing: &mut [bool],
    write_depth: bool,
    cull_back: bool,
) {
    let half = resolution as f32 * 0.5;
    let to_pixel = |p: [f32; 3]| [(p[0] + 1.0) * half, (p[1] + 1.0) * half, p[2]];

    let projected: Vec<[f32; 3]> = part
        .positions
        .iter()
        .map(|p| to_pixel(view.project(*p, centre, radius)))
        .collect();

    for (triangle, t) in part.indices.chunks_exact(3).enumerate() {
        let a = projected[t[0] as usize];
        let b = projected[t[1] as usize];
        let c = projected[t[2] as usize];

        let min_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
        let max_x = (a[0].max(b[0]).max(c[0]).ceil() as isize).clamp(0, resolution as isize - 1)
            as usize;
        let min_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
        let max_y = (a[1].max(b[1]).max(c[1]).ceil() as isize).clamp(0, resolution as isize - 1)
            as usize;
        if min_x > max_x || min_y > max_y {
            continue;
        }

        let area = edge(a, b, c);
        if area.abs() < 1e-9 {
            continue;
        }
        // Both windings are drawn. A visibility test that culled back faces would delete any part
        // the source model happens to have wound inwards, and source models are not reliable about
        // that — the E36 declares every one of its materials two-sided.
        //
        // Which winding it was is recorded rather than acted on, because the console *does* cull
        // and something has to reconcile the two. Screen y is up here and the basis is right-handed,
        // so a front face — counter-clockwise to the viewer, which is what `sceGuFrontFace` selects
        // — has positive signed area.
        let back = area < 0.0;
        if cull_back && back {
            continue;
        }
        let inv = 1.0 / area;

        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let p = [x as f32 + 0.5, y as f32 + 0.5, 0.0];
                let w0 = edge(b, c, p) * inv;
                let w1 = edge(c, a, p) * inv;
                let w2 = edge(a, b, p) * inv;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
                let at = y * resolution + x;
                if z > depth[at] {
                    if write_depth {
                        depth[at] = z;
                    }
                    owner[at] = (first_triangle + triangle) as u32;
                    facing[at] = back;
                }
            }
        }
    }
}

/// Twice the signed area of the triangle `a b p`, which is the standard edge function.
fn edge(a: [f32; 3], b: [f32; 3], p: [f32; 3]) -> f32 {
    (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0])
}

fn radius_of(b: &Bounds) -> f32 {
    let s = b.size();
    0.5 * (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt()
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    if len > 1e-9 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [1.0, 0.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wheels::tests::box_part;

    fn model_of(parts: Vec<Part>) -> SourceModel {
        SourceModel {
            source: "test".into(),
            credit: Default::default(),
            parts,
            materials: Vec::new(),
            images: Vec::new(),
        }
    }

    /// The case this exists for: a box wholly inside another box is worth nothing, however many
    /// triangles it has. That is the engine under the bonnet.
    #[test]
    fn a_part_sealed_inside_another_is_seen_by_nobody() {
        let model = model_of(vec![
            box_part("shell", [0.0, 1.0, 0.0], [2.0, 2.0, 4.0], 4),
            box_part("engine", [0.0, 1.0, 1.0], [1.0, 1.0, 1.0], 6),
        ]);
        let v = measure(&model, &[false, false]);
        assert!(v.pixels[0] > 0, "the shell must be visible");
        assert_eq!(v.pixels[1], 0, "the engine is inside the shell");
        assert_eq!(v.hidden_parts(), 1);
    }

    /// And the converse: nothing outside is dropped for being small.
    #[test]
    fn a_small_part_on_the_outside_is_still_seen() {
        let model = model_of(vec![
            box_part("shell", [0.0, 1.0, 0.0], [2.0, 2.0, 4.0], 4),
            // A wing mirror, stuck on the side.
            box_part("mirror", [1.1, 1.4, 0.6], [0.25, 0.15, 0.3], 2),
        ]);
        let v = measure(&model, &[false, false]);
        assert!(v.pixels[1] > 0, "the mirror is on the outside of the car");
    }

    /// The underside is never looked at, which is how a floor pan comes to be worth deleting
    /// without anybody having to write down that it is a floor pan.
    ///
    /// The floor here sits under a body rather than out in the open, which is the difference
    /// between a fair test and a trivial one: a slab on its own is perfectly visible from above,
    /// and it is the body over it that makes it worthless.
    #[test]
    fn a_floor_pan_under_a_body_is_worth_far_less_than_its_roof() {
        let model = model_of(vec![
            box_part("body", [0.0, 1.15, 0.0], [1.8, 1.7, 4.0], 3),
            box_part("floor", [0.0, 0.15, 0.0], [1.7, 0.1, 3.9], 2),
        ]);
        let v = measure(&model, &[false, false]);
        assert!(
            v.pixels[0] > v.pixels[1] * 8,
            "body {} against floor {}",
            v.pixels[0],
            v.pixels[1]
        );
    }

    /// Glass must not delete the cabin behind it, and must not be deleted itself.
    #[test]
    fn what_is_behind_glass_is_still_visible_and_so_is_the_glass() {
        let parts = vec![
            box_part("seat", [0.0, 1.0, 0.0], [1.0, 1.0, 1.0], 2),
            // A pane wrapped around the seat.
            box_part("glass", [0.0, 1.0, 0.0], [1.6, 1.6, 1.6], 2),
        ];
        let model = model_of(parts);

        let opaque = measure(&model, &[false, false]);
        assert_eq!(opaque.pixels[0], 0, "an opaque box hides what is inside it");

        let glazed = measure(&model, &[false, true]);
        assert!(glazed.pixels[0] > 0, "the seat is visible through the glass");
        assert!(glazed.pixels[1] > 0, "the glass is visible too");
    }

    fn two_sided(v: &Visibility, part: usize, model: &SourceModel) -> usize {
        v.two_sided_triangles(part, model.parts[part].triangles())
            .filter(|n| *n)
            .count()
    }

    /// A solid wound the right way round costs nothing to cull, and must not be made to pay for
    /// two-sided drawing it does not need. This is the case that keeps the whole car from ending up
    /// with culling switched off.
    #[test]
    fn a_box_wound_outwards_is_culled_like_anything_else() {
        let model = model_of(vec![box_part("shell", [0.0, 1.0, 0.0], [2.0, 2.0, 4.0], 4)]);
        let v = measure(&model, &[false]);
        assert!(v.pixels[0] > 0);
        assert_eq!(two_sided(&v, 0, &model), 0);
    }

    /// And the case the measurement exists for: the same box inside out. Every triangle the camera
    /// can see is one the hardware would throw away, and the box would disappear entirely.
    ///
    /// This is not a contrived shape — it is the E36's whole right-hand side, whose mirrored parts
    /// came out of the exporter wound the other way.
    #[test]
    fn a_box_wound_inwards_has_to_be_drawn_two_sided() {
        let mut model = model_of(vec![box_part("shell", [0.0, 1.0, 0.0], [2.0, 2.0, 4.0], 4)]);
        for t in model.parts[0].indices.chunks_exact_mut(3) {
            t.swap(0, 1);
        }
        let v = measure(&model, &[false]);
        let flagged = two_sided(&v, 0, &model);
        assert!(
            flagged > 0,
            "an inside-out box would be culled away to nothing"
        );
        // Not every triangle: the underside is never looked at, so nothing about it is known and
        // nothing about it needs to be. What matters is that the sides somebody sees are covered.
        assert!(
            flagged * 4 > model.parts[0].triangles(),
            "only {flagged} of {} triangles",
            model.parts[0].triangles()
        );
    }

    /// A sheet set into a solid, wound the wrong way, is the grille: a few hundred triangles inside
    /// a part that is otherwise a perfectly ordinary shell. The answer has to be about the sheet
    /// and not about the part, or the whole bonnet loses its culling to fix a radiator grille.
    #[test]
    fn a_sheet_inside_a_solid_is_told_apart_from_it() {
        let mut shell = box_part("shell", [0.0, 1.0, 0.0], [2.0, 2.0, 4.0], 6);
        // A panel standing just clear of the nose, wound away from it.
        let mut panel = box_part("panel", [0.0, 1.0, 2.2], [1.4, 0.8, 0.02], 4);
        for t in panel.indices.chunks_exact_mut(3) {
            t.swap(0, 1);
        }
        let first = shell.positions.len() as u32;
        shell.positions.extend(panel.positions.iter().copied());
        shell.indices.extend(panel.indices.iter().map(|i| i + first));
        shell.normals.clear();
        shell.ensure_normals();

        let solid_triangles = shell.triangles() - panel.triangles();
        let model = model_of(vec![shell]);
        let v = measure(&model, &[false]);
        let flagged: Vec<bool> = v
            .two_sided_triangles(0, model.parts[0].triangles())
            .collect();
        assert!(
            flagged[..solid_triangles].iter().all(|n| !*n),
            "the shell around the panel is a solid and must stay culled"
        );
        assert!(
            flagged[solid_triangles..].iter().any(|n| *n),
            "the panel is only ever seen from behind"
        );
    }
}
