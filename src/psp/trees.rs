//! Pines: crossed billboards cut from a procedural atlas, placed in stands along the road.
//!
//! Where each tree stands is `angle_zero::scenery::tree_sites`; what a pine looks like is
//! `angle_zero::texgen::pine_atlas`. This file puts the two together into chunked geometry the
//! renderer culls like everything else, and draws it with an alpha test so the silhouette is the
//! texture's, not the quad's.

use core::ffi::c_void;

use angle_zero::math::Vec3;
use angle_zero::mesh::{self, Chunk};
use angle_zero::scenery::{self, TreeSite};
use angle_zero::texgen::{self, pine_palette, ATLAS_H, ATLAS_W, PINE_CELL, PINE_PITCH};
use angle_zero::track::{Track, BAY_FROM, BAY_SIDE, BAY_TO};
use psp::sys::{
    self, AlphaFunc, ClutPixelFormat, GuPrimitive, GuState, GuTexWrapMode, MipmapLevel,
    TextureColorComponent, TextureEffect, TextureFilter, TextureLevelMode, TextureMapMode,
    TexturePixelFormat, VertexType,
};

use super::render::rgba;

/// A textured world vertex, in the order the GE reads one: UV, colour, position.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct TexVertex {
    pub u: f32,
    pub v: f32,
    pub color: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl TexVertex {
    pub const ZERO: TexVertex = TexVertex { u: 0.0, v: 0.0, color: 0, x: 0.0, y: 0.0, z: 0.0 };
}

pub const TEX_VERTEX_FORMAT: VertexType = VertexType::from_bits_truncate(
    VertexType::TEXTURE_32BITF.bits()
        | VertexType::COLOR_8888.bits()
        | VertexType::VERTEX_32BITF.bits()
        | VertexType::TRANSFORM_3D.bits(),
);

/// Upper bound on trees; `tree_sites` stops when it is reached.
const MAX_TREES: usize = 2400;
const VERTS_PER_TREE: usize = 12;
const TREE_VERTS: usize = MAX_TREES * VERTS_PER_TREE;

static mut SITES: [TreeSite; MAX_TREES] = [TreeSite::ZERO; MAX_TREES];
static mut MESH: psp::Align16<[TexVertex; TREE_VERTS]> = psp::Align16([TexVertex::ZERO; TREE_VERTS]);
static mut CHUNKS: [Chunk; mesh::CHUNK_COUNT] =
    [Chunk { start: 0, count: 0, center: Vec3::ZERO, radius: 0.0 }; mesh::CHUNK_COUNT];

#[repr(C, align(16))]
struct Atlas {
    level0: [u8; ATLAS_W * ATLAS_H / 2],
    level1: [u8; ATLAS_W * ATLAS_H / 8],
    clut: [u32; 16],
}
static mut ATLAS: Atlas =
    Atlas { level0: [0; ATLAS_W * ATLAS_H / 2], level1: [0; ATLAS_W * ATLAS_H / 8], clut: [0; 16] };

/// Foot and crown of a tree before its own tint, lifted for the atlas's darker needles.
const TREE_FOOT: (u32, u32, u32) = (0x17, 0x23, 0x1B);
const TREE_CROWN: (u32, u32, u32) = (0x2E, 0x42, 0x37);

fn shade(c: (u32, u32, u32), k: f32) -> u32 {
    let f = |x: u32| ((x as f32 * k) as u32).min(255);
    rgba(f(c.0), f(c.1), f(c.2), 0xFF)
}

unsafe fn build_atlas() {
    let a = &mut *(&raw mut ATLAS);
    let mut idx = [0u8; ATLAS_W * ATLAS_H];
    texgen::pine_atlas(&mut idx);
    texgen::pack4(&idx, &mut a.level0);
    let mut half = [0u8; ATLAS_W * ATLAS_H / 4];
    texgen::downsample_alpha(&idx, ATLAS_W, ATLAS_H, &mut half);
    texgen::pack4(&half, &mut a.level1);
    for (i, (r, g, b, al)) in pine_palette().iter().enumerate() {
        a.clut[i] = rgba(*r as u32, *g as u32, *b as u32, *al as u32);
    }
}

unsafe fn build_mesh(track: &Track) {
    let sites = &mut *(&raw mut SITES);
    let bay = |node: usize, side: f32| {
        side * BAY_SIDE > 0.0 && node + 6 >= BAY_FROM && node <= BAY_TO + 6
    };
    let count = scenery::tree_sites(track, sites, bay, super::banks::surface);
    let verts = &mut (*(&raw mut MESH)).0;
    let chunks = &mut *(&raw mut CHUNKS);
    let nodes_per_chunk = mesh::CHUNK_NODES * mesh::RENDER_STRIDE;

    let mut w = 0usize;
    let mut site = 0usize;
    for (c, chunk) in chunks.iter_mut().enumerate() {
        let start = w;
        let (mut lo, mut hi) = (Vec3::new(f32::MAX, f32::MAX, f32::MAX), Vec3::new(f32::MIN, f32::MIN, f32::MIN));
        while site < count && (sites[site].node as usize) / nodes_per_chunk == c {
            let t = &sites[site];
            let half = t.height * PINE_CELL as f32 / ATLAS_H as f32 * 0.5;
            let u0 = (t.variant as usize * PINE_PITCH) as f32 / ATLAS_W as f32;
            let u1 = u0 + PINE_CELL as f32 / ATLAS_W as f32;
            let foot = shade(TREE_FOOT, t.tint);
            let crown = shade(TREE_CROWN, t.tint);
            let (bx, by, bz) = (t.base.x, t.base.y, t.base.z);
            for (ax, az) in [t.dir, t.nrm] {
                let v = |s: f32, up: bool| TexVertex {
                    u: if s < 0.0 { u0 } else { u1 },
                    v: if up { 0.0 } else { 1.0 },
                    color: if up { crown } else { foot },
                    x: bx + ax * s * half,
                    y: by + if up { t.height } else { 0.0 },
                    z: bz + az * s * half,
                };
                let (a, b, cc, d) = (v(-1.0, false), v(1.0, false), v(1.0, true), v(-1.0, true));
                verts[w..w + 6].copy_from_slice(&[a, b, cc, a, cc, d]);
                w += 6;
            }
            for p in [
                Vec3::new(bx - half, by, bz - half),
                Vec3::new(bx + half, by + t.height, bz + half),
            ] {
                lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
                hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
            }
            site += 1;
        }
        let n = w - start;
        let center = if n > 0 { lo.add(hi).scale(0.5) } else { Vec3::ZERO };
        let radius = if n > 0 { hi.sub(center).length() } else { 0.0 };
        *chunk = Chunk { start: start as u32, count: n as u32, center, radius };
    }
}

pub unsafe fn init(track: &Track) {
    build_atlas();
    build_mesh(track);
}

unsafe fn bind_atlas() {
    let a = &*(&raw const ATLAS);
    sys::sceGuEnable(GuState::Texture2D);
    sys::sceGuClutMode(ClutPixelFormat::Psm8888, 0, 0x0f, 0);
    sys::sceGuClutLoad(2, a.clut.as_ptr() as *const c_void);
    sys::sceGuTexMode(TexturePixelFormat::PsmT4, 1, 0, 0);
    sys::sceGuTexImage(MipmapLevel::None, ATLAS_W as i32, ATLAS_H as i32, ATLAS_W as i32, a.level0.as_ptr() as *const c_void);
    sys::sceGuTexImage(
        MipmapLevel::Level1,
        (ATLAS_W / 2) as i32,
        (ATLAS_H / 2) as i32,
        (ATLAS_W / 2) as i32,
        a.level1.as_ptr() as *const c_void,
    );
    sys::sceGuTexFunc(TextureEffect::Modulate, TextureColorComponent::Rgba);
    sys::sceGuTexFilter(TextureFilter::LinearMipmapNearest, TextureFilter::Linear);
    sys::sceGuTexLevelMode(TextureLevelMode::Auto, 0.0);
    sys::sceGuTexWrap(GuTexWrapMode::Clamp, GuTexWrapMode::Clamp);
    sys::sceGuTexMapMode(TextureMapMode::TextureCoords, 0, 0);
    sys::sceGuAlphaFunc(AlphaFunc::Greater, 0x7f, 0xff);
    sys::sceGuEnable(GuState::AlphaTest);
}

/// Draws the visible chunks. Culling must be off: a crossed quad has no single facing.
pub unsafe fn draw(visible: impl Fn(&Chunk) -> bool) -> u32 {
    #[cfg(feature = "devtools")]
    if super::render::debug_mode() == super::render::MODE_NO_TEXTURES {
        return 0;
    }
    bind_atlas();
    let verts = &raw const MESH as *const TexVertex;
    let mut drawn = 0u32;
    for chunk in (*(&raw const CHUNKS)).iter() {
        if chunk.count == 0 || !visible(chunk) {
            continue;
        }
        drawn += chunk.count;
        sys::sceGumDrawArray(
            GuPrimitive::Triangles,
            TEX_VERTEX_FORMAT,
            chunk.count as i32,
            core::ptr::null(),
            verts.add(chunk.start as usize) as *const c_void,
        );
    }
    sys::sceGuDisable(GuState::AlphaTest);
    sys::sceGuDisable(GuState::Texture2D);
    drawn
}
