//! The road's paint and wear: centre dashes, speed bars, patches, cracks, drain grates, raised
//! markers, the words in the lane and other drivers' tyre marks.
//!
//! What goes where is `angle_zero::roadpaint`. This builds it once into chunked meshes and draws it
//! over the road. Everything here lies flat on the tarmac a few centimetres up, so it is drawn with
//! the same depth bias the light pools use: a quad laid by arclength and a ribbon cut at render
//! nodes are two slightly different piecewise surfaces, and without the bias which one wins is
//! decided pixel by pixel.

use core::ffi::c_void;

use angle_zero::math::Vec3;
use angle_zero::mesh::{self, Chunk, Vertex};
use angle_zero::roadpaint::{self, Paint, DASH_LENGTH, DASH_PERIOD, EDGE_IN, LANE_IN, SKID_SAMPLES};
use angle_zero::texgen::{self, TEXT_H, TEXT_W};
use angle_zero::track::{node_at_arclength, Track};
use psp::sys::{
    self, AlphaFunc, ClutPixelFormat, GuPrimitive, GuState, GuTexWrapMode, MipmapLevel,
    TextureColorComponent, TextureEffect, TextureFilter, TextureLevelMode, TextureMapMode,
    TexturePixelFormat, VertexType,
};

use super::render::{rgb, rgba};
use super::trees::{TexVertex, TEX_VERTEX_FORMAT};

const VERTEX_FORMAT: VertexType = VertexType::from_bits_truncate(
    VertexType::COLOR_8888.bits() | VertexType::VERTEX_32BITF.bits() | VertexType::TRANSFORM_3D.bits(),
);

/// The pools' bias, for the same reason: see the module comment.
const PAINT_DEPTH_BIAS: i32 = 64;
/// How far above the crowned surface each layer lies. Paint over patches over bare road.
const PATCH_LIFT: f32 = 0.012;
const PAINT_LIFT: f32 = 0.025;
const SKID_LIFT: f32 = 0.02;

const WHITE: u32 = rgb(0xB8, 0xB2, 0xA2);
const PATCH: u32 = rgb(0x12, 0x13, 0x16);
const CRACK: u32 = rgb(0x0B, 0x0C, 0x0E);
const GRATE: u32 = rgb(0x0A, 0x0B, 0x0C);
const GRATE_BAR: u32 = rgb(0x3A, 0x3C, 0x40);
const STUD: u32 = rgb(0x6B, 0x5A, 0x3A);

const PAINT_VERTS: usize = 30_000;
const TEXT_VERTS: usize = 3_000;
const SKID_VERTS: usize = 30_000;

static mut PAINT_MESH: psp::Align16<[Vertex; PAINT_VERTS]> = psp::Align16([Vertex::ZERO; PAINT_VERTS]);
static mut TEXT_MESH: psp::Align16<[TexVertex; TEXT_VERTS]> = psp::Align16([TexVertex::ZERO; TEXT_VERTS]);
static mut SKID_MESH: psp::Align16<[Vertex; SKID_VERTS]> = psp::Align16([Vertex::ZERO; SKID_VERTS]);
const NO_CHUNK: Chunk = Chunk { start: 0, count: 0, center: Vec3::ZERO, radius: 0.0 };
static mut PAINT_CHUNKS: [Chunk; mesh::CHUNK_COUNT] = [NO_CHUNK; mesh::CHUNK_COUNT];
static mut TEXT_CHUNKS: [Chunk; mesh::CHUNK_COUNT] = [NO_CHUNK; mesh::CHUNK_COUNT];
static mut SKID_CHUNKS: [Chunk; mesh::CHUNK_COUNT] = [NO_CHUNK; mesh::CHUNK_COUNT];

#[repr(C, align(16))]
struct Atlas {
    level0: [u8; TEXT_W * TEXT_H / 2],
    level1: [u8; TEXT_W * TEXT_H / 8],
    clut: [u32; 16],
}
static mut ATLAS: Atlas = Atlas { level0: [0; TEXT_W * TEXT_H / 2], level1: [0; TEXT_W * TEXT_H / 8], clut: [0; 16] };

/// Which chunk a point along the road belongs to.
fn chunk_of(track: &Track, s: f32) -> usize {
    let node = node_at_arclength(track, s.max(0.0));
    (node / (mesh::CHUNK_NODES * mesh::RENDER_STRIDE)).min(mesh::CHUNK_COUNT - 1)
}

/// Collects geometry in any order, filed by chunk, then lays it out one chunk after another.
struct Sorter<V: Copy, const N: usize> {
    verts: [V; N],
    tags: [u8; N],
    w: usize,
}

impl<V: Copy, const N: usize> Sorter<V, N> {
    fn push(&mut self, chunk: usize, verts: &[V]) {
        if self.w + verts.len() > N {
            return;
        }
        for v in verts {
            self.verts[self.w] = *v;
            self.tags[self.w] = chunk as u8;
            self.w += 1;
        }
    }

    /// Writes what was pushed into `out` grouped by chunk, fills in `chunks`, and empties itself.
    fn finish(&mut self, out: &mut [V], chunks: &mut [Chunk], pos: impl Fn(&V) -> Vec3) {
        let mut w = 0;
        for (c, chunk) in chunks.iter_mut().enumerate() {
            let start = w;
            let (mut lo, mut hi) = (Vec3::new(f32::MAX, f32::MAX, f32::MAX), Vec3::new(f32::MIN, f32::MIN, f32::MIN));
            for i in 0..self.w {
                if self.tags[i] as usize == c && w < out.len() {
                    out[w] = self.verts[i];
                    let p = pos(&out[w]);
                    lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
                    hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
                    w += 1;
                }
            }
            let n = w - start;
            let center = if n > 0 { lo.add(hi).scale(0.5) } else { Vec3::ZERO };
            let radius = if n > 0 { hi.sub(center).length() } else { 0.0 };
            *chunk = Chunk { start: start as u32, count: n as u32, center, radius };
        }
        self.w = 0;
    }
}

fn quad(a: Vec3, b: Vec3, c: Vec3, d: Vec3, color: u32) -> [Vertex; 6] {
    let v = |p: Vec3| Vertex::new(p.x, p.y, p.z, color);
    [v(a), v(b), v(c), v(a), v(c), v(d)]
}

/// A rectangle lying on the road from `s0` to `s1` and `u0` to `u1`, cut every `step` metres along
/// so it follows the curve and the fall of the road.
fn strip(track: &Track, s0: f32, s1: f32, u0: f32, u1: f32, lift: f32, color: u32, mut each: impl FnMut(f32, [Vertex; 6])) {
    let steps = ((s1 - s0) / 1.4) as usize + 1;
    for k in 0..steps {
        let (a, b) = (s0 + (s1 - s0) * k as f32 / steps as f32, s0 + (s1 - s0) * (k + 1) as f32 / steps as f32);
        let p = |s: f32, u: f32| roadpaint::surface(track, s, u, lift);
        each(a, quad(p(a, u0), p(a, u1), p(b, u1), p(b, u0), color));
    }
}

/// A line through (s, u) points, `width` across.
fn line(track: &Track, pts: &[(f32, f32)], width: f32, lift: f32, color: u32, mut each: impl FnMut(f32, [Vertex; 6])) {
    for w in pts.windows(2) {
        let ((s0, u0), (s1, u1)) = (w[0], w[1]);
        let h = width * 0.5;
        let p = |s: f32, u: f32| roadpaint::surface(track, s, u, lift);
        each(s0, quad(p(s0, u0 - h), p(s0, u0 + h), p(s1, u1 + h), p(s1, u1 - h), color));
    }
}

static mut SCRATCH: Sorter<Vertex, SKID_VERTS> = Sorter { verts: [Vertex::ZERO; SKID_VERTS], tags: [0; SKID_VERTS], w: 0 };
static mut TEXT_SCRATCH: Sorter<TexVertex, TEXT_VERTS> = Sorter { verts: [TexVertex::ZERO; TEXT_VERTS], tags: [0; TEXT_VERTS], w: 0 };

unsafe fn build_paint(track: &Track, paint: &Paint) {
    let b = &mut *(&raw mut SCRATCH);
    // Patches first, so the paint over them is drawn after.
    roadpaint::patches(track, |p| {
        strip(track, p.s, p.s + p.length, p.u0, p.u1, PATCH_LIFT, PATCH, |s, q| b.push(chunk_of(track, s), &q));
    });
    roadpaint::cracks(track, |pts| line(track, pts, 0.07, PATCH_LIFT, CRACK, |s, q| b.push(chunk_of(track, s), &q)));
    roadpaint::grates(track, |s, u| {
        strip(track, s, s + 0.9, u - 0.3, u + 0.3, PATCH_LIFT, GRATE, |s, q| b.push(chunk_of(track, s), &q));
        for k in 1..5 {
            let at = s + k as f32 * 0.18;
            strip(track, at - 0.03, at + 0.03, u - 0.27, u + 0.27, PAINT_LIFT, GRATE_BAR, |s, q| b.push(chunk_of(track, s), &q));
        }
    });
    // The centre dashes, on the straights.
    let mut s = 0.0;
    while s < track.length - DASH_LENGTH {
        if paint.dash_starts_at(s) {
            strip(track, s, s + DASH_LENGTH, -0.075, 0.075, PAINT_LIFT, WHITE, |s, q| b.push(chunk_of(track, s), &q));
        }
        s += DASH_PERIOD;
    }
    // Speed bars across our lane.
    paint.bars(|s| strip(track, s, s + 0.45, -EDGE_IN + 0.1, -LANE_IN - 0.1, PAINT_LIFT, WHITE, |s, q| b.push(chunk_of(track, s), &q)));
    // The raised markers' bodies; their light is drawn by the reflection pass.
    paint.studs(|s| strip(track, s - 0.06, s + 0.06, -0.06, 0.06, PAINT_LIFT + 0.01, STUD, |s, q| b.push(chunk_of(track, s), &q)));
    b.finish(&mut (*(&raw mut PAINT_MESH)).0, &mut *(&raw mut PAINT_CHUNKS), |v| Vec3::new(v.x, v.y, v.z));
}

unsafe fn build_text(track: &Track, paint: &Paint) {
    let b = &mut *(&raw mut TEXT_SCRATCH);
    let cell = 32.0 / TEXT_W as f32;
    paint.texts(|t| {
        let u0 = t.glyph as usize as f32 * cell;
        let (l, r) = (t.u - t.width * 0.5, t.u + t.width * 0.5);
        // Cut in three along its length so it follows the road.
        for k in 0..3 {
            let (a, c) = (t.s + t.length * k as f32 / 3.0, t.s + t.length * (k + 1) as f32 / 3.0);
            // V runs from the glyph's bottom at the near end to its top at the far end.
            let (va, vc) = (1.0 - k as f32 / 3.0, 1.0 - (k + 1) as f32 / 3.0);
            let p = |s: f32, u: f32, tu: f32, tv: f32| {
                let q = roadpaint::surface(track, s, u, PAINT_LIFT);
                TexVertex { u: tu, v: tv, color: WHITE, x: q.x, y: q.y, z: q.z }
            };
            // Our lane is on the left, and the text is read from behind it: its left edge is at
            // the smaller `u`.
            let (p0, p1, p2, p3) = (p(a, l, u0, va), p(a, r, u0 + cell, va), p(c, r, u0 + cell, vc), p(c, l, u0, vc));
            b.push(chunk_of(track, t.s), &[p0, p1, p2, p0, p2, p3]);
        }
    });
    b.finish(&mut (*(&raw mut TEXT_MESH)).0, &mut *(&raw mut TEXT_CHUNKS), |v| Vec3::new(v.x, v.y, v.z));

    let a = &mut *(&raw mut ATLAS);
    let mut idx = [0u8; TEXT_W * TEXT_H];
    texgen::text_atlas(&mut idx);
    texgen::pack4(&idx, &mut a.level0);
    let mut half = [0u8; TEXT_W * TEXT_H / 4];
    texgen::downsample_alpha(&idx, TEXT_W, TEXT_H, &mut half);
    texgen::pack4(&half, &mut a.level1);
    for (i, (r, g, bl, al)) in texgen::text_palette().iter().enumerate() {
        a.clut[i] = rgba(*r as u32, *g as u32, *bl as u32, *al as u32);
    }
}

unsafe fn build_skids(track: &Track, paint: &Paint) {
    let b = &mut *(&raw mut SCRATCH);
    paint.skids(|pts: &[(f32, f32)], dark| {
        debug_assert!(pts.len() == SKID_SAMPLES);
        let color = rgba(0x06, 0x06, 0x08, (dark * 150.0) as u32);
        line(track, pts, 0.22, SKID_LIFT, color, |s, q| b.push(chunk_of(track, s), &q));
    });
    b.finish(&mut (*(&raw mut SKID_MESH)).0, &mut *(&raw mut SKID_CHUNKS), |v| Vec3::new(v.x, v.y, v.z));
}

pub unsafe fn init(track: &Track) {
    let paint = Paint::new(track);
    build_paint(track, &paint);
    build_text(track, &paint);
    build_skids(track, &paint);
}

unsafe fn draw_chunks<V>(verts: *const V, chunks: &[Chunk], format: VertexType, visible: &impl Fn(&Chunk) -> bool) -> u32 {
    let mut drawn = 0;
    for chunk in chunks {
        if chunk.count == 0 || !visible(chunk) {
            continue;
        }
        drawn += chunk.count;
        sys::sceGumDrawArray(GuPrimitive::Triangles, VertexType::from_bits_truncate(format.bits()), chunk.count as i32, core::ptr::null(), verts.add(chunk.start as usize) as *const c_void);
    }
    drawn
}

/// Everything painted on the road, then the words, then the tyre marks. Called straight after the
/// road ribbon, with culling off.
pub unsafe fn draw(visible: impl Fn(&Chunk) -> bool) -> u32 {
    let mut drawn = 0;
    sys::sceGuDepthOffset(PAINT_DEPTH_BIAS);
    drawn += draw_chunks(&raw const PAINT_MESH as *const Vertex, &*(&raw const PAINT_CHUNKS), VERTEX_FORMAT, &visible);

    #[cfg(feature = "devtools")]
    let textured = super::render::debug_mode() != super::render::MODE_NO_TEXTURES;
    #[cfg(not(feature = "devtools"))]
    let textured = true;
    if textured {
        let a = &*(&raw const ATLAS);
        sys::sceGuEnable(GuState::Texture2D);
        sys::sceGuClutMode(ClutPixelFormat::Psm8888, 0, 0x0f, 0);
        sys::sceGuClutLoad(2, a.clut.as_ptr() as *const c_void);
        sys::sceGuTexMode(TexturePixelFormat::PsmT4, 1, 0, 0);
        sys::sceGuTexImage(MipmapLevel::None, TEXT_W as i32, TEXT_H as i32, TEXT_W as i32, a.level0.as_ptr() as *const c_void);
        sys::sceGuTexImage(MipmapLevel::Level1, (TEXT_W / 2) as i32, (TEXT_H / 2) as i32, (TEXT_W / 2) as i32, a.level1.as_ptr() as *const c_void);
        sys::sceGuTexFunc(TextureEffect::Modulate, TextureColorComponent::Rgba);
        sys::sceGuTexFilter(TextureFilter::LinearMipmapNearest, TextureFilter::Linear);
        sys::sceGuTexLevelMode(TextureLevelMode::Auto, 0.0);
        sys::sceGuTexWrap(GuTexWrapMode::Clamp, GuTexWrapMode::Clamp);
        sys::sceGuTexMapMode(TextureMapMode::TextureCoords, 0, 0);
        sys::sceGuAlphaFunc(AlphaFunc::Greater, 0x7f, 0xff);
        sys::sceGuEnable(GuState::AlphaTest);
        drawn += draw_chunks(&raw const TEXT_MESH as *const TexVertex, &*(&raw const TEXT_CHUNKS), TEX_VERTEX_FORMAT, &visible);
        sys::sceGuDisable(GuState::AlphaTest);
        sys::sceGuDisable(GuState::Texture2D);
    }

    sys::sceGuEnable(GuState::Blend);
    sys::sceGuBlendFunc(sys::BlendOp::Add, sys::BlendFactor::SrcAlpha, sys::BlendFactor::OneMinusSrcAlpha, 0, 0);
    sys::sceGuDepthMask(1);
    drawn += draw_chunks(&raw const SKID_MESH as *const Vertex, &*(&raw const SKID_CHUNKS), VERTEX_FORMAT, &visible);
    sys::sceGuDepthMask(0);
    sys::sceGuDisable(GuState::Blend);
    sys::sceGuDepthOffset(0);
    drawn
}
