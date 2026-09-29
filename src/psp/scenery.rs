//! The far scenery: ridgelines, the lit valley and the cloud across the moon.
//!
//! Where any of it stands is decided in `angle_zero::scenery`; this file only turns those numbers
//! into vertices and draw calls. All of it is drawn in the sky pass, which writes no depth, so the
//! order of the calls is the whole of the occlusion: far ridge, mid, near, then the valley.

use core::ffi::c_void;

use angle_zero::camera::Camera;
use angle_zero::math::{clamp, cos, floor, radians, sin, tan, Vec3, TAU};
use angle_zero::mesh::Vertex;
use angle_zero::scenery::{
    self, rim_light, ridge_centre, ridge_height, track_footprint, LightKind, ValleyLight,
    RIDGE_BANDS, RIDGE_COLUMNS, VALLEY_LIGHTS,
};
use angle_zero::track::Track;
use psp::sys::{self, GuPrimitive, MatrixMode, ScePspFVector3, VertexType};

use super::render::{rgb, rgba, FOG_COLOR};

const VERTEX_FORMAT: VertexType = VertexType::from_bits_truncate(
    VertexType::COLOR_8888.bits() | VertexType::VERTEX_32BITF.bits() | VertexType::TRANSFORM_3D.bits(),
);

// --- palette ------------------------------------------------------------------------------------

/// Body and moonlit rim of each band, far to near: lighter and bluer with distance.
const BAND_BODY: [(u32, u32, u32); 3] = [(0x23, 0x34, 0x4D), (0x18, 0x25, 0x3B), (0x10, 0x1A, 0x2B)];
const BAND_RIM: [(u32, u32, u32); 3] = [(0x8F, 0xA5, 0xC7), (0x5E, 0x75, 0x98), (0x3F, 0x54, 0x75)];

fn mix(a: (u32, u32, u32), b: (u32, u32, u32), t: f32) -> u32 {
    let l = |x: u32, y: u32| (x as f32 + (y as f32 - x as f32) * t) as u32;
    rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

// --- ridgelines ---------------------------------------------------------------------------------

/// Two strips per band: crest to just under it (the rim), and from there to the foot.
const STRIP_VERTS: usize = (RIDGE_COLUMNS + 1) * 2;
const BAND_VERTS: usize = STRIP_VERTS * 2;
static mut RIDGES: psp::Align16<[Vertex; BAND_VERTS * 3]> = psp::Align16([Vertex::ZERO; BAND_VERTS * 3]);
static mut TRACK_CENTRE: Vec3 = Vec3::ZERO;

/// Builds every band around the origin; each is moved to its own centre when drawn.
unsafe fn build_ridges() {
    let out = &mut (*(&raw mut RIDGES)).0;
    for (b, band) in RIDGE_BANDS.iter().enumerate() {
        let base = b * BAND_VERTS;
        // Deep enough to read as a lit edge two or three pixels thick at this distance.
        let rim_depth = band.radius * 0.012;
        for c in 0..=RIDGE_COLUMNS {
            let a = (c % RIDGE_COLUMNS) as f32 / RIDGE_COLUMNS as f32 * TAU;
            let (x, z) = (sin(a) * band.radius, cos(a) * band.radius);
            let crest = ridge_height(band, a);
            let lit = rim_light(a);
            let rim = mix(BAND_BODY[b], BAND_RIM[b], lit);
            let body = mix(BAND_BODY[b], BAND_RIM[b], lit * 0.1);
            out[base + c * 2] = Vertex::new(x, crest, z, rim);
            out[base + c * 2 + 1] = Vertex::new(x, crest - rim_depth, z, body);
            out[base + STRIP_VERTS + c * 2] = Vertex::new(x, crest - rim_depth, z, body);
            out[base + STRIP_VERTS + c * 2 + 1] = Vertex::new(x, band.foot, z, FOG_COLOR);
        }
    }
}

/// Draws the bands far to near. The caller has depth writes off and culling off: a strip seen
/// from inside its ring winds the other way on the far side.
pub unsafe fn draw_ridges(camera: &Camera) {
    let verts = &raw const RIDGES as *const Vertex;
    sys::sceGumMatrixMode(MatrixMode::Model);
    for (b, band) in RIDGE_BANDS.iter().enumerate() {
        let c = ridge_centre(band, TRACK_CENTRE, camera.pos);
        sys::sceGumLoadIdentity();
        sys::sceGumTranslate(&ScePspFVector3 { x: c.x, y: 0.0, z: c.z });
        for strip in 0..2 {
            sys::sceGumDrawArray(
                GuPrimitive::TriangleStrip,
                VERTEX_FORMAT,
                STRIP_VERTS as i32,
                core::ptr::null(),
                verts.add(b * BAND_VERTS + strip * STRIP_VERTS) as *const c_void,
            );
        }
    }
    sys::sceGumLoadIdentity();
}

// --- projecting points to pixels ----------------------------------------------------------------

/// The camera the GE is given this frame, rebuilt on the CPU so points can be snapped to pixels
/// before they are drawn. Mirrors `set_camera`: `Mat4::look_at` for the view, `sceGumPerspective`
/// at 16:9 for the projection.
pub struct Projector {
    eye: Vec3,
    forward: Vec3,
    side: Vec3,
    up: Vec3,
    fx: f32,
    fy: f32,
}

impl Projector {
    pub fn new(camera: &Camera) -> Option<Projector> {
        let forward = camera.look_at.sub(camera.pos).normalized();
        let side = forward.cross(Vec3::new(0.0, 1.0, 0.0)).normalized();
        if forward.length() < 0.5 || side.length() < 0.5 {
            return None;
        }
        let up = side.cross(forward);
        let t = tan(radians(camera.fov) * 0.5);
        Some(Projector {
            eye: camera.pos,
            forward,
            side,
            up,
            fy: 136.0 / t,
            fx: 240.0 / (16.0 / 9.0 * t),
        })
    }

    /// Screen position of a direction (a star: the eye's position drops out).
    pub fn direction(&self, dir: Vec3) -> Option<(f32, f32)> {
        let depth = dir.dot(self.forward);
        if depth <= 0.0 {
            return None;
        }
        Some((
            240.0 + dir.dot(self.side) / depth * self.fx,
            136.0 - dir.dot(self.up) / depth * self.fy,
        ))
    }

    /// Screen position of a point in the world.
    pub fn point(&self, p: Vec3) -> Option<(f32, f32)> {
        self.direction(p.sub(self.eye))
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Pixel2D {
    pub color: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Writes a pixel square of side `size` centred on `(sx, sy)` as one sprite (two vertices), or
/// nothing when it is off screen. Returns the vertices written.
pub unsafe fn push_pixel(out: *mut Pixel2D, sx: f32, sy: f32, size: f32, color: u32) -> usize {
    let x = floor(sx - (size - 1.0) * 0.5);
    let y = floor(sy - (size - 1.0) * 0.5);
    if x + size <= 0.0 || y + size <= 0.0 || x >= 480.0 || y >= 272.0 {
        return 0;
    }
    *out = Pixel2D { color, x, y, z: 0.0 };
    *out.add(1) = Pixel2D { color, x: x + size, y: y + size, z: 0.0 };
    2
}

pub unsafe fn draw_pixels(verts: *const Pixel2D, count: usize) {
    if count == 0 {
        return;
    }
    sys::sceGumDrawArray(
        GuPrimitive::Sprites,
        VertexType::COLOR_8888 | VertexType::VERTEX_32BITF | VertexType::TRANSFORM_2D,
        count as i32,
        core::ptr::null(),
        verts as *const c_void,
    );
}

// --- the valley ---------------------------------------------------------------------------------

static mut VALLEY: [ValleyLight; VALLEY_LIGHTS] = [ValleyLight::ZERO; VALLEY_LIGHTS];
static mut VALLEY_COUNT: usize = 0;

const HAZE_SEGMENTS: usize = 16;
const HAZE_VERTS: usize = HAZE_SEGMENTS * 3 * 2;
static mut HAZE: psp::Align16<[Vertex; HAZE_VERTS]> = psp::Align16([Vertex::ZERO; HAZE_VERTS]);

const HAZE_COLOR: u32 = rgba(0xFF, 0x9A, 0x4A, 0x2C);

fn light_color(kind: LightKind) -> (u32, u32, u32) {
    match kind {
        LightKind::Sodium => (0xFF, 0xC4, 0x78),
        LightKind::WarmWhite => (0xFF, 0xE0, 0xA8),
        LightKind::ColdWhite => (0xD8, 0xE6, 0xFF),
        LightKind::Amber => (0xFF, 0x9A, 0x4A),
        LightKind::Red => (0xFF, 0x5A, 0x4A),
        LightKind::Pink => (0xFF, 0x5F, 0xB0),
        LightKind::Cyan => (0x5F, 0xE6, 0xFF),
        LightKind::Green => (0xB8, 0xFF, 0xD8),
    }
}

unsafe fn build_valley(track: &Track) {
    VALLEY_COUNT = scenery::valley_lights(track, &mut *(&raw mut VALLEY));

    // A warm glow lying over each town: a flat disc, so from the pass it foreshortens into the thin
    // band of light a town gives off on a hazy night.
    let out = &mut (*(&raw mut HAZE)).0;
    let rim = HAZE_COLOR & 0x00ff_ffff;
    let mut w = 0;
    for town in scenery::towns(track).iter() {
        let (cx, cy, cz) = (town.centre.x, town.centre.y + 30.0, town.centre.z);
        let r = town.radius * 2.2;
        let edge = |k: usize| {
            let a = (k % HAZE_SEGMENTS) as f32 / HAZE_SEGMENTS as f32 * TAU;
            Vertex::new(cx + cos(a) * r, cy, cz + sin(a) * r, rim)
        };
        for k in 0..HAZE_SEGMENTS {
            out[w] = Vertex::new(cx, cy, cz, HAZE_COLOR);
            out[w + 1] = edge(k);
            out[w + 2] = edge(k + 1);
            w += 3;
        }
    }
}

/// The town glow and then the lights themselves. Called in the sky pass after the ridges, with
/// depth writes off; it sets up and tears down its own blending.
pub unsafe fn draw_valley(camera: &Camera) {
    sys::sceGuEnable(sys::GuState::Blend);
    sys::sceGuBlendFunc(sys::BlendOp::Add, sys::BlendFactor::SrcAlpha, sys::BlendFactor::Fix, 0, 0xffff_ffff);
    sys::sceGumMatrixMode(MatrixMode::Model);
    sys::sceGumLoadIdentity();
    sys::sceGumDrawArray(
        GuPrimitive::Triangles,
        VERTEX_FORMAT,
        HAZE_VERTS as i32,
        core::ptr::null(),
        &raw const HAZE as *const c_void,
    );
    sys::sceGuDisable(sys::GuState::Blend);

    let Some(proj) = Projector::new(camera) else {
        return;
    };
    let verts = super::scratch::alloc::<Pixel2D>(VALLEY_COUNT * 2);
    if verts.is_null() {
        return;
    }
    let mut w = 0usize;
    let lights = &*(&raw const VALLEY);
    for l in lights[..VALLEY_COUNT].iter() {
        let Some((sx, sy)) = proj.point(l.p) else {
            continue;
        };
        // Fainter with distance, smoothly: a light may dim as the car comes down the hill, but it
        // never flickers, because its brightness is not a function of which pixel it lands on.
        let d = l.p.horizontal_distance(camera.pos);
        let k = clamp(1.3 - d / 2200.0, 0.55, 1.0);
        let (r, g, b) = light_color(l.kind);
        let color = rgb((r as f32 * k) as u32, (g as f32 * k) as u32, (b as f32 * k) as u32);
        w += push_pixel(verts.add(w), sx, sy, l.size as f32, color);
    }
    draw_pixels(verts, w);
    super::city::draw();
    draw_mist();
}

// --- mist in the valley ------------------------------------------------------------------------

const MIST_SEGMENTS: usize = 32;
const MIST_RINGS: usize = 3;
const MIST_VERTS: usize = MIST_SEGMENTS * MIST_RINGS * 6;
static mut MIST: psp::Align16<[Vertex; MIST_VERTS]> = psp::Align16([Vertex::ZERO; MIST_VERTS]);

/// A ring of low mist over the valley floor, thickest partway out and gone at both edges. From the
/// pass it is seen nearly edge-on, which foreshortens it into a pale band lying over the lights.
/// Its density varies around the ring so the band is patchy rather than a stripe.
unsafe fn build_mist(track: &Track) {
    let out = &mut (*(&raw mut MIST)).0;
    let (centre, reach) = track_footprint(track);
    let floor = scenery::towns(track)[1].centre.y + 15.0 + 22.0;
    let radii = [reach + 240.0, reach + 420.0, reach + 640.0, reach + 900.0];
    let peak = [0.0f32, 1.0, 0.7, 0.0];
    let mut w = 0;
    let point = |ring: usize, k: usize| {
        let a = (k % MIST_SEGMENTS) as f32 / MIST_SEGMENTS as f32 * TAU;
        let patch = 0.55 + 0.45 * sin(a * 5.0 + 1.3) * cos(a * 3.0);
        let alpha = (0x34 as f32 * peak[ring] * patch) as u32;
        Vertex::new(
            centre.x + sin(a) * radii[ring],
            floor,
            centre.z + cos(a) * radii[ring],
            rgba(0x9F, 0xB4, 0xD6, alpha),
        )
    };
    for ring in 0..MIST_RINGS {
        for k in 0..MIST_SEGMENTS {
            let (a, b, c, d) = (point(ring, k), point(ring, k + 1), point(ring + 1, k + 1), point(ring + 1, k));
            out[w..w + 6].copy_from_slice(&[a, b, c, a, c, d]);
            w += 6;
        }
    }
}

unsafe fn draw_mist() {
    sys::sceGuEnable(sys::GuState::Blend);
    sys::sceGuBlendFunc(sys::BlendOp::Add, sys::BlendFactor::SrcAlpha, sys::BlendFactor::OneMinusSrcAlpha, 0, 0);
    sys::sceGumMatrixMode(MatrixMode::Model);
    sys::sceGumLoadIdentity();
    sys::sceGumDrawArray(GuPrimitive::Triangles, VERTEX_FORMAT, MIST_VERTS as i32, core::ptr::null(), &raw const MIST as *const c_void);
    sys::sceGuDisable(sys::GuState::Blend);
}

// --- cloud across the moon ----------------------------------------------------------------------

const CLOUD_SEGMENTS: usize = 12;
const CLOUD_VERTS: usize = CLOUD_SEGMENTS * 3 * 2;
static mut CLOUD: psp::Align16<[Vertex; CLOUD_VERTS]> = psp::Align16([Vertex::ZERO; CLOUD_VERTS]);

unsafe fn build_cloud(sky_radius: f32) {
    let out = &mut (*(&raw mut CLOUD)).0;
    let yaw = scenery::MOON_YAW;
    let height = scenery::MOON_HEIGHT;
    let ring = (1.0 - height * height).max(0.0);
    let ring = angle_zero::math::sqrt(ring);
    let (mx, my, mz) = (sin(yaw) * ring * sky_radius, height * sky_radius, cos(yaw) * ring * sky_radius);
    let (tx, tz) = (cos(yaw), -sin(yaw));
    let mut w = 0;
    // Long thin wisps, soft at the edges: dark enough to take a slice out of the moon's halo.
    for (along, drop, half_w, half_h, color) in [
        (40.0f32, 10.0f32, 300.0f32, 16.0f32, rgba(0x1A, 0x26, 0x3C, 0xC8)),
        (-70.0, 70.0, 230.0, 12.0, rgba(0x16, 0x21, 0x36, 0x9C)),
    ] {
        let (cx, cy, cz) = (mx + tx * along, my - drop, mz + tz * along);
        let rim = color & 0x00ff_ffff;
        let edge = |k: usize| {
            let a = (k % CLOUD_SEGMENTS) as f32 / CLOUD_SEGMENTS as f32 * TAU;
            Vertex::new(cx + tx * cos(a) * half_w, cy + sin(a) * half_h, cz + tz * cos(a) * half_w, rim)
        };
        for k in 0..CLOUD_SEGMENTS {
            out[w] = Vertex::new(cx, cy, cz, color);
            out[w + 1] = edge(k);
            out[w + 2] = edge(k + 1);
            w += 3;
        }
    }
}

/// The wisps over the moon. Called with the moon's own state: model matrix on the camera,
/// alpha blending on, culling off.
pub unsafe fn draw_cloud() {
    sys::sceGumDrawArray(
        GuPrimitive::Triangles,
        VERTEX_FORMAT,
        CLOUD_VERTS as i32,
        core::ptr::null(),
        &raw const CLOUD as *const c_void,
    );
}

pub unsafe fn init(track: &Track, sky_radius: f32) {
    TRACK_CENTRE = track_footprint(track).0;
    build_ridges();
    build_valley(track);
    super::city::init(track);
    build_mist(track);
    build_cloud(sky_radius);
}
