//! The 3D scene: the road, the night look and the car.
//!
//! All geometry is generated once at boot and lives in statics; nothing is allocated per frame.
//! Chunks outside fog range or behind the camera are skipped, which is what keeps a 3.5 km track
//! affordable.
//!
//! Lighting is baked into vertex colours rather than computed by the GE: the night
//! look is a fixed hemisphere plus moon term, so there is nothing to recompute per frame.

use core::ffi::c_void;

use angle_zero::azcar;
use angle_zero::camera::Camera;
use angle_zero::effects::Effects;
use angle_zero::math::{cos, sin, sqrt, Mat4, Vec3, TAU};
use angle_zero::mesh::{self, ribbon_capacity, Chunk, Ribbon, Station, Vertex};
use angle_zero::track::{Track, BAY_FROM, BAY_NODE, BAY_SIDE, BAY_TO, CORNER_CURVATURE};
use angle_zero::vehicle::{CarState, Vehicle};
use psp::sys::{
    self, GuPrimitive, GuState, MatrixMode, ScePspFMatrix4, ScePspFVector3, VertexType,
};

/// PSP colours are ABGR, not ARGB.
pub const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    0xff00_0000 | (b << 16) | (g << 8) | r
}

/// Same, with an explicit alpha for the additive passes.
pub const fn rgba(r: u32, g: u32, b: u32, a: u32) -> u32 {
    (a << 24) | (b << 16) | (g << 8) | r
}

// --- night palette --------------------------------------------------------------
pub const SKY_CLEAR: u32 = rgb(0x07, 0x0A, 0x12);
pub const FOG_COLOR: u32 = rgb(0x08, 0x0C, 0x15);
pub const FOG_NEAR: f32 = 45.0;
pub const FOG_FAR: f32 = 330.0;
/// Projection far plane, and the distance past which chunks are culled. Geometry beyond this is
/// clipped by the hardware anyway, so nothing visible can be lost by using it.
pub const DRAW_DISTANCE: f32 = 2400.0;

const ROAD_COLOR: u32 = rgb(0x1A, 0x1C, 0x20);
const EDGE_COLOR: u32 = rgb(0xA9, 0xA2, 0x93);
const RAIL_COLOR: u32 = rgb(0x7C, 0x84, 0x8C);
const DASH_COLOR: u32 = rgb(0x8C, 0x7A, 0x45);
// The car's palette is not here any more. Its colours are baked per vertex by the asset compiler,
// out of the source model's own materials, so there is nothing left for the renderer to decide.

/// How far the terrain ribbon reaches to either side of the centreline.
///
/// The road is on a pass with the ground falling away on both sides, so from a camera near the
/// surface a sight line can leave the ribbon sideways and never come back down to it — the ground
/// drops at about 0.4 m per metre out here, which is steeper than the shallow rays, so they escape.
/// Widening the ribbon does not fix that: it only moves the escape further out. Measured across
/// five orbit angles, 380 m still left the worst one untouched, and 1200 m — enough to close it —
/// costs 30 terrain chunks and 52k vertices a frame against 14 and 31k.
///
/// So the ribbon stays the size the *scenery* wants to be, and [`draw_ground_backdrop`] deals with
/// what is behind it.
const TERRAIN_HALF_WIDTH: f32 = 190.0;
const TERRAIN_EDGE_DROP: f32 = -78.0;

/// The hillside, shaded darker as it falls away so the slope reads at night.
const fn terrain(lateral: f32, y: f32, shade: u32) -> Station {
    Station::new(lateral, y, rgb(shade * 34 / 100, shade * 48 / 100, shade * 28 / 100))
}

const TERRAIN_STATIONS: [Station; 12] = [
    terrain(-TERRAIN_HALF_WIDTH, TERRAIN_EDGE_DROP, 34),
    terrain(-96.0, -40.0, 44),
    terrain(-48.0, -17.0, 58),
    terrain(-22.0, -4.2, 74),
    terrain(-11.0, -0.9, 92),
    terrain(-7.2, -0.25, 100),
    terrain(7.2, -0.25, 100),
    terrain(11.0, -0.9, 92),
    terrain(22.0, -4.2, 74),
    terrain(48.0, -17.0, 58),
    terrain(96.0, -40.0, 44),
    terrain(TERRAIN_HALF_WIDTH, TERRAIN_EDGE_DROP, 34),
];

const ROAD_STATIONS: [Station; 5] = [
    Station::new(-angle_zero::track::ROAD_SHOULDER, 0.0, rgb(0x14, 0x16, 0x19)),
    Station::new(-5.2, 0.02, ROAD_COLOR),
    Station::new(0.0, 0.03, rgb(0x1E, 0x20, 0x25)),
    Station::new(5.2, 0.02, ROAD_COLOR),
    Station::new(angle_zero::track::ROAD_SHOULDER, 0.0, rgb(0x14, 0x16, 0x19)),
];

// The two edge lines must be separate ribbons. Built as one four-station ribbon, the quad
// between the inner stations paints the entire road white.
const EDGE_LEFT: [Station; 2] = [
    Station::new(-5.05, 0.05, EDGE_COLOR),
    Station::new(-4.75, 0.05, EDGE_COLOR),
];
const EDGE_RIGHT: [Station; 2] = [
    Station::new(4.75, 0.05, EDGE_COLOR),
    Station::new(5.05, 0.05, EDGE_COLOR),
];

const RAIL_LEFT: [Station; 2] = [
    Station::new(-7.5, 0.55, rgb(0x4E, 0x54, 0x5B)),
    Station::new(-7.5, 0.95, RAIL_COLOR),
];
const RAIL_RIGHT: [Station; 2] = [
    Station::new(7.5, 0.55, rgb(0x4E, 0x54, 0x5B)),
    Station::new(7.5, 0.95, RAIL_COLOR),
];

const ROAD_CAP: usize = ribbon_capacity(5);
const TERRAIN_CAP: usize = ribbon_capacity(12);
const LINE_CAP: usize = ribbon_capacity(2);

static mut TERRAIN_MESH: Ribbon<TERRAIN_CAP> = Ribbon::EMPTY;
static mut ROAD_MESH: Ribbon<ROAD_CAP> = Ribbon::EMPTY;
static mut EDGE_L_MESH: Ribbon<LINE_CAP> = Ribbon::EMPTY;
static mut EDGE_R_MESH: Ribbon<LINE_CAP> = Ribbon::EMPTY;
static mut RAIL_L_MESH: Ribbon<LINE_CAP> = Ribbon::EMPTY;
static mut RAIL_R_MESH: Ribbon<LINE_CAP> = Ribbon::EMPTY;

/// The car is not built here any more. It is compiled from a 3D model by `anglezero-asset`, loaded
/// off the memory stick by `super::car`, and drawn straight out of the buffer it was read into —
/// see `draw_car`.

/// Centre dashes, bucketed by chunk so they cull with everything else.
const DASH_STRIDE: usize = 7;
const DASHES_PER_CHUNK: usize = (mesh::CHUNK_NODES * mesh::RENDER_STRIDE) / DASH_STRIDE + 2;
const DASH_VERTS: usize = DASHES_PER_CHUNK * 6 * mesh::CHUNK_COUNT;
static mut DASH_MESH: psp::Align16<[Vertex; DASH_VERTS]> = psp::Align16([Vertex::ZERO; DASH_VERTS]);
static mut DASH_CHUNKS: [Chunk; mesh::CHUNK_COUNT] = [Chunk {
    start: 0,
    count: 0,
    center: Vec3::ZERO,
    radius: 0.0,
}; mesh::CHUNK_COUNT];

const VERTEX_FORMAT: VertexType = VertexType::from_bits_truncate(
    VertexType::COLOR_8888.bits() | VertexType::VERTEX_32BITF.bits() | VertexType::TRANSFORM_3D.bits(),
);

/// The same vertices, drawn through an index buffer. Compiled cars are indexed because their
/// geometry is a welded mesh rather than the loose triangles everything generated here is: the E36
/// is 9,500 vertices behind 45,000 indices, and unindexed it would be three times the memory and
/// three times the vertex fetch.
const CAR_VERTEX_FORMAT: VertexType = VertexType::from_bits_truncate(
    VERTEX_FORMAT.bits() | VertexType::INDEX_16BIT.bits(),
);

/// Render-state overrides for diagnosing hardware-only faults, cycled with the L trigger.
///
/// The scenery dropout at the bottom of the screen does not reproduce in any emulator backend
/// available here, so the mechanism has to be identified on the console itself. Each mode turns
/// off one suspect; whichever one makes the fault disappear names the cause.
///
/// Interactively these are cycled with the L trigger. A harness run sets one from its script
/// instead, which is what makes the overrides usable as a bisection: run the same frames with one
/// suspect disabled and see whether the artifact survives.
#[cfg(feature = "devtools")]
pub const DEBUG_MODES: u32 = 13;
#[cfg(feature = "devtools")]
static mut DEBUG_MODE: u32 = 0;

#[cfg(feature = "devtools")]
pub fn set_debug_mode(mode: u32) {
    unsafe { DEBUG_MODE = mode % DEBUG_MODES }
}

#[cfg(feature = "devtools")]
pub fn debug_mode() -> u32 {
    unsafe { DEBUG_MODE }
}

/// Builds every static mesh. Call once, after the track is generated.
pub fn init(track: &Track) {
    unsafe {
        (*(&raw mut TERRAIN_MESH)).build_shelved(track, &TERRAIN_STATIONS);
        (*(&raw mut ROAD_MESH)).build(track, &ROAD_STATIONS);
        (*(&raw mut EDGE_L_MESH)).build(track, &EDGE_LEFT);
        (*(&raw mut EDGE_R_MESH)).build(track, &EDGE_RIGHT);

        // The bay side has no rail across the pull-off, so the player can drive in.
        let bay_gap = Some((BAY_FROM, BAY_TO));
        if BAY_SIDE > 0.0 {
            (*(&raw mut RAIL_L_MESH)).build(track, &RAIL_LEFT);
            (*(&raw mut RAIL_R_MESH)).build_gapped(track, &RAIL_RIGHT, bay_gap);
        } else {
            (*(&raw mut RAIL_L_MESH)).build_gapped(track, &RAIL_LEFT, bay_gap);
            (*(&raw mut RAIL_R_MESH)).build(track, &RAIL_RIGHT);
        }

        build_dashes(track);
        build_props(track);
        build_mountains(track);
        build_starfield();
        sys::sceKernelDcacheWritebackAll();
    }
}

/// Centre dashes: flat quads laid along the road, every seventh centreline node.
unsafe fn build_dashes(track: &Track) {
    let verts = &raw mut DASH_MESH as *mut Vertex;
    let mut w = 0usize;

    for c in 0..mesh::CHUNK_COUNT {
        let start = w;
        let first_node = c * mesh::CHUNK_NODES * mesh::RENDER_STRIDE;
        let last_node = core::cmp::min(
            first_node + mesh::CHUNK_NODES * mesh::RENDER_STRIDE,
            angle_zero::track::NODE_COUNT - 1,
        );

        let (mut lo, mut hi) = (
            Vec3::new(f32::MAX, f32::MAX, f32::MAX),
            Vec3::new(f32::MIN, f32::MIN, f32::MIN),
        );

        let mut n = first_node - first_node % DASH_STRIDE;
        if n < first_node {
            n += DASH_STRIDE;
        }
        while n <= last_node {
            let node = &track.nodes[n];
            // 0.16 x 2.6 m, aligned to the road heading.
            let (fx, fz) = (node.dir.x * 1.3, node.dir.z * 1.3);
            let (sx, sz) = (node.nrm.x * 0.08, node.nrm.z * 0.08);
            let y = node.p.y + 0.06;
            let corner = |a: f32, b: f32| {
                Vertex::new(
                    node.p.x + fx * a + sx * b,
                    y,
                    node.p.z + fz * a + sz * b,
                    DASH_COLOR,
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
            for v in quad.iter() {
                *verts.add(w) = *v;
                w += 1;
                lo = Vec3::new(
                    fmin(lo.x, v.x),
                    fmin(lo.y, v.y),
                    fmin(lo.z, v.z),
                );
                hi = Vec3::new(
                    fmax(hi.x, v.x),
                    fmax(hi.y, v.y),
                    fmax(hi.z, v.z),
                );
            }
            n += DASH_STRIDE;
        }

        let count = w - start;
        let center = if count > 0 {
            lo.add(hi).scale(0.5)
        } else {
            Vec3::ZERO
        };
        let mut radius = 0.0f32;
        for i in start..w {
            let v = *verts.add(i);
            let d = Vec3::new(v.x, v.y, v.z).sub(center).length();
            radius = fmax(radius, d);
        }
        DASH_CHUNKS[c] = Chunk {
            start: start as u32,
            count: count as u32,
            center,
            radius,
        };
    }
}

/// Roadside props: street lamps and trees.
///
/// Baked into one static, chunk-bucketed buffer in world space rather than instanced with a
/// matrix per prop. There are over a thousand of them; a draw call each would dominate the frame.
const PROPS_PER_CHUNK: usize = 3400;
const PROP_VERTS: usize = PROPS_PER_CHUNK * mesh::CHUNK_COUNT;
static mut PROP_MESH: psp::Align16<[Vertex; PROP_VERTS]> = psp::Align16([Vertex::ZERO; PROP_VERTS]);
static mut PROP_CHUNKS: [Chunk; mesh::CHUNK_COUNT] = [Chunk {
    start: 0,
    count: 0,
    center: Vec3::ZERO,
    radius: 0.0,
}; mesh::CHUNK_COUNT];

/// Additive light pools and lamp glows. Kept in their own chunked buffer
/// because they need a separate blended, depth-write-off pass after the opaque world.
const GLOWS_PER_CHUNK: usize = 512;
const PROP_GLOW_VERTS: usize = GLOWS_PER_CHUNK * mesh::CHUNK_COUNT;
static mut PROP_GLOW_MESH: psp::Align16<[Vertex; PROP_GLOW_VERTS]> =
    psp::Align16([Vertex::ZERO; PROP_GLOW_VERTS]);
static mut PROP_GLOW_CHUNKS: [Chunk; mesh::CHUNK_COUNT] = [Chunk {
    start: 0,
    count: 0,
    center: Vec3::ZERO,
    radius: 0.0,
}; mesh::CHUNK_COUNT];

/// Warm light, as the lamp glows have it.
const LAMP_GLOW: u32 = rgba(0xFF, 0xEC, 0xBE, 0x8C);
/// Ground pools are wide and faint; at 0.55 opacity across 19 m a flat quad would wash the road
/// out, so these fade from the centre like the sprites they stand in for.
const LAMP_POOL: u32 = rgba(0xFF, 0xE4, 0xB0, 0x54);
/// How far toward the camera the ground-pool pass is biased, out of the 65535 the depth range
/// spans. Enough to win a tie against the facet a pool lies on; small enough that a pool cannot
/// climb in front of something genuinely nearer, such as the car or a guard rail.
const POOL_DEPTH_BIAS: i32 = 64;
const FLOOD_POOL: u32 = rgba(0xDA, 0xE4, 0xF2, 0x4A);

const TREE_STRIDE: usize = 4;
const LAMP_STRIDE: usize = 58;
const CONE_STRIDE: usize = 11;
const FLOODLIGHT_STRIDE: usize = 90;
/// The design puts a tyre stack every third node of every corner. At 2620 nodes that is thousands
/// of eight-sided cylinders, so they are thinned out — the wall still reads as continuous.
const TYRE_WALL_STRIDE: usize = 15;

const CONE_COLOR: u32 = rgb(0xE4, 0x62, 0x2F);
const TYRE_STACK: u32 = rgb(0x16, 0x16, 0x1A);
const FLOOD_PANEL: u32 = rgb(0xDA, 0xE4, 0xF2);

const TREE_LOW: u32 = rgb(0x1A, 0x21, 0x1C);
const TREE_HIGH: u32 = rgb(0x30, 0x39, 0x37);
const LAMP_POLE: u32 = rgb(0x3A, 0x40, 0x46);
const LAMP_HEAD: u32 = rgb(0xFF, 0xEC, 0xBE);

unsafe fn build_props(track: &Track) {
    let verts = core::slice::from_raw_parts_mut(&raw mut PROP_MESH as *mut Vertex, PROP_VERTS);
    let glows =
        core::slice::from_raw_parts_mut(&raw mut PROP_GLOW_MESH as *mut Vertex, PROP_GLOW_VERTS);
    let mut w = 0usize;
    let mut gw = 0usize;

    for c in 0..mesh::CHUNK_COUNT {
        let start = w;
        let budget = start + PROPS_PER_CHUNK;
        let glow_start = gw;
        let glow_budget = glow_start + GLOWS_PER_CHUNK;
        let (mut glo, mut ghi) = (
            Vec3::new(f32::MAX, f32::MAX, f32::MAX),
            Vec3::new(f32::MIN, f32::MIN, f32::MIN),
        );
        let first_node = c * mesh::CHUNK_NODES * mesh::RENDER_STRIDE;
        let last_node = core::cmp::min(
            first_node + mesh::CHUNK_NODES * mesh::RENDER_STRIDE,
            angle_zero::track::NODE_COUNT - 1,
        );

        let (mut lo, mut hi) = (
            Vec3::new(f32::MAX, f32::MAX, f32::MAX),
            Vec3::new(f32::MIN, f32::MIN, f32::MIN),
        );
        let note = |v: &Vertex, lo: &mut Vec3, hi: &mut Vec3| {
            *lo = Vec3::new(fmin(lo.x, v.x), fmin(lo.y, v.y), fmin(lo.z, v.z));
            *hi = Vec3::new(fmax(hi.x, v.x), fmax(hi.y, v.y), fmax(hi.z, v.z));
        };

        for i in first_node..=last_node {
            let node = &track.nodes[i];

            // --- trees, both sides, pseudo-random offsets ---
            if i % TREE_STRIDE == 0 {
                for k in 0..2usize {
                    if w + 12 > budget {
                        break;
                    }
                    let side = if k == 0 { -1.0 } else { 1.0 };
                    let off = 15.0 + ((i * 37 + k * 91) % 34) as f32;
                    let height = 7.0 + ((i * 13 + k * 7) % 7) as f32;
                    let half = 0.62 * height * 0.5;
                    // Trees stand on the hillside, which falls away from the road.
                    let drop = if off < 14.0 {
                        off * 0.11
                    } else {
                        1.6 + (off - 14.0) * 0.4
                    };
                    let bx = node.p.x + node.nrm.x * side * off;
                    let bz = node.p.z + node.nrm.z * side * off;
                    let by = node.p.y - drop;

                    // Two tapering tiers give a conifer silhouette. A plain quad here reads as a
                    // building, which is exactly what a wall of them looked like.
                    //
                    // Crossed in an X over two axes so the tree holds up from any viewing angle
                    // without being rotated toward the camera every frame.
                    for axis in 0..2 {
                        let (ax, az) = if axis == 0 {
                            (node.dir.x, node.dir.z)
                        } else {
                            (node.nrm.x, node.nrm.z)
                        };
                        let tiers = [
                            (half, height * 0.22, height * 0.70, TREE_LOW),
                            (half * 0.62, height * 0.55, height, TREE_HIGH),
                        ];
                        for (w_half, base_y, tip_y, color) in tiers {
                            let corners = [
                                (-w_half, base_y, color),
                                (w_half, base_y, color),
                                (0.0, tip_y, TREE_HIGH),
                            ];
                            for (dx, dy, c) in corners {
                                let v = Vertex::new(bx + ax * dx, by + dy, bz + az * dx, c);
                                verts[w] = v;
                                note(&v, &mut lo, &mut hi);
                                w += 1;
                            }
                        }
                    }
                }
            }

            // --- street lamps, alternating sides ---
            if i % LAMP_STRIDE == 0 && w + 108 <= budget {
                let side = if (i / LAMP_STRIDE) % 2 == 0 { -1.0 } else { 1.0 };
                let lateral = 8.4 * side;
                let bx = node.p.x + node.nrm.x * lateral;
                let bz = node.p.z + node.nrm.z * lateral;
                let by = node.p.y;

                // Pole, arm reaching over the road, and a glowing head.
                w += mesh::build_box(&mut verts[w..], 0.26, 7.4, 0.26, bx, by + 3.7, bz, LAMP_POLE);
                let arm_x = bx - node.nrm.x * side * 1.1;
                let arm_z = bz - node.nrm.z * side * 1.1;
                w += mesh::build_box(
                    &mut verts[w..],
                    2.2,
                    0.16,
                    0.16,
                    arm_x,
                    by + 7.3,
                    arm_z,
                    LAMP_POLE,
                );
                let head_x = bx - node.nrm.x * side * 2.2;
                let head_z = bz - node.nrm.z * side * 2.2;
                w += mesh::build_box(
                    &mut verts[w..],
                    0.5,
                    0.2,
                    0.9,
                    head_x,
                    by + 7.15,
                    head_z,
                    LAMP_HEAD,
                );
                for v in &verts[w - 108..w] {
                    note(&(*v), &mut lo, &mut hi);
                }

                // A glow around the head, and a 19 m pool of light spilling onto the road.
                if gw + GROUND_GLOW_VERTS + GLOW_VERTS <= glow_budget {
                    push_ground_glow(
                        glows,
                        &mut gw,
                        node.p.x + node.nrm.x * 5.6 * side,
                        node.p.y + 0.04,
                        node.p.z + node.nrm.z * 5.6 * side,
                        9.5,
                        LAMP_POOL,
                    );
                    push_blob_glow(
                        glows,
                        &mut gw,
                        head_x,
                        node.p.y + 7.15,
                        head_z,
                        2.75,
                        LAMP_GLOW,
                    );
                    for v in &glows[gw - GROUND_GLOW_VERTS - GLOW_VERTS * 2..gw] {
                        note(&(*v), &mut glo, &mut ghi);
                    }
                }
            }

            // --- cones, alternating sides of the road ---
            if i % CONE_STRIDE == 0 && w + 18 <= budget {
                let side = if (i / CONE_STRIDE) % 2 == 0 { -1.0 } else { 1.0 };
                let lateral = 6.0 * side;
                let before = w;
                w += mesh::build_cone(
                    &mut verts[w..],
                    6,
                    0.24,
                    0.62,
                    node.p.x + node.nrm.x * lateral,
                    node.p.y,
                    node.p.z + node.nrm.z * lateral,
                    CONE_COLOR,
                );
                for v in &verts[before..w] {
                    note(&(*v), &mut lo, &mut hi);
                }
            }

            // --- tyre walls on the outside of corners ---
            if i % TYRE_WALL_STRIDE == 0 && node.curv.abs() >= CORNER_CURVATURE {
                // `curv` is signed by turn direction, so the outside of the corner is the side
                // the car is pushed toward.
                let side = if node.curv > 0.0 { 1.0 } else { -1.0 };
                let lateral = 8.4 * side;
                let (bx, bz) = (
                    node.p.x + node.nrm.x * lateral,
                    node.p.z + node.nrm.z * lateral,
                );
                for tier in 0..3 {
                    if w + 72 > budget {
                        break;
                    }
                    let before = w;
                    w += mesh::build_upright_cylinder(
                        &mut verts[w..],
                        6,
                        0.5,
                        0.28,
                        bx,
                        node.p.y + tier as f32 * 0.28,
                        bz,
                        TYRE_STACK,
                    );
                    for v in &verts[before..w] {
                        note(&(*v), &mut lo, &mut hi);
                    }
                }
            }

            // --- floodlight towers ---
            if i % FLOODLIGHT_STRIDE == 0 && w + 108 <= budget {
                let side = if (i / FLOODLIGHT_STRIDE) % 2 == 0 { -1.0 } else { 1.0 };
                let lateral = 12.0 * side;
                let bx = node.p.x + node.nrm.x * lateral;
                let bz = node.p.z + node.nrm.z * lateral;
                let before = w;
                w += mesh::build_box(&mut verts[w..], 0.4, 12.0, 0.4, bx, node.p.y + 6.0, bz, LAMP_POLE);
                w += mesh::build_box(&mut verts[w..], 2.6, 0.5, 0.6, bx, node.p.y + 12.2, bz, LAMP_POLE);
                w += mesh::build_box(&mut verts[w..], 2.2, 0.3, 0.2, bx, node.p.y + 11.9, bz, FLOOD_PANEL);
                for v in &verts[before..w] {
                    note(&(*v), &mut lo, &mut hi);
                }

                if gw + GROUND_GLOW_VERTS + GLOW_VERTS * 2 <= glow_budget {
                    let g0 = gw;
                    push_ground_glow(
                        glows,
                        &mut gw,
                        node.p.x + node.nrm.x * 7.0 * side,
                        node.p.y + 0.04,
                        node.p.z + node.nrm.z * 7.0 * side,
                        11.0,
                        FLOOD_POOL,
                    );
                    push_blob_glow(glows, &mut gw, bx, node.p.y + 11.9, bz, 6.0, LAMP_GLOW);
                    for v in &glows[g0..gw] {
                        note(&(*v), &mut glo, &mut ghi);
                    }
                }
            }
        }

        // --- the emergency pull-off's furniture ---
        if first_node <= angle_zero::track::FINISH_NODE
            && angle_zero::track::FINISH_NODE <= last_node
        {
            let before = w;
            w += build_finish(track, &mut verts[w..]);
            for v in &verts[before..w] {
                note(&(*v), &mut lo, &mut hi);
            }
        }

        if first_node <= BAY_NODE && BAY_NODE <= last_node {
            let before = w;
            w += build_bay_props(track, &mut verts[w..]);
            for v in &verts[before..w] {
                note(&(*v), &mut lo, &mut hi);
            }

            // The 26 m pool over the pad, and a glow on the lamp head above it.
            //
            // Everything here is placed by arclength through `bay_surface`, the same way the paving
            // and the props it lights are. Extrapolating from `BAY_NODE` along its `dir` instead —
            // which is what this did — runs straight while the road curves, and holds one height
            // while the pass drops 7.4 cm a metre, so the pool sat the best part of a metre off the
            // ground it was supposed to be lying on.
            if gw + GROUND_GLOW_VERTS + GLOW_VERTS * 2 <= glow_budget {
                use angle_zero::track::bay_surface;
                let g0 = gw;
                // These sit on the paving, not on the shelf cut underneath it. The two are a
                // quarter of a metre apart, which is enough to bury a ground pool completely.
                push_bay_pool(track, glows, &mut gw, 10.0, 10.0, 12.0, 0.06, LAMP_POOL);
                // The head of the lamp built in `build_bay_props`, which stands at lateral 7.6.
                let head = bay_surface(track, 12.0, 9.8);
                let foot = bay_surface(track, 12.0, 7.6);
                push_blob_glow(glows, &mut gw, head.x, foot.y + 7.15, head.z, 2.75, LAMP_GLOW);
                // The vending machine throws a small warm pool of its own.
                push_bay_pool(track, glows, &mut gw, -7.0, 14.6, 4.2, 0.07, LAMP_POOL);
                let v = bay_surface(track, -7.0, 15.6);
                push_blob_glow(glows, &mut gw, v.x, v.y + 1.25, v.z, 1.5, LAMP_GLOW);
                for v in &glows[g0..gw] {
                    note(&(*v), &mut glo, &mut ghi);
                }
            }
        }

        let count = w - start;
        let center = if count > 0 {
            lo.add(hi).scale(0.5)
        } else {
            Vec3::ZERO
        };
        let mut radius = 0.0f32;
        for v in &verts[start..w] {
            radius = fmax(radius, Vec3::new(v.x, v.y, v.z).sub(center).length());
        }
        PROP_CHUNKS[c] = Chunk {
            start: start as u32,
            count: count as u32,
            center,
            radius,
        };

        let glow_count = gw - glow_start;
        let glow_center = if glow_count > 0 {
            glo.add(ghi).scale(0.5)
        } else {
            Vec3::ZERO
        };
        let mut glow_radius = 0.0f32;
        for v in &glows[glow_start..gw] {
            glow_radius = fmax(
                glow_radius,
                Vec3::new(v.x, v.y, v.z).sub(glow_center).length(),
            );
        }
        PROP_GLOW_CHUNKS[c] = Chunk {
            start: glow_start as u32,
            count: glow_count as u32,
            center: glow_center,
            radius: glow_radius,
        };
    }
}

/// The finish: a gantry over the road with a lit banner, and a line painted across the tarmac.
///
/// Without it the road simply runs on into the dark and the results screen arrives unannounced.
/// The line sits at `FINISH_NODE`, which is where the run actually ends — 98.5% of the centreline,
/// not the last node — so crossing it and finishing are the same moment.
fn build_finish(track: &Track, out: &mut [Vertex]) -> usize {
    let node = &track.nodes[angle_zero::track::FINISH_NODE];
    let dir = node.dir;
    let nrm = node.nrm;
    let y = node.p.y;
    let mut w = 0usize;

    let at = |along: f32, lateral: f32| {
        (
            node.p.x + dir.x * along + nrm.x * lateral,
            node.p.z + dir.z * along + nrm.z * lateral,
        )
    };

    // The line itself: a broad pale stripe across the full width of the road, laid flat.
    w += mesh::build_ground_quad(
        &mut out[w..], node.p.x, y + 0.05, node.p.z, dir, 0.7, 6.4, rgb(0xE8, 0xE4, 0xD8),
    );
    // A marker a few metres before it, so the line reads as deliberate rather than as a stray
    // piece of road marking.
    let (lx, lz) = at(-6.0, 0.0);
    w += mesh::build_ground_quad(
        &mut out[w..], lx, y + 0.045, lz, dir, 0.25, 6.4, rgb(0x8C, 0x7A, 0x45),
    );

    // Two posts and a beam across the top, carrying a lit panel.
    for side in [-1.0f32, 1.0] {
        let (px, pz) = at(0.0, 7.4 * side);
        w += mesh::build_box(&mut out[w..], 0.34, 6.0, 0.34, px, y + 3.0, pz, rgb(0x3A, 0x3E, 0x44));
        // A foot, so the post meets the ground rather than ending in mid-air on a camber.
        w += mesh::build_box(&mut out[w..], 0.7, 0.3, 0.7, px, y + 0.15, pz, rgb(0x2A, 0x2E, 0x33));
    }
    let (bx, bz) = at(0.0, 0.0);
    w += mesh::build_box(&mut out[w..], 15.2, 0.4, 0.4, bx, y + 5.9, bz, rgb(0x3A, 0x3E, 0x44));
    // The banner: an emissive panel slung under the beam, bright enough to be the thing you aim at.
    w += mesh::build_box(&mut out[w..], 9.0, 1.1, 0.18, bx, y + 5.1, bz, rgb(0xE8, 0xC0, 0x30));
    w += mesh::build_box(&mut out[w..], 8.4, 0.5, 0.1, bx, y + 5.1, bz, rgb(0x1A, 0x1A, 0x1C));

    w
}

/// The viewpoint — the lay-by the title screen looks at.
///
/// A widened, paved shoulder rather than a gravel pad dropped onto the hillside: the surface is
/// the same asphalt as the road and runs straight out of it, and a low stone parapet follows the
/// outer edge. The parapet is the point of the thing. The hillside has to be cut back to make
/// level ground here, and a raw cut looks like a mistake; a wall along it looks like a road
/// engineer put it there, which is what a mountain lay-by actually has.
///
/// A lit vending machine does the rest of the work — it is the one warm light for a hundred
/// metres, it says somebody comes up here, and it costs two boxes.
fn build_bay_props(track: &Track, out: &mut [Vertex]) -> usize {
    use angle_zero::track::{bay_surface, BAY_APRON_INNER, BAY_APRON_OUTER, BAY_HALF_LENGTH};
    let mut w = 0usize;

    // Everything here is placed by arclength and takes its height from the node it stands above.
    // The pass drops 2.8 m across the lay-by, so anything laid out flat from one node is buried
    // at the top end and hanging in the air at the bottom — see `tests/bay.rs`.
    let at = |along: f32, lateral: f32| bay_surface(track, along, lateral);

    // Where the paving is cut across the pass.
    //
    // Its inner edge *is* the road ribbon's outer edge, so the two must be cut on the same
    // centreline nodes — the ribbon samples every `RENDER_STRIDE`-th one. Sharing the vertices is
    // what makes the butt joint watertight, and it is the only way to meet the road without either
    // a crack or an overlap. Cutting the two independently, which is what a fixed step count did,
    // is precisely what the old 0.4 m overlap existed to hide, and that overlap fought.
    //
    // The ends snap *inwards* to a node: the shelf is only fully cut through the pull-off proper,
    // and paving that ran past it would sit on ground the hillside is still climbing back into.
    let spacing = mesh::ribbon_spacing(track);
    let s0 = track.nodes[BAY_NODE].s;
    let (first, last) = mesh::ribbon_samples_within(track, s0, BAY_HALF_LENGTH);
    let steps = last - first;
    let along_at = |i: usize| (first + i) as f32 * spacing - s0;

    // The apron, as a ribbon of quads down the hill rather than one slab across it.
    for i in 0..steps {
        let (a, b) = (along_at(i), along_at(i + 1));
        w += mesh::build_quad(
            &mut out[w..],
            at(a, BAY_APRON_INNER),
            at(b, BAY_APRON_INNER),
            at(b, BAY_APRON_OUTER),
            at(a, BAY_APRON_OUTER),
            rgb(0x24, 0x26, 0x2A),
        );
    }

    // The parapet, chained so it follows both the curve of the road and the fall of the pass. On
    // the same cuts as the paving, so the wall starts and ends where the paving does rather than
    // overhanging it onto the shelf.
    const WALL_LATERAL: f32 = 19.6;
    for i in 0..steps {
        w += mesh::build_wall_segment(
            &mut out[w..],
            at(along_at(i), WALL_LATERAL),
            at(along_at(i + 1), WALL_LATERAL),
            0.22,
            0.78,
            0.12,
            rgb(0x55, 0x55, 0x4E),
            rgb(0x6E, 0x6E, 0x66),
        );
    }

    // The vending machine, and the crate of empties beside it.
    let v = at(-7.0, 15.6);
    w += mesh::build_box(&mut out[w..], 1.3, 2.0, 0.85, v.x, v.y + 1.0, v.z, rgb(0xC0, 0x2E, 0x2A));
    // The lit front panel faces the road, which is the side the camera orbits.
    let f = at(-7.0, 15.2);
    w += mesh::build_box(&mut out[w..], 1.05, 1.35, 0.1, f.x, v.y + 1.25, f.z, rgb(0xFF, 0xEC, 0xBE));
    w += mesh::build_box(&mut out[w..], 1.05, 0.3, 0.12, f.x, v.y + 0.42, f.z, rgb(0x2A, 0x2A, 0x2E));
    let c = at(-8.6, 15.4);
    w += mesh::build_box(&mut out[w..], 0.6, 0.5, 0.42, c.x, c.y + 0.25, c.z, rgb(0x2C, 0x3A, 0x30));

    // A route sign at the far end.
    let s = at(13.0, 16.4);
    w += mesh::build_box(&mut out[w..], 0.14, 2.2, 0.14, s.x, s.y + 1.1, s.z, rgb(0x3A, 0x3E, 0x44));
    w += mesh::build_box(&mut out[w..], 2.0, 0.62, 0.1, s.x, s.y + 2.25, s.z, rgb(0x1C, 0x3A, 0x2C));
    w += mesh::build_box(&mut out[w..], 1.7, 0.14, 0.12, s.x, s.y + 2.34, s.z, rgb(0xD8, 0xDE, 0xE4));

    // The lamp over the apron, its head reaching out towards the road.
    let l = at(12.0, 7.6);
    w += mesh::build_box(&mut out[w..], 0.26, 7.4, 0.26, l.x, l.y + 3.7, l.z, LAMP_POLE);
    let h = at(12.0, 9.8);
    w += mesh::build_box(&mut out[w..], 2.2, 0.16, 0.16, (l.x + h.x) * 0.5, l.y + 7.3, (l.z + h.z) * 0.5, LAMP_POLE);
    w += mesh::build_box(&mut out[w..], 0.5, 0.2, 0.9, h.x, l.y + 7.15, h.z, LAMP_HEAD);

    w
}

/// Mountain ring. Thirty four-sided cones ringing the track, drawn without fog as a
/// pure silhouette: at 700 m+ they sit far beyond the 330 m fog range, so fogging them would
/// erase the horizon entirely.
const MOUNTAIN_COUNT: usize = 30;
const MOUNTAIN_VERTS: usize = MOUNTAIN_COUNT * 4 * 3;
static mut MOUNTAINS: psp::Align16<[Vertex; MOUNTAIN_VERTS]> =
    psp::Align16([Vertex::ZERO; MOUNTAIN_VERTS]);

unsafe fn build_mountains(track: &Track) {
    let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
    let (mut min_z, mut max_z) = (f32::MAX, f32::MIN);
    let mut max_y = f32::MIN;
    for n in track.nodes.iter() {
        min_x = fmin(min_x, n.p.x);
        max_x = fmax(max_x, n.p.x);
        min_z = fmin(min_z, n.p.z);
        max_z = fmax(max_z, n.p.z);
        max_y = fmax(max_y, n.p.y);
    }
    let cx = (min_x + max_x) * 0.5;
    let cz = (min_z + max_z) * 0.5;
    let span = fmax(max_x - min_x, max_z - min_z);
    let base_y = max_y - 240.0;

    let out = core::slice::from_raw_parts_mut(&raw mut MOUNTAINS as *mut Vertex, MOUNTAIN_VERTS);
    let mut w = 0usize;

    for k in 0..MOUNTAIN_COUNT {
        let angle = (k as f32 / MOUNTAIN_COUNT as f32) * TAU;
        let radius = span * 0.85 + 700.0 + (k % 5) as f32 * 240.0;
        let height = 320.0 + (k % 7) as f32 * 130.0;
        let color = if k % 2 == 0 {
            rgb(0x1B, 0x24, 0x34)
        } else {
            rgb(0x23, 0x2F, 0x42)
        };
        let px = cx + sin(angle) * radius;
        let pz = cz + cos(angle) * radius;
        // A four-sided cone: apex plus a square base, yawed to face the track.
        let base = height * 0.62;
        let apex = Vertex::new(px, base_y + height, pz, color);
        for s in 0..4 {
            let a0 = angle + (s as f32) * (TAU / 4.0);
            let a1 = angle + ((s + 1) as f32) * (TAU / 4.0);
            out[w] = apex;
            out[w + 1] = Vertex::new(px + sin(a0) * base, base_y, pz + cos(a0) * base, color);
            out[w + 2] = Vertex::new(px + sin(a1) * base, base_y, pz + cos(a1) * base, color);
            w += 3;
        }
    }
}

/// Stars and moon.
///
/// Placed on a dome that is translated to the camera every frame rather than drawn in screen
/// space. Screen-locked stars slide across the sky whenever the camera turns, which on a road
/// this twisty is immediately obvious.
const STAR_COUNT: usize = 700;
const SKY_RADIUS: f32 = 1800.0;
const STAR_VERTS: usize = STAR_COUNT * 6 + 12; // stars, plus the moon and its halo
static mut STARFIELD: psp::Align16<[Vertex; STAR_VERTS]> = psp::Align16([Vertex::ZERO; STAR_VERTS]);

unsafe fn build_starfield() {
    let out = core::slice::from_raw_parts_mut(&raw mut STARFIELD as *mut Vertex, STAR_VERTS);
    let mut w = 0usize;
    // Deterministic, so a headless capture of the sky is reproducible.
    let mut rng: u32 = 0x5EED_1234;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 17;
        rng ^= rng << 5;
        (rng >> 8) as f32 / (1 << 24) as f32
    };

    for i in 0..STAR_COUNT {
        let yaw = next() * TAU;
        // Biased toward the upper sky.
        let height = 0.12 + next() * 0.88;
        let ring = sqrt(1.0 - height * height);
        let (cx, cy, cz) = (
            sin(yaw) * ring * SKY_RADIUS,
            height * SKY_RADIUS,
            cos(yaw) * ring * SKY_RADIUS,
        );
        // Three brightnesses, as the palette has it.
        let color = match i % 3 {
            0 => rgb(0xFF, 0xFF, 0xFF),
            1 => rgb(0xCD, 0xDC, 0xF2),
            _ => rgb(0x7F, 0x93, 0xAD),
        };
        let s = 3.0 + next() * 4.0;
        // Billboarded roughly toward the origin by using the ring tangent as the horizontal axis.
        let (tx, tz) = (cos(yaw), -sin(yaw));
        for (a, b) in [
            (-1.0, -1.0),
            (1.0, -1.0),
            (1.0, 1.0),
            (-1.0, -1.0),
            (1.0, 1.0),
            (-1.0, 1.0),
        ] {
            out[w] = Vertex::new(cx + tx * s * a, cy + s * b, cz + tz * s * a, color);
            w += 1;
        }
    }

    // The moon, with a faint halo behind it.
    let yaw = 2.1;
    let height = 0.55;
    let ring = sqrt(1.0 - height * height);
    let (mx, my, mz) = (
        sin(yaw) * ring * SKY_RADIUS,
        height * SKY_RADIUS,
        cos(yaw) * ring * SKY_RADIUS,
    );
    let (tx, tz) = (cos(yaw), -sin(yaw));
    for (radius, color) in [(90.0f32, rgba(0xCF, 0xDA, 0xEA, 0x28)), (34.0, rgb(0xE8, 0xEE, 0xF7))] {
        for (a, b) in [
            (-1.0, -1.0),
            (1.0, -1.0),
            (1.0, 1.0),
            (-1.0, -1.0),
            (1.0, 1.0),
            (-1.0, 1.0),
        ] {
            out[w] = Vertex::new(
                mx + tx * radius * a,
                my + radius * b,
                mz + tz * radius * a,
                color,
            );
            w += 1;
        }
    }
    debug_assert!(w == STAR_VERTS);
}

/// Draws the night sky: a vertical gradient behind everything, then the mountain silhouette.
pub fn draw_sky(camera: &Camera) {
    unsafe {
        #[cfg(feature = "devtools")]
        if DEBUG_MODE == 5 {
            return;
        }
        // The gradient is 2D, so it needs no camera and cannot be occluded by anything.
        sys::sceGuDisable(GuState::DepthTest);
        sys::sceGuDisable(GuState::Texture2D);
        sys::sceGuDisable(GuState::Fog);
        // And no culling: screen space has Y pointing down, so these quads wind the opposite way
        // to everything in the world. With culling left on, the whole gradient is discarded and
        // the background is simply whatever the buffer was cleared to — which looks close enough
        // to a night sky to go unnoticed, until a hole in the scenery shows the same colour.
        sys::sceGuDisable(GuState::CullFace);

        // #05070F at the zenith through to #1A2836 at the horizon.
        #[repr(C)]
        #[derive(Clone, Copy)]
        struct Sky2D {
            color: u32,
            x: f32,
            y: f32,
            z: f32,
        }
        const BANDS: [(f32, u32); 5] = [
            (0.0, rgb(0x05, 0x07, 0x0F)),
            (0.30, rgb(0x0A, 0x10, 0x20)),
            (0.55, rgb(0x15, 0x24, 0x38)),
            (0.78, rgb(0x22, 0x35, 0x4B)),
            (1.0, rgb(0x1A, 0x28, 0x36)),
        ];
        let verts = super::scratch::alloc::<Sky2D>((BANDS.len() - 1) * 6);
        if verts.is_null() {
            return;
        }
        let mut w = 0usize;
        for i in 0..BANDS.len() - 1 {
            let (t0, c0) = BANDS[i];
            let (t1, c1) = BANDS[i + 1];
            let y0 = t0 * 272.0;
            let y1 = t1 * 272.0;
            let quad = [
                Sky2D { color: c0, x: 0.0, y: y0, z: 0.0 },
                Sky2D { color: c0, x: 480.0, y: y0, z: 0.0 },
                Sky2D { color: c1, x: 480.0, y: y1, z: 0.0 },
                Sky2D { color: c0, x: 0.0, y: y0, z: 0.0 },
                Sky2D { color: c1, x: 480.0, y: y1, z: 0.0 },
                Sky2D { color: c1, x: 0.0, y: y1, z: 0.0 },
            ];
            for v in quad {
                *verts.add(w) = v;
                w += 1;
            }
        }
        sys::sceGumDrawArray(
            GuPrimitive::Triangles,
            VertexType::COLOR_8888 | VertexType::VERTEX_32BITF | VertexType::TRANSFORM_2D,
            w as i32,
            core::ptr::null(),
            verts as *const c_void,
        );

        // Everything below the horizon that the scenery does not reach. Before the stars and the
        // mountains, so both still paint over it.
        draw_ground_backdrop(camera);

        // Stars and moon sit on a dome centred on the camera, so they never come closer and
        // never slide as the car turns. Depth writes off, and blended for the moon's halo.
        sys::sceGuEnable(GuState::DepthTest);
        sys::sceGuDepthMask(1);
        sys::sceGuEnable(GuState::Blend);
        sys::sceGuBlendFunc(
            sys::BlendOp::Add,
            sys::BlendFactor::SrcAlpha,
            sys::BlendFactor::OneMinusSrcAlpha,
            0,
            0,
        );
        sys::sceGumMatrixMode(MatrixMode::Model);
        sys::sceGumLoadIdentity();
        sys::sceGumTranslate(&ScePspFVector3 {
            x: camera.pos.x,
            y: camera.pos.y,
            z: camera.pos.z,
        });
        sys::sceGumDrawArray(
            GuPrimitive::Triangles,
            VERTEX_FORMAT,
            STAR_VERTS as i32,
            core::ptr::null(),
            &raw const STARFIELD as *const c_void,
        );
        sys::sceGuDisable(GuState::Blend);
        sys::sceGuEnable(GuState::CullFace);

        // Mountains sit in front of the gradient but behind everything else. Depth writes stay
        // off so the scene proper is never occluded by geometry a kilometre away.
        sys::sceGumLoadIdentity();
        sys::sceGumDrawArray(
            GuPrimitive::Triangles,
            VERTEX_FORMAT,
            MOUNTAIN_VERTS as i32,
            core::ptr::null(),
            &raw const MOUNTAINS as *const c_void,
        );

        sys::sceGuDepthMask(0);
        sys::sceGuEnable(GuState::Fog);
    }
}

/// Everything below the horizon that the scenery does not reach, in the colour distance already is.
///
/// The terrain ribbon is a strip 190 m to either side of the road, and on a pass with the ground
/// falling away it is possible to look past its edge — not because the ribbon is too small, but
/// because a shallow sight line and a 0.4-per-metre slope diverge. Behind it sat the sky gradient,
/// several times brighter than ground the fog has taken almost to [`SKY_CLEAR`], so the gap read as
/// a hard blue wedge with the hillside stopping dead against it rather than as distance.
///
/// A disc in [`FOG_COLOR`] settles it for good, for a fixed two dozen triangles. Chasing the same
/// result with geometry meant a ribbon six times wider and 68% more vertices in every frame of the
/// game — to cover ground nobody can make out, at a distance where fog has flattened it to one flat
/// colour anyway.
///
/// It is drawn in the sky pass, which writes no depth at all, so this cannot occlude anything: the
/// world is drawn afterwards and paints straight over it. That also means it needs no depth test of
/// its own — painter's order does the work, and the depth buffer is cleared to 0 here, which a disc
/// two kilometres out could not be relied on to pass.
///
/// The disc has to sit *below* the eye. A plane through the camera projects to a line, not an area,
/// which is worth knowing before spending a build wondering why nothing changed. A metre is plenty:
/// at this radius it puts the rim within a twentieth of a degree of the true horizon.
unsafe fn draw_ground_backdrop(camera: &Camera) {
    // Just inside the far plane, so it is behind every piece of real scenery without being clipped.
    const R: f32 = DRAW_DISTANCE * 0.9;
    const DROP: f32 = 1.0;
    const SEGMENTS: usize = 24;

    let verts = super::scratch::alloc::<Vertex>(SEGMENTS * 3);
    if verts.is_null() {
        return;
    }
    let (cx, cy, cz) = (camera.pos.x, camera.pos.y - DROP, camera.pos.z);
    let rim = |k: usize| {
        let a = (k % SEGMENTS) as f32 / SEGMENTS as f32 * TAU;
        Vertex::new(cx + cos(a) * R, cy, cz + sin(a) * R, FOG_COLOR)
    };
    let mut w = 0usize;
    for k in 0..SEGMENTS {
        *verts.add(w) = Vertex::new(cx, cy, cz, FOG_COLOR);
        *verts.add(w + 1) = rim(k);
        *verts.add(w + 2) = rim(k + 1);
        w += 3;
    }
    // Culling and the depth test are already off for the gradient, and the fan is seen from below.
    sys::sceGumMatrixMode(MatrixMode::Model);
    sys::sceGumLoadIdentity();
    sys::sceGumDrawArray(
        GuPrimitive::Triangles,
        VERTEX_FORMAT,
        w as i32,
        core::ptr::null(),
        verts as *const c_void,
    );
}

/// Sets the projection and view matrices for this frame.
pub fn set_camera(camera: &Camera) {
    unsafe {
        sys::sceGumMatrixMode(MatrixMode::Projection);
        sys::sceGumLoadIdentity();
        // 480x272 is 16:9. Far is deliberately short — fog hides everything past
        // 330 m anyway, and a tighter range buys depth precision.
        sys::sceGumPerspective(camera.fov, 16.0 / 9.0, 0.4, DRAW_DISTANCE);

        sys::sceGumMatrixMode(MatrixMode::View);
        // Deliberately not `sceGumLookAt`: in rust-psp 0.3.13 it is a no-op, because its
        // `gum_look_at` helper shadows its own `&mut` output parameter with a local and never
        // writes back. The result is an identity view matrix, which renders a world that looks
        // superficially fine but is positioned as though the camera were at the world origin.
        let view = Mat4::look_at(camera.pos, camera.look_at, Vec3::new(0.0, 1.0, 0.0));
        sys::sceGumLoadMatrix(&*(view.columns().as_ptr() as *const ScePspFMatrix4));

        sys::sceGumMatrixMode(MatrixMode::Model);
        sys::sceGumLoadIdentity();
    }
}

/// Chunks behind the camera, or past the projection's far plane, contribute nothing.
///
/// `forward` must be the real view direction, not the chase heading. The camera sits several
/// metres above the road looking down at it, so the near plane is pitched; ground that is behind
/// the camera *horizontally* can still be well inside the frustum.
///
/// The far limit is the fog distance. Because the test compares `distance - radius`, and a
/// chunk's bounding sphere grows with how much the track curves through it, a nearer chunk can be
/// culled while a farther one is kept — an index gap in the drawn set. That is real, and visible
/// in a trace, but harmless: anything culled by this test has every vertex beyond 330 m, where
/// fog has already taken it to within a shade of the sky.
///
/// Raising the limit to the far plane closes those gaps and costs five times the vertices —
/// 93k a frame against 18k — for geometry nobody can see. Not worth it. The fault this was
/// briefly suspected of causing turned out to be the ribbon simply ending; see `mesh::APRON_NODES`.
fn visible(chunk: &Chunk, eye: Vec3, forward: Vec3) -> bool {
    mesh::chunk_visible(chunk, eye, forward, FOG_FAR)
}

/// Per-frame tally of what actually got submitted to the GE.
///
/// Cheap enough to keep in every build so the two behave identically, but only read under
/// `devtools`. It exists to answer one question that a screenshot cannot: when part of the world
/// vanishes, was the draw call issued at all?
#[derive(Clone, Copy, Debug, Default)]
pub struct DrawStats {
    pub road: u16,
    pub terrain: u16,
    pub lines: u16,
    pub rails: u16,
    pub dashes: u16,
    pub props: u16,
    pub verts: u32,
    /// One bit per chunk index actually submitted. There are 28 chunks, so a u32 holds the lot.
    ///
    /// The counts alone have a blind spot: a chunk missing from the *middle* of the visible run
    /// leaves a gap across the screen while the count merely dips by one, which reads as normal
    /// variation. A hole between set bits is unambiguous.
    pub road_mask: u32,
    pub terrain_mask: u32,
    /// The same, for the two passes that still decide chunk by chunk. Worth its own bits: these
    /// carry the light pools, and a pool blinking out is the artifact that reads as the road
    /// surface itself flickering.
    pub prop_mask: u32,
    pub glow_mask: u32,
}

static mut STATS: DrawStats = DrawStats {
    road: 0,
    terrain: 0,
    lines: 0,
    rails: 0,
    dashes: 0,
    props: 0,
    verts: 0,
    road_mask: 0,
    terrain_mask: 0,
    prop_mask: 0,
    glow_mask: 0,
};
/// Which counter `draw_ribbon` should attribute its chunks to.
static mut STATS_SLOT: u8 = 0;

/// Snapshot and clear the tally. Call once per frame, after drawing.
pub fn take_stats() -> DrawStats {
    unsafe {
        let s = STATS;
        STATS = DrawStats::default();
        s
    }
}

unsafe fn tally_index(slot: u8, index: usize) {
    match slot {
        0 => STATS.road_mask |= 1 << (index & 31),
        1 => STATS.terrain_mask |= 1 << (index & 31),
        _ => {}
    }
}

unsafe fn tally(slot: u8, verts: u32) {
    // Saturating, so a build that somehow stopped clearing these cannot wrap or trap.
    match slot {
        0 => STATS.road = STATS.road.saturating_add(1),
        1 => STATS.terrain = STATS.terrain.saturating_add(1),
        2 => STATS.lines = STATS.lines.saturating_add(1),
        3 => STATS.rails = STATS.rails.saturating_add(1),
        4 => STATS.dashes = STATS.dashes.saturating_add(1),
        _ => STATS.props = STATS.props.saturating_add(1),
    }
    STATS.verts = STATS.verts.saturating_add(verts);
}

/// First and last chunk that survives culling, or `None` if the ribbon is entirely off screen.
///
/// Chunks run in order along the track, so what the player can see is a contiguous run of them.
/// The per-chunk test does not produce one: it compares `distance - radius`, and a chunk's
/// bounding sphere grows with how much the track curves through it, so a nearer chunk can be
/// rejected while a farther one is kept. That leaves a chunk-shaped hole with scenery on both
/// sides of it.
///
/// Drawing the whole span between the first and last survivor is hole-free by construction, and
/// costs only the handful of chunks it fills in.
fn visible_span<const V: usize>(r: &Ribbon<V>, eye: Vec3, forward: Vec3) -> Option<(usize, usize)> {
    let mut lo = None;
    let mut hi = 0usize;
    for (index, chunk) in r.chunks.iter().enumerate() {
        if visible(chunk, eye, forward) {
            if lo.is_none() {
                lo = Some(index);
            }
            hi = index;
        }
    }
    lo.map(|l| (l, hi))
}

/// Draws a ribbon over a span decided once for the whole world.
///
/// Every ribbon used to cull itself, and `chunk_visible` scales its threshold by the chunk's
/// bounding radius. A terrain chunk reaches 190 m to either side of the centreline, so its sphere
/// has a radius of a couple of hundred metres; a road chunk is twelve metres wide and has a radius
/// of forty-odd. The terrain therefore survived culling in places the road did not, and the
/// hillside was drawn over ground the tarmac should have covered — a hard-edged wedge of grass
/// lying across the road, worst on the title screen where the camera swings low and wide.
/// `tests/ribbon_spans.rs` pins the two together.
fn draw_ribbon<const V: usize>(r: &Ribbon<V>, span: (usize, usize)) {
    unsafe {
        let (lo, hi) = span;
        for (index, chunk) in r.chunks.iter().enumerate() {
            if index < lo || index > hi || chunk.count == 0 {
                continue;
            }
            tally(STATS_SLOT, chunk.count);
            tally_index(STATS_SLOT, index);
            sys::sceGumDrawArray(
                GuPrimitive::TriangleStrip,
                VERTEX_FORMAT,
                chunk.count as i32,
                core::ptr::null(),
                r.verts.as_ptr().add(chunk.start as usize) as *const c_void,
            );
        }
    }
}

/// Draws the world. The car is drawn separately so it can carry its own transform.
pub fn draw_world(camera: &Camera) {
    #[cfg(feature = "devtools")]
    unsafe {
        match DEBUG_MODE {
            1 => sys::sceGuDisable(GuState::CullFace),
            2 => sys::sceGuDisable(GuState::DepthTest),
            3 => sys::sceGuDisable(GuState::Fog),
            4 => {
                sys::sceGuDisable(GuState::CullFace);
                sys::sceGuDisable(GuState::DepthTest);
                sys::sceGuDisable(GuState::Fog);
            }
            _ => {}
        }
    }
    let eye = camera.pos;
    // Where the camera actually points, including its downward tilt onto the road.
    let forward = camera.look_at.sub(eye).normalized();

    // One decision for the whole world, taken from the terrain because it is the widest ribbon
    // and so the most generous. Everything else sits on it and must be drawn wherever it is.
    let Some(span) = visible_span(unsafe { &*(&raw const TERRAIN_MESH) }, eye, forward) else {
        return;
    };

    unsafe {
        sys::sceGumMatrixMode(MatrixMode::Model);
        sys::sceGumLoadIdentity();

        // The terrain ribbon reaches 190 m to either side, which is wider than the radius of
        // this track's hairpins — so on the inside of a tight corner it folds over itself and
        // those triangles come out wound the other way. There is no single winding that is
        // correct for all of it, so it is drawn double-sided. Culling it leaves holes you can
        // see the sky through, on the inside of corners.
        sys::sceGuDisable(GuState::CullFace);
        STATS_SLOT = 1;
        // Mode 6 leaves the terrain out, so what remains is only the road.
        #[cfg(feature = "devtools")]
        let skip_terrain = DEBUG_MODE == 6;
        #[cfg(not(feature = "devtools"))]
        let skip_terrain = false;
        if !skip_terrain {
            draw_ribbon(&*(&raw const TERRAIN_MESH), span);
        }
        sys::sceGuEnable(GuState::CullFace);
        STATS_SLOT = 0;
        // Mode 7 is the converse: terrain only.
        #[cfg(feature = "devtools")]
        let skip_road = DEBUG_MODE == 7;
        #[cfg(not(feature = "devtools"))]
        let skip_road = false;
        if !skip_road {
            draw_ribbon(&*(&raw const ROAD_MESH), span);
        }

        // Markings sit fractions of a metre above the road; draw them after so they win ties.
        STATS_SLOT = 2;
        draw_ribbon(&*(&raw const EDGE_L_MESH), span);
        draw_ribbon(&*(&raw const EDGE_R_MESH), span);

        let dash_verts = &raw const DASH_MESH as *const Vertex;
        let dash_chunks = &*(&raw const DASH_CHUNKS);
        for (index, chunk) in dash_chunks.iter().enumerate() {
            if index < span.0 || index > span.1 || chunk.count == 0 {
                continue;
            }
            tally(4, chunk.count);
            sys::sceGumDrawArray(
                GuPrimitive::Triangles,
                VERTEX_FORMAT,
                chunk.count as i32,
                core::ptr::null(),
                dash_verts.add(chunk.start as usize) as *const c_void,
            );
        }

        // The rails are double-sided. A two-station ribbon standing vertically has
        // one winding, so with culling on it disappears when seen from behind — which for the
        // left-hand rail is nearly always, since the chase camera sits inside the road.
        sys::sceGuDisable(GuState::CullFace);
        STATS_SLOT = 3;
        draw_ribbon(&*(&raw const RAIL_L_MESH), span);
        draw_ribbon(&*(&raw const RAIL_R_MESH), span);

        // Trees are crossed quads with no single facing, so they must not be back-face culled
        // either; culling stays off through to the end of the pass.
        let prop_verts = &raw const PROP_MESH as *const Vertex;
        let prop_chunks = &*(&raw const PROP_CHUNKS);
        for (index, chunk) in prop_chunks.iter().enumerate() {
            if !visible(chunk, eye, forward) {
                continue;
            }
            tally(5, chunk.count);
            STATS.prop_mask |= 1 << (index & 31);
            sys::sceGumDrawArray(
                GuPrimitive::Triangles,
                VERTEX_FORMAT,
                chunk.count as i32,
                core::ptr::null(),
                prop_verts.add(chunk.start as usize) as *const c_void,
            );
        }
        sys::sceGuEnable(GuState::CullFace);

        // Light pools and lamp glows, added on top of the world they fall on. Depth-tested so a
        // pool is hidden by a hill in front of it, but never written, so nothing behind is lost.
        //
        // A ground pool is a flat disc and the road it lies on is a strip of flat facets, so 4 cm of
        // clearance is not a margin at all: over most of the disc the two surfaces are within a
        // depth step of each other, and which one wins is decided facet by facet. Driving past, the
        // lit area marches down the screen and snaps back once per centreline node — the road
        // appearing to flicker under the car. Biasing the pass toward the camera settles it without
        // moving any geometry, and unlike raising the disc it works just as well at the far end of
        // the track, where a 16-bit depth buffer has far coarser steps than 4 cm.
        sys::sceGuDepthOffset(POOL_DEPTH_BIAS);
        sys::sceGuEnable(GuState::Blend);
        sys::sceGuBlendFunc(
            sys::BlendOp::Add,
            sys::BlendFactor::SrcAlpha,
            sys::BlendFactor::Fix,
            0,
            0xffff_ffff,
        );
        sys::sceGuDepthMask(1);
        sys::sceGuDisable(GuState::CullFace);

        let glow_verts = &raw const PROP_GLOW_MESH as *const Vertex;
        // Mode 12 drops the roadside pools. They are the only warm light on the road that is not in
        // one of the passes above, so removing them is the last step of narrowing down something
        // bright and blinking down there.
        #[cfg(feature = "devtools")]
        let skip_glows = DEBUG_MODE == 12;
        #[cfg(not(feature = "devtools"))]
        let skip_glows = false;

        let glow_chunks = &*(&raw const PROP_GLOW_CHUNKS);
        for (index, chunk) in glow_chunks.iter().enumerate() {
            if skip_glows || !visible(chunk, eye, forward) {
                continue;
            }
            STATS.glow_mask |= 1 << (index & 31);
            sys::sceGumDrawArray(
                GuPrimitive::Triangles,
                VERTEX_FORMAT,
                chunk.count as i32,
                core::ptr::null(),
                glow_verts.add(chunk.start as usize) as *const c_void,
            );
        }

        sys::sceGuEnable(GuState::CullFace);
        sys::sceGuDepthMask(0);
        sys::sceGuDisable(GuState::Blend);
        // Back to no bias: this is context state, not a per-draw flag, and everything after it —
        // the car, the effects, the next frame's world — would otherwise inherit it.
        sys::sceGuDepthOffset(0);
    }
}

/// Draws the car, wheels included, at its current pose and attitude.
///
/// Nothing here knows what car it is drawing. The meshes, their materials, how many wheels there
/// are, where the hubs sit and which of them steer all come out of the compiled asset, so a second
/// car is a second file and no code at all.
///
/// Two passes rather than one, in the order the depth buffer needs: everything opaque, then
/// everything that blends. Glass drawn before the seats behind it would blend against the sky.
pub fn draw_car(vehicle: &Vehicle, track: &Track) {
    let Some(car) = super::car::get(vehicle.model) else {
        return;
    };
    let st: &CarState = &vehicle.state;

    unsafe {
        sys::sceGumMatrixMode(MatrixMode::Model);
        sys::sceGumLoadIdentity();
        sys::sceGumTranslate(&ScePspFVector3 {
            x: st.x,
            y: st.y,
            z: st.z,
        });
        sys::sceGumRotateY(st.yaw);
        // Follow the road's slope, but only insofar as the car points along it.
        sys::sceGumRotateX(vehicle.body_pitch(track));
        sys::sceGumRotateZ(vehicle.roll());

        for blended in [false, true] {
            if blended {
                sys::sceGuEnable(GuState::Blend);
                sys::sceGuBlendFunc(
                    sys::BlendOp::Add,
                    sys::BlendFactor::SrcAlpha,
                    sys::BlendFactor::OneMinusSrcAlpha,
                    0,
                    0,
                );
            }
            let mut culling = true;

            for i in 0..car.mesh_count() {
                let mesh = car.mesh(i);
                let material = car.material(mesh.material as usize);
                if material.blended() != blended {
                    continue;
                }
                // Glass, seats and door cards are modelled as single sheets with nothing behind
                // them, and culled they show as holes into the cabin.
                let want_culling = !material.two_sided();
                if want_culling != culling {
                    if want_culling {
                        sys::sceGuEnable(GuState::CullFace);
                    } else {
                        sys::sceGuDisable(GuState::CullFace);
                    }
                    culling = want_culling;
                }

                if mesh.wheel == azcar::NO_WHEEL {
                    draw_car_mesh(car, &mesh);
                    continue;
                }

                // A wheel's geometry is stored about its own hub, so it can be put where it
                // belongs and then turned, rather than being turned about the car's origin.
                let wheel = car.wheel(mesh.wheel as usize);
                sys::sceGumPushMatrix();
                sys::sceGumTranslate(&ScePspFVector3 {
                    x: wheel.hub[0],
                    y: wheel.hub[1],
                    z: wheel.hub[2],
                });
                if wheel.steers {
                    sys::sceGumRotateY(st.steer);
                }
                sys::sceGumRotateX(st.wheel_spin);
                draw_car_mesh(car, &mesh);
                sys::sceGumPopMatrix();
            }

            if !culling {
                sys::sceGuEnable(GuState::CullFace);
            }
            if blended {
                sys::sceGuDisable(GuState::Blend);
            }
        }
    }
}

/// One indexed run out of the car's buffers.
///
/// Both pointers are into the arena the file was read into: the vertices are already in the GE's
/// own layout and the indices are already 16-bit, so there is nothing between the file on the
/// memory stick and the hardware.
unsafe fn draw_car_mesh(car: &azcar::Car<'static>, mesh: &azcar::Mesh) {
    sys::sceGumDrawArray(
        GuPrimitive::Triangles,
        CAR_VERTEX_FORMAT,
        mesh.index_count as i32,
        car.indices_ptr().add(mesh.first_index as usize * 2) as *const c_void,
        car.vertices_ptr() as *const c_void,
    );
}

/// Skid marks and tyre smoke.
///
/// Both come out of fixed pools in the core, so the worst case is known up front and the whole
/// lot fits in one draw call each.
pub fn draw_effects(effects: &Effects, camera: &Camera) {
    // Mode 11 drops them.
    #[cfg(feature = "devtools")]
    if debug_mode() == 11 {
        return;
    }
    unsafe {
        sys::sceGumMatrixMode(MatrixMode::Model);
        sys::sceGumLoadIdentity();

        // --- skid marks: dark quads laid flat on the road ---
        sys::sceGuEnable(GuState::Blend);
        sys::sceGuBlendFunc(
            sys::BlendOp::Add,
            sys::BlendFactor::SrcAlpha,
            sys::BlendFactor::OneMinusSrcAlpha,
            0,
            0,
        );
        // Marks sit fractions of a metre above the road and must not fight it for depth.
        sys::sceGuDepthMask(1);
        sys::sceGuDisable(GuState::CullFace);

        let live = effects.skids().iter().filter(|s| s.active).count();
        if live > 0 {
            if let Some(verts) = alloc_verts(live * 6) {
                let mut w = 0usize;
                for s in effects.skids().iter().filter(|s| s.active) {
                    let (sy, cy) = (sin(s.yaw), cos(s.yaw));
                    // 0.3 x 1.1 m, stretched lengthwise with speed.
                    let (hw, hl) = (0.15, 0.55 * s.stretch);
                    let corner = |a: f32, b: f32| {
                        Vertex::new(
                            s.x + a * hw * cy + b * hl * sy,
                            s.y,
                            s.z - a * hw * sy + b * hl * cy,
                            rgba(0x08, 0x08, 0x0A, 0x8C),
                        )
                    };
                    for v in [
                        corner(-1.0, -1.0),
                        corner(1.0, -1.0),
                        corner(1.0, 1.0),
                        corner(-1.0, -1.0),
                        corner(1.0, 1.0),
                        corner(-1.0, 1.0),
                    ] {
                        *verts.add(w) = v;
                        w += 1;
                    }
                }
                sys::sceGumDrawArray(
                    GuPrimitive::Triangles,
                    VERTEX_FORMAT,
                    w as i32,
                    core::ptr::null(),
                    verts as *const c_void,
                );
            }
        }

        // --- smoke: additive billboards facing the camera ---
        let puffs = effects.puffs().iter().filter(|p| p.life > 0.0).count();
        if puffs > 0 {
            sys::sceGuBlendFunc(
                sys::BlendOp::Add,
                sys::BlendFactor::SrcAlpha,
                sys::BlendFactor::Fix,
                0,
                0xffff_ffff,
            );
            sys::sceGuDisable(GuState::Fog);
            // Face the camera, so a puff never collapses to an edge-on sliver.
            let (right_x, right_z) = (cos(camera.yaw), -sin(camera.yaw));

            if let Some(verts) = alloc_verts(puffs * GLOW_VERTS) {
                let mut w = 0usize;
                for p in effects.puffs().iter().filter(|p| p.life > 0.0) {
                    let r = p.scale() * 0.5;
                    let a = (p.alpha() * 255.0) as u32;
                    let color = rgba(0xC8, 0xCE, 0xD8, a);
                    // Soft-edged, for the same reason the lamp glows are: a flat additive quad
                    // reads as a grey card rather than a puff.
                    push_glow(verts, &mut w, p.x, p.y, p.z, (right_x, right_z), r, r, color);
                }
                sys::sceGumDrawArray(
                    GuPrimitive::Triangles,
                    VERTEX_FORMAT,
                    w as i32,
                    core::ptr::null(),
                    verts as *const c_void,
                );
            }
            sys::sceGuEnable(GuState::Fog);
        }

        sys::sceGuEnable(GuState::CullFace);
        sys::sceGuDepthMask(0);
        sys::sceGuDisable(GuState::Blend);
    }
}

/// Segments in a radial glow sprite.
const GLOW_SEGMENTS: usize = 8;
/// Vertices one glow costs, as a triangle fan expanded into separate triangles.
const GLOW_VERTS: usize = GLOW_SEGMENTS * 3;

/// Writes a camera-facing radial glow: bright in the middle, fading to nothing at the rim.
///
/// Gradient sprites would be the obvious way to do these. A flat quad of the same size and opacity
/// saturates instead of glowing — two braking tail lamps at 0.95 alpha turn the whole car into a
/// red slab — so the falloff is built into the vertex colours.
#[allow(clippy::too_many_arguments)]
unsafe fn push_glow(
    verts: *mut Vertex,
    w: &mut usize,
    cx: f32,
    cy: f32,
    cz: f32,
    right: (f32, f32),
    half_w: f32,
    half_h: f32,
    color: u32,
) {
    let core = color;
    let rim = color & 0x00ff_ffff; // same hue, zero alpha
    let centre = Vertex::new(cx, cy, cz, core);
    let edge = |k: usize| {
        let a = (k % GLOW_SEGMENTS) as f32 / GLOW_SEGMENTS as f32 * TAU;
        Vertex::new(
            cx + right.0 * cos(a) * half_w,
            cy + sin(a) * half_h,
            cz + right.1 * cos(a) * half_w,
            rim,
        )
    };
    for k in 0..GLOW_SEGMENTS {
        *verts.add(*w) = centre;
        *verts.add(*w + 1) = edge(k);
        *verts.add(*w + 2) = edge(k + 1);
        *w += 3;
    }
}

/// Vertices a flat ground pool costs.
const GROUND_GLOW_VERTS: usize = GLOW_SEGMENTS * 3;

/// A pool of light lying on the ground, fading out from the middle. Flat, so unlike the lamp
/// billboards it needs no camera and can be baked once.
unsafe fn push_ground_glow(
    out: &mut [Vertex],
    w: &mut usize,
    cx: f32,
    cy: f32,
    cz: f32,
    radius: f32,
    color: u32,
) {
    let rim = color & 0x00ff_ffff;
    let centre = Vertex::new(cx, cy, cz, color);
    let edge = |k: usize| {
        let a = (k % GLOW_SEGMENTS) as f32 / GLOW_SEGMENTS as f32 * TAU;
        Vertex::new(cx + cos(a) * radius, cy, cz + sin(a) * radius, rim)
    };
    for k in 0..GLOW_SEGMENTS {
        out[*w] = centre;
        out[*w + 1] = edge(k);
        out[*w + 2] = edge(k + 1);
        *w += 3;
    }
}

/// A ground pool laid on the pull-off's paving rather than on a horizontal plane.
///
/// [`push_ground_glow`] puts every vertex at one height, which is right on the flat but wrong here:
/// the pass falls 7.4 cm per metre, so a 12 m disc pinned to a single node is buried nearly a metre
/// at its uphill rim and hangs well over a metre in the air at its downhill one. Where it crosses
/// the paving the depth comparison is marginal, and the crossing line crawls and shimmers as the
/// camera orbits — the flicker on the tarmac.
///
/// Laying it out in the pull-off's own `(along, lateral)` frame fixes both halves of that at once:
/// every vertex takes its height from the node it actually stands above, exactly as the paving
/// does, and the disc follows the road's curve instead of running straight off it.
#[allow(clippy::too_many_arguments)]
unsafe fn push_bay_pool(
    track: &Track,
    out: &mut [Vertex],
    w: &mut usize,
    along: f32,
    lateral: f32,
    radius: f32,
    lift: f32,
    color: u32,
) {
    use angle_zero::track::bay_surface;
    let rim = color & 0x00ff_ffff;
    let c = bay_surface(track, along, lateral);
    let centre = Vertex::new(c.x, c.y + lift, c.z, color);
    let edge = |k: usize| {
        let a = (k % GLOW_SEGMENTS) as f32 / GLOW_SEGMENTS as f32 * TAU;
        let p = bay_surface(track, along + cos(a) * radius, lateral + sin(a) * radius);
        Vertex::new(p.x, p.y + lift, p.z, rim)
    };
    for k in 0..GLOW_SEGMENTS {
        out[*w] = centre;
        out[*w + 1] = edge(k);
        out[*w + 2] = edge(k + 1);
        *w += 3;
    }
}

/// A glow around a lamp head, built as two crossed vertical fans. Baked rather than billboarded:
/// these are fixed to the scenery, and a crossed pair reads from any angle without needing to be
/// rebuilt every frame for every lamp on the track.
unsafe fn push_blob_glow(
    out: &mut [Vertex],
    w: &mut usize,
    cx: f32,
    cy: f32,
    cz: f32,
    radius: f32,
    color: u32,
) {
    let rim = color & 0x00ff_ffff;
    let centre = Vertex::new(cx, cy, cz, color);
    for axis in 0..2 {
        let (ax, az) = if axis == 0 { (1.0, 0.0) } else { (0.0, 1.0) };
        let edge = |k: usize| {
            let a = (k % GLOW_SEGMENTS) as f32 / GLOW_SEGMENTS as f32 * TAU;
            Vertex::new(
                cx + ax * cos(a) * radius,
                cy + sin(a) * radius,
                cz + az * cos(a) * radius,
                rim,
            )
        };
        for k in 0..GLOW_SEGMENTS {
            out[*w] = centre;
            out[*w + 1] = edge(k);
            out[*w + 2] = edge(k + 1);
            *w += 3;
        }
    }
}

/// Frame-lived vertex storage, or `None` when the arena is exhausted.
unsafe fn alloc_verts(n: usize) -> Option<*mut Vertex> {
    let p = super::scratch::alloc::<Vertex>(n);
    if p.is_null() {
        None
    } else {
        Some(p)
    }
}

/// Additive glows around the head and tail lamps. The tail lamps jump in size and
/// brightness under braking, which is most of what tells you what the car ahead is doing.
pub fn draw_lamp_glows(st: &CarState, camera: &Camera, braking: bool) {
    // Mode 10 drops them.
    #[cfg(feature = "devtools")]
    if debug_mode() == 10 {
        return;
    }
    unsafe {
        sys::sceGuEnable(GuState::Blend);
        sys::sceGuBlendFunc(
            sys::BlendOp::Add,
            sys::BlendFactor::SrcAlpha,
            sys::BlendFactor::Fix,
            0,
            0xffff_ffff,
        );
        sys::sceGuDepthMask(1);
        sys::sceGuDisable(GuState::CullFace);
        sys::sceGuDisable(GuState::Fog);
        sys::sceGumMatrixMode(MatrixMode::Model);
        sys::sceGumLoadIdentity();

        let (right_x, right_z) = (cos(camera.yaw), -sin(camera.yaw));
        let (s, c) = (sin(st.yaw), cos(st.yaw));
        // Local (x, z) offset of a lamp, into world space.
        let place = |lx: f32, lz: f32| (st.x + lx * c + lz * s, st.z - lx * s + lz * c);

        let tail_alpha = if braking { 0xF2 } else { 0x66 };
        let (tail_w, tail_h) = if braking { (0.95, 0.65) } else { (0.60, 0.40) };

        // A lamp glow is a billboard with no depth of its own, so without this the headlamps
        // shine straight through the car whenever the camera is behind it — which, with a chase
        // camera, is almost always. Show each pair only from the side it actually faces.
        let to_camera_x = camera.pos.x - st.x;
        let to_camera_z = camera.pos.z - st.z;
        let facing = to_camera_x * s + to_camera_z * c;
        let show_head = facing > 0.0;
        let show_tail = facing < 0.0;

        let lamps: [(f32, f32, f32, f32, f32, u32, bool); 4] = [
            (-0.60, 2.09, 1.00, 0.85, 0.60, rgba(0xFF, 0xF3, 0xD2, 0x59), show_head),
            (0.60, 2.09, 1.00, 0.85, 0.60, rgba(0xFF, 0xF3, 0xD2, 0x59), show_head),
            (-0.64, -2.02, 1.00, tail_w, tail_h, rgba(0xFF, 0x55, 0x44, tail_alpha), show_tail),
            (0.64, -2.02, 1.00, tail_w, tail_h, rgba(0xFF, 0x55, 0x44, tail_alpha), show_tail),
        ];

        if let Some(verts) = alloc_verts(lamps.len() * GLOW_VERTS) {
            let mut w = 0usize;
            for (lx, lz, ly, hw, hh, color, visible) in lamps {
                if !visible {
                    continue;
                }
                let (px, pz) = place(lx, lz);
                push_glow(
                    verts,
                    &mut w,
                    px,
                    st.y + ly,
                    pz,
                    (right_x, right_z),
                    hw,
                    hh,
                    color,
                );
            }
            sys::sceGumDrawArray(
                GuPrimitive::Triangles,
                VERTEX_FORMAT,
                w as i32,
                core::ptr::null(),
                verts as *const c_void,
            );
        }

        sys::sceGuEnable(GuState::Fog);
        sys::sceGuEnable(GuState::CullFace);
        sys::sceGuDepthMask(0);
        sys::sceGuDisable(GuState::Blend);
    }
}

/// The two additive ground beams that stand in for a real headlight spot.
pub fn draw_headlight_beams(st: &CarState) {
    // Mode 9 drops them. The additive passes all land on the road in front of the car, on top of
    // each other, so telling which one is responsible for something there means removing them one
    // at a time.
    #[cfg(feature = "devtools")]
    if debug_mode() == 9 {
        return;
    }
    unsafe {
        sys::sceGuEnable(GuState::Blend);
        sys::sceGuBlendFunc(
            sys::BlendOp::Add,
            sys::BlendFactor::SrcAlpha,
            sys::BlendFactor::Fix,
            0,
            0xffff_ffff,
        );
        sys::sceGuDepthMask(1); // no depth write for the glow
        sys::sceGuDisable(GuState::Fog);
        // These quads are wound right-then-forward, which faces away from a camera looking down at
        // the road, so with culling on they were thrown away and the headlights lit nothing at all.
        // Every other additive ground pass here draws double-sided for the same reason: a flat quad
        // has one winding and the camera can be on either side of it.
        sys::sceGuDisable(GuState::CullFace);
        sys::sceGumMatrixMode(MatrixMode::Model);
        sys::sceGumLoadIdentity();
        // Clearance measured against what is actually under here rather than guessed: the road is
        // crowned, so `ROAD_STATIONS` puts its centre 3 cm above the centreline the car's `y`
        // follows, and the edge-line ribbons sit at 5 cm. The old 4 cm left the beam *below* the
        // paint and one centimetre off the tarmac — survivable in the software rasteriser, but
        // hardware has a real 16-bit depth buffer, which is where that kind of margin stops holding.
        sys::sceGumTranslate(&ScePspFVector3 {
            x: st.x,
            y: st.y + 0.08,
            z: st.z,
        });
        // The beam is bolted to the body, so it points where the car points. It used to be swung by
        // `steer` as well, which read as the lamps tracking the front wheels — they are fixed units
        // behind a fixed grille, and a car that is sliding should have its beams pointing where its
        // nose is, not where its tyres are.
        sys::sceGumRotateY(st.yaw);

        // Two 3.6 x 22 m quads, bright at the car and fading out ahead.
        let near = rgba(0xFF, 0xF3, 0xD2, 0x50);
        let far = rgba(0xFF, 0xF3, 0xD2, 0x00);
        let quad = super::scratch::alloc::<Vertex>(12);
        if quad.is_null() {
            return;
        }
        let mut w = 0;
        for side in [-1.0f32, 1.0] {
            let cx = side * 0.56;
            let (l, r) = (cx - 1.8, cx + 1.8);
            let (z0, z1) = (2.2, 24.2);
            let corners = [
                Vertex::new(l, 0.0, z0, near),
                Vertex::new(r, 0.0, z0, near),
                Vertex::new(r, 0.0, z1, far),
                Vertex::new(l, 0.0, z0, near),
                Vertex::new(r, 0.0, z1, far),
                Vertex::new(l, 0.0, z1, far),
            ];
            for c in corners {
                *quad.add(w) = c;
                w += 1;
            }
        }
        sys::sceGumDrawArray(
            GuPrimitive::Triangles,
            VERTEX_FORMAT,
            12,
            core::ptr::null(),
            quad as *const c_void,
        );

        sys::sceGuDepthMask(0);
        sys::sceGuDisable(GuState::Blend);
        sys::sceGuEnable(GuState::Fog);
        sys::sceGuEnable(GuState::CullFace);
    }
}

#[inline]
fn fmin(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}

#[inline]
fn fmax(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}
