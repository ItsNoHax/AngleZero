//! The city in the basin below the summit car park.
//!
//! Seen from the title camera it fills the right of the frame: a street grid of lights, a downtown
//! of towers with a lattice broadcast tower beside them, a smaller cluster further right, two
//! elevated expressways and a railway. Nothing of it stands behind the course, so the first
//! hairpin and its trees read against dark hillside on the left.
//!
//! It is all placed from the car park, by bearing and distance off the way the parked car faces,
//! so it stays in the title shot however the car park is moved. Like the rest of the scenery it is
//! a pure function of the track and fixed seeds.
//!
//! Everything here is lights and flat boxes. The shell draws the lights as single-pixel points the
//! GE transforms itself, so a city of a few thousand costs no per-light work on the CPU.

use crate::math::{abs, atan2, cos, hypot, max, min, sin, Vec3, PI};
use crate::scenery::{nearer_than_ridges_from, track_footprint, LightKind, Rng, ValleyLight};
use crate::track::{carpark_parking, Track};

/// The basin floor the city stands on, relative to the start line.
pub const CITY_FLOOR: f32 = -300.0;

/// How many lights the city may have: streets, expressways and rail together.
pub const CITY_LIGHTS: usize = 2600;
/// How many towers.
pub const CITY_TOWERS: usize = 48;

/// The sector the city fills, as bearings right of the way the car park faces, in degrees, and
/// distances from the parked car. West of its left edge the course's own hillside stands up to 7°
/// below the horizon from the title camera and would hide it; past its far edge the ridges begin.
pub const CITY_LEFT: f32 = -5.0;
pub const CITY_RIGHT: f32 = 55.0;
pub const CITY_NEAR: f32 = 850.0;
pub const CITY_FAR: f32 = 1750.0;

/// No city light closer to the road than this, as for the valley lights.
pub const CITY_CLEARANCE: f32 = 260.0;

/// Where the parked car is, and the bearing it faces, in radians from +x toward +z.
pub fn origin(track: &Track) -> (f32, f32, f32) {
    let (x, _, z, yaw, _) = carpark_parking(track);
    // `yaw` is the game's heading, measured from +z toward +x.
    (x, z, PI * 0.5 - yaw)
}

/// A point on the basin floor `right` degrees right of the car park's facing and `dist` metres out.
pub fn at(track: &Track, right: f32, dist: f32) -> Vec3 {
    let (x, z, facing) = origin(track);
    // Bearings grow toward +z, which is to the car's right as it faces the city.
    let b = facing + right * PI / 180.0;
    Vec3::new(x + cos(b) * dist, CITY_FLOOR, z + sin(b) * dist)
}

/// The downtown tower cluster, the smaller one further right, and the broadcast tower.
pub fn downtown(track: &Track) -> Vec3 {
    at(track, 8.0, 1350.0)
}
pub fn subcentre(track: &Track) -> Vec3 {
    at(track, 22.0, 1650.0)
}
pub fn broadcast_tower(track: &Track) -> (Vec3, f32) {
    (at(track, 0.5, 1450.0), 250.0)
}

/// Where a point lies from the car park: degrees right of its facing, and metres out.
pub fn bearing_of(track: &Track, p: Vec3) -> (f32, f32) {
    Basin::new(track).bearing_of(p)
}

/// Inside the city's sector.
pub fn in_city(track: &Track, p: Vec3) -> bool {
    Basin::new(track).in_city(p)
}

/// The points every placement is measured from, worked out once. Each takes a walk over the car
/// park or the whole course to find, and the street grid alone asks after some 35,000 candidate
/// lights: finding them afresh every time was most of the black screen at boot.
struct Basin {
    x: f32,
    z: f32,
    facing: f32,
    downtown: Vec3,
    subcentre: Vec3,
    track_centre: Vec3,
}

impl Basin {
    fn new(track: &Track) -> Basin {
        let (x, z, facing) = origin(track);
        Basin {
            x,
            z,
            facing,
            downtown: downtown(track),
            subcentre: subcentre(track),
            track_centre: track_footprint(track).0,
        }
    }

    fn bearing_of(&self, p: Vec3) -> (f32, f32) {
        let mut d = atan2(p.z - self.z, p.x - self.x) - self.facing;
        while d > PI {
            d -= 2.0 * PI;
        }
        while d < -PI {
            d += 2.0 * PI;
        }
        (d * 180.0 / PI, hypot(p.x - self.x, p.z - self.z))
    }

    fn in_city(&self, p: Vec3) -> bool {
        let (b, d) = self.bearing_of(p);
        (CITY_LEFT..=CITY_RIGHT).contains(&b) && (CITY_NEAR..=CITY_FAR).contains(&d)
    }

    /// How built up the city is at `p`: 1 at the middle of the clusters, falling off quickly.
    fn density(&self, p: Vec3) -> f32 {
        let mut k = 0.0;
        for (c, r, w) in [(self.downtown, 420.0, 1.0), (self.subcentre, 300.0, 0.6)] {
            let d = p.horizontal_distance(c) / r;
            k += w / (1.0 + d * d * d * d);
        }
        min(1.0, 0.1 + k)
    }

    /// A place a light may go: in the sector, clear of the road, and in front of the ridges from
    /// the top of the course.
    fn allowed(&self, track: &Track, p: Vec3) -> bool {
        self.in_city(p)
            && clear_of_road(track, p)
            && nearer_than_ridges_from(track, self.track_centre, p, CITY_SEEN_FROM)
    }
}

fn clear_of_road(track: &Track, p: Vec3) -> bool {
    let mut i = 0;
    while i < track.nodes.len() {
        let d = track.nodes[i].p.horizontal_distance(p);
        if d < CITY_CLEARANCE {
            return false;
        }
        i = track.next_in_reach(i, 8, d, CITY_CLEARANCE);
    }
    true
}

/// The stretch of course the city must stay in front of the ridges from: the summit and the first
/// hairpin below it, where it fills the view.
pub const CITY_SEEN_FROM: usize = 400;

/// Fills `out` with the city's lights and returns how many were placed.
pub fn city_lights(track: &Track, out: &mut [ValleyLight]) -> usize {
    let mut rng = Rng::new(0xC17E_0011);
    let basin = Basin::new(track);
    let mut w = 0usize;
    let cap = out.len();
    let push = |out: &mut [ValleyLight], w: &mut usize, p: Vec3, kind: LightKind| {
        if *w < cap {
            out[*w] = ValleyLight { p, kind, size: 1 };
            *w += 1;
        }
    };

    // The street grid, skewed to the view and centred on downtown: a light every 12 m along
    // streets 110 m apart, and a brighter arterial every fifth street.
    let centre = basin.downtown;
    let g = basin.facing + 40.0 * PI / 180.0;
    let (ca, sa) = (cos(g), sin(g));
    for axis in 0..2 {
        for j in -24i32..=24 {
            let off = j as f32 * 110.0 + (rng.next() - 0.5) * 30.0;
            let major = j % 5 == 0;
            for t in -180i32..=180 {
                let s = t as f32 * 12.0;
                let (u, v) = if axis == 0 { (s, off) } else { (off, s) };
                let p = Vec3::new(centre.x + u * ca - v * sa, CITY_FLOOR + 1.5, centre.z + u * sa + v * ca);
                let keep = basin.density(p) * if major { 1.0 } else { 0.7 };
                if rng.next() > keep || !basin.allowed(track, p) {
                    continue;
                }
                let r = rng.next();
                let kind = if major || r < 0.45 {
                    LightKind::Sodium
                } else if r < 0.8 {
                    LightKind::WarmWhite
                } else {
                    LightKind::ColdWhite
                };
                // Now and then a sign, where it is busiest.
                let d = basin.density(p);
                let kind = if rng.next() < 0.03 * d * d {
                    match (rng.next() * 3.0) as u32 {
                        0 => LightKind::Pink,
                        1 => LightKind::Cyan,
                        _ => LightKind::Red,
                    }
                } else {
                    kind
                };
                push(out, &mut w, p, kind);
            }
        }
    }

    // Two elevated expressways, 18 m up: sodium lamps every 7 m, and the tail lights and
    // headlights of the traffic on them.
    let lines: [[(f32, f32); 5]; 2] = [
        [(37.0, 950.0), (23.0, 1150.0), (8.0, 1350.0), (-3.0, 1600.0), (-4.0, 1740.0)],
        [(45.0, 1500.0), (25.0, 1450.0), (12.0, 1250.0), (2.0, 1000.0), (-2.0, 900.0)],
    ];
    for line in lines.iter() {
        for k in 0..line.len() - 1 {
            let a = at(track, line[k].0, line[k].1);
            let b = at(track, line[k + 1].0, line[k + 1].1);
            let n = (a.horizontal_distance(b) / 7.0) as usize;
            for i in 0..n {
                let t = i as f32 / n as f32;
                let p = Vec3::new(a.x + (b.x - a.x) * t, CITY_FLOOR + 18.0, a.z + (b.z - a.z) * t);
                if !basin.allowed(track, p) {
                    continue;
                }
                push(out, &mut w, p, LightKind::Sodium);
                if i % 3 == 0 {
                    let q = Vec3::new(p.x + 3.0, p.y, p.z + 3.0);
                    push(out, &mut w, q, if i % 2 == 0 { LightKind::Red } else { LightKind::ColdWhite });
                }
            }
        }
    }

    // A railway, in a gentle curve across the city, with a lit train on it.
    let (a, b) = (at(track, 50.0, 1200.0), at(track, -2.0, 1700.0));
    for k in 0..500 {
        let t = k as f32 / 500.0;
        let bend = sin(t * 5.0) * 90.0;
        let p = Vec3::new(a.x + (b.x - a.x) * t + bend, CITY_FLOOR + 8.0, a.z + (b.z - a.z) * t);
        let train = t > 0.3 && t < 0.33;
        if (train || k % 4 == 0) && basin.allowed(track, p) {
            push(out, &mut w, p, if train { LightKind::ColdWhite } else { LightKind::Green });
        }
    }
    w
}

/// One tower: a flat dark box standing on the basin floor.
#[derive(Clone, Copy, Debug)]
pub struct Tower {
    pub base: Vec3,
    pub half_w: f32,
    pub half_d: f32,
    pub height: f32,
    /// Warm windows, or cold office ones.
    pub warm: bool,
}

impl Tower {
    pub const ZERO: Tower = Tower { base: Vec3::ZERO, half_w: 0.0, half_d: 0.0, height: 0.0, warm: true };

    /// Tall enough to carry a red aircraft beacon.
    pub fn beacon(&self) -> bool {
        self.height > 110.0
    }
}

/// Fills `out` with the towers and returns how many were placed, farthest from the car park first.
///
/// Drawn without a depth test in that order, so each one paints over the ones behind it. From
/// anywhere else on the course the order is nearly the same, since the city is far from all of it.
pub fn city_towers(track: &Track, out: &mut [Tower]) -> usize {
    let mut rng = Rng::new(0x70E4_2211);
    let basin = Basin::new(track);
    let mut w = 0usize;
    for (c, n, spread, top) in [(basin.downtown, 34, 260.0, 260.0), (basin.subcentre, 14, 180.0, 170.0)] {
        for _ in 0..n {
            if w >= out.len() {
                break;
            }
            let a = rng.next() * 2.0 * PI;
            let d = abs(rng.spread()) * spread * 0.5;
            let base = Vec3::new(c.x + cos(a) * d, CITY_FLOOR, c.z + sin(a) * d);
            let r = rng.next();
            let height = r * r * top * max(0.4, 1.0 - d / spread * 0.5) + 40.0;
            let (hw, hd) = (13.0 + rng.next() * 11.0, 13.0 + rng.next() * 11.0);
            let warm = rng.next() < 0.6;
            if !basin.allowed(track, base) {
                continue;
            }
            out[w] = Tower { base, half_w: hw, half_d: hd, height, warm };
            w += 1;
        }
    }
    // Farthest first.
    let far = |t: &Tower| hypot(t.base.x - basin.x, t.base.z - basin.z);
    for i in 1..w {
        let mut j = i;
        while j > 0 && far(&out[j]) > far(&out[j - 1]) {
            out.swap(j, j - 1);
            j -= 1;
        }
    }
    w
}
