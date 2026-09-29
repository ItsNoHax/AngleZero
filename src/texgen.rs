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
    /// Sprayed-concrete slope protection: a grid of cast frames with rougher fill between.
    Lattice,
}

pub const SURFACES: [Surface; 3] = [Surface::Asphalt, Surface::Grass, Surface::Lattice];

/// How many metres of world one tile covers.
pub fn tile_metres(s: Surface) -> f32 {
    match s {
        Surface::Asphalt => 2.6,
        Surface::Grass => 7.0,
        // Two frames per tile, so each cell is 1.6 m across.
        Surface::Lattice => 3.2,
    }
}

/// The multiplier ramp, darkest first, as 8-bit RGB.
pub fn palette(s: Surface) -> [(u8, u8, u8); 16] {
    let (dark, light) = match s {
        Surface::Asphalt => ((112u8, 112u8, 120u8), (255u8, 255u8, 255u8)),
        Surface::Grass => ((132, 138, 104), (255, 255, 250)),
        Surface::Lattice => ((96, 100, 96), (255, 255, 255)),
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
        Surface::Lattice => {
            // Distance to the nearest frame, in texels, on a 32-texel grid.
            let gx = (x % 32).min(32 - x % 32) as f32;
            let gy = (y % 32).min(32 - y % 32) as f32;
            let d = gx.min(gy);
            let rough = lattice(fx, fy, 16, 31);
            if d < 3.0 {
                // The cast frame: bright, with its upper edge catching more light than its lower.
                0.78 + (white - 0.5) * 0.12 - d * 0.03
            } else {
                // The fill between: darker shotcrete, blotched with damp and moss.
                0.36 + (rough - 0.5) * 0.4 + (white - 0.5) * 0.18
            }
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
        Surface::Lattice => LATTICE_LIFT,
    }
}

const LATTICE_LIFT: u32 = 149;

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

// --- pine billboards ----------------------------------------------------------------------------

/// The tree atlas: three pines side by side, each in a cell [`PINE_CELL`] texels wide.
pub const ATLAS_W: usize = 128;
pub const ATLAS_H: usize = 64;
pub const PINE_CELL: usize = 42;
/// Cell `k` starts at `k * PINE_PITCH`, leaving a transparent column between neighbours so linear
/// filtering never pulls one pine's edge into the next.
pub const PINE_PITCH: usize = 43;

/// Index 0 is transparent; 1..=15 a ramp from shadowed needles to moonlit tips.
pub fn pine_palette() -> [(u8, u8, u8, u8); 16] {
    let mut out = [(0u8, 0u8, 0u8, 0u8); 16];
    for (i, px) in out.iter_mut().enumerate().skip(1) {
        let t = (i - 1) as f32 / 14.0;
        let c = |a: f32, b: f32| lerp(a, b, t) as u8;
        *px = (c(110.0, 255.0), c(118.0, 255.0), c(112.0, 245.0), 255);
    }
    out
}

/// Draws the three pines. Each is a trunk under tiers of foliage that flare and step back in, with
/// a ragged edge, and is lit from its right (the moon's side in the default view) and from above.
pub fn pine_atlas(out: &mut [u8; ATLAS_W * ATLAS_H]) {
    out.fill(0);
    for k in 0..3usize {
        let x0 = k * PINE_PITCH;
        let cx = x0 as f32 + PINE_CELL as f32 * 0.5;
        let seed = 0x9100 + k as u32 * 97;
        let tiers = [6.0f32, 7.0, 5.0][k];
        let fullness = [0.47f32, 0.40, 0.5][k];
        let top = [1usize, 3, 2][k];
        let foliage_bottom = ATLAS_H - 7;
        for y in top..ATLAS_H {
            let yy = y as f32;
            // 0 at the tip, 1 at the lowest branches.
            let t = (yy - top as f32) / (foliage_bottom - top) as f32;
            let row_seed = seed + y as u32 * 131;
            if y > foliage_bottom {
                // Trunk.
                for x in 0..PINE_CELL {
                    let dx = (x0 + x) as f32 + 0.5 - cx;
                    if abs_f(dx) < 1.6 {
                        out[y * ATLAS_W + x0 + x] = 2 + (dx > 0.0) as u8;
                    }
                }
                continue;
            }
            let tier = t * tiers;
            let frac = tier - floor(tier);
            let envelope = PINE_CELL as f32 * fullness * (0.12 + 0.88 * t);
            let half = envelope * (0.5 + 0.5 * frac);
            for x in 0..PINE_CELL {
                let dx = (x0 + x) as f32 + 0.5 - cx;
                let ragged = (hash(row_seed + x as u32) >> 8) as f32 / (1u32 << 24) as f32;
                let edge = half * (0.82 + 0.3 * ragged);
                if abs_f(dx) > edge {
                    continue;
                }
                // Gaps near the edge where branches part.
                if abs_f(dx) > edge * 0.7 && ragged < 0.2 {
                    continue;
                }
                let side = clamp(0.5 + dx / (2.0 * edge.max(1.0)), 0.0, 1.0);
                let droop = 1.0 - frac; // the top of each tier catches the light
                let light = 0.2 + 0.45 * side + 0.25 * droop * (1.0 - t * 0.5) + (ragged - 0.5) * 0.25;
                out[y * ATLAS_W + x0 + x] = 1 + (clamp(light, 0.0, 1.0) * 14.0 + 0.5) as u8;
            }
        }
    }
}

fn abs_f(x: f32) -> f32 {
    if x < 0.0 { -x } else { x }
}

/// Halves the atlas. A texel is opaque when at least two of its four sources are, and takes the
/// mean of those; averaging in the transparent index would darken every edge.
pub fn downsample_alpha(src: &[u8], w: usize, h: usize, dst: &mut [u8]) {
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let q = [
                src[2 * y * w + 2 * x],
                src[2 * y * w + 2 * x + 1],
                src[(2 * y + 1) * w + 2 * x],
                src[(2 * y + 1) * w + 2 * x + 1],
            ];
            let (mut n, mut sum) = (0u32, 0u32);
            for v in q {
                if v > 0 {
                    n += 1;
                    sum += v as u32;
                }
            }
            dst[y * (w / 2) + x] = if n >= 2 { ((sum + n / 2) / n) as u8 } else { 0 };
        }
    }
}

// --- road text ----------------------------------------------------------------------------------

/// The road-text atlas: each of `roadglyphs::GLYPHS` in its own 32-texel cell, left to right.
pub const TEXT_W: usize = 256;
pub const TEXT_H: usize = 32;

/// Index 0 is bare road; 1..=15 paint, from worn to fresh.
pub fn text_palette() -> [(u8, u8, u8, u8); 16] {
    let mut out = [(0u8, 0u8, 0u8, 0u8); 16];
    for (i, px) in out.iter_mut().enumerate().skip(1) {
        let t = (i - 1) as f32 / 14.0;
        let c = lerp(150.0, 255.0, t) as u8;
        *px = (c, c, (c as f32 * 0.96) as u8, 255);
    }
    out
}

/// Paints the glyphs into the atlas, worn: a little grain everywhere and the odd texel gone
/// where tyres have taken the paint off.
pub fn text_atlas(out: &mut [u8; TEXT_W * TEXT_H]) {
    use crate::roadglyphs::{GLYPHS, GLYPH_SIZE};
    out.fill(0);
    for (g, rows) in GLYPHS.iter().enumerate() {
        for (y, row) in rows.iter().enumerate() {
            // Thickened by a texel each side: road paint is bolder than the font, and a stroke
            // four texels wide is under a pixel by the time it is twenty metres off.
            let bold = row | row << 1 | row >> 1;
            for x in 0..GLYPH_SIZE {
                if bold >> x & 1 == 0 {
                    continue;
                }
                let n = (hash((g * 4096 + y * 64 + x) as u32 ^ 0x7E47) >> 8) as f32 / (1u32 << 24) as f32;
                if n < 0.07 {
                    continue;
                }
                out[y * TEXT_W + g * GLYPH_SIZE + x] = 1 + (10.0 + n * 4.9) as u8;
            }
        }
    }
}
