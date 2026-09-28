//! The world's surface tiles on the GE: asphalt on the road, grass on the hillside.
//!
//! The tiles themselves come from `angle_zero::texgen`, built once at boot into main memory. None of
//! the world's vertices carry texture coordinates. The GE makes them from each vertex's position
//! instead (texture projection mapping, `TexMapMode(TextureMatrix)` with `TexProjMapMode(Position)`),
//! through a matrix that takes world X and Z to U and V at the tile's scale. The road and the
//! hillside are nearly level, so a top-down projection is all they need, and the 16-byte vertex the
//! whole world is built from stays as it is.

use core::ffi::c_void;

use angle_zero::texgen::{self, level_bytes, Surface, LEVELS, SURFACES, TILE};
use psp::sys::{
    self, ClutPixelFormat, GuState, GuTexWrapMode, MatrixMode, MipmapLevel, ScePspFMatrix4,
    ScePspFVector4, TextureColorComponent, TextureEffect, TextureFilter, TextureLevelMode,
    TextureMapMode, TexturePixelFormat, TextureProjectionMapMode,
};

use super::render::rgb;

const LEVEL0: usize = level_bytes(0);
const LEVEL1: usize = level_bytes(1);

#[repr(C, align(16))]
struct Tile {
    level0: [u8; LEVEL0],
    level1: [u8; LEVEL1],
    clut: [u32; 16],
}

static mut TILES: [Tile; SURFACES.len()] = [const {
    Tile { level0: [0; LEVEL0], level1: [0; LEVEL1], clut: [0; 16] }
}; SURFACES.len()];

fn slot(s: Surface) -> usize {
    SURFACES.iter().position(|&x| x == s).unwrap_or(0)
}

/// Builds every tile. The caller writes the data cache back afterwards, as for the meshes.
pub unsafe fn init() {
    let _ = LEVELS;
    let tiles = &mut *(&raw mut TILES);
    let mut idx = [0u8; TILE * TILE];
    let mut half = [0u8; TILE * TILE / 4];
    for s in SURFACES {
        let t = &mut tiles[slot(s)];
        texgen::indices(s, &mut idx);
        texgen::pack4(&idx, &mut t.level0);
        texgen::downsample(&idx, TILE, &mut half);
        texgen::pack4(&half, &mut t.level1);
        for (i, (r, g, b)) in texgen::palette(s).iter().enumerate() {
            t.clut[i] = rgb(*r as u32, *g as u32, *b as u32);
        }
    }
}

/// Textures everything drawn until [`unbind`] with `s`, projected from world X and Z.
pub unsafe fn bind(s: Surface) {
    #[cfg(feature = "devtools")]
    if super::render::debug_mode() == super::render::MODE_NO_TEXTURES {
        return;
    }
    let t = &(*(&raw const TILES))[slot(s)];
    sys::sceGuEnable(GuState::Texture2D);
    sys::sceGuClutMode(ClutPixelFormat::Psm8888, 0, 0x0f, 0);
    sys::sceGuClutLoad(2, t.clut.as_ptr() as *const c_void);
    sys::sceGuTexMode(TexturePixelFormat::PsmT4, (LEVELS - 1) as i32, 0, 0);
    sys::sceGuTexImage(MipmapLevel::None, TILE as i32, TILE as i32, TILE as i32, t.level0.as_ptr() as *const c_void);
    sys::sceGuTexImage(
        MipmapLevel::Level1,
        (TILE / 2) as i32,
        (TILE / 2) as i32,
        (TILE / 2) as i32,
        t.level1.as_ptr() as *const c_void,
    );
    sys::sceGuTexFunc(TextureEffect::Modulate, TextureColorComponent::Rgb);
    sys::sceGuTexFilter(TextureFilter::LinearMipmapNearest, TextureFilter::Linear);
    sys::sceGuTexLevelMode(TextureLevelMode::Auto, 0.0);
    sys::sceGuTexWrap(GuTexWrapMode::Repeat, GuTexWrapMode::Repeat);
    sys::sceGuTexMapMode(TextureMapMode::TextureMatrix, 0, 0);
    sys::sceGuTexProjMapMode(TextureProjectionMapMode::Position);

    // World X to U, world Z to V, one tile per `tile_metres`; q = 1 so nothing is divided.
    let k = 1.0 / texgen::tile_metres(s);
    let m = psp::Align16(ScePspFMatrix4 {
        x: ScePspFVector4 { x: k, y: 0.0, z: 0.0, w: 0.0 },
        y: ScePspFVector4 { x: 0.0, y: 0.0, z: 0.0, w: 0.0 },
        z: ScePspFVector4 { x: 0.0, y: k, z: 0.0, w: 0.0 },
        w: ScePspFVector4 { x: 0.0, y: 0.0, z: 1.0, w: 0.0 },
    });
    sys::sceGuSetMatrix(MatrixMode::Texture, &m.0);
}

/// Back to untextured, and to ordinary UV mapping for whatever binds a texture next (the car).
pub unsafe fn unbind() {
    sys::sceGuTexMapMode(TextureMapMode::TextureCoords, 0, 0);
    sys::sceGuTexProjMapMode(TextureProjectionMapMode::Uv);
    sys::sceGuTexLevelMode(TextureLevelMode::Auto, 0.0);
    sys::sceGuDisable(GuState::Texture2D);
}
