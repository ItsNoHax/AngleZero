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
    locked: &[bool],
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
    // Vertices the caller needs left where they are; see `compile::bead_locks`. Empty for none.
    let locks = if locked.len() == vertices.len() { locked.to_vec() } else { vec![false; vertices.len()] };
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

/// Decimates one part for a coarse level of detail, and says whether it had to be clustered.
///
/// The same two simplifiers as `reduce`, in the same order, changed in the three ways a level whose
/// every target is small needs. Kept apart from `reduce` so that LOD0, which is reached through
/// that, is untouched by any of it.
///
/// * **Collapse stops at about a pixel.** `reduce` gives collapse an open error limit, and with
///   `Prune` that limit is also how big a component may be removed whole. At LOD0 the targets are
///   large enough that nothing big is ever pruned; at LOD2 the AE86's paint was asked for 440
///   triangles, the error rose to a fifth of the car's length and the door skins went, which is a
///   see-through hole the size of a door. Held at `pixel` — how much of the car one pixel covers at
///   the level's own distance, 7.6 cm at LOD1 and 19 cm at LOD2 — pruning takes the badges, bolts
///   and trim nobody can see from there and nothing that anyone can.
/// * **The clustering fallback's texture is repaired.** Collapse stops short on almost every large
///   part at these targets — a body shell is hundreds of open borders (every shut line, every lamp
///   and window opening), and a border loop can shrink but never close, so the Civic EJ's paint
///   stops at 1,431 triangles when it is asked for 436. Clustering is then the only thing that gets
///   there, and it keeps corners from anywhere in a grid cell: a triangle whose corners came from
///   different islands of the atlas stretches the texture between them across its whole face. That
///   was the gold concentric stripes over the Civic's body and the tan wedges on the 720S's and
///   Golf R's windscreens. Such a triangle is found by stretching the texture far more than the
///   part's own triangles do, and its corners are given the colour the texture has under each of
///   them, baked into the vertex, and a coordinate that samples white (`FlatTexel`): it shades
///   between three true colours of the surface instead of smearing the atlas across itself. Giving
///   the whole triangle its first corner's texel instead was tried, and on a two-tone livery it is
///   a coin toss per triangle — the AE86's white flanks came out black wherever the first corner
///   landed in the black band below them. That is still what happens on an atlas with no white in
///   it to point at, which no car here has.
/// * **Clustering is asked again when it lands far short.** Its grid is coarse, and a tyre asked
///   for 51 triangles came back at 7. It is asked for 40% more each time, up to eight times, until
///   it lands within four fifths of the target; every triangle it leaves unspent on a body shell is
///   a hole it did not have to open.
///
/// `untextured` bakes every clustered triangle that way, not only the bridging ones, for a part
/// whose texture has no business being seen at all once it is this coarse: the M5's cabin is black
/// through its glass at LOD0, and clustered into a dozen triangles spanning the seats it drew the
/// red of its seat stitching a hand-span wide across the rear window.
///
/// A part that was clustered is returned as such because clustering also flips triangles, and the
/// caller draws it two-sided rather than let culling turn the flipped ones into holes.
pub fn reduce_coarse(
    vertices: &mut Vec<Vertex>,
    attrs: &mut Vec<Attr>,
    indices: &mut Vec<u32>,
    target_triangles: usize,
    tile_span: f32,
    pixel: f32,
    flat: Option<&FlatTexel>,
    untextured: bool,
) -> bool {
    if indices.len() / 3 <= target_triangles || vertices.is_empty() {
        compact(vertices, attrs, indices);
        return false;
    }
    let positions: Vec<f32> = vertices.iter().flat_map(|v| [v.x, v.y, v.z]).collect();
    let bytes = unsafe {
        std::slice::from_raw_parts(
            positions.as_ptr() as *const u8,
            std::mem::size_of_val(&positions[..]),
        )
    };
    let Ok(adapter) = meshopt::VertexDataAdapter::new(bytes, 12, 0) else {
        return false;
    };
    // The same texture weighting as `reduce`, for the same reason.
    let uvs: Vec<f32> = attrs.iter().flat_map(|a| a.uv).collect();
    let weight = UV_WEIGHT / tile_span.max(1.0e-6);
    let weights = [weight, weight];
    let locks = vec![false; vertices.len()];
    let simplify = |idx: &[u32], limit: f32, options: meshopt::SimplifyOptions| {
        meshopt::simplify_with_attributes_and_locks(
            idx,
            &adapter,
            &uvs,
            &weights,
            2 * core::mem::size_of::<f32>(),
            &locks,
            target_triangles * 3,
            limit,
            options | meshopt::SimplifyOptions::ErrorAbsolute,
            None,
        )
    };
    let mut reduced = simplify(indices, pixel, meshopt::SimplifyOptions::Prune);
    // See `reduce`: pruning can take everything, and collapse alone always leaves a surface.
    if reduced.is_empty() {
        reduced = simplify(indices, pixel, meshopt::SimplifyOptions::None);
    }
    if !reduced.is_empty() {
        *indices = reduced;
    }
    let mut clustered = false;
    if indices.len() / 3 > target_triangles * 5 / 4 {
        // How far the part's own triangles stretch the texture, before clustering changes them.
        // The median, so a few slivers in the source do not set the bar.
        let mut before: Vec<f32> =
            indices.chunks_exact(3).map(|t| uv_stretch(vertices, attrs, t)).collect();
        before.sort_by(|a, b| a.total_cmp(b));
        let typical = before[before.len() / 2];
        // What colour the texture is around each vertex, measured before clustering throws the
        // triangles away: the area-weighted mean of the texel at the middle of every triangle
        // using it. Not the texel under the vertex itself, because a vertex sits on the edge of
        // its island more often than not, and the edge is where the stitching, the pinstripe and
        // the gutter are: the M5's black cabin, sampled at its vertices, came out red.
        let around = flat.map(|flat| {
            let mut sum = vec![[0.0f32; 4]; vertices.len()];
            for t in indices.chunks_exact(3) {
                let (a, b, c) = (&vertices[t[0] as usize], &vertices[t[1] as usize], &vertices[t[2] as usize]);
                let e1 = [b.x - a.x, b.y - a.y, b.z - a.z];
                let e2 = [c.x - a.x, c.y - a.y, c.z - a.z];
                let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                let area = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1.0e-9);
                let uv = t.iter().fold([0.0f32; 2], |m, &i| {
                    let u = attrs[i as usize].uv;
                    [m[0] + u[0] / 3.0, m[1] + u[1] / 3.0]
                });
                let texel = flat.texel(uv);
                for &i in t {
                    let s = &mut sum[i as usize];
                    for k in 0..3 {
                        s[k] += texel[k] * area;
                    }
                    s[3] += area;
                }
            }
            sum
        });

        let mut ask = target_triangles;
        let mut best: Vec<u32> = Vec::new();
        // The smallest answer seen, for a part clustering cannot bring within reach of its target
        // at all. Used only if nothing better turns up, and still subject to the caller's rule for
        // parts that will not come down.
        let mut smallest: Vec<u32> = Vec::new();
        for _ in 0..8 {
            let sloppy = meshopt::simplify_sloppy(indices, &adapter, ask * 3, 1.0, None);
            if !sloppy.is_empty() && (smallest.is_empty() || sloppy.len() < smallest.len()) {
                smallest = sloppy.clone();
            }
            if sloppy.len() / 3 <= target_triangles && sloppy.len() > best.len() {
                best = sloppy;
            }
            if best.len() / 3 * 5 >= target_triangles * 4 {
                break;
            }
            ask = (ask * 7 / 5).max(ask + 1);
        }
        if best.is_empty() {
            best = smallest;
        }
        if !best.is_empty() && best.len() < indices.len() {
            *indices = best;
            clustered = true;
        }

        if clustered {
            // A texture coordinate can differ between islands by a whole tile, so anything more
            // than a few times the part's usual stretch is a triangle bridging islands, not one
            // following a curve.
            let limit = (typical * 4.0).max(1.0e-3);
            let bridging: Vec<bool> = indices
                .chunks_exact(3)
                .map(|t| (untextured && flat.is_some()) || uv_stretch(vertices, attrs, t) > limit)
                .collect();
            let mut out: Vec<u32> = Vec::with_capacity(indices.len());
            let mut copies: HashMap<(u32, u32, u32), u32> = HashMap::new();
            for (t, bridging) in indices.chunks_exact(3).zip(bridging) {
                if !bridging {
                    out.extend_from_slice(t);
                    continue;
                }
                match flat {
                    // Each corner keeps the colour the texture had under it, baked into the
                    // vertex, and samples white: the triangle shades between three true colours
                    // of the surface instead of stretching the atlas between them. One copy per
                    // corner, shared by every bridging triangle that uses it.
                    Some(flat) => {
                        for &i in t {
                            let at = *copies.entry((i, 0, 0)).or_insert_with(|| {
                                let a = attrs[i as usize];
                                let mut v = vertices[i as usize];
                                let texel = match &around {
                                    Some(sum) if sum[i as usize][3] > 0.0 => {
                                        let s = sum[i as usize];
                                        [s[0] / s[3], s[1] / s[3], s[2] / s[3]]
                                    }
                                    _ => flat.texel(a.uv),
                                };
                                v.color = FlatTexel::bake(v.color, texel);
                                vertices.push(v);
                                attrs.push(Attr { light: a.light, uv: flat.white });
                                (vertices.len() - 1) as u32
                            });
                            out.push(at);
                        }
                    }
                    // No white texel to point at: the whole triangle takes its first corner's
                    // coordinate, and draws that one colour.
                    None => {
                        let uv = attrs[t[0] as usize].uv;
                        out.push(t[0]);
                        for &i in &t[1..] {
                            let key = (i, uv[0].to_bits(), uv[1].to_bits());
                            let at = *copies.entry(key).or_insert_with(|| {
                                vertices.push(vertices[i as usize]);
                                attrs.push(Attr { light: attrs[i as usize].light, uv });
                                (vertices.len() - 1) as u32
                            });
                            out.push(at);
                        }
                    }
                }
            }
            *indices = out;
        }
    }
    *indices = meshopt::optimize_vertex_cache(indices, vertices.len());
    compact(vertices, attrs, indices);
    clustered
}

/// The atlas, and a coordinate in it that samples pure white, for baking a texture into vertex
/// colours: a vertex that carries the texel's colour and samples white draws exactly what it did.
pub struct FlatTexel<'a> {
    pub pixels: &'a [u8],
    pub size: usize,
    pub white: [f32; 2],
}

impl FlatTexel<'_> {
    /// The texel under `uv`, nearest, as linear 0–1 RGB.
    fn texel(&self, uv: [f32; 2]) -> [f32; 3] {
        let x = ((uv[0] * self.size as f32) as usize).min(self.size - 1);
        let y = ((uv[1] * self.size as f32) as usize).min(self.size - 1);
        let p = &self.pixels[(y * self.size + x) * 4..(y * self.size + x) * 4 + 3];
        [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0]
    }

    /// A packed colour multiplied by a texel, as the console's `Modulate` would have multiplied
    /// it. Alpha is left alone.
    fn bake(colour: u32, texel: [f32; 3]) -> u32 {
        let mut out = colour & 0xFF00_0000;
        for (k, t) in texel.iter().enumerate() {
            let c = ((colour >> (8 * k)) & 0xFF) as f32;
            out |= ((c * t).round().clamp(0.0, 255.0) as u32) << (8 * k);
        }
        out
    }
}

/// The most any edge of a triangle stretches its texture, as texture length per metre.
fn uv_stretch(vertices: &[Vertex], attrs: &[Attr], t: &[u32]) -> f32 {
    let mut worst = 0.0f32;
    for k in 0..3 {
        let (a, b) = (t[k] as usize, t[(k + 1) % 3] as usize);
        let (va, vb) = (&vertices[a], &vertices[b]);
        let len = ((va.x - vb.x).powi(2) + (va.y - vb.y).powi(2) + (va.z - vb.z).powi(2)).sqrt();
        let (ua, ub) = (attrs[a].uv, attrs[b].uv);
        let uv_len = ((ua[0] - ub[0]).powi(2) + (ua[1] - ub[1]).powi(2)).sqrt();
        worst = worst.max(uv_len / len.max(1.0e-4));
    }
    worst
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

pub(crate) fn quantise(v: f32) -> i32 {
    (v / WELD_GRID).round() as i32
}

/// How far apart two points may be and still be the same point, when the only thing being kept is
/// an outline.
///
/// Five millimetres, against `WELD_GRID`'s tenth of one. That would be far too coarse for a car —
/// it would run a trim strip into the paint beside it — and is exactly right for a silhouette,
/// where there is no trim, no paint, no texture and no normal to smear. All that survives is where
/// the edge of the car is, and nothing about a car's edge is decided at half a centimetre.
const SHELL_WELD_GRID: f32 = 0.005;

/// Reduces a positions-only shell to a triangle target, as **one mesh**.
///
/// This exists because of what a silhouette looked like when it did not. The pipeline decimates a
/// car piece by piece — a bumper, a wing, a door — which is right for a car, where each piece has
/// its own material and its own texture and the seams between them are real. Ask that machinery for
/// six hundred triangles and each piece gets a handful, every piece shrinks away from its
/// neighbours, and the car arrives covered in cracks with slivers hanging off it.
///
/// A silhouette has no pieces. It is one flat shape, so the parts can be welded into one another
/// on position alone and simplified as a single surface, and then a collapse is free to run a wing
/// into the door behind it — which is precisely the collapse that keeps an outline whole. It is
/// also smaller: the seams stop carrying two copies of every vertex.
///
/// Note the direction of travel. Nothing here splits a mesh in order to decimate it — that cracks
/// bodywork and is the one thing this pipeline must never do. This is the opposite: it merges
/// meshes in order to decimate them together.
pub fn reduce_shell(positions: &mut Vec<[f32; 3]>, indices: &mut Vec<u32>, target_triangles: usize) {
    // Weld first, or there is nothing to collapse across: the parts arrive as separate arrays whose
    // seams merely touch, and a decimator cannot see that two coincident vertices are one point.
    let mut seen: HashMap<[i32; 3], u32> = HashMap::new();
    let mut welded = Vec::with_capacity(positions.len());
    let mut remap = Vec::with_capacity(positions.len());
    for p in positions.iter() {
        let key = [
            (p[0] / SHELL_WELD_GRID).round() as i32,
            (p[1] / SHELL_WELD_GRID).round() as i32,
            (p[2] / SHELL_WELD_GRID).round() as i32,
        ];
        let at = *seen.entry(key).or_insert_with(|| {
            welded.push(*p);
            (welded.len() - 1) as u32
        });
        remap.push(at);
    }
    for i in indices.iter_mut() {
        *i = remap[*i as usize];
    }
    // A collapsed triangle is two of its corners having become one vertex. They draw nothing and
    // the decimator counts them against the target, so they go now.
    indices.retain_triangles();
    *positions = welded;
    if positions.is_empty() {
        return;
    }

    // Throw away everything that is behind something else from every direction, before anything
    // is simplified. See `visibility::outward_triangles` for why that is free in a silhouette.
    //
    // This is what makes the silhouette independent of how a car's parts are categorised: the
    // shell is built from every part, cabin and glass and trim included, and the ones that cannot
    // be seen cost nothing because they are no longer there. It also matters to the simplifier
    // below, which measures every collapse against the planes of the surfaces around it: an inner
    // sill skin two centimetres inside the outer one is a plane that says the sill may as well be
    // two centimetres further in.
    let outward = |t: &[u32]| -> Vec<u32> {
        let seen = crate::visibility::outward_triangles(positions, t);
        t.chunks_exact(3)
            .zip(&seen)
            .filter(|(_, s)| **s)
            .flat_map(|(t, _)| t.iter().copied())
            .collect()
    };
    let source = outward(indices);

    // Edge collapse, and not the vertex clustering this used to be.
    //
    // Clustering (`meshopt::simplify_sloppy`) snaps the shell to a uniform grid. It was chosen over
    // meshoptimizer's own edge collapse because that one left the E39 at 600 triangles as a smooth
    // wedge with no boot — but that shell was mostly hidden surfaces, and meshoptimizer's collapse
    // will not move a vertex whose topology is complicated, which on a shell welded out of dozens
    // of overlapping parts is most of them. What a uniform grid costs is the car's detail. At a
    // thousand triangles the cells come out at about 25 cm, because a car has some 30 m² of
    // surface and a grid spends the same on a flat roof as on the edge of a wing, so anything
    // thinner than a cell went: the NSX's, R34's and 720S's wings, every splitter, and the lower
    // edge of every sill, which stood a cell too high the whole length of the car. The S14's wing
    // was snapped into the same cells as its boot lid and came out as a slab joining the two.
    //
    // `collapse` below spends where the shape is. It takes the cheapest edge first by how far the
    // move takes the surface off the planes it started on, so a roof panel goes to a few
    // triangles and a wing, whose blade is metres of plane standing off the body, is one of the
    // last things touched. With the hidden surfaces already gone, the E39 keeps its boot at this
    // budget, and it runs across seams that meshoptimizer would refuse.
    //
    // Pruning again afterwards can find a little more that is now covered, which would leave the
    // budget part spent; one more pass asks for correspondingly more.
    let mut asked = target_triangles;
    *indices = source.clone();
    for _ in 0..3 {
        if source.len() / 3 <= target_triangles {
            break;
        }
        let kept = outward(&collapse(positions, &source, asked));
        let n = kept.len() / 3;
        *indices = kept;
        if n * 100 >= target_triangles * 97 || n == 0 {
            break;
        }
        asked = asked * target_triangles / n;
    }

    // Drop whatever is no longer indexed, so the section carries no vertices nothing draws.
    let mut remap = vec![u32::MAX; positions.len()];
    let mut out = Vec::with_capacity(positions.len());
    for i in indices.iter_mut() {
        let old = *i as usize;
        if remap[old] == u32::MAX {
            remap[old] = out.len() as u32;
            out.push(positions[old]);
        }
        *i = remap[old];
    }
    *positions = out;
}

/// How sharply the surface must turn at an edge for `collapse` to hold it like an open edge: the
/// cosine of the angle between the two faces' normals, so 0.2 is about 78°.
///
/// Set against the golden views over the fleet, which is where it was chosen from rather than
/// reasoned to. Holding every fold past 60° spends the budget on a body's ordinary creases and the
/// S14's sills went again; only past 120° misses the square rims most thin parts have. Past 78° is
/// the rim of a wing, a splitter or a fin and very little else.
const FOLD_COS: f64 = 0.2;

/// A fold's weight against an open edge's, which is 1. A third, because a fold has a face on
/// either side of it already resisting the move, where an open edge has only one; at the full
/// weight the fleet's worst views were a little worse, not better.
const FOLD_WEIGHT: f64 = 0.3;

/// Simplifies a shell to a triangle target by half-edge collapse under plane quadrics, with no
/// regard for topology.
///
/// The textbook method (Garland and Heckbert), in the simplest form that serves an outline:
///
/// * **Every vertex remembers the planes of the triangles it started on**, weighted by area, and a
///   collapse costs the summed squared distance of where the vertex ends up from all of them. Flat
///   panels are free to simplify; anything that sticks out is not.
/// * **Half-edge collapse.** One end of the edge moves onto the other, so every vertex that
///   survives is a vertex of the source. Nothing is placed at an optimum between them, which on a
///   curved surface lands outside it and makes the silhouette bigger than the car.
/// * **Open edges are held.** Where a triangle has no neighbour — the rim of a panel, the edge
///   of a wing blade, and everywhere the pruning above took a hidden surface away — a plane at
///   right angles to the triangle through that edge is added, weighted by the edge's length
///   squared. Without it a free edge costs nothing to pull back along its own panel, and every
///   one of them retreats. Heavier than this (tried at five times) and the budget goes on rims
///   the car is covered in; lighter (a fifth) and the sills and wings start to go again.
/// * **Folds are held too, more lightly.** An edge where the surface turns through more than
///   about 78° gets the same plane from each of its two faces, at [`FOLD_WEIGHT`]. This is for
///   thin parts. A wing blade is two sheets a centimetre apart joined round a narrow rim, and
///   sliding a vertex along the blade moves it off none of the blade's planes: all that resists
///   it is the rim, whose faces have almost no area. So the blade could be drawn in from its
///   tips at no cost, and the 720S's was, half of it at a time. The rim of a thin part is a fold,
///   and holding folds is what keeps it.
/// * **No topology is kept.** Non-manifold edges, parts that merely overlap and edges shared by
///   three triangles are all collapsed like any other, which is the point of this function
///   rather than meshoptimizer's: its collapse locks every vertex it cannot classify.
/// * **Nothing may turn over.** A collapse that would flip a surviving triangle more than about
///   80° is refused. A silhouette is unculled, so a flipped triangle is not a lighting fault but
///   a triangle that now covers somewhere else.
///
/// Keeping topology would be the right call for a car's own surfaces, and is not here for the
/// same reason `reduce_shell` merges its parts: a silhouette is one flat colour, and a collapse
/// that runs a bumper into the wing beside it is exactly the collapse that keeps the outline
/// whole.
fn collapse(positions: &[[f32; 3]], indices: &[u32], target_triangles: usize) -> Vec<u32> {
    use std::cmp::Ordering;
    use std::collections::BinaryHeap;

    let n = positions.len();
    let pos: Vec<[f64; 3]> = positions
        .iter()
        .map(|p| [p[0] as f64, p[1] as f64, p[2] as f64])
        .collect();
    let mut tris: Vec<[u32; 3]> = indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    let mut tri_alive = vec![true; tris.len()];
    let mut alive = tris.len();
    let mut quadric = vec![Quadric::default(); n];
    // The triangles each vertex is in. Grows as collapses hand one vertex's triangles to another;
    // dead entries are skipped rather than removed, and swept out now and then.
    let mut around: Vec<Vec<u32>> = vec![Vec::new(); n];

    let normal = |t: [u32; 3]| cross3(sub3(pos[t[1] as usize], pos[t[0] as usize]), sub3(pos[t[2] as usize], pos[t[0] as usize]));

    // How many triangles use each edge, and one of them.
    let mut edges: HashMap<(u32, u32), (u32, u32, u32)> = HashMap::new();
    for (ti, t) in tris.iter().enumerate() {
        for v in t {
            around[*v as usize].push(ti as u32);
        }
        for (u, v) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let e = edges.entry((u.min(v), u.max(v))).or_insert((0, ti as u32, u32::MAX));
            e.0 += 1;
            if e.0 == 2 {
                e.2 = ti as u32;
            }
        }
        let nn = normal(*t);
        let twice_area = len3(nn);
        if twice_area < 1e-15 {
            continue;
        }
        let unit = scale3(nn, 1.0 / twice_area);
        for v in t {
            quadric[*v as usize].add_plane(unit, pos[t[0] as usize], 0.5 * twice_area);
        }
    }
    // In a fixed order: these sums are floating point, and a car must compile to the same bytes
    // every time.
    let mut keys: Vec<(u32, u32)> = edges.keys().copied().collect();
    keys.sort_unstable();
    for (u, v) in &keys {
        let (uses, t1, t2) = edges[&(*u, *v)];
        let faces: Vec<(u32, f64)> = if uses == 1 {
            vec![(t1, 1.0)]
        } else if uses == 2 {
            let (a, b) = (normal(tris[t1 as usize]), normal(tris[t2 as usize]));
            if dot3(a, b) < FOLD_COS * len3(a) * len3(b) {
                vec![(t1, FOLD_WEIGHT), (t2, FOLD_WEIGHT)]
            } else {
                continue;
            }
        } else {
            continue;
        };
        let (pu, pv) = (pos[*u as usize], pos[*v as usize]);
        let along = sub3(pv, pu);
        for (ti, w) in faces {
            let across = cross3(along, normal(tris[ti as usize]));
            let l = len3(across);
            if l < 1e-15 {
                continue;
            }
            let unit = scale3(across, 1.0 / l);
            let weight = w * dot3(along, along);
            for i in [*u, *v] {
                quadric[i as usize].add_plane(unit, pu, weight);
            }
        }
    }

    // Candidates, cheapest first, each stamped with its two ends' versions so that one made stale
    // by a later collapse is recognised and dropped when it comes up.
    struct Candidate {
        cost: f64,
        from: u32,
        to: u32,
        stamps: (u32, u32),
    }
    impl PartialEq for Candidate {
        fn eq(&self, o: &Self) -> bool {
            self.cost == o.cost
        }
    }
    impl Eq for Candidate {}
    impl PartialOrd for Candidate {
        fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
            Some(self.cmp(o))
        }
    }
    impl Ord for Candidate {
        fn cmp(&self, o: &Self) -> Ordering {
            // Reversed, so the max-heap pops the cheapest; ties on the vertex, so the order is the
            // same on every machine and a car compiles to the same bytes twice.
            o.cost
                .partial_cmp(&self.cost)
                .unwrap_or(Ordering::Equal)
                .then(o.from.cmp(&self.from))
                .then(o.to.cmp(&self.to))
        }
    }
    let mut stamp = vec![0u32; n];
    let candidate = |q: &[Quadric], stamp: &[u32], a: u32, b: u32| {
        let mut both = q[a as usize];
        both.add(&q[b as usize]);
        let (onto_b, onto_a) = (both.eval(pos[b as usize]), both.eval(pos[a as usize]));
        let (cost, from, to) = if onto_b <= onto_a { (onto_b, a, b) } else { (onto_a, b, a) };
        Candidate { cost, from, to, stamps: (stamp[from as usize], stamp[to as usize]) }
    };
    let mut heap: BinaryHeap<Candidate> =
        keys.iter().map(|(a, b)| candidate(&quadric, &stamp, *a, *b)).collect();
    let mut vertex_alive = vec![true; n];

    while alive > target_triangles {
        let Some(c) = heap.pop() else {
            break;
        };
        let (u, v) = (c.from, c.to);
        if !vertex_alive[u as usize]
            || !vertex_alive[v as usize]
            || c.stamps != (stamp[u as usize], stamp[v as usize])
        {
            continue;
        }

        let turns_over = around[u as usize].iter().any(|&ti| {
            let t = tris[ti as usize];
            if !tri_alive[ti as usize] || t.contains(&v) {
                return false;
            }
            let before = normal(t);
            let after = normal(t.map(|i| if i == u { v } else { i }));
            dot3(before, after) < 0.2 * len3(before) * len3(after)
        });
        if turns_over {
            continue;
        }

        for ti in std::mem::take(&mut around[u as usize]) {
            if !tri_alive[ti as usize] {
                continue;
            }
            let t = &mut tris[ti as usize];
            if t.contains(&v) {
                tri_alive[ti as usize] = false;
                alive -= 1;
                continue;
            }
            for i in t.iter_mut() {
                if *i == u {
                    *i = v;
                }
            }
            around[v as usize].push(ti);
        }
        vertex_alive[u as usize] = false;
        let moved = quadric[u as usize];
        quadric[v as usize].add(&moved);
        stamp[v as usize] += 1;
        around[v as usize].retain(|t| tri_alive[*t as usize]);

        let mut neighbours: Vec<u32> =
            around[v as usize].iter().flat_map(|t| tris[*t as usize]).filter(|w| *w != v).collect();
        neighbours.sort_unstable();
        neighbours.dedup();
        for w in neighbours {
            heap.push(candidate(&quadric, &stamp, v, w));
        }
    }

    let mut out = Vec::with_capacity(alive * 3);
    for (t, a) in tris.iter().zip(&tri_alive) {
        if *a {
            out.extend_from_slice(t);
        }
    }
    out
}

/// The summed, weighted squared distance to a set of planes, as the symmetric 4×4 matrix `K` for
/// which it is `[p 1]·K·[p 1]`. Only the upper triangle is stored.
#[derive(Clone, Copy, Default)]
struct Quadric {
    k: [f64; 10],
}

impl Quadric {
    fn add_plane(&mut self, normal: [f64; 3], through: [f64; 3], weight: f64) {
        let v = [normal[0], normal[1], normal[2], -dot3(normal, through)];
        let mut at = 0;
        for i in 0..4 {
            for j in i..4 {
                self.k[at] += weight * v[i] * v[j];
                at += 1;
            }
        }
    }

    fn add(&mut self, other: &Quadric) {
        for (a, b) in self.k.iter_mut().zip(&other.k) {
            *a += b;
        }
    }

    fn eval(&self, p: [f64; 3]) -> f64 {
        let v = [p[0], p[1], p[2], 1.0];
        let mut at = 0;
        let mut sum = 0.0;
        for i in 0..4 {
            for j in i..4 {
                let twice = if i == j { 1.0 } else { 2.0 };
                sum += twice * self.k[at] * v[i] * v[j];
                at += 1;
            }
        }
        sum
    }
}

fn sub3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross3(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn len3(a: [f64; 3]) -> f64 {
    dot3(a, a).sqrt()
}

fn scale3(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// Drops triangles that have collapsed to a line or a point, which welding leaves behind.
trait RetainTriangles {
    fn retain_triangles(&mut self);
}

impl RetainTriangles for Vec<u32> {
    fn retain_triangles(&mut self) {
        let mut out = Vec::with_capacity(self.len());
        for t in self.chunks_exact(3) {
            if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                out.extend_from_slice(t);
            }
        }
        *self = out;
    }
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

        let error = reduce(&mut vertices, &mut light, &mut indices, 200, 0.125, &[]);
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
        let error = reduce(&mut vertices, &mut light, &mut indices, 5000, 0.125, &[]);
        assert_eq!(indices.len(), 3);
        assert_eq!(vertices.len(), 3);
        assert_eq!(error, 0.0);
    }

    /// A box of `n`×`n` quads a face, centred on the origin, as positions and indices.
    fn gridded_box(half: f32, n: usize, positions: &mut Vec<[f32; 3]>, indices: &mut Vec<u32>) {
        for axis in 0..3 {
            for side in [-1.0f32, 1.0] {
                let base = positions.len() as u32;
                for i in 0..=n {
                    for j in 0..=n {
                        let a = -half + 2.0 * half * i as f32 / n as f32;
                        let b = -half + 2.0 * half * j as f32 / n as f32;
                        let mut p = [0.0; 3];
                        p[axis] = side * half;
                        p[(axis + 1) % 3] = a;
                        p[(axis + 2) % 3] = b;
                        positions.push(p);
                    }
                }
                let row = n as u32 + 1;
                for i in 0..n as u32 {
                    for j in 0..n as u32 {
                        let q = base + i * row + j;
                        indices.extend([q, q + row, q + 1, q + 1, q + row, q + row + 1]);
                    }
                }
            }
        }
    }

    #[test]
    fn a_shell_loses_what_is_inside_it_and_keeps_its_extent() {
        // A box with a smaller box inside it: the inner one is covered from every direction, so it
        // can go without changing a pixel of the outline, and the outer one must keep its corners.
        let (mut positions, mut indices) = (Vec::new(), Vec::new());
        gridded_box(1.0, 8, &mut positions, &mut indices);
        gridded_box(0.5, 8, &mut positions, &mut indices);
        reduce_shell(&mut positions, &mut indices, 100);
        assert!(indices.len() / 3 <= 100, "{} triangles", indices.len() / 3);
        assert!(!indices.is_empty());
        assert!(positions.iter().all(|p| p.iter().any(|c| c.abs() > 0.99)), "inner box survived");
        for axis in 0..3 {
            for side in [-1.0f32, 1.0] {
                assert!(positions.iter().any(|p| (p[axis] - side).abs() < 1e-4));
            }
        }
    }

    #[test]
    fn a_thin_plate_standing_off_the_body_survives() {
        // A 2 m box with a thin wing 30 cm above it: 1.6 m by 0.3 m by 1 cm. This is the shape a
        // clustering grid snapped into the boot lid, and at 30 triangles for the two of them the
        // wing has to come through at its full span.
        let (mut positions, mut indices) = (Vec::new(), Vec::new());
        gridded_box(1.0, 10, &mut positions, &mut indices);
        let (mut wing, mut wing_indices) = (Vec::new(), Vec::new());
        gridded_box(1.0, 10, &mut wing, &mut wing_indices);
        let base = positions.len() as u32;
        positions.extend(wing.iter().map(|p| [p[0] * 0.8, 1.3 + p[1] * 0.005, p[2] * 0.15]));
        indices.extend(wing_indices.iter().map(|i| base + i));
        reduce_shell(&mut positions, &mut indices, 30);
        let wing_x = positions
            .iter()
            .filter(|p| p[1] > 1.2)
            .map(|p| p[0].abs())
            .fold(0.0f32, f32::max);
        assert!(wing_x > 0.79, "the wing's tips were drawn in to {wing_x}");
    }
}
