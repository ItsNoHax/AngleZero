//! Guard-rail posts, chevron boards, road mirrors and the reflectors that answer the headlights.
//!
//! Where each of these stands, which way it faces and how brightly it returns the car's lamps are
//! all `angle_zero::scenery`'s. This file builds the static geometry once, and every frame emits an
//! additive glow for whatever the beams are falling on.

use core::ffi::c_void;

use angle_zero::camera::Camera;
use angle_zero::math::{cos, sin, sqrt, Vec3, TAU};
use angle_zero::mesh::{self, Chunk, Vertex};
use angle_zero::scenery::{self, Sign, SignKind, REFLECT_FAR};
use angle_zero::track::{node_at_arclength, Track, BAY_FROM, BAY_SIDE, BAY_TO, RAIL_LIMIT};
use angle_zero::vehicle::Vehicle;
use psp::sys::{self, GuPrimitive, GuState, MatrixMode, VertexType};

use super::render::{rgb, rgba};

const VERTEX_FORMAT: VertexType = VertexType::from_bits_truncate(
    VertexType::COLOR_8888.bits() | VertexType::VERTEX_32BITF.bits() | VertexType::TRANSFORM_3D.bits(),
);

const POST_COLOR: u32 = rgb(0x3A, 0x40, 0x46);
const BOARD_BACK: u32 = rgb(0x12, 0x12, 0x10);
const CHEVRON_DIM: u32 = rgb(0x5E, 0x4C, 0x16);
const MIRROR_POLE: u32 = rgb(0xC8, 0x5A, 0x28);
const MIRROR_FACE: u32 = rgb(0x2F, 0x3B, 0x52);

/// Metres of rail between posts.
const POST_SPACING: f32 = 4.0;
/// Posts are drawn only this close. Past it a post is under half a pixel wide, and a shape that
/// small covers a different set of pixels every frame: it would twinkle, as the stars once did.
const POST_NEAR: f32 = 60.0;

/// Nodes per post chunk: small, so the near-only rule can be applied a chunk at a time.
const POST_CHUNK_NODES: usize = 8;
const POST_CHUNKS: usize = angle_zero::track::NODE_COUNT / POST_CHUNK_NODES + 1;
const MAX_POSTS: usize = 2000;
const POST_VERTS: usize = MAX_POSTS * 6;

static mut POSTS: psp::Align16<[Vertex; POST_VERTS]> = psp::Align16([Vertex::ZERO; POST_VERTS]);
static mut POST_CHUNK: [Chunk; POST_CHUNKS] =
    [Chunk { start: 0, count: 0, center: Vec3::ZERO, radius: 0.0 }; POST_CHUNKS];

const MAX_SIGNS: usize = 1100;
static mut SIGNS: [Sign; MAX_SIGNS] = [Sign::ZERO; MAX_SIGNS];
static mut SIGN_COUNT: usize = 0;

const SIGN_VERTS: usize = 24_000;
static mut SIGN_MESH: psp::Align16<[Vertex; SIGN_VERTS]> = psp::Align16([Vertex::ZERO; SIGN_VERTS]);
static mut SIGN_CHUNKS: [Chunk; mesh::CHUNK_COUNT] =
    [Chunk { start: 0, count: 0, center: Vec3::ZERO, radius: 0.0 }; mesh::CHUNK_COUNT];

fn rail_gap(node: usize, side: f32) -> bool {
    side * BAY_SIDE > 0.0 && node >= BAY_FROM && node <= BAY_TO
}

/// Bounds of a run of vertices as a culling sphere.
fn bounds(v: &[Vertex]) -> (Vec3, f32) {
    if v.is_empty() {
        return (Vec3::ZERO, 0.0);
    }
    let (mut lo, mut hi) = (Vec3::new(f32::MAX, f32::MAX, f32::MAX), Vec3::new(f32::MIN, f32::MIN, f32::MIN));
    for p in v {
        lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    let c = lo.add(hi).scale(0.5);
    (c, hi.sub(c).length())
}

fn quad(out: &mut [Vertex], w: &mut usize, a: Vec3, b: Vec3, c: Vec3, d: Vec3, color: u32) {
    let v = |p: Vec3| Vertex::new(p.x, p.y, p.z, color);
    out[*w..*w + 6].copy_from_slice(&[v(a), v(b), v(c), v(a), v(c), v(d)]);
    *w += 6;
}

unsafe fn build_posts(track: &Track) {
    let out = &mut (*(&raw mut POSTS)).0;
    let chunks = &mut *(&raw mut POST_CHUNK);
    let foot = scenery::terrain_drop(RAIL_LIMIT + 0.15) - 0.25;
    let mut w = 0usize;
    let mut s = 1.0f32;
    let mut chunk = 0usize;
    let mut start = 0usize;
    let close = |chunks: &mut [Chunk], upto: usize, start: &mut usize, w: usize, out: &[Vertex], chunk: &mut usize| {
        while *chunk < upto.min(POST_CHUNKS) {
            let (center, radius) = bounds(&out[*start..w]);
            chunks[*chunk] = Chunk { start: *start as u32, count: (w - *start) as u32, center, radius };
            *start = w;
            *chunk += 1;
        }
    };
    while s < track.length - 1.0 && w + 12 <= POST_VERTS {
        let i = node_at_arclength(track, s);
        close(chunks, i / POST_CHUNK_NODES, &mut start, w, out, &mut chunk);
        let n = &track.nodes[i];
        for side in [-1.0f32, 1.0] {
            if rail_gap(i, side) {
                continue;
            }
            // Just behind the beam, facing along the road: that is the face the chase camera sees.
            let lateral = side * (RAIL_LIMIT + 0.1);
            let (cx, cz) = (n.p.x + n.nrm.x * lateral, n.p.z + n.nrm.z * lateral);
            let (hx, hz) = (n.nrm.x * 0.08, n.nrm.z * 0.08);
            let (y0, y1) = (n.p.y + foot, n.p.y + 0.9);
            quad(
                out,
                &mut w,
                Vec3::new(cx - hx, y0, cz - hz),
                Vec3::new(cx + hx, y0, cz + hz),
                Vec3::new(cx + hx, y1, cz + hz),
                Vec3::new(cx - hx, y1, cz - hz),
                POST_COLOR,
            );
        }
        s += POST_SPACING;
    }
    close(chunks, POST_CHUNKS, &mut start, w, out, &mut chunk);
}

/// The board's frame: `r` across its face toward the inside of the bend, and its face normal.
fn board_frame(track: &Track, sign: &Sign) -> ((f32, f32), (f32, f32)) {
    let n = &track.nodes[sign.node as usize];
    let mut r = (-sign.face.1, sign.face.0);
    // Toward the road, i.e. away from the side the board stands on.
    let (ox, oz) = (sign.at.x - n.p.x, sign.at.z - n.p.z);
    if r.0 * ox + r.1 * oz > 0.0 {
        r = (-r.0, -r.1);
    }
    (r, sign.face)
}

/// Three chevrons pointing across the board toward the inside of the bend, `lift` metres in front
/// of the board's face. Writes 36 vertices.
fn chevrons(out: &mut [Vertex], w: &mut usize, track: &Track, sign: &Sign, lift: f32, color: u32) {
    let (r, f) = board_frame(track, sign);
    let c = sign.at;
    let p = |u: f32, v: f32| Vec3::new(c.x + r.0 * u + f.0 * lift, c.y + v, c.z + r.1 * u + f.1 * lift);
    let (half_h, stroke, depth) = (0.22, 0.13, 0.2);
    for k in 0..3 {
        let u = -0.5 + k as f32 * 0.36;
        // Upper arm from the back of the chevron to its point, then the lower arm back again.
        quad(out, w, p(u, half_h), p(u + stroke, half_h), p(u + stroke + depth, 0.0), p(u + depth, 0.0), color);
        quad(out, w, p(u + depth, 0.0), p(u + stroke + depth, 0.0), p(u + stroke, -half_h), p(u, -half_h), color);
    }
}

/// A disc facing `face`, as a fan: `segments * 3` vertices.
#[allow(clippy::too_many_arguments)]
fn disc(out: &mut [Vertex], w: &mut usize, c: Vec3, face: (f32, f32), radius: f32, lift: f32, core: u32, rim: u32) {
    const SEGMENTS: usize = 10;
    let r = (-face.1, face.0);
    let centre = Vec3::new(c.x + face.0 * lift, c.y, c.z + face.1 * lift);
    let edge = |k: usize| {
        let a = (k % SEGMENTS) as f32 / SEGMENTS as f32 * TAU;
        Vertex::new(centre.x + r.0 * cos(a) * radius, centre.y + sin(a) * radius, centre.z + r.1 * cos(a) * radius, rim)
    };
    for k in 0..SEGMENTS {
        out[*w] = Vertex::new(centre.x, centre.y, centre.z, core);
        out[*w + 1] = edge(k);
        out[*w + 2] = edge(k + 1);
        *w += 3;
    }
}

unsafe fn build_signs(track: &Track) {
    let signs = &mut *(&raw mut SIGNS);
    SIGN_COUNT = scenery::road_signs(track, signs, rail_gap);
    let out = &mut (*(&raw mut SIGN_MESH)).0;
    let chunks = &mut *(&raw mut SIGN_CHUNKS);
    let nodes_per_chunk = mesh::CHUNK_NODES * mesh::RENDER_STRIDE;
    let mut w = 0usize;
    for (c, chunk) in chunks.iter_mut().enumerate() {
        let start = w;
        for sign in signs[..SIGN_COUNT].iter() {
            if sign.node as usize / nodes_per_chunk != c || w + 120 > SIGN_VERTS {
                continue;
            }
            let n = &track.nodes[sign.node as usize];
            let (r, f) = board_frame(track, sign);
            let at = sign.at;
            let ground = n.p.y + scenery::terrain_drop(RAIL_LIMIT + 1.0) - 0.2;
            match sign.kind {
                // Reflectors have no body worth drawing: at night only their return is visible.
                SignKind::Reflector => {}
                SignKind::Chevron => {
                    let p = |u: f32, y: f32, lift: f32| Vec3::new(at.x + r.0 * u + f.0 * lift, y, at.z + r.1 * u + f.1 * lift);
                    let (top, bottom) = (at.y + 0.3, at.y - 0.3);
                    quad(out, &mut w, p(-0.7, bottom, 0.0), p(0.7, bottom, 0.0), p(0.7, top, 0.0), p(-0.7, top, 0.0), BOARD_BACK);
                    for u in [-0.55f32, 0.55] {
                        quad(
                            out,
                            &mut w,
                            p(u - 0.04, ground, -0.03),
                            p(u + 0.04, ground, -0.03),
                            p(u + 0.04, bottom, -0.03),
                            p(u - 0.04, bottom, -0.03),
                            POST_COLOR,
                        );
                    }
                    chevrons(out, &mut w, track, sign, 0.02, CHEVRON_DIM);
                }
                SignKind::Mirror => {
                    let (hx, hz) = (r.0 * 0.05, r.1 * 0.05);
                    let base = Vec3::new(at.x - f.0 * 0.05, ground, at.z - f.1 * 0.05);
                    let top = at.y - 0.4;
                    quad(
                        out,
                        &mut w,
                        Vec3::new(base.x - hx, ground, base.z - hz),
                        Vec3::new(base.x + hx, ground, base.z + hz),
                        Vec3::new(base.x + hx, top, base.z + hz),
                        Vec3::new(base.x - hx, top, base.z - hz),
                        MIRROR_POLE,
                    );
                    disc(out, &mut w, at, f, 0.5, 0.0, MIRROR_POLE, MIRROR_POLE);
                    disc(out, &mut w, at, f, 0.4, 0.02, rgb(0x46, 0x56, 0x74), MIRROR_FACE);
                }
            }
        }
        let (center, radius) = bounds(&out[start..w]);
        *chunk = Chunk { start: start as u32, count: (w - start) as u32, center, radius };
    }
}

pub unsafe fn init(track: &Track) {
    build_ring();
    build_posts(track);
    build_signs(track);
}

/// Posts near the eye and every visible sign. Called in the prop pass, culling off.
pub unsafe fn draw_static(eye: Vec3, visible: impl Fn(&Chunk) -> bool) -> u32 {
    let mut drawn = 0u32;
    let posts = &raw const POSTS as *const Vertex;
    for chunk in (*(&raw const POST_CHUNK)).iter() {
        if chunk.count == 0 || chunk.center.sub(eye).length() > POST_NEAR + chunk.radius {
            continue;
        }
        drawn += chunk.count;
        sys::sceGumDrawArray(GuPrimitive::Triangles, VERTEX_FORMAT, chunk.count as i32, core::ptr::null(), posts.add(chunk.start as usize) as *const c_void);
    }
    let signs = &raw const SIGN_MESH as *const Vertex;
    for chunk in (*(&raw const SIGN_CHUNKS)).iter() {
        if chunk.count == 0 || !visible(chunk) {
            continue;
        }
        drawn += chunk.count;
        sys::sceGumDrawArray(GuPrimitive::Triangles, VERTEX_FORMAT, chunk.count as i32, core::ptr::null(), signs.add(chunk.start as usize) as *const c_void);
    }
    drawn
}

const GLOW_SEGMENTS: usize = 8;

/// The rim of a glow as (cos, sin) pairs, worked out once. `sin` and `cos` are software routines
/// on this target, and at two of each per rim vertex a frame with sixty lit reflectors spent eight
/// milliseconds on nothing but trigonometry.
static mut RING: [(f32, f32); GLOW_SEGMENTS + 1] = [(0.0, 0.0); GLOW_SEGMENTS + 1];

unsafe fn build_ring() {
    let ring = &mut *(&raw mut RING);
    for (k, r) in ring.iter_mut().enumerate() {
        let a = (k % GLOW_SEGMENTS) as f32 / GLOW_SEGMENTS as f32 * TAU;
        *r = (cos(a), sin(a));
    }
}

/// A glow facing the camera, bright in the middle and gone at the rim: 24 vertices.
fn glow(out: &mut [Vertex], w: &mut usize, c: Vec3, right: (f32, f32), radius: f32, color: u32) {
    let rim = color & 0x00ff_ffff;
    let ring = unsafe { &*(&raw const RING) };
    let edge = |k: usize| {
        let (ca, sa) = ring[k];
        Vertex::new(c.x + right.0 * ca * radius, c.y + sa * radius, c.z + right.1 * ca * radius, rim)
    };
    for k in 0..GLOW_SEGMENTS {
        out[*w] = Vertex::new(c.x, c.y, c.z, color);
        out[*w + 1] = edge(k);
        out[*w + 2] = edge(k + 1);
        *w += 3;
    }
}

fn alpha(color: u32, g: f32) -> u32 {
    let a = ((color >> 24) as f32 * g) as u32;
    (color & 0x00ff_ffff) | (a.min(255) << 24)
}

/// The headlights coming back off everything in front of the car. Additive, after the car's own
/// lamp glows, with the same depth bias they use.
pub unsafe fn draw_reflections(vehicle: &Vehicle, camera: &Camera, track: &Track) {
    #[cfg(feature = "devtools")]
    if super::render::debug_mode() == super::render::MODE_NO_REFLECTIONS {
        return;
    }
    let st = vehicle.state;
    let car = Vec3::new(st.x, st.y + 0.7, st.z);
    let forward = (sin(st.yaw), cos(st.yaw));
    let all = &*(&raw const SIGNS);
    let signs = &all[..SIGN_COUNT];
    // What the lit signs will cost, counted before asking the arena for it: a fixed budget large
    // enough for the worst corner would take most of the arena every frame.
    const MAX_LIT: usize = 64;
    let verts_for = |k: SignKind| match k {
        SignKind::Reflector => 24,
        SignKind::Chevron => 60,
        SignKind::Mirror => 24,
    };
    let lit_now = |sign: &Sign| {
        let (dx, dz) = (sign.at.x - car.x, sign.at.z - car.z);
        dx * dx + dz * dz <= REFLECT_FAR * REFLECT_FAR && scenery::sign_gain(sign, car, forward) >= 0.03
    };
    let need: usize = signs.iter().filter(|s| lit_now(s)).take(MAX_LIT).map(|s| verts_for(s.kind)).sum();
    if need == 0 {
        return;
    }
    let ptr = super::scratch::alloc::<Vertex>(need);
    if ptr.is_null() {
        return;
    }
    let out = core::slice::from_raw_parts_mut(ptr, need);
    const BUDGET: usize = MAX_LIT;
    let right = (cos(camera.yaw), -sin(camera.yaw));
    let mut w = 0usize;
    let mut lit = 0usize;
    for sign in signs {
        let (dx, dz) = (sign.at.x - car.x, sign.at.z - car.z);
        if dx * dx + dz * dz > REFLECT_FAR * REFLECT_FAR {
            continue;
        }
        let g = scenery::sign_gain(sign, car, forward);
        if g < 0.03 || lit >= BUDGET {
            continue;
        }
        lit += 1;
        let d = sqrt(dx * dx + dz * dz);
        match sign.kind {
            SignKind::Reflector => {
                // Never under about a pixel and a half, so it cannot twinkle as it recedes.
                let radius = 0.22 + d * 0.008;
                glow(out, &mut w, sign.at, right, radius, alpha(rgba(0xFF, 0xC8, 0x80, 0xFF), g));
            }
            SignKind::Chevron => {
                chevrons(out, &mut w, track, sign, 0.03, alpha(rgba(0xFF, 0xD2, 0x3C, 0xE0), g));
                glow(out, &mut w, sign.at, right, 1.3, alpha(rgba(0xFF, 0xC8, 0x40, 0x40), g));
            }
            SignKind::Mirror => {
                let at = Vec3::new(sign.at.x + sign.face.0 * 0.05, sign.at.y, sign.at.z + sign.face.1 * 0.05);
                glow(out, &mut w, at, right, 0.6, alpha(rgba(0xE6, 0xEE, 0xFF, 0xC0), g));
            }
        }
    }
    if w == 0 {
        return;
    }
    sys::sceGuEnable(GuState::Blend);
    sys::sceGuBlendFunc(sys::BlendOp::Add, sys::BlendFactor::SrcAlpha, sys::BlendFactor::Fix, 0, 0xffff_ffff);
    sys::sceGuDepthMask(1);
    sys::sceGuDepthOffset(256);
    sys::sceGuDisable(GuState::CullFace);
    sys::sceGuDisable(GuState::Fog);
    sys::sceGumMatrixMode(MatrixMode::Model);
    sys::sceGumLoadIdentity();
    sys::sceGumDrawArray(GuPrimitive::Triangles, VERTEX_FORMAT, w as i32, core::ptr::null(), out.as_ptr() as *const c_void);
    sys::sceGuDepthOffset(0);
    sys::sceGuEnable(GuState::Fog);
    sys::sceGuEnable(GuState::CullFace);
    sys::sceGuDepthMask(0);
    sys::sceGuDisable(GuState::Blend);
}
