//! Deterministic track generation.
//!
//! The whole 3.5 km descent is generated at boot from the section plan below, so nothing has to
//! ship as a mesh. A turtle walk lays down control points, a Catmull-Rom spline is sampled through
//! them, and every gameplay query afterwards runs against that flat sampled array rather than the
//! spline itself.
//!
//! The spline is a uniformly parameterised Catmull-Rom with tension 0.5, open at both ends. The
//! parameterisation matters: centripetal or chordal spacing would place the nodes differently and
//! move every corner, so it is fixed here rather than left to taste.

use crate::math::{Vec2, Vec3, asin, atan2, clamp, cos, hypot, radians, sin, PI};

/// `[curvature in degrees per step, number of steps]`, applied in order.
const PLAN: [(f32, u16); 34] = [
    (0.0, 4),
    (2.6, 7),
    (-9.6, 13),
    (0.0, 3),
    (6.2, 8),
    (10.2, 13),
    (-2.2, 5),
    (-7.0, 10),
    (9.6, 13),
    (0.0, 3),
    (-4.6, 9),
    (-10.4, 13),
    (3.2, 6),
    (8.2, 9),
    (-9.2, 13),
    (0.0, 3),
    (5.4, 8),
    (-3.2, 7),
    (10.6, 13),
    (-2.4, 5),
    (-8.6, 13),
    (4.2, 7),
    (-5.2, 8),
    (9.4, 13),
    (0.0, 3),
    (-9.8, 13),
    (3.4, 6),
    (6.4, 8),
    (-4.2, 7),
    (-10.0, 13),
    (2.6, 6),
    (7.6, 9),
    (-8.8, 13),
    (0.0, 5),
];

/// Horizontal distance covered by one turtle step.
const STEP: f32 = 12.0;

const fn count_control_points() -> usize {
    // Two lead-in points, then one per turtle step. The turtle's own origin is not a control
    // point — the first step has already moved by the time the first one is pushed.
    let mut n = 2;
    let mut i = 0;
    while i < PLAN.len() {
        n += PLAN[i].1 as usize;
        i += 1;
    }
    n
}

pub const CONTROL_POINT_COUNT: usize = count_control_points();
pub const SAMPLES_PER_CONTROL_POINT: usize = 9;
/// Sampling `9 × controlPoints` divisions yields one more point than divisions.
pub const NODE_COUNT: usize = CONTROL_POINT_COUNT * SAMPLES_PER_CONTROL_POINT + 1;

/// Beyond this `|curv|`, a node counts as being in a corner (used for prop placement).
pub const CORNER_CURVATURE: f32 = 0.045;

// --- Emergency pull-off ---------------------------------------------------------
/// Node the gravel pad is centred on; also where the title screen parks the car.
pub const BAY_NODE: usize = 30;
/// Which side of the centreline the bay is on, as a sign applied to `lat`.
pub const BAY_SIDE: f32 = 1.0;
/// Node range over which the guard rail on the bay side is omitted.
pub const BAY_FROM: usize = 16;
pub const BAY_TO: usize = 46;

/// Lateral distance at which the guard rail stops the car.
pub const RAIL_LIMIT: f32 = 7.15;
/// The rail is missing across the bay, so containment opens up to the far edge of the pull-off.
pub const BAY_LIMIT: f32 = 16.5;
/// Beyond this lateral offset the car is off the tarmac and on to loose surface.
pub const TARMAC_HALF_WIDTH: f32 = 5.3;

/// One sampled point on the centreline. Every gameplay query resolves to one of these.
#[derive(Clone, Copy, Debug, Default)]
pub struct Node {
    /// World position.
    pub p: Vec3,
    /// Normalised horizontal tangent.
    pub dir: Vec2,
    /// Horizontal left normal — positive `lat` is to the left of the direction of travel.
    pub nrm: Vec2,
    /// Cumulative arclength from the start.
    pub s: f32,
    /// Slope angle, negative going downhill.
    pub pitch: f32,
    /// Signed turn tightness over a ±4 node window.
    pub curv: f32,
}

pub struct Track {
    pub nodes: [Node; NODE_COUNT],
    /// Total arclength in metres.
    pub length: f32,
}

impl Track {
    /// A zeroed track. Large enough (~100 KB) that callers should place it in a `static` on the
    /// PSP, or box it on the host, rather than holding one on the stack.
    pub const EMPTY: Track = Track {
        nodes: [Node {
            p: Vec3::ZERO,
            dir: Vec2::new(0.0, 0.0),
            nrm: Vec2::new(0.0, 0.0),
            s: 0.0,
            pitch: 0.0,
            curv: 0.0,
        }; NODE_COUNT],
        length: 0.0,
    };

    /// Fills `dst` with the generated centreline. Writes in place so the caller's storage — a
    /// `static mut` on the PSP — never has to be copied.
    pub fn generate(dst: &mut Track) {
        let ctrl = build_control_points();
        sample_spline(&ctrl, dst);
        derive_frames(dst);
        derive_curvature(dst);
    }

    #[inline]
    pub fn last_index(&self) -> usize {
        NODE_COUNT - 1
    }

    /// How far down the descent a node index is, in `0.0..=1.0`.
    #[inline]
    pub fn progress(&self, index: usize) -> f32 {
        index as f32 / (NODE_COUNT - 1) as f32
    }
}

/// The turtle walk.
fn build_control_points() -> [Vec3; CONTROL_POINT_COUNT] {
    let mut ctrl = [Vec3::ZERO; CONTROL_POINT_COUNT];

    ctrl[0] = Vec3::new(0.0, 0.0, 34.0);
    ctrl[1] = Vec3::new(0.0, 0.0, 16.0);

    let (mut x, mut y, mut z) = (0.0f32, 0.0f32, 0.0f32);
    let mut heading = PI;
    let mut w = 2;

    for &(degrees, steps) in PLAN.iter() {
        let curvature = radians(degrees);
        // Tight turns descend more shallowly, which keeps hairpins from becoming ski jumps.
        let drop = 0.92 - crate::math::min(0.45, crate::math::abs(degrees) * 0.045);
        for _ in 0..steps {
            heading += curvature;
            x += sin(heading) * STEP;
            z += cos(heading) * STEP;
            y -= drop;
            ctrl[w] = Vec3::new(x, y, z);
            w += 1;
        }
    }

    debug_assert!(w == CONTROL_POINT_COUNT);
    ctrl
}

/// One axis of a Catmull-Rom segment, as a cubic through the two inner control points.
#[inline]
fn cubic(x0: f32, x1: f32, x2: f32, x3: f32, tension: f32, w: f32) -> f32 {
    let t0 = tension * (x2 - x0);
    let t1 = tension * (x3 - x1);
    let c0 = x1;
    let c1 = t0;
    let c2 = -3.0 * x1 + 3.0 * x2 - 2.0 * t0 - t1;
    let c3 = 2.0 * x1 - 2.0 * x2 + t0 + t1;
    c0 + c1 * w + c2 * w * w + c3 * w * w * w
}

const TENSION: f32 = 0.5;

/// Evaluates the open, uniformly-parameterised Catmull-Rom spline at `t ∈ [0, 1]`.
fn spline_point(ctrl: &[Vec3; CONTROL_POINT_COUNT], t: f32) -> Vec3 {
    let l = CONTROL_POINT_COUNT;
    let p = (l - 1) as f32 * t;
    let mut int_point = crate::math::floor(p) as usize;
    let mut weight = p - int_point as f32;

    // The very last sample lands exactly on a knot; walk it back into the final segment.
    if weight == 0.0 && int_point == l - 1 {
        int_point = l - 2;
        weight = 1.0;
    }

    // The ends have no neighbour to reach for, so one is reflected outward.
    let p0 = if int_point > 0 {
        ctrl[int_point - 1]
    } else {
        ctrl[0].scale(2.0).sub(ctrl[1])
    };
    let p1 = ctrl[int_point];
    let p2 = ctrl[int_point + 1];
    let p3 = if int_point + 2 < l {
        ctrl[int_point + 2]
    } else {
        ctrl[l - 1].scale(2.0).sub(ctrl[l - 2])
    };

    Vec3::new(
        cubic(p0.x, p1.x, p2.x, p3.x, TENSION, weight),
        cubic(p0.y, p1.y, p2.y, p3.y, TENSION, weight),
        cubic(p0.z, p1.z, p2.z, p3.z, TENSION, weight),
    )
}

fn sample_spline(ctrl: &[Vec3; CONTROL_POINT_COUNT], dst: &mut Track) {
    let divisions = NODE_COUNT - 1;
    for d in 0..=divisions {
        dst.nodes[d].p = spline_point(ctrl, d as f32 / divisions as f32);
    }
}

/// Tangent, normal, pitch and arclength from each node's neighbours.
fn derive_frames(dst: &mut Track) {
    let last = NODE_COUNT - 1;
    let mut acc = 0.0f32;

    for i in 0..NODE_COUNT {
        let a = dst.nodes[if i == 0 { 0 } else { i - 1 }].p;
        let b = dst.nodes[if i + 1 > last { last } else { i + 1 }].p;

        let (mut dx, mut dz) = (b.x - a.x, b.z - a.z);
        let horizontal = hypot(dx, dz);
        let l = if horizontal == 0.0 { 1.0 } else { horizontal };
        dx /= l;
        dz /= l;

        if i > 0 {
            acc += dst.nodes[i].p.sub(dst.nodes[i - 1].p).length();
        }

        let node = &mut dst.nodes[i];
        node.dir = Vec2::new(dx, dz);
        node.nrm = Vec2::new(-dz, dx);
        node.pitch = atan2(b.y - a.y, horizontal);
        node.s = acc;
    }

    dst.length = acc;
}

/// Signed turn tightness across a ±4 node window.
fn derive_curvature(dst: &mut Track) {
    let last = NODE_COUNT - 1;
    for i in 0..NODE_COUNT {
        let a = dst.nodes[i.saturating_sub(4)].dir;
        let b = dst.nodes[if i + 4 > last { last } else { i + 4 }].dir;
        let cross = a.x * b.z - a.z * b.x;
        dst.nodes[i].curv = asin(clamp(-cross, -1.0, 1.0));
    }
}

/// Where the car sits relative to the centreline.
#[derive(Clone, Copy, Debug, Default)]
pub struct Query {
    /// Index of the nearest node.
    pub index: usize,
    /// Signed lateral offset from the centreline; positive is to the left.
    pub lat: f32,
    /// Signed longitudinal offset within that node.
    pub along: f32,
    /// Non-zero only when clamped at either end of the track — used for end containment.
    pub over: f32,
}

/// Stateful nearest-node search. Keeping the previous index turns a 2620-node scan into a
/// ~150-node one, which is what makes this affordable per substep on hardware.
#[derive(Clone, Copy, Debug, Default)]
pub struct Locator {
    pub last_idx: usize,
}

impl Locator {
    pub const fn new() -> Self {
        Locator { last_idx: 0 }
    }

    /// Resets the search window, for when the car is teleported (respawn, run start).
    pub fn reset_to(&mut self, index: usize) {
        self.last_idx = index;
    }

    pub fn nearest(&mut self, track: &Track, px: f32, pz: f32) -> Query {
        let last = NODE_COUNT - 1;
        let from = self.last_idx.saturating_sub(60);
        let to = crate::math::umin(last, self.last_idx + 90);

        let mut best_i = self.last_idx;
        let mut best_d = f32::INFINITY;
        for i in from..=to {
            let dx = px - track.nodes[i].p.x;
            let dz = pz - track.nodes[i].p.z;
            let d = dx * dx + dz * dz;
            if d < best_d {
                best_d = d;
                best_i = i;
            }
        }

        // Lost the thread — the car has been thrown clear of the window. Sweep the whole track
        // at stride 3; the follow-up query will re-narrow.
        if best_d > 6000.0 {
            best_d = f32::INFINITY;
            let mut i = 0;
            while i < NODE_COUNT {
                let dx = px - track.nodes[i].p.x;
                let dz = pz - track.nodes[i].p.z;
                let d = dx * dx + dz * dz;
                if d < best_d {
                    best_d = d;
                    best_i = i;
                }
                i += 3;
            }
        }

        self.last_idx = best_i;
        let n = &track.nodes[best_i];
        let vx = px - n.p.x;
        let vz = pz - n.p.z;
        let along = vx * n.dir.x + vz * n.dir.z;

        let mut over = 0.0;
        if best_i <= 1 && along < 0.0 {
            over = along;
        }
        if best_i >= last - 1 && along > 0.0 {
            over = along;
        }

        Query {
            index: best_i,
            lat: vx * n.nrm.x + vz * n.nrm.z,
            along,
            over,
        }
    }
}
