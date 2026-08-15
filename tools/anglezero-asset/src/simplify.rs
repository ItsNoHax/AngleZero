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

/// Geometric error that counts as free, in metres.
///
/// Half a millimetre, which is five times the weld grid and so barely above the noise the welder
/// has already decided is nothing.
///
/// This was 5 mm, on the argument that 5 mm is under a pixel at every distance the car is seen
/// from — 4 cm to a pixel from the chase camera — and so free on the bodywork. The arithmetic is
/// right and the conclusion does not follow, because the pass this feeds asks for two triangles and
/// takes whatever the error limit allows. What that flattens is not the *flat* geometry it was
/// meant for but the *smooth* geometry, and a car body is smooth: a door skin curves by a few
/// millimetres across its whole width, so at 5 mm the whole panel collapses, and every crease,
/// shutline and trim strip standing proud of it goes with it. The E39 is the case that showed it —
/// 124,949 welded body triangles came out at 3,137 and stayed within a few hundred of that under a
/// budget twenty-five times its allocation, which is the tell, because a part that comes in under
/// its allocation never has the allocation enforced and no weight in any config can rescue it. Its
/// grille slats, headlight surrounds and bumper valance were all inside 5 mm of the paint behind
/// them. At half a millimetre the same body compiles to 10,283 and they come back.
///
/// Nothing was lost by dropping it. Truly flat geometry — glass, floor pans, door cards — collapses
/// at an error of essentially zero, so it never needed the headroom; the headroom only ever bought
/// the collapse of things that had a shape.
const FREE_ERROR: f32 = 0.0005;
/// …but never more than this much of a part's own size, whichever is smaller.
///
/// Absolute alone was wrong in one direction and relative alone is wrong in the other, which is why
/// this is both. An absolute limit that suits a body shell is the *entire relief* of a badge: a VW
/// roundel is 11 cm across and stands about 5 mm off the grille, so a flat disc was within the free
/// error of the real thing and the cheap pass returned one. That is exactly what the Golf did — its
/// badge and the trim around it went from 3,298 triangles to 238 and the emblem disappeared, and
/// raising `chrome` did not put it back, because a part that comes in under its allocation never
/// has the allocation enforced.
///
/// A fifth of a per cent scales that down with the part: the roundel is allowed 0.22 mm and has to
/// keep its shape. Now that `FREE_ERROR` is half a millimetre this only binds below about 25 cm of
/// extent — the small trim and badges it was written for — and everything larger is held by the
/// absolute figure.
const FREE_ERROR_FRACTION: f32 = 0.002;
/// The smallest thing worth asking for when finding out what a part costs at no visual price.
const MIN_TRIANGLES: usize = 2;
/// What a unit of texture slide costs against a metre of geometry, when deciding a collapse, in
/// the **source's** UV space — a whole image across, not a whole atlas across.
///
/// This was 4.0 and meant atlas units, which quietly made the number depend on the packer. A tile
/// was an eighth of the atlas on every car, so 4.0 was really 0.5 per unit of source texture; when
/// the grid started being sized by the images a tile became a third of the atlas on the E39, the
/// same slide suddenly cost two and a half times more, and the simplifier kept a different set of
/// triangles on every car.
///
/// That is not a tuning question, it is a coupling: how much a texture may slide is a fact about
/// the picture on the part, and it must not change because the packer laid the atlas out
/// differently. `reduce` divides by the tile's span to put the question back in source units, and
/// 0.5 is the value that was in force for every car that has ever been looked at.
const UV_WEIGHT: f32 = 0.5;

/// What a vertex carries besides its position and colour, until the very end.
///
/// The light term is kept out of the colour because welding averages and decimation moves
/// vertices, and both would smear a shaded colour in ways that are hard to undo. The texture
/// coordinate is here for the opposite reason: it must survive both stages *without* being
/// smeared, and the only way to guarantee that is for both stages to know it exists.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Attr {
    pub light: f32,
    pub uv: [f32; 2],
}

/// Merges vertices that share a position, a base colour and a texture coordinate, averaging their
/// light.
///
/// The UV is part of the key, not merely carried: two vertices at the same point with different
/// UVs are a seam the model author put there deliberately, and welding them picks one side's
/// texture and stretches it across the other. Positions merge to a tenth of a millimetre; UVs to
/// a thousandth of the atlas, which is finer than one texel of any tile in it.
///
/// Returns how many vertices went away, which is worth reporting: on a model that welds badly, it
/// is the number that explains why the decimation afterwards achieved nothing.
pub fn weld(
    vertices: &mut Vec<Vertex>,
    attrs: &mut Vec<Attr>,
    indices: &mut Vec<u32>,
    tile_span: f32,
) -> usize {
    let before = vertices.len();
    let span = tile_span.max(1.0e-6);
    let mut map: HashMap<(i32, i32, i32, u32, i32, i32), u32> = HashMap::with_capacity(before);
    let mut out: Vec<Vertex> = Vec::with_capacity(before);
    let mut light_sum: Vec<f32> = Vec::with_capacity(before);
    let mut light_count: Vec<u32> = Vec::with_capacity(before);
    let mut uv: Vec<[f32; 2]> = Vec::with_capacity(before);
    let mut remap: Vec<u32> = Vec::with_capacity(before);

    for (i, v) in vertices.iter().enumerate() {
        let a = attrs[i];
        let key = (
            quantise(v.x),
            quantise(v.y),
            quantise(v.z),
            v.color,
            // Divided by the tile's span for the same reason `reduce` divides its weight: these
            // are atlas coordinates, and how far apart two texture coordinates have to be before
            // they are a seam is a fact about the source's UVs, not about the grid the packer
            // happened to choose. Left in atlas units, a bigger tile pulled the same two vertices
            // further apart, welded fewer of them, and handed the decimator a different mesh.
            (a.uv[0] / span * 1000.0) as i32,
            (a.uv[1] / span * 1000.0) as i32,
        );
        let at = *map.entry(key).or_insert_with(|| {
            out.push(*v);
            light_sum.push(0.0);
            light_count.push(0);
            uv.push(a.uv);
            (out.len() - 1) as u32
        });
        light_sum[at as usize] += a.light;
        light_count[at as usize] += 1;
        remap.push(at);
    }

    // A collapsed triangle is two of its corners becoming one vertex. They draw nothing and they
    // confuse the decimator's topology, so they go.
    //
    // So does a triangle that repeats one already kept, corner for corner and the same way round.
    // Two of those rasterise the same pixels the same colour, so nothing on screen says they are
    // there — but every edge they share is an edge with four faces on it, and a decimator will not
    // collapse across that. The E39's front bumper is 15,784 triangles of which 3,201 are a second
    // copy of a triangle already in it, and welding them without this left 6,248 non-manifold edges
    // out of 17,770: meshoptimizer's topological pass could not touch it, `Prune` then discarded the
    // whole part as unreachable components, and what reached the console was the sloppy fallback's
    // cluster soup. It looked like a shattered bumper and it was a mesh that had been drawn twice.
    //
    // Only a repeat with the same winding. The other way round is a sheet a model deliberately made
    // two-sided by backing it with itself, and dropping half of that leaves a surface that
    // disappears when you walk round it.
    let mut seen: HashMap<[u32; 3], ()> = HashMap::with_capacity(indices.len() / 3);
    let mut kept = Vec::with_capacity(indices.len());
    for t in indices.chunks_exact(3) {
        let (a, b, c) = (
            remap[t[0] as usize],
            remap[t[1] as usize],
            remap[t[2] as usize],
        );
        if a == b || b == c || a == c {
            continue;
        }
        // Rotated so the smallest corner leads, which makes the key the same for the three ways of
        // writing one triangle and different for the way round it is wound.
        let key = if a <= b && a <= c {
            [a, b, c]
        } else if b <= c {
            [b, c, a]
        } else {
            [c, a, b]
        };
        if seen.insert(key, ()).is_none() {
            kept.extend_from_slice(&[a, b, c]);
        }
    }

    *attrs = light_sum
        .iter()
        .zip(&light_count)
        .zip(&uv)
        .map(|((s, n), uv)| Attr {
            light: if *n > 0 { s / *n as f32 } else { 1.0 },
            uv: *uv,
        })
        .collect();
    *vertices = out;
    *indices = kept;
    before - vertices.len()
}

/// How big a part is, as the diagonal of its bounding box. Metres, like everything else here.
fn extent(vertices: &[Vertex]) -> f32 {
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in vertices {
        for (i, c) in [v.x, v.y, v.z].into_iter().enumerate() {
            lo[i] = lo[i].min(c);
            hi[i] = hi[i].max(c);
        }
    }
    if vertices.is_empty() {
        return 0.0;
    }
    let d = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

/// Decimates to a triangle target and compacts what is left.
///
/// Returns the error meshoptimizer reports, as a fraction of the mesh's own extent. It is the one
/// number that says whether a budget was affordable: the same target that costs a body panel 0.2%
/// costs a wheel 4%, and that difference is the whole argument for spending the budget unevenly.
pub fn reduce(
    vertices: &mut Vec<Vertex>,
    attrs: &mut Vec<Attr>,
    indices: &mut Vec<u32>,
    target_triangles: usize,
    tile_span: f32,
) -> f32 {
    if indices.len() / 3 <= target_triangles || vertices.is_empty() {
        compact(vertices, attrs, indices);
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

    // Texture coordinates, and how much a change in one costs against a change in position.
    //
    // Without this the decimator sees only geometry, and the collapse it likes best on a flat
    // panel is exactly the one that drags a decal across it: the shape is unchanged and the
    // texture slides.
    //
    // These UVs are in atlas space, because that is what the vertex has to carry out to the
    // console, and atlas space is not a fixed unit — a tile is whatever fraction of the image the
    // packer gave it. Dividing the weight by that fraction asks the question in the source's own
    // units instead, so a part's texture is allowed to slide by the same amount whatever grid it
    // ended up in. Scaling the weight is the same as scaling the attribute; meshoptimizer squares
    // the product either way.
    let uvs: Vec<f32> = attrs.iter().flat_map(|a| a.uv).collect();
    let weight = UV_WEIGHT / tile_span.max(1.0e-6);
    let weights = [weight, weight];
    let locks = vec![false; vertices.len()];
    let simplify = |target: usize, error_limit: f32, options, out: &mut f32| {
        meshopt::simplify_with_attributes_and_locks(
            indices,
            &adapter,
            &uvs,
            &weights,
            // Bytes, not floats. Passing 2 here is accepted by the release build, which reads two
            // floats out of every eight bytes of a tightly packed array and simplifies against
            // noise; only the debug assertion inside meshoptimizer says so.
            2 * core::mem::size_of::<f32>(),
            &locks,
            target,
            error_limit,
            options,
            Some(out),
        )
    };

    // First, find out what this part costs at no visual price at all.
    //
    // meshoptimizer stops at whichever binds first, the triangle target or the error limit. Asked
    // for almost nothing within an error nobody can see, it returns the cheapest mesh that is still
    // honestly the same shape. Flat glass, floor pans and door cards collapse to a fraction of
    // their allocation this way, and a budget spent on a windscreen that a quarter of the triangles
    // would have drawn identically is a budget taken from the wheels.
    //
    // Only used when it comes in under the allocation. When it does not, the allocation is what
    // gets enforced — which is why the limit below has to be right in *both* directions. A part
    // that this pass flattens is a part no weight in any config can rescue.
    let mut free_error = 0.0f32;
    let cheap = simplify(
        MIN_TRIANGLES * 3,
        FREE_ERROR.min(extent(vertices) * FREE_ERROR_FRACTION),
        meshopt::SimplifyOptions::Prune | meshopt::SimplifyOptions::ErrorAbsolute,
        &mut free_error,
    );
    if !cheap.is_empty() && cheap.len() / 3 <= target_triangles && cheap.len() < indices.len() {
        *indices = cheap;
        *indices = meshopt::optimize_vertex_cache(indices, vertices.len());
        compact(vertices, attrs, indices);
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
    let mut reduced = simplify(
        target_triangles * 3,
        1.0,
        meshopt::SimplifyOptions::Prune,
        &mut error,
    );

    // Pruning can take everything, and an empty answer is about pruning rather than about the part.
    // Components are removed whole, so a part that is a hundred small shells and no large one has
    // nothing left once the target is small enough — which is every part of every car at LOD2.
    //
    // Reading that as "the simplifier could not help" and keeping the original is the worst
    // available answer, because the original is then far enough over the target to trip the sloppy
    // pass below, which is how a bumper becomes shrapnel. Collapse alone always returns a surface,
    // so ask for one.
    if reduced.is_empty() {
        error = 0.0;
        reduced = simplify(
            target_triangles * 3,
            1.0,
            meshopt::SimplifyOptions::None,
            &mut error,
        );
    }
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
    compact(vertices, attrs, indices);
    error
}

/// Drops vertices nothing indexes any more, and renumbers what is left in first-use order.
fn compact(vertices: &mut Vec<Vertex>, attrs: &mut Vec<Attr>, indices: &mut [u32]) {
    let mut remap = vec![u32::MAX; vertices.len()];
    let mut out = Vec::with_capacity(vertices.len());
    let mut out_attrs = Vec::with_capacity(vertices.len());

    for i in indices.iter_mut() {
        let old = *i as usize;
        if remap[old] == u32::MAX {
            remap[old] = out.len() as u32;
            out.push(vertices[old]);
            // Full light rather than `Attr::default`, whose zero would be black.
            out_attrs.push(attrs.get(old).copied().unwrap_or(Attr {
                light: 1.0,
                uv: [0.0; 2],
            }));
        }
        *i = remap[old];
    }

    *vertices = out;
    *attrs = out_attrs;
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

    fn lit(light: f32) -> Attr {
        Attr { light, uv: [0.0, 0.0] }
    }

    fn attrs(n: usize) -> Vec<Attr> {
        vec![lit(1.0); n]
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
        let mut light = attrs(6);
        let mut indices = vec![0, 1, 2, 3, 4, 5];

        let dropped = weld(&mut vertices, &mut light, &mut indices, 0.125);
        assert_eq!(dropped, 2);
        assert_eq!(vertices.len(), 4);
        assert_eq!(indices.len(), 6, "both triangles survive");
    }

    /// Positions that differ only by float noise are the same position.
    #[test]
    fn near_identical_positions_weld() {
        let mut vertices = vec![v(1.0, 0.0, 0.0, 7), v(1.000_001, 0.0, 0.0, 7)];
        let mut light = attrs(2);
        let mut indices = vec![];
        weld(&mut vertices, &mut light, &mut indices, 0.125);
        assert_eq!(vertices.len(), 1);
    }

    /// Two colours meeting at a corner stay two vertices, or the boundary between them smears.
    #[test]
    fn a_colour_boundary_is_not_welded_away() {
        let mut vertices = vec![v(0.0, 0.0, 0.0, 0xFF00_0000), v(0.0, 0.0, 0.0, 0xFF00_00FF)];
        let mut light = attrs(2);
        let mut indices = vec![];
        weld(&mut vertices, &mut light, &mut indices, 0.125);
        assert_eq!(vertices.len(), 2);
    }

    /// Shading is an attribute of a vertex, not a reason to split one. Merged corners average.
    #[test]
    fn merged_vertices_average_their_light() {
        let mut vertices = vec![v(0.0, 0.0, 0.0, 3), v(0.0, 0.0, 0.0, 3)];
        let mut light = vec![lit(0.2), lit(0.8)];
        let mut indices = vec![];
        weld(&mut vertices, &mut light, &mut indices, 0.125);
        assert_eq!(vertices.len(), 1);
        assert!((light[0].light - 0.5).abs() < 1e-6, "light was {}", light[0].light);
    }

    #[test]
    fn triangles_collapsed_by_welding_are_dropped() {
        // Two of this triangle's corners are the same point.
        let mut vertices = vec![v(0.0, 0.0, 0.0, 1), v(0.0, 0.0, 0.0, 1), v(1.0, 0.0, 0.0, 1)];
        let mut light = attrs(3);
        let mut indices = vec![0, 1, 2];
        weld(&mut vertices, &mut light, &mut indices, 0.125);
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
                light.push(lit(1.0));
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

        let error = reduce(&mut vertices, &mut light, &mut indices, 200, 0.125);
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
        let mut light = attrs(3);
        let mut indices = vec![0, 1, 2];
        let error = reduce(&mut vertices, &mut light, &mut indices, 5000, 0.125);
        assert_eq!(indices.len(), 3);
        assert_eq!(vertices.len(), 3);
        assert_eq!(error, 0.0);
    }
}
