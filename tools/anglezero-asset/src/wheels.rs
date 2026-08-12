//! Finding the wheels, and working out which corner each one is.
//!
//! Wheels have to leave the body mesh: they steer and they spin, and a wheel welded into the shell
//! can do neither. What makes that awkward is that a wheel is not one object. On the E36 each
//! corner is a tyre, a rim, a brake disc and a caliper — four separate nodes that must all turn
//! together — and the four corners are told apart only by where they sit.
//!
//! So identification is two questions. Which parts belong to a wheel at all, which comes from the
//! config because no naming convention survives contact with a second model; and which corner each
//! belongs to, which comes from the geometry, because that is a question the geometry answers
//! better than any name.

use crate::config::CarConfig;
use crate::mat::Bounds;
use crate::model::{Part, SourceModel};

/// The four corners in the order the format numbers them.
pub const CORNER_NAMES: [&str; 4] = ["wheel_fl", "wheel_fr", "wheel_rl", "wheel_rr"];

pub struct Wheel {
    pub corner: u8,
    /// Where the axle is, in the model's space at the time of identification.
    pub hub: [f32; 3],
    pub radius: f32,
    pub width: f32,
    /// Indices into `SourceModel::parts`.
    pub parts: Vec<usize>,
    /// The heaviest part of the assembly, which on every model seen so far is the tyre.
    pub tyre: Option<usize>,
}

#[derive(Default)]
pub struct Found {
    pub wheels: Vec<Wheel>,
    pub warnings: Vec<String>,
}

impl Found {
    /// Which wheel a part belongs to, or `None` for the body.
    pub fn corner_of(&self, part: usize) -> Option<u8> {
        self.wheels
            .iter()
            .find(|w| w.parts.contains(&part))
            .map(|w| w.corner)
    }
}

/// Cuts merged wheel geometry into one part per corner, in place.
///
/// The E36 needs none of this: its wheels are twelve separate nodes and the only question is which
/// corner each belongs to. The AE86 is the other kind of model entirely — one node called
/// `ae86-Body` with everything under it split by material rather than by object, so all four tyres
/// are a single 98,000-triangle mesh and all four rims are another. Node names cannot separate
/// what the exporter never separated.
///
/// Geometry can. A wheel is at a corner by definition, so each triangle goes to the quadrant its
/// centroid falls in, about the middle of everything the wheel patterns matched. Parts that were
/// already one wheel each land wholly in one quadrant and are left untouched, which is why this
/// can run on every model rather than only on the ones that need it.
fn split_merged_wheels(model: &mut SourceModel, matched: &[usize]) {
    let mut all = Bounds::EMPTY;
    for &i in matched {
        let b = model.parts[i].bounds();
        all.add(b.min);
        all.add(b.max);
    }
    let mid_x = (all.min[0] + all.max[0]) * 0.5;
    let mid_z = (all.min[2] + all.max[2]) * 0.5;

    let mut added: Vec<Part> = Vec::new();
    for &i in matched {
        let part = &model.parts[i];
        let mut quadrant: Vec<Vec<u32>> = vec![Vec::new(); 4];
        for t in part.indices.chunks_exact(3) {
            let c = centroid(part, t);
            let q = ((c[0] > mid_x) as usize) | (((c[2] > mid_z) as usize) << 1);
            quadrant[q].extend_from_slice(t);
        }
        if quadrant.iter().filter(|q| !q.is_empty()).count() < 2 {
            continue;
        }

        // The first non-empty quadrant keeps the part; the rest become new ones. Each is compacted
        // to only the vertices it actually uses, or four copies of a 65,000-vertex tyre mesh would
        // be carried through the whole pipeline.
        let mut pieces = quadrant.into_iter().filter(|q| !q.is_empty());
        let first = pieces.next().expect("at least two non-empty quadrants");
        let rest: Vec<Vec<u32>> = pieces.collect();
        for indices in rest {
            added.push(extract(&model.parts[i], &indices));
        }
        model.parts[i] = extract(&model.parts[i], &first);
    }
    model.parts.extend(added);
}

/// A part made of just the triangles listed, with the vertices they use and no others.
fn extract(part: &Part, indices: &[u32]) -> Part {
    let mut remap = vec![u32::MAX; part.positions.len()];
    let mut out = Part {
        node: part.node.clone(),
        parent: part.parent.clone(),
        material: part.material,
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        indices: Vec::with_capacity(indices.len()),
    };
    for &i in indices {
        let old = i as usize;
        if remap[old] == u32::MAX {
            remap[old] = out.positions.len() as u32;
            out.positions.push(part.positions[old]);
            if let Some(n) = part.normals.get(old) {
                out.normals.push(*n);
            }
            if let Some(uv) = part.uvs.get(old) {
                out.uvs.push(*uv);
            }
        }
        out.indices.push(remap[old]);
    }
    out
}

fn centroid(part: &Part, t: &[u32]) -> [f32; 3] {
    let (a, b, c) = (
        part.positions[t[0] as usize],
        part.positions[t[1] as usize],
        part.positions[t[2] as usize],
    );
    [
        (a[0] + b[0] + c[0]) / 3.0,
        (a[1] + b[1] + c[1]) / 3.0,
        (a[2] + b[2] + c[2]) / 3.0,
    ]
}

/// Splits the model's parts into four wheels and everything else.
///
/// Returns no wheels rather than wrong ones. A car with its wheels left in the body still drives
/// and still looks like itself standing still; a car with a wheel identified as the wrong corner
/// steers with one rear wheel, which is much harder to see and much worse.
pub fn identify(model: &mut SourceModel, config: &CarConfig) -> Found {
    let mut found = Found::default();

    // Config first: an explicitly named corner is never overruled by geometry.
    if let Some(corners) = config.named_corners() {
        let mut groups: Vec<Vec<usize>> = vec![Vec::new(); 4];
        for (i, part) in model.parts.iter().enumerate() {
            for (corner, fragment) in corners.iter().enumerate() {
                if matches(&part.node, fragment) || matches(&part.parent, fragment) {
                    groups[corner].push(i);
                    break;
                }
            }
        }
        for (corner, parts) in groups.iter().enumerate() {
            if parts.is_empty() {
                found.warnings.push(format!(
                    "no part matches the {} wheel's node `{}`; wheels left in the body",
                    CORNER_NAMES[corner], corners[corner]
                ));
                return Found {
                    wheels: Vec::new(),
                    warnings: found.warnings,
                };
            }
        }
        found.wheels = groups
            .into_iter()
            .enumerate()
            .map(|(corner, parts)| measure(model, corner as u8, parts))
            .collect();
        return found;
    }

    if config.wheels.patterns.is_empty() {
        found
            .warnings
            .push("no wheel nodes detected: the config names none, so the wheels cannot turn".into());
        return found;
    }

    let matched = matching(model, &config.wheels.patterns);
    if matched.is_empty() {
        found.warnings.push(format!(
            "no wheel nodes detected: nothing matches {:?}",
            config.wheels.patterns
        ));
        return found;
    }

    // A model may have all four wheels in one mesh. Cut them apart before asking which corner each
    // is, since otherwise the answer is "all of them".
    split_merged_wheels(model, &matched);
    let matched = matching(model, &config.wheels.patterns);

    // Split about the middle of what matched, not about the origin: a model can be off-centre, and
    // the wheels are the most symmetric thing on a car, so their own extent is the best axis.
    let mut all = Bounds::EMPTY;
    for &i in &matched {
        let b = model.parts[i].bounds();
        all.add(b.min);
        all.add(b.max);
    }
    let mid_x = (all.min[0] + all.max[0]) * 0.5;
    let mid_z = (all.min[2] + all.max[2]) * 0.5;

    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); 4];
    for &i in &matched {
        let c = centre(&model.parts[i].bounds());
        // The car faces +Z with +Y up, which in a right-handed frame puts +X on its left.
        let left = c[0] > mid_x;
        let front = c[2] > mid_z;
        groups[match (front, left) {
            (true, true) => 0,
            (true, false) => 1,
            (false, true) => 2,
            (false, false) => 3,
        }]
        .push(i);
    }

    if let Some(empty) = groups.iter().position(|g| g.is_empty()) {
        found.warnings.push(format!(
            "wheel parts matched {} of 4 corners — nothing landed in the {} — so the wheels are \
             left in the body",
            groups.iter().filter(|g| !g.is_empty()).count(),
            CORNER_NAMES[empty]
        ));
        return found;
    }

    found.wheels = groups
        .into_iter()
        .enumerate()
        .map(|(corner, parts)| measure(model, corner as u8, parts))
        .collect();
    found
}

/// Measures a corner: where its axle is, and how big the tyre is.
fn measure(model: &SourceModel, corner: u8, parts: Vec<usize>) -> Wheel {
    let mut all = Bounds::EMPTY;
    for &i in &parts {
        let b = model.parts[i].bounds();
        all.add(b.min);
        all.add(b.max);
    }

    // The rolling radius comes off the largest part rather than the assembly, because a caliper or
    // a bolt head sticking proud of the tread would otherwise inflate it, and a radius too large
    // makes the wheels scrub the road at a speed that does not match the car's.
    let tyre = parts.iter().copied().max_by_key(|&i| model.parts[i].triangles());
    let tyre_bounds = tyre.map(|i| model.parts[i].bounds()).unwrap_or(all);
    let s = tyre_bounds.size();
    let radius = (s[1] + s[2]) * 0.25;

    Wheel {
        corner,
        hub: centre(&all),
        radius,
        width: all.size()[0],
        parts,
        tyre,
    }
}

fn centre(b: &Bounds) -> [f32; 3] {
    [
        (b.min[0] + b.max[0]) * 0.5,
        (b.min[1] + b.max[1]) * 0.5,
        (b.min[2] + b.max[2]) * 0.5,
    ]
}

/// Every part whose node or parent name contains one of the fragments.
fn matching(model: &SourceModel, patterns: &[String]) -> Vec<usize> {
    (0..model.parts.len())
        .filter(|&i| {
            let p = &model.parts[i];
            patterns
                .iter()
                .any(|f| matches(&p.node, f) || matches(&p.parent, f))
        })
        .collect()
}

/// Case-insensitive substring match, which is what a config author means by a node fragment.
fn matches(name: &str, fragment: &str) -> bool {
    !fragment.is_empty() && name.to_ascii_lowercase().contains(&fragment.to_ascii_lowercase())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::Material;

    /// A car whose four wheels are a tyre and a rim each, at the corners of a 1.4 m by 2.6 m
    /// rectangle, plus a body part that must not be mistaken for one.
    pub(crate) fn four_wheeled_model() -> SourceModel {
        let mut parts = Vec::new();
        for (x, z, tag) in [
            (0.7f32, 1.3f32, "fl"),
            (-0.7, 1.3, "fr"),
            (0.7, -1.3, "rl"),
            (-0.7, -1.3, "rr"),
        ] {
            // The tyre: 0.6 m across, and the heavier of the two, which is how it is recognised.
            parts.push(box_part(&format!("tyre_{tag}"), [x, 0.3, z], [0.2, 0.6, 0.6], 3));
            parts.push(box_part(&format!("rim_{tag}"), [x, 0.3, z], [0.22, 0.4, 0.4], 1));
        }
        parts.push(box_part("shell", [0.0, 0.8, 0.0], [1.8, 1.2, 4.2], 12));

        SourceModel {
            source: "test".into(),
            credit: Default::default(),
            parts,
            materials: vec![Material {
                name: "paint".into(),
                base_color: [1.0; 4],
                metallic: 0.0,
                roughness: 1.0,
                emissive: [0.0; 3],
                image: None,
                double_sided: false,
                transparent: false,
            }],
            images: Vec::new(),
        }
    }

    /// A closed box, each face split into an `n` by `n` grid, so it has 12n² triangles.
    ///
    /// Real geometry rather than a stand-in: the corners are duplicated between faces exactly as a
    /// real exporter duplicates them, so welding has something to weld, and the faces are grids, so
    /// decimation has interior edges it is allowed to collapse. A part made of arbitrary triangle
    /// soup would pass through both stages untouched and prove nothing about either.
    pub(crate) fn box_part(node: &str, at: [f32; 3], size: [f32; 3], n: usize) -> Part {
        let h = [size[0] * 0.5, size[1] * 0.5, size[2] * 0.5];
        let mut positions = Vec::new();
        let mut indices: Vec<u32> = Vec::new();

        // Each face names its outward axis and the two it spans, in the order whose cross product
        // is the outward normal — so every face is wound the same way round.
        for (axis, sign, u, v) in [
            (0usize, 1.0f32, 1usize, 2usize),
            (0, -1.0, 2, 1),
            (1, 1.0, 2, 0),
            (1, -1.0, 0, 2),
            (2, 1.0, 0, 1),
            (2, -1.0, 1, 0),
        ] {
            let first = positions.len() as u32;
            for j in 0..=n {
                for i in 0..=n {
                    let mut p = at;
                    p[axis] += sign * h[axis];
                    p[u] += (i as f32 / n as f32 - 0.5) * size[u];
                    p[v] += (j as f32 / n as f32 - 0.5) * size[v];
                    positions.push(p);
                }
            }
            let row = (n + 1) as u32;
            for j in 0..n as u32 {
                for i in 0..n as u32 {
                    let a = first + j * row + i;
                    let (b, c, d) = (a + 1, a + row, a + row + 1);
                    indices.extend_from_slice(&[a, b, d, a, d, c]);
                }
            }
        }
        let mut p = Part {
            node: node.to_string(),
            parent: node.to_string(),
            material: 0,
            positions,
            normals: Vec::new(),
            uvs: Vec::new(),
            indices,
        };
        p.ensure_normals();
        p
    }

    pub(crate) fn config_matching(patterns: &[&str]) -> CarConfig {
        let mut c = CarConfig::unconfigured("Test");
        c.wheels.patterns = patterns.iter().map(|s| s.to_string()).collect();
        c
    }

    #[test]
    fn corners_are_told_apart_by_where_they_sit() {
        let mut model = four_wheeled_model();
        let found = identify(&mut model, &config_matching(&["tyre_", "rim_"]));
        assert!(found.warnings.is_empty(), "{:?}", found.warnings);
        assert_eq!(found.wheels.len(), 4);

        for w in &found.wheels {
            assert_eq!(w.parts.len(), 2, "each corner is a tyre and a rim");
            // The rolling radius is the tyre's, not the assembly's.
            assert!(
                (w.radius - 0.3).abs() < 1e-5,
                "corner {} measured {} m",
                w.corner,
                w.radius
            );
            let expect_front = w.corner == 0 || w.corner == 1;
            assert_eq!(w.hub[2] > 0.0, expect_front);
            let expect_left = w.corner == 0 || w.corner == 2;
            assert_eq!(w.hub[0] > 0.0, expect_left);
        }
    }

    /// The AE86's shape of problem: one mesh containing all four wheels, because the exporter
    /// merged everything by material. No name can separate them, so the geometry has to.
    #[test]
    fn four_wheels_merged_into_one_mesh_are_cut_apart() {
        let mut parts = vec![box_part("shell", [0.0, 0.8, 0.0], [1.8, 1.2, 4.2], 12)];

        // One part with four wheels in it, built by merging four boxes into a single mesh.
        let mut merged = box_part("wheels_all", [0.7, 0.3, 1.3], [0.2, 0.6, 0.6], 3);
        for at in [[-0.7f32, 0.3, 1.3], [0.7, 0.3, -1.3], [-0.7, 0.3, -1.3]] {
            let other = box_part("wheels_all", at, [0.2, 0.6, 0.6], 3);
            let base = merged.positions.len() as u32;
            merged.positions.extend_from_slice(&other.positions);
            merged.normals.extend_from_slice(&other.normals);
            merged
                .indices
                .extend(other.indices.iter().map(|i| i + base));
        }
        parts.push(merged);

        let mut model = four_wheeled_model();
        model.parts = parts;

        let found = identify(&mut model, &config_matching(&["wheels_all"]));
        assert!(found.warnings.is_empty(), "{:?}", found.warnings);
        assert_eq!(found.wheels.len(), 4);

        for w in &found.wheels {
            assert_eq!(w.parts.len(), 1, "one quarter of the merged mesh each");
            assert!((w.radius - 0.3).abs() < 1e-5, "corner {} is {} m", w.corner, w.radius);
            assert_eq!(w.hub[2] > 0.0, w.corner == 0 || w.corner == 1);
            assert_eq!(w.hub[0] > 0.0, w.corner == 0 || w.corner == 2);
        }

        // Every triangle survives the cut — the four quarters add up to the whole.
        let wheel_triangles: usize = found
            .wheels
            .iter()
            .flat_map(|w| w.parts.iter())
            .map(|&i| model.parts[i].triangles())
            .sum();
        assert_eq!(wheel_triangles, 4 * 12 * 3 * 3, "triangles were lost in the cut");
    }

    /// Cutting must not disturb a model whose wheels were already separate, or the E36 would be
    /// paying for the AE86's problem.
    #[test]
    fn parts_that_are_already_one_wheel_each_are_left_alone() {
        let mut model = four_wheeled_model();
        let before = model.parts.len();
        let found = identify(&mut model, &config_matching(&["tyre_", "rim_"]));
        assert_eq!(model.parts.len(), before, "parts were split that need not be");
        assert_eq!(found.wheels.len(), 4);
    }

    #[test]
    fn the_body_is_not_mistaken_for_a_wheel() {
        let mut model = four_wheeled_model();
        let found = identify(&mut model, &config_matching(&["tyre_", "rim_"]));
        let shell = model.parts.iter().position(|p| p.node == "shell").unwrap();
        assert_eq!(found.corner_of(shell), None);
    }

    /// Three corners is not a car. Better to leave every wheel in the body than to steer with a
    /// rear wheel.
    #[test]
    fn a_pattern_that_finds_three_corners_finds_none() {
        let mut model = four_wheeled_model();
        let found = identify(&mut model, &config_matching(&["tyre_fl", "tyre_fr", "tyre_rl"]));
        assert!(found.wheels.is_empty());
        assert!(found.warnings[0].contains("wheel_rr"), "{:?}", found.warnings);
    }

    #[test]
    fn a_model_with_no_wheel_config_says_so_and_carries_on() {
        let mut model = four_wheeled_model();
        let found = identify(&mut model, &CarConfig::unconfigured("Test"));
        assert!(found.wheels.is_empty());
        assert!(found.warnings[0].contains("no wheel nodes detected"));
    }

    #[test]
    fn named_corners_win_over_geometry() {
        let mut model = four_wheeled_model();
        let mut config = CarConfig::unconfigured("Test");
        // Deliberately crossed: the corner named front-left is the one at the back right.
        config.wheels.front_left = Some(crate::config::Corner { node: "_rr".into() });
        config.wheels.front_right = Some(crate::config::Corner { node: "_rl".into() });
        config.wheels.rear_left = Some(crate::config::Corner { node: "_fr".into() });
        config.wheels.rear_right = Some(crate::config::Corner { node: "_fl".into() });

        let found = identify(&mut model, &config);
        assert_eq!(found.wheels.len(), 4);
        let fl = found.wheels.iter().find(|w| w.corner == 0).unwrap();
        assert!(fl.hub[2] < 0.0, "the config said the front left is at the back");
    }
}
