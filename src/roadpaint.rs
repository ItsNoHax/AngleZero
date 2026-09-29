//! What is painted on the road, and what has happened to it since.
//!
//! The pass is two lanes: traffic keeps left, so the car's own lane is the one on the negative
//! (left) side of each node's `nrm`. Nothing here changes where the car can go — the tarmac is as
//! wide as it ever was and the rail is where it was — it only decides what the surface looks like:
//! the lines, the words painted in the lane, the patches, and other people's tyre marks.
//!
//! All of it is placed by arclength along the centreline (`s`) and metres across it (`u`), from the
//! track and a fixed seed, so it is the same on every boot.

use crate::math::{abs, clamp, cos, Vec3, PI};
use crate::scenery::{apexes, hash, outside_of_bend, unit};
use crate::track::{node_at_arclength, Track, CORNER_CURVATURE};

/// Where the lane ends and the edge line begins, either side.
pub const EDGE_IN: f32 = 3.55;
/// Outer edge of the edge line. 0.2 m, a shade over a real one, so it stays a pixel wide further off.
pub const EDGE_OUT: f32 = 3.75;
/// Where a lane starts, just clear of the centre marking.
pub const LANE_IN: f32 = 0.3;
/// Where the paved shoulder's paler surface begins.
pub const SHOULDER_IN: f32 = 3.85;

/// Centre line: 5 m painted, 5 m gap.
pub const DASH_LENGTH: f32 = 5.0;
pub const DASH_PERIOD: f32 = 10.0;
/// The double yellow runs from this far before a bend to this far after it.
pub const YELLOW_LEAD: f32 = 12.0;
pub const YELLOW_TAIL: f32 = 10.0;
/// Offset of each yellow line's centre from the centreline, and their width.
pub const YELLOW_OFFSET: f32 = 0.2;
pub const YELLOW_WIDTH: f32 = 0.12;

/// A bend that gets furniture: from where the road starts turning to where it straightens.
#[derive(Clone, Copy, Debug)]
pub struct Bend {
    pub apex: u32,
    pub start_s: f32,
    pub end_s: f32,
    /// Outside of the bend, in the `nrm` frame.
    pub outside: f32,
}

pub const MAX_BENDS: usize = 64;

/// Everything painted on the pass, worked out once from the track.
pub struct Paint {
    pub bends: [Bend; MAX_BENDS],
    pub count: usize,
    /// Every apex, with the outside of its bend: one bend can hold several, where the road never
    /// straightens between them.
    pub apexes: [(f32, f32); MAX_BENDS],
    pub apex_count: usize,
}

impl Paint {
    pub fn new(track: &Track) -> Paint {
        let mut apex = [0u32; MAX_BENDS];
        let n = apexes(track, &mut apex);
        let mut bends = [Bend { apex: 0, start_s: 0.0, end_s: 0.0, outside: 0.0 }; MAX_BENDS];
        let mut count = 0;
        let mut apexes = [(0.0f32, 0.0f32); MAX_BENDS];
        let quiet = CORNER_CURVATURE * 0.6;
        for (k, &a) in apex[..n].iter().enumerate() {
            let a = a as usize;
            apexes[k] = (track.nodes[a].s, outside_of_bend(track, a));
            let (mut i, mut j) = (a, a);
            while i > 0 && abs(track.nodes[i].curv) > quiet {
                i -= 1;
            }
            while j < track.nodes.len() - 1 && abs(track.nodes[j].curv) > quiet {
                j += 1;
            }
            // Apexes with no straight between them are one bend.
            if count > 0 && bends[count - 1].start_s == track.nodes[i].s {
                continue;
            }
            bends[count] = Bend {
                apex: a as u32,
                start_s: track.nodes[i].s,
                end_s: track.nodes[j].s,
                outside: outside_of_bend(track, a),
            };
            count += 1;
        }
        Paint { bends, count, apexes, apex_count: n }
    }

    pub fn bends(&self) -> &[Bend] {
        &self.bends[..self.count]
    }

    /// True where the centre is double yellow: through a bend and either side of it.
    pub fn yellow_at(&self, s: f32) -> bool {
        self.bends().iter().any(|b| s >= b.start_s - YELLOW_LEAD && s <= b.end_s + YELLOW_TAIL)
    }

    /// True where a white centre dash is painted: on the straights, 5 m on and 5 m off. A dash
    /// that would run into the yellow is left out whole rather than cut short.
    pub fn dash_starts_at(&self, s: f32) -> bool {
        !self.yellow_at(s) && !self.yellow_at(s + DASH_LENGTH)
    }

    /// Arclengths of the optical speed bars before each bend, nearest the bend last. Bars that
    /// would land in the previous bend or its yellow are left out.
    pub fn bars(&self, mut each: impl FnMut(f32)) {
        const GAPS: [f32; 7] = [0.0, 4.0, 3.6, 3.2, 2.9, 2.6, 2.3];
        let total: f32 = GAPS.iter().sum();
        for (k, b) in self.bends().iter().enumerate() {
            let prev_end = if k > 0 { self.bends[k - 1].end_s + 6.0 } else { 0.0 };
            let mut s = b.start_s - 20.0 - total;
            for g in GAPS {
                s += g;
                if s > prev_end + 4.0 {
                    each(s);
                }
            }
        }
    }

    /// Road text before each bend: the speed limit, then 急カーブ ("sharp bend") nearest-first.
    pub fn texts(&self, mut each: impl FnMut(RoadText)) {
        for (k, b) in self.bends().iter().enumerate() {
            let prev_end = if k > 0 { self.bends[k - 1].end_s + 6.0 } else { 0.0 };
            let limit_s = b.start_s - 20.0 - 23.6 - 9.0;
            if limit_s > prev_end + 4.0 {
                // "40": two glyphs side by side across the lane.
                each(RoadText { glyph: Glyph::Four, s: limit_s, u: -1.95 - 0.55, length: 4.5, width: 1.2 });
                each(RoadText { glyph: Glyph::Zero, s: limit_s, u: -1.95 + 0.55, length: 4.5, width: 1.2 });
            }
            let first = b.start_s - 18.5;
            if first > prev_end + 4.0 {
                for (i, g) in [Glyph::Kyu, Glyph::Ka, Glyph::Bar, Glyph::Bu].iter().enumerate() {
                    // Stretched to more than twice their width, as road text is, so they read from
                    // a low eye at a distance.
                    each(RoadText { glyph: *g, s: first + i as f32 * 4.3, u: -1.95, length: 3.8, width: 1.9 });
                }
            }
        }
    }

    /// Raised markers between the yellow lines, every 8 m.
    pub fn studs(&self, mut each: impl FnMut(f32)) {
        for b in self.bends() {
            let mut s = b.start_s - YELLOW_LEAD + 2.0;
            while s < b.end_s + YELLOW_TAIL - 2.0 {
                each(s);
                s += 8.0;
            }
        }
    }

    /// Other drivers' tyre marks through each bend, as curves in (s, u): each call is one tyre's
    /// line, sampled at `SKID_SAMPLES` points, with how dark it is.
    pub fn skids(&self, mut each: impl FnMut(&[(f32, f32)], f32)) {
        // The same for every line, and a cosine is dear on the PSP.
        let mut swings = [0.0f32; SKID_SAMPLES];
        for (j, swing) in swings.iter_mut().enumerate() {
            let t = j as f32 / (SKID_SAMPLES - 1) as f32;
            *swing = 0.5 - 0.5 * cos(t * PI);
        }
        for (k, &(apex_s, outside)) in self.apexes[..self.apex_count].iter().enumerate() {
            for line in 0..5u32 {
                let key = (k as u32) * 64 + line;
                let s0 = apex_s - 24.0 + unit(key) * 8.0;
                let span = 28.0 + unit(key ^ 0x11) * 12.0;
                // From out wide on the entry, swinging across to the inside at the apex.
                let u_out = outside * (1.0 + unit(key ^ 0x22) * 2.6);
                let u_in = -outside * (1.8 + unit(key ^ 0x33) * 2.2);
                let dark = 0.35 + 0.4 * unit(key ^ 0x44);
                for tyre in [-0.75f32, 0.75] {
                    let mut pts = [(0.0f32, 0.0f32); SKID_SAMPLES];
                    for (j, p) in pts.iter_mut().enumerate() {
                        let t = j as f32 / (SKID_SAMPLES - 1) as f32;
                        let swing = swings[j];
                        let u = u_out + (u_in - u_out) * swing + tyre * (0.5 + 0.5 * t);
                        *p = (s0 + t * span, clamp(u, -5.1, 5.1));
                    }
                    each(&pts, dark);
                }
            }
        }
    }
}

pub const SKID_SAMPLES: usize = 16;

/// A glyph painted in the lane, stretched along the road as road text is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RoadText {
    pub glyph: Glyph,
    /// Arclength of its near edge, and metres across to its centre.
    pub s: f32,
    pub u: f32,
    pub length: f32,
    pub width: f32,
}

/// The glyphs, in `roadglyphs::GLYPHS` order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    Four = 0,
    Zero = 1,
    Kyu = 2,
    Ka = 3,
    Bar = 4,
    Bu = 5,
}

/// A stretch of newer tarmac let into one lane.
#[derive(Clone, Copy, Debug)]
pub struct Patch {
    pub s: f32,
    pub length: f32,
    pub u0: f32,
    pub u1: f32,
}

/// Patches along the pass: about one every 90 m, in one lane or the other.
pub fn patches(track: &Track, mut each: impl FnMut(Patch)) {
    let mut k = 0u32;
    let mut s = 20.0;
    while s < track.length - 20.0 {
        let key = 0x9A7C_0000 + k;
        let left = unit(key) < 0.5;
        let width = 1.4 + unit(key ^ 1) * 1.6;
        let start = 0.5 + unit(key ^ 2) * (EDGE_IN - 0.9 - width);
        let (u0, u1) = if left { (-(start + width), -start) } else { (start, start + width) };
        each(Patch { s, length: 3.0 + unit(key ^ 3) * 6.0, u0, u1 });
        s += 60.0 + unit(key ^ 4) * 60.0;
        k += 1;
    }
}

/// Sealed cracks: short zigzags, each a run of (s, u) points, about one every 25 m.
pub fn cracks(track: &Track, mut each: impl FnMut(&[(f32, f32)])) {
    let mut k = 0u32;
    let mut s = 8.0;
    while s < track.length - 10.0 {
        let key = 0xC4AC_0000 + k * 8;
        let mut u = (unit(key) - 0.5) * 2.0 * (EDGE_IN - 0.4);
        let mut pts = [(0.0f32, 0.0f32); 6];
        for (j, p) in pts.iter_mut().enumerate() {
            u = clamp(u + (unit(key + j as u32 + 1) - 0.5) * 0.6, -(EDGE_IN - 0.2), EDGE_IN - 0.2);
            *p = (s + j as f32 * 0.8, u);
        }
        each(&pts);
        s += 15.0 + unit(key ^ 7) * 20.0;
        k += 1;
    }
}

/// Drain grates in the shoulder, alternating sides, every 20 m: (s, u of the grate's centre).
pub fn grates(track: &Track, mut each: impl FnMut(f32, f32)) {
    let mut s = 10.0;
    let mut side = 1.0;
    while s < track.length - 5.0 {
        each(s, side * (SHOULDER_IN + 0.8));
        side = -side;
        s += 20.0;
    }
}

/// How much darker or lighter the road is at `s`, around 1.0: it was resurfaced in sections, and
/// each section has weathered differently.
pub fn tone(s: f32) -> f32 {
    let section = (s / 210.0) as u32;
    0.9 + 0.2 * unit(0x5EC7 ^ hash(section))
}

/// A point on the road surface at (s, u), `lift` above it: the centreline's height plus the crown,
/// which falls 1 cm from the centre to the lane edges.
pub fn surface(track: &Track, s: f32, u: f32, lift: f32) -> Vec3 {
    let s = clamp(s, 0.0, track.length);
    let j = node_at_arclength(track, s).max(1);
    let (a, b) = (&track.nodes[j - 1], &track.nodes[j]);
    let t = clamp((s - a.s) / (b.s - a.s).max(1e-4), 0.0, 1.0);
    let l = |x: f32, y: f32| x + (y - x) * t;
    let crown = 0.03 - 0.01 * clamp(abs(u) / 5.2, 0.0, 1.0);
    let (nx, nz) = (l(a.nrm.x, b.nrm.x), l(a.nrm.z, b.nrm.z));
    Vec3::new(l(a.p.x, b.p.x) + nx * u, l(a.p.y, b.p.y) + crown + lift, l(a.p.z, b.p.z) + nz * u)
}

/// The road's heading at `s`, as a horizontal unit vector.
pub fn heading(track: &Track, s: f32) -> (f32, f32) {
    let n = &track.nodes[node_at_arclength(track, clamp(s, 0.0, track.length))];
    (n.dir.x, n.dir.z)
}

/// Where the traffic cones stand, as (s, u): a handful of spots rather than a cone every few
/// metres. Each spot has a reason to be there — shoulder works round a drain, a rock fall on the
/// inside of a bend — and all of them keep to the shoulders, so none
/// stands in a lane the car is driving through.
pub fn cones(track: &Track, paint: &Paint, mut each: impl FnMut(f32, f32)) {
    let len = track.length;
    let shoulder = SHOULDER_IN + 0.15;

    // Shoulder works on the left, about a quarter of the way down: a line of cones along the edge
    // line, tapering out to the verge at each end.
    let works = |each: &mut dyn FnMut(f32, f32), s0: f32, side: f32, n: usize| {
        each(s0 - 4.0, side * (shoulder + 0.9));
        each(s0 - 2.0, side * (shoulder + 0.4));
        for k in 0..n {
            each(s0 + k as f32 * 2.5, side * shoulder);
        }
        let end = s0 + (n - 1) as f32 * 2.5;
        each(end + 2.0, side * (shoulder + 0.4));
    };
    works(&mut each, len * 0.27, -1.0, 4);
    works(&mut each, len * 0.71, 1.0, 3);

    // A rock fall on the inside of a bend near the middle of the pass: three cones round it.
    if paint.apex_count > 0 {
        let (s, outside) = paint.apexes[paint.apex_count / 2];
        let u = -outside * (shoulder + 0.6);
        each(s - 2.2, u);
        each(s + 0.4, u - outside * 0.5);
        each(s + 2.6, u);
    }

}
