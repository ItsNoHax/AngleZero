//! Numbers the HUD displays. Pure functions so they can be checked without a renderer.
//!
//! Nothing here formats to a string: `no_std` has no allocator and the PSP HUD draws glyphs from
//! a bitmap font, so the caller gets the digits and does the drawing.

use crate::math::floor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gear {
    Reverse,
    Forward(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RevZone {
    Green,
    Amber,
    Red,
}

/// Splits a run time into `(minutes, seconds, hundredths)`.
///
/// Everything is derived from a single integer hundredths count so the three fields can never
/// disagree. The small bias absorbs f32 representation error — without it `9.87` arrives as
/// `9.869999...` and the clock reads `.86`.
pub fn split_time(t: f32) -> (u32, u32, u32) {
    let total = if t < 0.0 { 0.0 } else { t };
    let cs_total = floor(total * 100.0 + 0.01) as u32;
    (cs_total / 6000, (cs_total / 100) % 60, cs_total % 100)
}

/// Colour band for the rev bar, from `Engine::rev_fraction`.
pub fn rev_zone(rpm: f32) -> RevZone {
    if rpm > 0.88 {
        RevZone::Red
    } else if rpm > 0.7 {
        RevZone::Amber
    } else {
        RevZone::Green
    }
}

/// Splits a score into decimal digits, most significant first, for the digit renderer.
/// Returns how many digits were written.
pub fn score_digits(score: u32, out: &mut [u8; 10]) -> usize {
    if score == 0 {
        out[0] = 0;
        return 1;
    }
    let mut tmp = [0u8; 10];
    let mut n = 0;
    let mut v = score;
    while v > 0 && n < 10 {
        tmp[n] = (v % 10) as u8;
        v /= 10;
        n += 1;
    }
    for i in 0..n {
        out[i] = tmp[n - 1 - i];
    }
    n
}
