//! Where the scenery goes: the far ridgelines, the lit valley below the pass, and the arithmetic
//! behind the rest of the roadside dressing.
//!
//! Everything here is a pure function of the track and a fixed seed, so the world is identical on
//! every boot and a headless capture of it is reproducible byte for byte. The shell turns these
//! numbers into vertices; nothing in it decides where anything stands.

use crate::math::{abs, clamp, cos, max, sin, sqrt, Vec3, TAU};
use crate::track::Track;

/// Projection far plane, and the distance past which chunks are culled. Geometry beyond this is
/// clipped by the hardware anyway, so nothing visible can be lost by using it.
pub const DRAW_DISTANCE: f32 = 2400.0;

/// Where the moon hangs, as a yaw about +Y (0 = +Z) and the sine of its elevation. The sky dome and
/// every moonlit rim take their direction from these two numbers.
pub const MOON_YAW: f32 = 2.1;
pub const MOON_HEIGHT: f32 = 0.55;

/// Unit vector toward the moon.
pub fn moon_dir() -> Vec3 {
    let ring = sqrt(1.0 - MOON_HEIGHT * MOON_HEIGHT);
    Vec3::new(sin(MOON_YAW) * ring, MOON_HEIGHT, cos(MOON_YAW) * ring)
}

// --- deterministic noise ------------------------------------------------------------------------

/// A 32-bit integer hash (lowbias32). Stateless, so any placement can be recomputed from its index.
pub const fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    x
}

/// `hash` mapped onto `0.0..1.0`.
pub fn unit(x: u32) -> f32 {
    (hash(x) >> 8) as f32 / (1u32 << 24) as f32
}

/// A small xorshift stream for the places where a sequence is easier to read than an index.
pub struct Rng(u32);

impl Rng {
    pub const fn new(seed: u32) -> Rng {
        Rng(if seed == 0 { 0x9E37_79B9 } else { seed })
    }

    pub fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Roughly normal, mean 0, deviation about 1 (the sum of three uniforms).
    pub fn spread(&mut self) -> f32 {
        (self.next() + self.next() + self.next() - 1.5) * 2.0
    }
}

// --- ridgelines ---------------------------------------------------------------------------------

/// One ring of mountains around the pass.
///
/// A ring is centred on a point that slides from the middle of the track toward the camera by
/// `follow`: 1.0 would pin it to the eye like the moon (no parallax at all), 0.0 would fix it in
/// the world. In between, the far bands barely move and the near one drifts a little, which reads
/// as depth without letting any band come within reach of the far plane.
#[derive(Clone, Copy, Debug)]
pub struct RidgeBand {
    pub radius: f32,
    pub follow: f32,
    /// World height of the lowest and highest crest.
    pub low: f32,
    pub high: f32,
    /// Where the band's foot is, well below anything it could be seen against.
    pub foot: f32,
    pub seed: u32,
}

/// Far to near: the order they are drawn in.
pub const RIDGE_BANDS: [RidgeBand; 3] = [
    RidgeBand { radius: 2170.0, follow: 0.95, low: 60.0, high: 400.0, foot: -760.0, seed: 0x21 },
    RidgeBand { radius: 1990.0, follow: 0.93, low: -20.0, high: 250.0, foot: -760.0, seed: 0x34 },
    RidgeBand { radius: 1800.0, follow: 0.91, low: -110.0, high: 120.0, foot: -760.0, seed: 0x47 },
];

/// Columns per band: ~45 m apart at 1.8 km, fine enough for the 89th harmonic to show as a notch in
/// the crest rather than as a chain of straight segments.
pub const RIDGE_COLUMNS: usize = 256;

/// Integer harmonics, so the profile is exactly periodic and the ring closes without a seam. The
/// low ones are smooth and set where the massifs are; the rest are ridged (`1 - 2|sin|`), which is
/// what turns rolling hills into crests with sharp tops and broad gullies between them.
const HARMONICS: [(f32, f32, bool); 8] = [
    (3.0, 1.0, false),
    (5.0, 0.7, false),
    (8.0, 0.5, true),
    (13.0, 0.36, true),
    (21.0, 0.22, true),
    (34.0, 0.13, true),
    (55.0, 0.07, true),
    (89.0, 0.035, true),
];

/// Crest height of a band at `angle` (radians about +Y, 0 = +Z).
pub fn ridge_height(band: &RidgeBand, angle: f32) -> f32 {
    let mut sum = 0.0;
    let mut norm = 0.0;
    for (i, (k, amp, ridged)) in HARMONICS.iter().enumerate() {
        let phase = unit(band.seed * 16 + i as u32) * TAU;
        let v = if *ridged {
            // |sin(x/2)| repeats every 2π/k when x = k·angle, so this stays periodic.
            1.0 - 2.0 * abs(sin((k * angle + phase) * 0.5))
        } else {
            sin(k * angle + phase)
        };
        sum += amp * v;
        norm += amp;
    }
    let v = clamp(0.5 + 0.5 * sum / norm * 1.35, 0.0, 1.0);
    band.low + (band.high - band.low) * v
}

/// How strongly the moon picks out a crest at `angle`, `0.0..=1.0`.
///
/// Crests toward the moon are backlit and get a bright rim; those facing away keep a faint one so
/// the silhouette still separates from the band behind it.
pub fn rim_light(angle: f32) -> f32 {
    let facing = max(0.0, cos(angle - MOON_YAW));
    0.18 + 0.82 * facing * sqrt(facing)
}

/// Where a band's ring is centred this frame.
pub fn ridge_centre(band: &RidgeBand, track_centre: Vec3, eye: Vec3) -> Vec3 {
    Vec3::new(
        track_centre.x + (eye.x - track_centre.x) * band.follow,
        0.0,
        track_centre.z + (eye.z - track_centre.z) * band.follow,
    )
}

/// Middle of the track's horizontal footprint, and the farthest any node is from it.
pub fn track_footprint(track: &Track) -> (Vec3, f32) {
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
    for n in track.nodes.iter() {
        min_x = crate::math::min(min_x, n.p.x);
        max_x = max(max_x, n.p.x);
        min_z = crate::math::min(min_z, n.p.z);
        max_z = max(max_z, n.p.z);
    }
    let centre = Vec3::new((min_x + max_x) * 0.5, 0.0, (min_z + max_z) * 0.5);
    let mut reach = 0.0f32;
    for n in track.nodes.iter() {
        reach = max(reach, n.p.horizontal_distance(centre));
    }
    (centre, reach)
}

// --- the valley ---------------------------------------------------------------------------------

/// Lit points in the valley below the pass.
pub const VALLEY_LIGHTS: usize = 420;

/// How far below the lowest node the valley floor lies at the foot of the pass.
///
/// Not far. A town a few hundred metres under the car sits twenty or thirty degrees below the
/// horizon, where the hillside the car is on always covers it; the valley only shows as lights
/// strung along the foot of the far ridges, a few degrees down. So the floor is shallow, and
/// rises away from the pass (see [`VALLEY_RISE`]) the way the far side of a valley does.
pub const VALLEY_FLOOR_DROP: f32 = 45.0;

/// How much the valley floor climbs per metre away from the road.
pub const VALLEY_RISE: f32 = 0.11;

/// No light is placed closer than this to the road, which keeps every one of them off the terrain
/// ribbon (190 m to either side) and out past where the hillside could stand in front of it.
pub const VALLEY_CLEARANCE: f32 = 260.0;

/// What a light is, which the shell maps to a colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LightKind {
    Sodium,
    WarmWhite,
    ColdWhite,
    Amber,
    Red,
}

#[derive(Clone, Copy, Debug)]
pub struct ValleyLight {
    pub p: Vec3,
    pub kind: LightKind,
    /// Side of its square in pixels, 1 or 2.
    pub size: u8,
}

impl ValleyLight {
    pub const ZERO: ValleyLight = ValleyLight { p: Vec3::ZERO, kind: LightKind::Sodium, size: 1 };
}

/// A town: where its centre is, how big, how many lights.
#[derive(Clone, Copy, Debug)]
pub struct Town {
    pub centre: Vec3,
    pub radius: f32,
}

/// The two towns either side of the ridge the pass runs down, placed from the track's footprint.
pub fn towns(track: &Track) -> [Town; 2] {
    let (centre, _) = track_footprint(track);
    let (mut min_x, mut max_x, mut min_y) = (f32::MAX, f32::MIN, f32::MAX);
    for n in track.nodes.iter() {
        min_x = crate::math::min(min_x, n.p.x);
        max_x = max(max_x, n.p.x);
        min_y = crate::math::min(min_y, n.p.y);
    }
    let floor = min_y - VALLEY_FLOOR_DROP;
    [
        Town { centre: Vec3::new(max_x + 560.0, floor, centre.z - 120.0), radius: 200.0 },
        Town { centre: Vec3::new(min_x - 600.0, floor - 15.0, centre.z + 240.0), radius: 160.0 },
    ]
}

/// Horizontal distance from `p` to the nearest node, sampled every sixth.
fn road_distance(track: &Track, p: Vec3) -> f32 {
    let mut best = f32::MAX;
    let mut i = 0;
    while i < track.nodes.len() {
        best = crate::math::min(best, track.nodes[i].p.horizontal_distance(p));
        i += 6;
    }
    best
}

fn clear_of_road(track: &Track, p: Vec3) -> bool {
    road_distance(track, p) >= VALLEY_CLEARANCE
}

/// `p` lifted onto the valley floor's rise.
fn on_floor(track: &Track, p: Vec3) -> Vec3 {
    let d = road_distance(track, p);
    Vec3::new(p.x, p.y + max(0.0, d - VALLEY_CLEARANCE) * VALLEY_RISE, p.z)
}

fn kind_for(r: f32) -> LightKind {
    match (r * 100.0) as u32 {
        0..=44 => LightKind::Sodium,
        45..=69 => LightKind::WarmWhite,
        70..=81 => LightKind::ColdWhite,
        82..=93 => LightKind::Amber,
        _ => LightKind::Red,
    }
}

/// Fills `out` with the valley's lights and returns how many were placed.
///
/// Two towns, a string of street lights along the valley road joining them, and a scatter of
/// farm lights between. A candidate that lands too close to the road is dropped rather than moved,
/// so the count can come in a little under `out.len()`.
pub fn valley_lights(track: &Track, out: &mut [ValleyLight]) -> usize {
    let towns = towns(track);
    let mut rng = Rng::new(0x7A11_E7);
    let mut w = 0usize;
    let cap = out.len();
    let push = |out: &mut [ValleyLight], w: &mut usize, l: ValleyLight| {
        if *w < cap && clear_of_road(track, l.p) && nearer_than_ridges(track, l.p) {
            out[*w] = ValleyLight { p: on_floor(track, l.p), ..l };
            *w += 1;
        }
    };

    // Towns: dense at the middle, thinning out.
    for (t, town) in towns.iter().enumerate() {
        let n = if t == 0 { 190 } else { 140 };
        for _ in 0..n {
            let dx = rng.spread() * town.radius * 0.5;
            let dz = rng.spread() * town.radius * 0.5;
            let p = Vec3::new(town.centre.x + dx, town.centre.y + rng.next() * 6.0, town.centre.z + dz);
            let kind = kind_for(rng.next());
            let size = if rng.next() < 0.06 { 2 } else { 1 };
            push(out, &mut w, ValleyLight { p, kind, size });
        }
    }

    // A valley road through each town, running along the foot of the pass. Street lights are
    // sodium and evenly spaced, which is what makes a line of them read as a road.
    for (t, town) in towns.iter().enumerate() {
        let side = if t == 0 { 1.0 } else { -1.0 };
        for k in 0..44 {
            let u = k as f32 / 43.0 - 0.5;
            let p = Vec3::new(
                town.centre.x + side * sin(u * 2.4) * 140.0,
                town.centre.y + 2.0,
                town.centre.z + u * 900.0,
            );
            push(out, &mut w, ValleyLight { p, kind: LightKind::Sodium, size: 1 });
        }
    }

    // Farms and junctions: isolated, mostly warm, on the valley floor either side of the pass.
    let (centre, _) = track_footprint(track);
    for _ in 0..140 {
        let side = if rng.next() < 0.5 { -1.0 } else { 1.0 };
        let p = Vec3::new(
            centre.x + side * (380.0 + rng.next() * 700.0),
            towns[0].centre.y + rng.next() * 30.0,
            centre.z + (rng.next() - 0.5) * 1300.0,
        );
        let kind = if rng.next() < 0.7 { LightKind::WarmWhite } else { LightKind::Sodium };
        push(out, &mut w, ValleyLight { p, kind, size: 1 });
    }
    w
}

/// True when a light at `p` is always nearer to the eye than the nearest ridge band, from any point
/// on the track — the condition for drawing the lights after the ridges without a depth test.
pub fn nearer_than_ridges(track: &Track, p: Vec3) -> bool {
    let (centre, _) = track_footprint(track);
    let near = &RIDGE_BANDS[RIDGE_BANDS.len() - 1];
    let mut i = 0;
    while i < track.nodes.len() {
        let eye = track.nodes[i].p;
        let c = ridge_centre(near, centre, eye);
        // The band's closest approach to this eye.
        let closest = near.radius - eye.horizontal_distance(c);
        if eye.horizontal_distance(p) >= closest {
            return false;
        }
        i += 4;
    }
    true
}

/// How far out a band ever gets from any eye on the track.
pub fn ridge_farthest(track: &Track, band: &RidgeBand) -> f32 {
    let (centre, _) = track_footprint(track);
    let mut far = 0.0f32;
    for n in track.nodes.iter() {
        let c = ridge_centre(band, centre, n.p);
        far = max(far, band.radius + n.p.horizontal_distance(c));
    }
    // Height adds a little: the crest is above or below the eye as well as out from it.
    far + abs(band.high) * 0.05
}
