//! The world's surface tiles are drawn repeating and modulated by the vertex colours, at 480 × 272
//! with no antialiasing. A seam would show as a grid over the whole hillside, and a tile with too
//! much contrast crawls as the camera moves, so both are pinned here.

use angle_zero::texgen::{downsample, indices, lift_percent, mean_gain, pack4, palette, SURFACES, TILE};

fn tile(s: angle_zero::texgen::Surface) -> [u8; TILE * TILE] {
    let mut t = [0u8; TILE * TILE];
    indices(s, &mut t);
    t
}

/// Mean absolute step between neighbouring texels across a line, horizontally.
fn step_across(t: &[u8], a: usize, b: usize) -> f32 {
    (0..TILE).map(|y| (t[y * TILE + a] as f32 - t[y * TILE + b] as f32).abs()).sum::<f32>() / TILE as f32
}

#[test]
fn tiles_wrap_without_a_seam() {
    for s in SURFACES {
        let t = tile(s);
        // Wrap-around neighbours differ no more than ordinary interior neighbours do.
        let seam = step_across(&t, TILE - 1, 0);
        let interior = (1..TILE - 1).map(|x| step_across(&t, x - 1, x)).sum::<f32>() / (TILE - 2) as f32;
        assert!(seam < interior * 1.6 + 0.3, "{s:?}: seam {seam} against {interior}");
        // And vertically.
        let seam_v = (0..TILE).map(|x| (t[x] as f32 - t[(TILE - 1) * TILE + x] as f32).abs()).sum::<f32>() / TILE as f32;
        assert!(seam_v < interior * 1.6 + 0.3, "{s:?}: vertical seam {seam_v}");
    }
}

#[test]
fn indices_fit_in_four_bits() {
    for s in SURFACES {
        assert!(tile(s).iter().all(|&i| i < 16));
    }
}

#[test]
fn palettes_run_dark_to_light() {
    for s in SURFACES {
        let lum = |(r, g, b): (u8, u8, u8)| r as u32 + g as u32 + b as u32;
        let p = palette(s);
        assert!(p.windows(2).all(|w| lum(w[0]) <= lum(w[1])), "{s:?}");
        assert_eq!(p[15], (255, 255, if p[15].2 == 255 { 255 } else { p[15].2 }));
    }
}

#[test]
fn tiles_are_grain_not_pattern() {
    // Most of the tile sits in a narrow band: contrast is for texture, not for drawing shapes that
    // the eye would pick out as a repeat.
    for s in SURFACES {
        let g = mean_gain(s);
        assert!((0.6..=0.95).contains(&g), "{s:?} mean gain {g}");
        let t = tile(s);
        let mean = t.iter().map(|&i| i as f32).sum::<f32>() / t.len() as f32;
        let dev = (t.iter().map(|&i| (i as f32 - mean).powi(2)).sum::<f32>() / t.len() as f32).sqrt();
        assert!(dev < 3.6, "{s:?} deviation {dev} indices");
    }
}

#[test]
fn packing_puts_the_left_texel_low() {
    let mut out = [0u8; 2];
    pack4(&[1, 2, 15, 0], &mut out);
    assert_eq!(out, [0x21, 0x0f]);
}

#[test]
fn downsample_averages() {
    let src = [0u8, 4, 8, 12];
    let mut dst = [0u8; 1];
    downsample(&src, 2, &mut dst);
    assert_eq!(dst[0], 6);
}

#[test]
fn lift_undoes_the_tile_on_average() {
    for s in SURFACES {
        let net = mean_gain(s) * lift_percent(s) as f32 / 100.0;
        assert!((net - 1.0).abs() < 0.03, "{s:?} comes out at {net}");
    }
}

#[test]
fn pines_stay_inside_their_cells() {
    use angle_zero::texgen::{pine_atlas, ATLAS_H, ATLAS_W, PINE_CELL, PINE_PITCH};
    let mut a = [0u8; ATLAS_W * ATLAS_H];
    pine_atlas(&mut a);
    for y in 0..ATLAS_H {
        for x in 0..ATLAS_W {
            let cell = x / PINE_PITCH;
            let inside = x - cell * PINE_PITCH < PINE_CELL && cell < 3;
            if !inside {
                assert_eq!(a[y * ATLAS_W + x], 0, "texel {x},{y} outside every cell");
            }
        }
    }
    // Each pine covers a fair part of its cell, and touches its bottom row (the trunk).
    for k in 0..3 {
        let x0 = k * PINE_PITCH;
        let filled = (0..ATLAS_H)
            .flat_map(|y| (x0..x0 + PINE_CELL).map(move |x| (x, y)))
            .filter(|&(x, y)| a[y * ATLAS_W + x] > 0)
            .count();
        assert!(filled > PINE_CELL * ATLAS_H / 5, "pine {k} covers only {filled}");
        assert!((x0..x0 + PINE_CELL).any(|x| a[(ATLAS_H - 1) * ATLAS_W + x] > 0));
    }
}
