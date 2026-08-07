//! Geometry construction: the extruded ribbons, and the chunking that makes them cullable.
//!
//! Kept target-agnostic so it can be tested on the host, even though only the PSP ever draws the
//! result. Vertices are laid out exactly as `GU_COLOR_8888 | GU_VERTEX_32BITF` expects, so the
//! buffers can be handed to `sceGumDrawArray` without a copy.
//!
//! Two decisions keep this affordable on hardware:
//!
//! * The mesh samples every `RENDER_STRIDE`-th centreline node. 2620 nodes at 1.34 m is far finer
//!   than 480×272 can show; gameplay still queries the full-resolution array.
//! * Each chunk is one continuous triangle strip. Where a ribbon's stations would need separate
//!   strips, they are stitched with degenerate triangles instead, so a chunk costs one draw call
//!   rather than one per station pair.

use crate::math::{max, min, sqrt, Vec2, Vec3};
use crate::track::{Track, NODE_COUNT};

/// Only every third centreline node becomes geometry.
pub const RENDER_STRIDE: usize = 2;
pub const RENDER_NODES: usize = (NODE_COUNT + RENDER_STRIDE - 1) / RENDER_STRIDE;
/// Nodes per cullable chunk.
pub const CHUNK_NODES: usize = 32;

/// Render nodes of extrapolated road built beyond each end of the track.
///
/// The chase camera trails the car by up to 10.6 m, and a run starts at node 2 — three metres in.
/// Without this the camera sits behind where the ribbon begins and looks at ground that was never
/// built, so the bottom of the screen shows the clear colour. It is worse the faster you go,
/// because the camera pulls further back with speed.
pub const APRON_NODES: usize = 8;
/// How far that apron reaches, in metres. Node spacing is ~1.34 m and the mesh takes every
/// `RENDER_STRIDE`-th node.
pub const APRON_METRES: f32 = APRON_NODES as f32 * RENDER_STRIDE as f32 * 1.34;
pub const CHUNK_COUNT: usize = (RENDER_NODES + CHUNK_NODES - 1) / CHUNK_NODES;

/// Vertex layout for `GU_COLOR_8888 | GU_VERTEX_32BITF`. Colour precedes position, as the GU
/// requires.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vertex {
    pub color: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vertex {
    pub const ZERO: Vertex = Vertex {
        color: 0,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    #[inline]
    pub const fn new(x: f32, y: f32, z: f32, color: u32) -> Self {
        Vertex { color, x, y, z }
    }
}

/// One column of an extruded ribbon: how far out from the centreline, how far below it, and what
/// colour that edge is.
#[derive(Clone, Copy, Debug)]
pub struct Station {
    pub lateral: f32,
    pub y: f32,
    pub color: u32,
}

impl Station {
    pub const fn new(lateral: f32, y: f32, color: u32) -> Self {
        Station { lateral, y, color }
    }
}

/// A cullable run of the ribbon, drawn as a single strip.
#[derive(Clone, Copy, Debug, Default)]
pub struct Chunk {
    pub start: u32,
    pub count: u32,
    pub center: Vec3,
    pub radius: f32,
}

/// Whether a chunk can contribute anything to the frame.
///
/// `forward` must be the camera's real view direction, not its horizontal heading. The chase
/// camera sits several metres above the road looking down at it, so the near plane is pitched:
/// ground that is behind the camera *horizontally* can still be well inside the frustum.
///
/// The sphere test against the plane through the eye is exact, so the extra 12 m is pure margin
/// for the fact that a chunk's bounding sphere is a loose fit around 130 m of curving track.
///
/// Lives here rather than next to the renderer so it can be tested — it is pure geometry, and
/// getting it wrong deletes parts of the world that are plainly on screen.
pub fn chunk_visible(chunk: &Chunk, eye: Vec3, forward: Vec3, fog_far: f32) -> bool {
    if chunk.count == 0 {
        return false;
    }
    let to_chunk = chunk.center.sub(eye);
    if to_chunk.length() - chunk.radius > fog_far {
        return false;
    }
    // Reject only what the bounding sphere places entirely behind the near plane.
    to_chunk.dot(forward) > -(chunk.radius + 12.0)
}

/// Worst-case vertex count for a ribbon with `stations` columns.
pub const fn ribbon_capacity(stations: usize) -> usize {
    // One node of overlap keeps chunks watertight, so a chunk spans CHUNK_NODES + 1 nodes. The
    // first and last chunks carry the apron as well, so budget for it in every chunk.
    let n = CHUNK_NODES + 1 + APRON_NODES;
    let strips = stations - 1;
    // Two vertices per strip per node, plus two degenerates at each strip join.
    (strips * 2 * n + (strips - 1) * 2) * CHUNK_COUNT
}

pub struct Ribbon<const V: usize> {
    pub verts: [Vertex; V],
    pub chunks: [Chunk; CHUNK_COUNT],
    pub len: usize,
}

impl<const V: usize> Ribbon<V> {
    pub const EMPTY: Ribbon<V> = Ribbon {
        verts: [Vertex::ZERO; V],
        chunks: [Chunk {
            start: 0,
            count: 0,
            center: Vec3::ZERO,
            radius: 0.0,
        }; CHUNK_COUNT],
        len: 0,
    };

    /// Extrudes `stations` along the centreline into chunked triangle strips.
    pub fn build(&mut self, track: &Track, stations: &[Station]) {
        self.build_gapped(track, stations, None)
    }

    /// As `build`, but collapses the ribbon to zero width across `gap` (a range of *centreline*
    /// node indices). The guard rail uses this to leave the emergency pull-off open.
    pub fn build_gapped(
        &mut self,
        track: &Track,
        stations: &[Station],
        gap: Option<(usize, usize)>,
    ) {
        let mut w = 0usize;

        for c in 0..CHUNK_COUNT {
            // Signed, because the first chunk starts before the track does.
            let mut first = (c * CHUNK_NODES) as i32;
            // Overlap the next chunk by one node so there is no seam between them.
            let mut last = min_usize(c * CHUNK_NODES + CHUNK_NODES, RENDER_NODES - 1) as i32;
            if c == 0 {
                first -= APRON_NODES as i32;
            }
            if c == CHUNK_COUNT - 1 {
                last += APRON_NODES as i32;
            }
            let start = w;

            let (mut lo, mut hi) = (
                Vec3::new(f32::MAX, f32::MAX, f32::MAX),
                Vec3::new(f32::MIN, f32::MIN, f32::MIN),
            );

            for s in 0..stations.len() - 1 {
                if s > 0 {
                    // Stitch this strip to the previous one with two degenerate triangles.
                    let prev = self.verts[w - 1];
                    self.verts[w] = prev;
                    w += 1;
                    let next = station_vertex(track, first, &stations[s]);
                    self.verts[w] = next;
                    w += 1;
                }
                for n in first..=last {
                    let a = station_vertex(track, n, &stations[s]);
                    // Inside the gap both corners collapse onto the first station, so every
                    // triangle there has zero area and rasterises nothing.
                    let b = if in_gap(n, gap) {
                        a
                    } else {
                        station_vertex(track, n, &stations[s + 1])
                    };
                    self.verts[w] = a;
                    self.verts[w + 1] = b;
                    w += 2;
                    for v in [a, b] {
                        lo = Vec3::new(min(lo.x, v.x), min(lo.y, v.y), min(lo.z, v.z));
                        hi = Vec3::new(max(hi.x, v.x), max(hi.y, v.y), max(hi.z, v.z));
                    }
                }
            }

            let center = lo.add(hi).scale(0.5);
            let mut radius = 0.0f32;
            for v in &self.verts[start..w] {
                let d = sqrt(
                    (v.x - center.x) * (v.x - center.x)
                        + (v.y - center.y) * (v.y - center.y)
                        + (v.z - center.z) * (v.z - center.z),
                );
                radius = max(radius, d);
            }

            self.chunks[c] = Chunk {
                start: start as u32,
                count: (w - start) as u32,
                center,
                radius,
            };
        }

        self.len = w;
    }
}

/// Whether a render node falls inside a gap expressed in centreline node indices.
fn in_gap(render_node: i32, gap: Option<(usize, usize)>) -> bool {
    match gap {
        None => false,
        Some((from, to)) => {
            if render_node < 0 {
                return false;
            }
            let i = render_node as usize * RENDER_STRIDE;
            i >= from && i <= to
        }
    }
}

/// Position of one station at one render node.
///
/// The index is signed and may run past either end of the track: those become the apron, laid
/// along the tangent of the nearest real node so the extension carries on in the direction the
/// road was already going.
fn station_vertex(track: &Track, render_node: i32, st: &Station) -> Vertex {
    let last = (NODE_COUNT - 1) as i32;
    let raw = render_node * RENDER_STRIDE as i32;

    let (n, overshoot) = if raw < 0 {
        (&track.nodes[0], raw as f32)
    } else if raw > last {
        (&track.nodes[NODE_COUNT - 1], (raw - last) as f32)
    } else {
        (&track.nodes[raw as usize], 0.0)
    };

    // Mean node spacing. Uniform-t sampling makes it vary, but the apron is short and only has to
    // be continuous with the road, not exact.
    let spacing = track.length / (NODE_COUNT - 1) as f32;
    let along = overshoot * spacing;

    Vertex::new(
        n.p.x + n.dir.x * along + n.nrm.x * st.lateral,
        n.p.y + st.y,
        n.p.z + n.dir.z * along + n.nrm.z * st.lateral,
        st.color,
    )
}

/// Axis-aligned box as 12 triangles, centred on `(cx, cy, cz)`. Returns vertices written.
pub fn build_box(
    out: &mut [Vertex],
    w: f32,
    h: f32,
    d: f32,
    cx: f32,
    cy: f32,
    cz: f32,
    color: u32,
) -> usize {
    let (hx, hy, hz) = (w * 0.5, h * 0.5, d * 0.5);
    // Eight corners, indexed as a cube.
    let c = [
        (cx - hx, cy - hy, cz - hz),
        (cx + hx, cy - hy, cz - hz),
        (cx + hx, cy + hy, cz - hz),
        (cx - hx, cy + hy, cz - hz),
        (cx - hx, cy - hy, cz + hz),
        (cx + hx, cy - hy, cz + hz),
        (cx + hx, cy + hy, cz + hz),
        (cx - hx, cy + hy, cz + hz),
    ];
    // Counter-clockwise when viewed from outside each face.
    const FACES: [[usize; 6]; 6] = [
        [4, 5, 6, 4, 6, 7], // +Z
        [1, 0, 3, 1, 3, 2], // -Z
        [5, 1, 2, 5, 2, 6], // +X
        [0, 4, 7, 0, 7, 3], // -X
        [3, 7, 6, 3, 6, 2], // +Y
        [0, 1, 5, 0, 5, 4], // -Y
    ];

    let mut w_i = 0;
    for face in FACES.iter() {
        for &idx in face.iter() {
            let (x, y, z) = c[idx];
            out[w_i] = Vertex::new(x, y, z, color);
            w_i += 1;
        }
    }
    w_i
}

/// Upright cylinder standing on `(cx, cy, cz)` — stacked tyres and lamp posts.
pub fn build_upright_cylinder(
    out: &mut [Vertex],
    sides: usize,
    radius: f32,
    height: f32,
    cx: f32,
    cy: f32,
    cz: f32,
    color: u32,
) -> usize {
    let mut w = 0usize;
    for i in 0..sides {
        let a0 = (i as f32 / sides as f32) * crate::math::TAU;
        let a1 = ((i + 1) as f32 / sides as f32) * crate::math::TAU;
        let (x0, z0) = (cx + crate::math::cos(a0) * radius, cz + crate::math::sin(a0) * radius);
        let (x1, z1) = (cx + crate::math::cos(a1) * radius, cz + crate::math::sin(a1) * radius);
        let (lo, hi) = (cy, cy + height);

        out[w] = Vertex::new(x0, lo, z0, color);
        out[w + 1] = Vertex::new(x1, lo, z1, color);
        out[w + 2] = Vertex::new(x1, hi, z1, color);
        out[w + 3] = Vertex::new(x0, lo, z0, color);
        out[w + 4] = Vertex::new(x1, hi, z1, color);
        out[w + 5] = Vertex::new(x0, hi, z0, color);
        w += 6;

        // Top cap only; the underside is never visible on something sitting on the ground.
        out[w] = Vertex::new(cx, hi, cz, color);
        out[w + 1] = Vertex::new(x0, hi, z0, color);
        out[w + 2] = Vertex::new(x1, hi, z1, color);
        w += 3;
    }
    w
}

/// Upright cone standing on `(cx, cy, cz)` — traffic cones.
pub fn build_cone(
    out: &mut [Vertex],
    sides: usize,
    radius: f32,
    height: f32,
    cx: f32,
    cy: f32,
    cz: f32,
    color: u32,
) -> usize {
    let mut w = 0usize;
    let apex = Vertex::new(cx, cy + height, cz, color);
    for i in 0..sides {
        let a0 = (i as f32 / sides as f32) * crate::math::TAU;
        let a1 = ((i + 1) as f32 / sides as f32) * crate::math::TAU;
        let (x0, z0) = (cx + crate::math::cos(a0) * radius, cz + crate::math::sin(a0) * radius);
        let (x1, z1) = (cx + crate::math::cos(a1) * radius, cz + crate::math::sin(a1) * radius);
        out[w] = Vertex::new(x0, cy, z0, color);
        out[w + 1] = Vertex::new(x1, cy, z1, color);
        out[w + 2] = apex;
        w += 3;
    }
    w
}

/// Flat quad lying on the ground, oriented by a forward and a lateral axis.
#[allow(clippy::too_many_arguments)]
pub fn build_ground_quad(
    out: &mut [Vertex],
    cx: f32,
    cy: f32,
    cz: f32,
    forward: Vec2,
    half_length: f32,
    half_width: f32,
    color: u32,
) -> usize {
    let n = forward.lateral_normal();
    let corner = |a: f32, b: f32| {
        Vertex::new(
            cx + forward.x * half_length * a + n.x * half_width * b,
            cy,
            cz + forward.z * half_length * a + n.z * half_width * b,
            color,
        )
    };
    let quad = [
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, 1.0),
    ];
    for (i, v) in quad.iter().enumerate() {
        out[i] = *v;
    }
    quad.len()
}

/// Cylinder around the X axis — the shape used for tyres and rims. Returns vertices written.
pub fn build_cylinder(out: &mut [Vertex], sides: usize, radius: f32, width: f32, color: u32) -> usize {
    let hw = width * 0.5;
    let mut w = 0usize;

    for i in 0..sides {
        let a0 = (i as f32 / sides as f32) * crate::math::TAU;
        let a1 = ((i + 1) as f32 / sides as f32) * crate::math::TAU;
        let (y0, z0) = (crate::math::cos(a0) * radius, crate::math::sin(a0) * radius);
        let (y1, z1) = (crate::math::cos(a1) * radius, crate::math::sin(a1) * radius);

        // Side quad.
        out[w] = Vertex::new(-hw, y0, z0, color);
        out[w + 1] = Vertex::new(hw, y0, z0, color);
        out[w + 2] = Vertex::new(hw, y1, z1, color);
        out[w + 3] = Vertex::new(-hw, y0, z0, color);
        out[w + 4] = Vertex::new(hw, y1, z1, color);
        out[w + 5] = Vertex::new(-hw, y1, z1, color);
        w += 6;

        // End caps, fanned from the centre of each face.
        out[w] = Vertex::new(hw, 0.0, 0.0, color);
        out[w + 1] = Vertex::new(hw, y0, z0, color);
        out[w + 2] = Vertex::new(hw, y1, z1, color);
        w += 3;
        out[w] = Vertex::new(-hw, 0.0, 0.0, color);
        out[w + 1] = Vertex::new(-hw, y1, z1, color);
        out[w + 2] = Vertex::new(-hw, y0, z0, color);
        w += 3;
    }
    w
}

#[inline]
fn min_usize(a: usize, b: usize) -> usize {
    if a < b {
        a
    } else {
        b
    }
}
