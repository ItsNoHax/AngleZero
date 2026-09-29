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

/// Sections of [`PLAN`] that fall at a fixed rate instead of by their curvature, as
/// `(section, metres dropped per step)`.
///
/// The first section, which the summit car park stands beside, falls gently at 3.3 % so the
/// paving is near enough level. The next three, 276 m, fall at 13.75 %: the course drops off the
/// summit, and the first hairpin lies 28 m below the car park. The title camera looks down on that
/// hairpin; at the curvature rule's 0.5-0.9 m a step it lay only 14 m down and showed as a line
/// along the horizon.
const STEEP: [(usize, f32); 4] = [(0, 0.4), (1, 1.65), (2, 1.65), (3, 1.65)];

/// Metres section `i` falls per turtle step.
fn section_drop(i: usize, degrees: f32) -> f32 {
    let mut k = 0;
    while k < STEEP.len() {
        if STEEP[k].0 == i {
            return STEEP[k].1;
        }
        k += 1;
    }
    // Tight turns descend more shallowly, which keeps hairpins from becoming ski jumps.
    0.92 - crate::math::min(0.45, crate::math::abs(degrees) * 0.045)
}

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

// --- The summit car park ---------------------------------------------------------
//
// The course starts at the summit, and beside the start line on the right is a car park looking
// out over the city. The title screen parks the car in it. Its outline is described in the road's
// own frame, `along` metres of arclength from node 0 and `lateral` metres out, because it butts
// against the road ribbon and has to be cut on the same nodes (see `carpark_span`). The road is
// straight here, so that frame is also square to the world.
//
// Seen from above the paving is a wedge: a short end wall at the top running out from the rail, a
// 45° parapet facing down the valley toward the city, and a short end wall back to the rail at the
// bottom.

/// Which side of the centreline the car park is on, as a sign applied to `lat`.
pub const CARPARK_SIDE: f32 = 1.0;

/// Where the road ribbon's outermost station sits, and so where the paving has to meet it.
///
/// The road is drawn as an extruded cross-section that ends at this lateral offset, level with
/// the centreline. The car park's paving picks up from exactly here, which is the whole trick: it
/// is not a pad placed near the road, it is the road's own surface continuing outwards.
pub const ROAD_SHOULDER: f32 = 6.4;

/// The car park's centre and half-length along the road, before snapping to the ribbon's cuts.
const CARPARK_CENTRE: f32 = 23.0;
const CARPARK_HALF: f32 = 21.0;
/// Length of the top end wall, along the road, and how far out the parapet begins.
pub const CARPARK_NORTH_TAPER: f32 = 8.0;
pub const CARPARK_REACH: f32 = 42.0;
/// Length of the bottom end wall, along the road.
pub const CARPARK_SOUTH_TAPER: f32 = 4.0;

/// Where the paving starts and stops along the road: on the road ribbon's own cuts, so the two
/// share vertices along their joint and it is watertight.
///
/// This is what made the old lay-by stop flickering. Two surfaces cut at different intervals are
/// two piecewise-linear approximations of the same falling centreline, and where they meet they
/// sit millimetres apart; a 16-bit depth buffer cannot tell them apart, so which one wins changes
/// per pixel as the camera moves. A butt joint is only a seam when the edges are cut in different
/// places. See `tests/carpark.rs`.
pub fn carpark_span(_track: &Track) -> (f32, f32) {
    let (first, last) = crate::mesh::ribbon_samples_within(_track, CARPARK_CENTRE, CARPARK_HALF);
    let spacing = crate::mesh::ribbon_spacing(_track);
    (first as f32 * spacing, last as f32 * spacing)
}

/// Where the car park's outer wall stands, as a lateral offset `along` metres down the road: the
/// rail's line outside the car park, and the end walls and parapet across it.
pub fn carpark_edge(track: &Track, along: f32) -> f32 {
    let (s0, s1) = carpark_span(track);
    let north = s0 + CARPARK_NORTH_TAPER;
    let south = s1 - CARPARK_SOUTH_TAPER;
    if along <= s0 || along >= s1 {
        RAIL_LIMIT
    } else if along < north {
        RAIL_LIMIT + (CARPARK_REACH - RAIL_LIMIT) * (along - s0) / CARPARK_NORTH_TAPER
    } else if along <= south {
        CARPARK_REACH - (along - north)
    } else {
        let at_south = CARPARK_REACH - (south - north);
        at_south + (RAIL_LIMIT - at_south) * (along - south) / CARPARK_SOUTH_TAPER
    }
}

/// The parapet's two ends, as `(along, lateral)`: it runs at 45° from the top one to the bottom.
pub fn carpark_parapet(track: &Track) -> ((f32, f32), (f32, f32)) {
    let (s0, s1) = carpark_span(track);
    let (north, south) = (s0 + CARPARK_NORTH_TAPER, s1 - CARPARK_SOUTH_TAPER);
    ((north, CARPARK_REACH), (south, CARPARK_REACH - (south - north)))
}

/// Which way the car park faces, as `(d_along, d_lateral)`: square out of the parapet, down the
/// valley toward the city.
pub const CARPARK_FACING: (f32, f32) = (core::f32::consts::FRAC_1_SQRT_2, core::f32::consts::FRAC_1_SQRT_2);

/// Where the title screen parks the car: the middle bay, nose-in a metre short of the parapet,
/// facing the city. As `(x, y, z, yaw, node)`.
pub fn carpark_parking(track: &Track) -> (f32, f32, f32, f32, usize) {
    let ((na, nl), (sa, sl)) = carpark_parapet(track);
    let (fa, fl) = CARPARK_FACING;
    // Nose a metre from the rail: the car's centre is its half-length and that metre back.
    const BACK: f32 = 3.3;
    let (along, lateral) = ((na + sa) * 0.5 - fa * BACK, (nl + sl) * 0.5 - fl * BACK);
    let p = carpark_surface(track, along, lateral);
    let n = carpark_node_at(track, along);
    let dx = n.dir.x * fa + n.nrm.x * CARPARK_SIDE * fl;
    let dz = n.dir.z * fa + n.nrm.z * CARPARK_SIDE * fl;
    (p.x, p.y, p.z, atan2(dx, dz), node_at_arclength(track, along))
}

/// Height of the paved surface relative to the road, `lateral` metres from the centreline.
///
/// Level with the shoulder, because the point is that the car park reads as the road simply
/// getting wider. Past the shoulder it falls at 2 %, which is what a real road is built with, so
/// water runs off it instead of back across the carriageway.
pub fn carpark_fall(lateral: f32) -> f32 {
    -crate::math::max(0.0, crate::math::abs(lateral) - ROAD_SHOULDER) * 0.02
}

/// The node the car park stands on at `along` metres down the road.
///
/// Every piece takes its height from the node it actually stands above, never from one node for
/// the whole car park: the road falls 0.4 m across it.
pub fn carpark_node_at(track: &Track, along: f32) -> &Node {
    &track.nodes[node_at_arclength(track, crate::math::max(0.0, along))]
}

/// A point on the paved surface, `along` metres down the road and `lateral` metres out.
pub fn carpark_surface(track: &Track, along: f32, lateral: f32) -> Vec3 {
    let n = carpark_node_at(track, along);
    // Past the node's own arclength the point is carried on along the road's direction, so props
    // placed between nodes sit where they were asked to rather than snapping to a node.
    let d = along - n.s;
    Vec3::new(
        n.p.x + n.dir.x * d + n.nrm.x * lateral * CARPARK_SIDE,
        n.p.y + carpark_fall(lateral),
        n.p.z + n.dir.z * d + n.nrm.z * lateral * CARPARK_SIDE,
    )
}

/// How far down the hillside past the parapet falls for each metre out from it.
pub const CARPARK_CLIFF: f32 = 1.2;
/// Over how many metres along the road the ground eases back into the natural hillside past each
/// end of the car park.
pub const CARPARK_GROUND_BLEND: f32 = 16.0;

/// Height of the ground below the node, `lateral` metres out on the car park's side, given the
/// natural hillside's height `natural` there.
///
/// Under the paving the hillside is cut to a shelf a quarter of a metre below it, so the paving
/// always wins the depth test. Past the parapet it drops away as a cliff, steeper than the natural
/// hillside, so the camera in the car park looks straight down into the valley. Either side of the
/// car park the ground blends back into the hillside over [`CARPARK_GROUND_BLEND`] metres.
pub fn carpark_ground(track: &Track, along: f32, lateral: f32, natural: f32) -> f32 {
    let (s0, s1) = carpark_span(track);
    let w = if along < s0 {
        1.0 - (s0 - along) / CARPARK_GROUND_BLEND
    } else if along > s1 {
        1.0 - (along - s1) / CARPARK_GROUND_BLEND
    } else {
        1.0
    };
    if w <= 0.0 {
        return natural;
    }
    // Inside the car park the edge is where the wall stands; past its ends, carry the end wall's
    // line on so the blend has an edge to fall away from.
    let edge = crate::math::max(carpark_edge(track, crate::math::clamp(along, s0 + 0.01, s1 - 0.01)), RAIL_LIMIT);
    let l = crate::math::abs(lateral);
    let shelf = carpark_fall(l) - 0.25;
    let cut = if l <= edge + 1.0 {
        shelf
    } else {
        crate::math::min(natural, shelf - (l - edge - 1.0) * CARPARK_CLIFF)
    };
    crate::math::lerp(natural, cut, crate::math::clamp(w, 0.0, 1.0))
}

/// The node nearest a given distance along the track, by binary search on cumulative arclength.
pub fn node_at_arclength(track: &Track, s: f32) -> usize {
    let (mut lo, mut hi) = (0usize, NODE_COUNT - 1);
    while lo < hi {
        let mid = (lo + hi) / 2;
        if track.nodes[mid].s < s {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

/// The node the run ends on, and where the finish line stands.
///
/// A run finishes at 98.5% of the centreline rather than the very last node, so the line has to
/// be there and not at the end of the road, or the player crosses it after the fact.
pub const FINISH_NODE: usize = (NODE_COUNT - 1) * 985 / 1000;

/// Where the guard rail stands, and so where the car's bodywork has to stop.
///
/// This is the rail's own face and not a margin inside it: the renderer builds the rail ribbon on
/// this line, and containment measures the car's outermost corner against it, so the two agree by
/// construction. It used to be 7.15 against a rail drawn at 7.5 — a 35 cm allowance that stood in
/// for the car having a width, and that a car half a metre wider than it simply drove through.
pub const RAIL_LIMIT: f32 = 7.5;

/// The centreline nodes the car park is open over: where the rail on its side is left out.
///
/// Strictly inside the paving's span, so the rail runs right up to where each end wall begins.
pub fn carpark_open_nodes(track: &Track) -> (usize, usize) {
    let (s0, s1) = carpark_span(track);
    let spacing = crate::mesh::ribbon_spacing(track);
    (node_at_arclength(track, s0 + spacing), node_at_arclength(track, s1 - spacing))
}

/// Containment on the car park's side at `index`: the wall's line, less its thickness.
pub fn carpark_limit(track: &Track, index: usize) -> f32 {
    let along = track.nodes[index.min(NODE_COUNT - 1)].s;
    let edge = carpark_edge(track, along);
    if edge > RAIL_LIMIT {
        edge - 0.25
    } else {
        RAIL_LIMIT
    }
}

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

    for (i, &(degrees, steps)) in PLAN.iter().enumerate() {
        let curvature = radians(degrees);
        let drop = section_drop(i, degrees);
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
