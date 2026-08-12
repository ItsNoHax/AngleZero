//! Welding and decimation.
//!
//! Welding has to come first, and it is the step whose absence is hardest to diagnose. A source
//! model splits a vertex wherever an attribute changes across it — a UV seam, a hard edge, a
//! smoothing group boundary — so the E36's 366,209 vertices are 219,966 positions, and a surface
//! that looks continuous is a mesh made of thousands of pieces that merely touch. A decimator
//! collapses edges, and there are no edges between pieces that merely touch, so run on unwelded
//! geometry it reduces almost nothing and reports success while doing it.
//!
//! What is welded on matters as much. Position alone would run a black trim strip and the paint it
//! meets into one surface, and the boundary between them would smear by a vertex when the
//! decimator moved it. Position plus the part's base colour keeps that boundary and merges
//! everything else — which is why the light term is carried alongside rather than baked in yet:
//! two vertices at the same corner with different shading are the same vertex, and their light
//! should average, not keep them apart.

use std::collections::HashMap;

use angle_zero::mesh::Vertex;

/// Positions closer together than this are the same position. A tenth of a millimetre is far below
/// anything the console can resolve on a 4 m car, and far above the float noise left by
/// transforming the same source vertex through two different node paths.
const WELD_GRID: f32 = 1.0e-4;

/// Geometric error that counts as free, in metres, and absolute rather than relative on purpose.
///
/// A fraction of each part's own size would mean two entirely different things: half a per cent of
/// a wing mirror is a third of a millimetre, and half a per cent of a whole body shell is two
/// centimetres of panel that has moved. Five millimetres is under a pixel at every distance the
/// car is ever seen from — it is 4 cm to a pixel from the chase camera and 3 cm on the title
/// screen — so it is free on the bodywork and still lets flat glass collapse to nothing.
const FREE_ERROR: f32 = 0.005;
/// The smallest thing worth asking for when finding out what a part costs at no visual price.
const MIN_TRIANGLES: usize = 2;

/// Merges vertices that share a position and a base colour, averaging their light.
///
/// Returns how many vertices went away, which is worth reporting: on a model that welds badly, it
/// is the number that explains why the decimation afterwards achieved nothing.
pub fn weld(vertices: &mut Vec<Vertex>, light: &mut Vec<f32>, indices: &mut Vec<u32>) -> usize {
    let before = vertices.len();
    let mut map: HashMap<(i32, i32, i32, u32), u32> = HashMap::with_capacity(before);
    let mut out: Vec<Vertex> = Vec::with_capacity(before);
    let mut light_sum: Vec<f32> = Vec::with_capacity(before);
    let mut light_count: Vec<u32> = Vec::with_capacity(before);
    let mut remap: Vec<u32> = Vec::with_capacity(before);

    for (i, v) in vertices.iter().enumerate() {
        let key = (
            quantise(v.x),
            quantise(v.y),
            quantise(v.z),
            v.color,
        );
        let at = *map.entry(key).or_insert_with(|| {
            out.push(*v);
            light_sum.push(0.0);
            light_count.push(0);
            (out.len() - 1) as u32
        });
        light_sum[at as usize] += light[i];
        light_count[at as usize] += 1;
        remap.push(at);
    }

    // A collapsed triangle is two of its corners becoming one vertex. They draw nothing and they
    // confuse the decimator's topology, so they go.
    let mut kept = Vec::with_capacity(indices.len());
    for t in indices.chunks_exact(3) {
        let (a, b, c) = (
            remap[t[0] as usize],
            remap[t[1] as usize],
            remap[t[2] as usize],
        );
        if a != b && b != c && a != c {
            kept.extend_from_slice(&[a, b, c]);
        }
    }

    *light = light_sum
        .iter()
        .zip(&light_count)
        .map(|(s, n)| if *n > 0 { s / *n as f32 } else { 1.0 })
        .collect();
    *vertices = out;
    *indices = kept;
    before - vertices.len()
}

/// Decimates to a triangle target and compacts what is left.
///
/// Returns the error meshoptimizer reports, as a fraction of the mesh's own extent. It is the one
/// number that says whether a budget was affordable: the same target that costs a body panel 0.2%
/// costs a wheel 4%, and that difference is the whole argument for spending the budget unevenly.
pub fn reduce(
    vertices: &mut Vec<Vertex>,
    light: &mut Vec<f32>,
    indices: &mut Vec<u32>,
    target_triangles: usize,
) -> f32 {
    if indices.len() / 3 <= target_triangles || vertices.is_empty() {
        compact(vertices, light, indices);
        return 0.0;
    }

    // meshoptimizer wants positions on their own, at a stride it can walk.
    let positions: Vec<f32> = vertices.iter().flat_map(|v| [v.x, v.y, v.z]).collect();
    let bytes = unsafe {
        std::slice::from_raw_parts(
            positions.as_ptr() as *const u8,
            std::mem::size_of_val(&positions[..]),
        )
    };
    let Ok(adapter) = meshopt::VertexDataAdapter::new(bytes, 12, 0) else {
        return 0.0;
    };

    // First, find out what this part costs at no visual price at all.
    //
    // meshoptimizer stops at whichever binds first, the triangle target or the error limit. Asked
    // for almost nothing within half a per cent of the part's own size, it returns the cheapest
    // mesh that is still honestly the same shape. Flat glass, floor pans and door cards collapse
    // to a fraction of their allocation this way, and a budget spent on a windscreen that a
    // quarter of the triangles would have drawn identically is a budget taken from the wheels.
    //
    // Only used when it comes in under the allocation. When it does not, the allocation is what
    // gets enforced.
    let mut free_error = 0.0f32;
    let cheap = meshopt::simplify(
        indices,
        &adapter,
        MIN_TRIANGLES * 3,
        FREE_ERROR,
        meshopt::SimplifyOptions::Prune | meshopt::SimplifyOptions::ErrorAbsolute,
        Some(&mut free_error),
    );
    if !cheap.is_empty() && cheap.len() / 3 <= target_triangles && cheap.len() < indices.len() {
        *indices = cheap;
        *indices = meshopt::optimize_vertex_cache(indices, vertices.len());
        compact(vertices, light, indices);
        return free_error;
    }

    let mut error = 0.0f32;
    // The error limit is deliberately wide open. The budget is the constraint being enforced here;
    // whether it was affordable is reported afterwards rather than silently obeyed, because on a
    // car the right answer to an expensive budget is often to raise it.
    //
    // `Prune` is what makes a target reachable at all on this kind of model. Collapsing edges
    // cannot take a closed shell below four faces, so a brake caliper made of two hundred bolts,
    // clips and washers has a floor of eight hundred triangles no matter what it is asked for —
    // the E36's wheel hardware came out at 1,060 against a target of 180, at 24% error, which is a
    // decimator being asked to do something it structurally cannot. Pruning drops whole small
    // components instead, which is the only reduction that works on a bag of tiny closed shells.
    let reduced = meshopt::simplify(
        indices,
        &adapter,
        target_triangles * 3,
        1.0,
        meshopt::SimplifyOptions::Prune,
        Some(&mut error),
    );
    if !reduced.is_empty() {
        *indices = reduced;
    }

    // Edge collapse is not always able to reach a target, and on a scanned car it routinely is
    // not. The E36's engine block is 80,869 triangles of hoses, fins and fasteners with a great
    // deal of non-manifold junk in it, and asked for four triangles it returns all 80,869: there
    // is no legal collapse left that does not break topology it is trying to preserve. The error
    // it reports says so — 92% of the part's own extent, having achieved nothing — but a converter
    // that only reported it would still write a car twenty times the budget.
    //
    // So when the topological simplifier cannot get there, the vertex-clustering one does. It
    // ignores topology entirely and always reaches the target, at the cost of a mesh that is no
    // longer the shape it was. That trade is obviously right here and would be obviously wrong for
    // a body panel — and it is never asked for on a body panel, because on clean geometry the
    // first simplifier reaches the target and this never runs.
    if indices.len() / 3 > target_triangles * 5 / 4 {
        let mut sloppy_error = 0.0f32;
        let sloppy = meshopt::simplify_sloppy(
            indices,
            &adapter,
            target_triangles * 3,
            1.0,
            Some(&mut sloppy_error),
        );
        if !sloppy.is_empty() && sloppy.len() < indices.len() {
            *indices = sloppy;
            error = error.max(sloppy_error);
        }
    }

    // Order the triangles for the post-transform cache before compacting, so the vertex order that
    // comes out of compaction follows the order they are first used in.
    *indices = meshopt::optimize_vertex_cache(indices, vertices.len());
    compact(vertices, light, indices);
    error
}

/// Drops vertices nothing indexes any more, and renumbers what is left in first-use order.
fn compact(vertices: &mut Vec<Vertex>, light: &mut Vec<f32>, indices: &mut [u32]) {
    let mut remap = vec![u32::MAX; vertices.len()];
    let mut out = Vec::with_capacity(vertices.len());
    let mut out_light = Vec::with_capacity(vertices.len());

    for i in indices.iter_mut() {
        let old = *i as usize;
        if remap[old] == u32::MAX {
            remap[old] = out.len() as u32;
            out.push(vertices[old]);
            out_light.push(light.get(old).copied().unwrap_or(1.0));
        }
        *i = remap[old];
    }

    *vertices = out;
    *light = out_light;
}

fn quantise(v: f32) -> i32 {
    (v / WELD_GRID).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32, color: u32) -> Vertex {
        Vertex::new(x, y, z, color)
    }

    /// The case that matters: a quad exported as two triangles with no shared vertices, which is
    /// what a UV seam or a hard edge leaves behind.
    #[test]
    fn duplicated_corners_become_one_vertex() {
        let mut vertices = vec![
            v(0.0, 0.0, 0.0, 1),
            v(1.0, 0.0, 0.0, 1),
            v(1.0, 0.0, 1.0, 1),
            // The second triangle repeats two corners of the first.
            v(0.0, 0.0, 0.0, 1),
            v(1.0, 0.0, 1.0, 1),
            v(0.0, 0.0, 1.0, 1),
        ];
        let mut light = vec![1.0; 6];
        let mut indices = vec![0, 1, 2, 3, 4, 5];

        let dropped = weld(&mut vertices, &mut light, &mut indices);
        assert_eq!(dropped, 2);
        assert_eq!(vertices.len(), 4);
        assert_eq!(indices.len(), 6, "both triangles survive");
    }

    /// Positions that differ only by float noise are the same position.
    #[test]
    fn near_identical_positions_weld() {
        let mut vertices = vec![v(1.0, 0.0, 0.0, 7), v(1.000_001, 0.0, 0.0, 7)];
        let mut light = vec![1.0, 1.0];
        let mut indices = vec![];
        weld(&mut vertices, &mut light, &mut indices);
        assert_eq!(vertices.len(), 1);
    }

    /// Two colours meeting at a corner stay two vertices, or the boundary between them smears.
    #[test]
    fn a_colour_boundary_is_not_welded_away() {
        let mut vertices = vec![v(0.0, 0.0, 0.0, 0xFF00_0000), v(0.0, 0.0, 0.0, 0xFF00_00FF)];
        let mut light = vec![1.0, 1.0];
        let mut indices = vec![];
        weld(&mut vertices, &mut light, &mut indices);
        assert_eq!(vertices.len(), 2);
    }

    /// Shading is an attribute of a vertex, not a reason to split one. Merged corners average.
    #[test]
    fn merged_vertices_average_their_light() {
        let mut vertices = vec![v(0.0, 0.0, 0.0, 3), v(0.0, 0.0, 0.0, 3)];
        let mut light = vec![0.2, 0.8];
        let mut indices = vec![];
        weld(&mut vertices, &mut light, &mut indices);
        assert_eq!(vertices.len(), 1);
        assert!((light[0] - 0.5).abs() < 1e-6, "light was {}", light[0]);
    }

    #[test]
    fn triangles_collapsed_by_welding_are_dropped() {
        // Two of this triangle's corners are the same point.
        let mut vertices = vec![v(0.0, 0.0, 0.0, 1), v(0.0, 0.0, 0.0, 1), v(1.0, 0.0, 0.0, 1)];
        let mut light = vec![1.0; 3];
        let mut indices = vec![0, 1, 2];
        weld(&mut vertices, &mut light, &mut indices);
        assert!(indices.is_empty(), "a zero-area triangle survived welding");
    }

    /// A grid fine enough to have edges to collapse, reduced hard, must come back smaller — and
    /// must come back with its vertex array compacted rather than full of orphans.
    #[test]
    fn decimation_reduces_and_compacts() {
        let n = 33;
        let mut vertices = Vec::new();
        let mut light = Vec::new();
        for z in 0..n {
            for x in 0..n {
                vertices.push(v(x as f32 * 0.1, 0.0, z as f32 * 0.1, 0xFFFF_FFFF));
                light.push(1.0);
            }
        }
        let mut indices = Vec::new();
        for z in 0..n - 1 {
            for x in 0..n - 1 {
                let i = (z * n + x) as u32;
                let row = n as u32;
                indices.extend_from_slice(&[i, i + row, i + 1, i + 1, i + row, i + row + 1]);
            }
        }
        let before = indices.len() / 3;
        assert_eq!(before, 2048);

        let error = reduce(&mut vertices, &mut light, &mut indices, 200);
        let after = indices.len() / 3;
        assert!(after < before / 2, "reduced {before} to {after}");
        assert!(error.is_finite() && error >= 0.0);
        assert_eq!(
            light.len(),
            vertices.len(),
            "light must be compacted alongside the vertices"
        );
        // Nothing may index past the compacted array.
        assert!(indices.iter().all(|i| (*i as usize) < vertices.len()));
    }

    /// Asking for more triangles than there are is not an error, and must not disturb the mesh.
    #[test]
    fn a_budget_larger_than_the_mesh_leaves_it_alone() {
        let mut vertices = vec![v(0.0, 0.0, 0.0, 1), v(1.0, 0.0, 0.0, 1), v(0.0, 0.0, 1.0, 1)];
        let mut light = vec![1.0; 3];
        let mut indices = vec![0, 1, 2];
        let error = reduce(&mut vertices, &mut light, &mut indices, 5000);
        assert_eq!(indices.len(), 3);
        assert_eq!(vertices.len(), 3);
        assert_eq!(error, 0.0);
    }
}
