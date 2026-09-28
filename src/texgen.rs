//! Procedural surface tiles for the world: asphalt, grass and the rest.
//!
//! Like the track, they are generated at boot and cost no storage. Each is a 4-bit indexed tile
//! whose palette is a monotonic ramp of near-white multipliers: the GE modulates the tile by the
//! baked vertex colour, so the palette only adds grain and never decides the colour of a surface.
//! Because every ramp runs dark to light, a smaller mip level can be made by averaging indices.

use crate::math::{clamp, floor, lerp};
use crate::scenery::hash;

/// Side of a level-0 tile, in texels.
pub const TILE: usize = 64;

/// Mip levels generated per tile: 64 and 32. A third, 16 texels across, would be 8 bytes a row,
/// and the GE wants texture rows 16-byte aligned.
pub const LEVELS: usize = 2;

/// Bytes of packed 4-bit texels in a level.
pub const fn level_bytes(level: usize) -> usize {
    let side = TILE >> level;
    side * side / 2
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Surface {
    Asphalt,
    Grass,
}

pub const SURFACES: [Surface; 2] = [Surface::Asphalt, Surface::Grass];

/// How many metres of world one tile covers.
pub fn tile_metres(s: Surface) -> f32 {
    match s {
        Surface::Asphalt => 2.6,
        Surface::Grass => 7.0,
    }
}

/// The multiplier ramp, darkest first, as 8-bit RGB.
pub fn palette(s: Surface) -> [(u8, u8, u8); 16] {
    let (dark, light) = match s {
        Surface::Asphalt => ((112u8, 112u8, 120u8), (255u8, 255u8, 255u8)),
        Surface::Grass => ((132, 138, 104), (255, 255, 250)),
    };
    let mut out = [(0u8, 0u8, 0u8); 16];
    for (i, px) in out.iter_mut().enumerate() {
        let t = i as f32 / 15.0;
        let c = |a: u8, b: u8| lerp(a as f32, b as f32, t) as u8;
        *px = (c(dark.0, light.0), c(dark.1, light.1), c(dark.2, light.2));
    }
    out
}

/// Periodic value noise: a `period`-cell lattice across the tile, smoothly interpolated, so the
/// tile wraps without a seam at any period that divides [`TILE`].
fn lattice(x: f32, y: f32, period: u32, seed: u32) -> f32 {
    let cell = TILE as f32 / period as f32;
    let (gx, gy) = (x / cell, y / cell);
    let (ix, iy) = (floor(gx), floor(gy));
    let (fx, fy) = (gx - ix, gy - iy);
    let smooth = |t: f32| t * t * (3.0 - 2.0 * t);
    let (sx, sy) = (smooth(fx), smooth(fy));
    let at = |i: f32, j: f32| {
        let i = (i as i32).rem_euclid(period as i32) as u32;
        let j = (j as i32).rem_euclid(period as i32) as u32;
        (hash(seed ^ hash(i * 73_856_093 ^ j * 19_349_663)) >> 8) as f32 / (1u32 << 24) as f32
    };
    let a = lerp(at(ix, iy), at(ix + 1.0, iy), sx);
    let b = lerp(at(ix, iy + 1.0), at(ix + 1.0, iy + 1.0), sx);
    lerp(a, b, sy)
}

/// One texel's brightness, `0.0..=1.0`.
fn sample(s: Surface, x: usize, y: usize) -> f32 {
    let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
    let white = (hash((x * TILE + y) as u32 ^ 0x5A5A) >> 8) as f32 / (1u32 << 24) as f32;
    match s {
        Surface::Asphalt => {
            // Fine grain over faint blotches, then single-texel stones: a few light, a few dark.
            let base = 0.5 * lattice(fx, fy, 4, 11) + 0.3 * lattice(fx, fy, 16, 12) + 0.2 * lattice(fx, fy, 32, 13);
            let mut v = 0.42 + (base - 0.5) * 1.1 + (white - 0.5) * 0.4;
            if white > 0.955 {
                v += 0.35;
            } else if white < 0.05 {
                v -= 0.3;
            }
            v
        }
        Surface::Grass => {
            // Clumps, tussocks, and texel-level scatter.
            let clumps = lattice(fx, fy, 4, 21);
            let tussock = lattice(fx, fy, 16, 22);
            0.5 + (clumps - 0.5) * 0.9 + (tussock - 0.5) * 0.6 + (white - 0.5) * 0.35
        }
    }
}

/// Level-0 palette indices, one byte per texel, row-major.
pub fn indices(s: Surface, out: &mut [u8; TILE * TILE]) {
    for y in 0..TILE {
        for x in 0..TILE {
            let v = clamp(sample(s, x, y), 0.0, 1.0);
            out[y * TILE + x] = (v * 15.0 + 0.5) as u8;
        }
    }
}

/// Halves a level by averaging each 2 × 2 block of indices.
pub fn downsample(src: &[u8], side: usize, dst: &mut [u8]) {
    let half = side / 2;
    for y in 0..half {
        for x in 0..half {
            let s = src[2 * y * side + 2 * x] as u32
                + src[2 * y * side + 2 * x + 1] as u32
                + src[(2 * y + 1) * side + 2 * x] as u32
                + src[(2 * y + 1) * side + 2 * x + 1] as u32;
            dst[y * half + x] = ((s + 2) / 4) as u8;
        }
    }
}

/// Packs one-byte indices into the GE's 4-bit format: two texels a byte, the left one low.
pub fn pack4(src: &[u8], dst: &mut [u8]) {
    for (i, pair) in src.chunks(2).enumerate() {
        dst[i] = (pair[0] & 0x0f) | ((pair[1] & 0x0f) << 4);
    }
}

/// How much, in percent, the vertex colours under a surface are lifted so that once the tile has
/// been multiplied in the surface comes out the colour it was untextured. Kept a `const fn` so the
/// station tables can apply it; `tests/texgen.rs` checks it against [`mean_gain`].
pub const fn lift_percent(s: Surface) -> u32 {
    match s {
        Surface::Asphalt => 157,
        Surface::Grass => 123,
    }
}

/// Average multiplier a surface's tile applies, `0.0..=1.0`: what the vertex colours under it
/// have to be brightened by so the surface keeps the colour it had untextured.
pub fn mean_gain(s: Surface) -> f32 {
    let mut idx = [0u8; TILE * TILE];
    indices(s, &mut idx);
    let pal = palette(s);
    let mut sum = 0.0;
    for &i in idx.iter() {
        let (r, g, b) = pal[i as usize];
        sum += (r as f32 + g as f32 + b as f32) / (3.0 * 255.0);
    }
    sum / (TILE * TILE) as f32
}
