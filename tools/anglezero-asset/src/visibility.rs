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
}

impl Visibility {
    pub fn total(&self) -> u64 {
        self.pixels.iter().map(|p| *p as u64).sum()
    }

    pub fn hidden_parts(&self) -> usize {
        self.pixels.iter().filter(|p| **p == 0).count()
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

    let mut pixels = vec![0u32; model.parts.len()];
    let mut depth = vec![f32::NEG_INFINITY; RESOLUTION * RESOLUTION];
    let mut owner = vec![u32::MAX; RESOLUTION * RESOLUTION];
    let mut glass_depth = vec![f32::NEG_INFINITY; RESOLUTION * RESOLUTION];
    let mut glass_owner = vec![u32::MAX; RESOLUTION * RESOLUTION];
    let mut views = 0;

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
                raster(part, index, &view, centre, radius, &mut depth, &mut owner, true);
            }
            for slot in &owner {
                if *slot != u32::MAX {
                    pixels[*slot as usize] += 1;
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
                    index,
                    &view,
                    centre,
                    radius,
                    &mut glass_depth,
                    &mut glass_owner,
                    true,
                );
            }
            for slot in &glass_owner {
                if *slot != u32::MAX {
                    pixels[*slot as usize] += 1;
                }
            }
        }
    }

    Visibility { pixels, views }
}

/// Rasterises one part into the depth buffer.
#[allow(clippy::too_many_arguments)]
fn raster(
    part: &Part,
    index: usize,
    view: &View,
    centre: [f32; 3],
    radius: f32,
    depth: &mut [f32],
    owner: &mut [u32],
    write_depth: bool,
) {
    let half = RESOLUTION as f32 * 0.5;
    let to_pixel = |p: [f32; 3]| [(p[0] + 1.0) * half, (p[1] + 1.0) * half, p[2]];

    let projected: Vec<[f32; 3]> = part
        .positions
        .iter()
        .map(|p| to_pixel(view.project(*p, centre, radius)))
        .collect();

    for t in part.indices.chunks_exact(3) {
        let a = projected[t[0] as usize];
        let b = projected[t[1] as usize];
        let c = projected[t[2] as usize];

        let min_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
        let max_x = (a[0].max(b[0]).max(c[0]).ceil() as isize).clamp(0, RESOLUTION as isize - 1)
            as usize;
        let min_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
        let max_y = (a[1].max(b[1]).max(c[1]).ceil() as isize).clamp(0, RESOLUTION as isize - 1)
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
                let at = y * RESOLUTION + x;
                if z > depth[at] {
                    if write_depth {
                        depth[at] = z;
                    }
                    owner[at] = index as u32;
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
}
