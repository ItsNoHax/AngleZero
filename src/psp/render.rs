//! The 3D scene: the road, the night look and the car.
//!
//! All geometry is generated once at boot and lives in statics; nothing is allocated per frame.
//! Chunks outside fog range or behind the camera are skipped, which is what keeps a 3.5 km track
//! affordable.
//!
//! Lighting is baked into vertex colours rather than computed by the GE: the night
//! look is a fixed hemisphere plus moon term, so there is nothing to recompute per frame.

use core::ffi::c_void;

use angle_zero::camera::Camera;
use angle_zero::math::{cos, sin, Mat4, Vec3, TAU};
use angle_zero::mesh::{self, ribbon_capacity, Chunk, Ribbon, Station, Vertex};
use angle_zero::track::{Track, BAY_FROM, BAY_SIDE, BAY_TO};
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

const ROAD_COLOR: u32 = rgb(0x1A, 0x1C, 0x20);
const EDGE_COLOR: u32 = rgb(0xA9, 0xA2, 0x93);
const RAIL_COLOR: u32 = rgb(0x7C, 0x84, 0x8C);
const DASH_COLOR: u32 = rgb(0x8C, 0x7A, 0x45);
const PAINT: u32 = rgb(0x1F, 0x4F, 0xA8);
const GLASS: u32 = rgb(0x17, 0x1D, 0x24);
const TRIM: u32 = rgb(0x14, 0x17, 0x1B);
const CHROME: u32 = rgb(0x9A, 0xA4, 0xAD);
const HEADLAMP: u32 = rgb(0xFF, 0xF6, 0xDC);
const TAILLAMP: u32 = rgb(0xFF, 0x4A, 0x3C);
const TYRE: u32 = rgb(0x18, 0x18, 0x1C);
const RIM: u32 = rgb(0x9A, 0xA4, 0xAD);

/// The hillside, shaded darker as it falls away so the slope reads at night.
const fn terrain(lateral: f32, y: f32, shade: u32) -> Station {
    Station::new(lateral, y, rgb(shade * 34 / 100, shade * 48 / 100, shade * 28 / 100))
}

const TERRAIN_STATIONS: [Station; 12] = [
    terrain(-190.0, -78.0, 34),
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
    terrain(190.0, -78.0, 34),
];

const ROAD_STATIONS: [Station; 5] = [
    Station::new(-6.4, 0.0, rgb(0x14, 0x16, 0x19)),
    Station::new(-5.2, 0.02, ROAD_COLOR),
    Station::new(0.0, 0.03, rgb(0x1E, 0x20, 0x25)),
    Station::new(5.2, 0.02, ROAD_COLOR),
    Station::new(6.4, 0.0, rgb(0x14, 0x16, 0x19)),
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

/// 22 boxes, 36 vertices each.
const CAR_BOX_COUNT: usize = 22;
const CAR_VERTS: usize = CAR_BOX_COUNT * 36;
static mut CAR_MESH: psp::Align16<[Vertex; CAR_VERTS]> = psp::Align16([Vertex::ZERO; CAR_VERTS]);

/// One wheel: a 9-sided tyre and a 7-sided rim, reused for all four corners.
const WHEEL_VERTS: usize = 9 * 12 + 7 * 12;
static mut WHEEL_MESH: psp::Align16<[Vertex; WHEEL_VERTS]> =
    psp::Align16([Vertex::ZERO; WHEEL_VERTS]);
static mut TYRE_COUNT: usize = 0;
static mut RIM_COUNT: usize = 0;

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

/// Builds every static mesh. Call once, after the track is generated.
pub fn init(track: &Track) {
    unsafe {
        (*(&raw mut TERRAIN_MESH)).build(track, &TERRAIN_STATIONS);
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
        build_car();
        build_wheel();
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

/// The car body, origin at ground centre, +Z forward.
unsafe fn build_car() {
    let out = core::slice::from_raw_parts_mut(&raw mut CAR_MESH as *mut Vertex, CAR_VERTS);
    let mut w = 0usize;
    let mut add = |bw: f32, bh: f32, bd: f32, x: f32, y: f32, z: f32, c: u32| {
        w += mesh::build_box(&mut out[w..], bw, bh, bd, x, y, z, c);
    };

    add(1.78, 0.46, 4.24, 0.0, 0.50, 0.0, PAINT); // body lower
    add(1.82, 0.30, 3.90, 0.0, 0.80, -0.05, PAINT); // body shoulder
    add(1.70, 0.16, 1.30, 0.0, 0.96, 1.20, PAINT); // hood
    add(1.56, 0.44, 1.94, 0.0, 1.16, -0.22, GLASS); // greenhouse
    add(1.50, 0.10, 1.80, 0.0, 1.38, -0.30, PAINT); // roof
    add(1.30, 0.26, 0.16, 0.0, 1.44, -1.22, PAINT); // roof spoiler
    add(0.10, 0.34, 1.70, -0.78, 1.16, -0.24, PAINT); // pillars
    add(0.10, 0.34, 1.70, 0.78, 1.16, -0.24, PAINT);
    add(1.74, 0.22, 0.14, 0.0, 0.86, 2.10, TRIM); // grille
    add(1.62, 0.12, 0.10, 0.0, 1.00, 2.06, CHROME); // chrome bar
    add(0.46, 0.14, 0.10, -0.60, 0.99, 2.09, HEADLAMP); // headlights
    add(0.46, 0.14, 0.10, 0.60, 0.99, 2.09, HEADLAMP);
    add(0.40, 0.16, 0.08, -0.64, 1.00, -2.02, TAILLAMP); // tail lights
    add(0.40, 0.16, 0.08, 0.64, 1.00, -2.02, TAILLAMP);
    add(1.66, 0.20, 0.12, 0.0, 0.72, -2.06, TRIM); // rear valance
    add(0.16, 0.10, 0.12, -0.52, 0.42, -2.10, CHROME); // exhaust tips
    add(0.16, 0.10, 0.12, 0.52, 0.42, -2.10, CHROME);
    add(1.90, 0.12, 0.30, 0.0, 0.30, 1.86, TRIM); // front splitter
    add(0.08, 0.20, 1.60, -0.92, 0.36, -0.10, TRIM); // side skirts
    add(0.08, 0.20, 1.60, 0.92, 0.36, -0.10, TRIM);
    add(0.14, 0.16, 0.30, -0.94, 1.06, 0.72, TRIM); // mirrors
    add(0.14, 0.16, 0.30, 0.94, 1.06, 0.72, TRIM);

    debug_assert!(w == CAR_VERTS);
}

unsafe fn build_wheel() {
    let out = core::slice::from_raw_parts_mut(&raw mut WHEEL_MESH as *mut Vertex, WHEEL_VERTS);
    let tyre = mesh::build_cylinder(out, 9, 0.36, 0.26, TYRE);
    let rim = mesh::build_cylinder(&mut out[tyre..], 7, 0.22, 0.28, RIM);
    TYRE_COUNT = tyre;
    RIM_COUNT = rim;
}

/// Roadside props: street lamps and trees.
///
/// Baked into one static, chunk-bucketed buffer in world space rather than instanced with a
/// matrix per prop. There are over a thousand of them; a draw call each would dominate the frame.
const PROPS_PER_CHUNK: usize = 1024;
const PROP_VERTS: usize = PROPS_PER_CHUNK * mesh::CHUNK_COUNT;
static mut PROP_MESH: psp::Align16<[Vertex; PROP_VERTS]> = psp::Align16([Vertex::ZERO; PROP_VERTS]);
static mut PROP_CHUNKS: [Chunk; mesh::CHUNK_COUNT] = [Chunk {
    start: 0,
    count: 0,
    center: Vec3::ZERO,
    radius: 0.0,
}; mesh::CHUNK_COUNT];

const TREE_STRIDE: usize = 4;
const LAMP_STRIDE: usize = 58;

const TREE_LOW: u32 = rgb(0x1A, 0x21, 0x1C);
const TREE_HIGH: u32 = rgb(0x30, 0x39, 0x37);
const LAMP_POLE: u32 = rgb(0x3A, 0x40, 0x46);
const LAMP_HEAD: u32 = rgb(0xFF, 0xEC, 0xBE);

unsafe fn build_props(track: &Track) {
    let verts = core::slice::from_raw_parts_mut(&raw mut PROP_MESH as *mut Vertex, PROP_VERTS);
    let mut w = 0usize;

    for c in 0..mesh::CHUNK_COUNT {
        let start = w;
        let budget = start + PROPS_PER_CHUNK;
        let first_node = c * mesh::CHUNK_NODES * mesh::RENDER_STRIDE;
        let last_node = core::cmp::min(
            first_node + mesh::CHUNK_NODES * mesh::RENDER_STRIDE,
            angle_zero::track::NODE_COUNT - 1,
        );

        let (mut lo, mut hi) = (
            Vec3::new(f32::MAX, f32::MAX, f32::MAX),
            Vec3::new(f32::MIN, f32::MIN, f32::MIN),
        );
        let mut note = |v: &Vertex, lo: &mut Vec3, hi: &mut Vec3| {
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
    }
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

/// Draws the night sky: a vertical gradient behind everything, then the mountain silhouette.
pub fn draw_sky() {
    unsafe {
        // The gradient is 2D, so it needs no camera and cannot be occluded by anything.
        sys::sceGuDisable(GuState::DepthTest);
        sys::sceGuDisable(GuState::Texture2D);
        sys::sceGuDisable(GuState::Fog);

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

        // Mountains sit in front of the gradient but behind everything else. Depth writes stay
        // off so the scene proper is never occluded by geometry a kilometre away.
        sys::sceGuEnable(GuState::DepthTest);
        sys::sceGuDepthMask(1);
        sys::sceGumMatrixMode(MatrixMode::Model);
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

/// Sets the projection and view matrices for this frame.
pub fn set_camera(camera: &Camera) {
    unsafe {
        sys::sceGumMatrixMode(MatrixMode::Projection);
        sys::sceGumLoadIdentity();
        // 480x272 is 16:9. Far is deliberately short — fog hides everything past
        // 330 m anyway, and a tighter range buys depth precision.
        sys::sceGumPerspective(camera.fov, 16.0 / 9.0, 0.4, 2400.0);

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

/// Chunks beyond fog range or behind the camera contribute nothing.
fn visible(chunk: &Chunk, eye: Vec3, forward: Vec3) -> bool {
    if chunk.count == 0 {
        return false;
    }
    let to_chunk = chunk.center.sub(eye);
    let distance = to_chunk.length();
    if distance - chunk.radius > FOG_FAR {
        return false;
    }
    // Generous behind-test: only reject what is fully behind the eye plane.
    to_chunk.dot(forward) > -(chunk.radius + 12.0)
}

fn draw_ribbon<const V: usize>(r: &Ribbon<V>, eye: Vec3, forward: Vec3) {
    unsafe {
        for chunk in r.chunks.iter() {
            if !visible(chunk, eye, forward) {
                continue;
            }
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
    let eye = camera.pos;
    let forward = Vec3::new(sin(camera.yaw), 0.0, cos(camera.yaw));

    unsafe {
        sys::sceGumMatrixMode(MatrixMode::Model);
        sys::sceGumLoadIdentity();

        draw_ribbon(&*(&raw const TERRAIN_MESH), eye, forward);
        draw_ribbon(&*(&raw const ROAD_MESH), eye, forward);

        // Markings sit fractions of a metre above the road; draw them after so they win ties.
        draw_ribbon(&*(&raw const EDGE_L_MESH), eye, forward);
        draw_ribbon(&*(&raw const EDGE_R_MESH), eye, forward);

        let dash_verts = &raw const DASH_MESH as *const Vertex;
        let dash_chunks = &*(&raw const DASH_CHUNKS);
        for chunk in dash_chunks.iter() {
            if !visible(chunk, eye, forward) {
                continue;
            }
            sys::sceGumDrawArray(
                GuPrimitive::Triangles,
                VERTEX_FORMAT,
                chunk.count as i32,
                core::ptr::null(),
                dash_verts.add(chunk.start as usize) as *const c_void,
            );
        }

        draw_ribbon(&*(&raw const RAIL_L_MESH), eye, forward);
        draw_ribbon(&*(&raw const RAIL_R_MESH), eye, forward);

        // Trees are crossed quads with no single facing, so they must not be back-face culled.
        sys::sceGuDisable(GuState::CullFace);
        let prop_verts = &raw const PROP_MESH as *const Vertex;
        let prop_chunks = &*(&raw const PROP_CHUNKS);
        for chunk in prop_chunks.iter() {
            if !visible(chunk, eye, forward) {
                continue;
            }
            sys::sceGumDrawArray(
                GuPrimitive::Triangles,
                VERTEX_FORMAT,
                chunk.count as i32,
                core::ptr::null(),
                prop_verts.add(chunk.start as usize) as *const c_void,
            );
        }
        sys::sceGuEnable(GuState::CullFace);
    }
}

/// Draws the car, wheels included, at its current pose and attitude.
pub fn draw_car(vehicle: &Vehicle, track: &Track) {
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

        sys::sceGumDrawArray(
            GuPrimitive::Triangles,
            VERTEX_FORMAT,
            CAR_VERTS as i32,
            core::ptr::null(),
            &raw const CAR_MESH as *const c_void,
        );

        // Wheels: fronts also steer. Hub offsets from the car model.
        let hubs = [
            (-0.86f32, 1.32f32, true),
            (0.86, 1.32, true),
            (-0.86, -1.38, false),
            (0.86, -1.38, false),
        ];
        for (hx, hz, front) in hubs.iter() {
            sys::sceGumPushMatrix();
            sys::sceGumTranslate(&ScePspFVector3 {
                x: *hx,
                y: 0.36,
                z: *hz,
            });
            if *front {
                sys::sceGumRotateY(st.steer);
            }
            sys::sceGumRotateX(st.wheel_spin);
            sys::sceGumDrawArray(
                GuPrimitive::Triangles,
                VERTEX_FORMAT,
                (TYRE_COUNT + RIM_COUNT) as i32,
                core::ptr::null(),
                &raw const WHEEL_MESH as *const c_void,
            );
            sys::sceGumPopMatrix();
        }
    }
}

/// The two additive ground beams that stand in for a real headlight spot.
pub fn draw_headlight_beams(st: &CarState) {
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

        sys::sceGumMatrixMode(MatrixMode::Model);
        sys::sceGumLoadIdentity();
        sys::sceGumTranslate(&ScePspFVector3 {
            x: st.x,
            y: st.y + 0.04,
            z: st.z,
        });
        // The beam steers with the wheels.
        sys::sceGumRotateY(st.yaw + st.steer * 0.55);

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
