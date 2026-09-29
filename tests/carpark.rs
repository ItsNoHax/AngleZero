//! The summit car park is cut into the hillside beside the start, and meets the road edge to edge.
//!
//! The lay-by it replaces taught two things these tests pin. Laid out from one node, a slab is
//! buried at one end and hanging in the air at the other, because the road falls across it. And
//! two surfaces meeting along a falling centreline must be cut on the same nodes, or they
//! interpenetrate by millimetres and a 16-bit depth buffer flickers between them.

use angle_zero::mesh::{ribbon_samples_within, ribbon_spacing};
use angle_zero::track::{
    carpark_edge, carpark_fall, carpark_ground, carpark_limit, carpark_node_at, carpark_open_nodes,
    carpark_parapet, carpark_parking, carpark_span, carpark_surface, node_at_arclength, Track,
    CARPARK_SIDE, NODE_COUNT, RAIL_LIMIT, ROAD_SHOULDER,
};

fn track() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

/// Points across the paving, on the nodes it is cut on: from the shoulder out to the wall.
fn paving(t: &Track) -> Vec<(f32, f32)> {
    let (s0, s1) = carpark_span(t);
    let spacing = ribbon_spacing(t);
    let mut out = Vec::new();
    let mut k = (s0 / spacing).round() as usize;
    while k as f32 * spacing <= s1 + 1e-3 {
        let along = t.nodes[node_at_arclength(t, k as f32 * spacing)].s;
        let edge = carpark_edge(t, along);
        for j in 0..=8 {
            out.push((along, ROAD_SHOULDER + (edge - ROAD_SHOULDER) * j as f32 / 8.0));
        }
        k += 1;
    }
    out
}

#[test]
fn the_car_park_is_at_the_top_of_the_course() {
    let t = track();
    let (s0, s1) = carpark_span(&t);
    assert!(s0 < 4.0, "the car park starts {s0:.1} m down the road");
    assert!((38.0..48.0).contains(&(s1 - s0)), "the car park is {:.1} m long", s1 - s0);
    // Near enough level: the road falls under half a metre across it.
    let fall = carpark_surface(&t, s0, 10.0).y - carpark_surface(&t, s1, 10.0).y;
    assert!((0.0..0.6).contains(&fall), "the paving falls {fall:.2} m along the road");
}

#[test]
fn the_paving_meets_the_road_without_a_step_and_drains_away_from_it() {
    assert_eq!(carpark_fall(ROAD_SHOULDER), 0.0);
    let outer = carpark_fall(40.0);
    assert!((-0.8..-0.4).contains(&outer), "the crossfall drops {outer:.2} m across the paving");
}

/// The joint the title camera looks along: shared vertices, not a T-junction.
#[test]
fn the_paving_is_cut_on_the_same_nodes_as_the_road_ribbon() {
    let t = track();
    let spacing = ribbon_spacing(&t);
    let (s0, s1) = carpark_span(&t);
    let first = (s0 / spacing).round() as usize;
    let last = (s1 / spacing).round() as usize;
    assert!((s0 - first as f32 * spacing).abs() < 1e-3 && (s1 - last as f32 * spacing).abs() < 1e-3);
    assert_eq!(ribbon_samples_within(&t, (s0 + s1) / 2.0, (s1 - s0) / 2.0 + 0.1), (first, last));
    for k in first..=last {
        let n = &t.nodes[node_at_arclength(&t, k as f32 * spacing)];
        let road = (n.p.x + n.nrm.x * ROAD_SHOULDER * CARPARK_SIDE, n.p.y, n.p.z + n.nrm.z * ROAD_SHOULDER * CARPARK_SIDE);
        let p = carpark_surface(&t, n.s, ROAD_SHOULDER);
        let off = ((p.x - road.0).powi(2) + (p.y - road.1).powi(2) + (p.z - road.2).powi(2)).sqrt();
        assert!(off < 1e-4, "at sample {k} the paving misses the road's edge by {off:.4} m");
    }
}

#[test]
fn the_paving_sits_a_quarter_metre_above_the_ground_cut_for_it() {
    let t = track();
    for (along, lateral) in paving(&t) {
        let n = carpark_node_at(&t, along);
        let ground = n.p.y + carpark_ground(&t, along, lateral, -3.0);
        let clearance = carpark_surface(&t, along, lateral).y - ground;
        assert!(
            (0.15..=0.4).contains(&clearance),
            "at along {along:.1} lateral {lateral:.1} the paving clears the ground by {clearance:+.3} m",
        );
    }
}

#[test]
fn past_the_parapet_the_hillside_falls_away_as_a_cliff() {
    let t = track();
    let ((na, nl), (sa, sl)) = carpark_parapet(&t);
    let mid = ((na + sa) / 2.0, (nl + sl) / 2.0);
    let natural = -8.0;
    let beyond = carpark_ground(&t, mid.0, mid.1 + 15.0, natural);
    assert!(beyond < -15.0, "15 m past the parapet the ground is only {beyond:.1} m down");
    // Well clear of the car park, the hillside is left as it was.
    assert_eq!(carpark_ground(&t, 120.0, 30.0, natural), natural);
}

#[test]
fn the_parapet_runs_at_45_degrees_to_the_road() {
    let t = track();
    let ((na, nl), (sa, sl)) = carpark_parapet(&t);
    assert!(((nl - sl) / (sa - na) - 1.0).abs() < 1e-4);
    assert!(nl > 35.0 && sl > RAIL_LIMIT + 4.0, "parapet from {nl:.1} to {sl:.1} m out");
}

#[test]
fn the_rail_is_left_out_only_where_the_paving_is() {
    let t = track();
    let (s0, s1) = carpark_span(&t);
    let (a, b) = carpark_open_nodes(&t);
    assert!(t.nodes[a].s > s0 && t.nodes[b].s < s1, "rail gap {a}..{b} runs past the paving");
    assert!(b > a + 10, "rail gap {a}..{b} is too short");
}

#[test]
fn containment_opens_across_the_car_park_and_nowhere_else() {
    let t = track();
    let (a, b) = carpark_open_nodes(&t);
    for i in a..=b {
        let edge = carpark_edge(&t, t.nodes[i].s);
        assert!((carpark_limit(&t, i) - (edge - 0.25)).abs() < 1e-4);
    }
    for i in [0, b + 20, 400, NODE_COUNT - 1] {
        assert_eq!(carpark_limit(&t, i), RAIL_LIMIT, "node {i} is open");
    }
}

#[test]
fn the_title_car_is_parked_on_the_paving_facing_the_parapet() {
    let t = track();
    let (x, y, z, yaw, node) = carpark_parking(&t);
    let n = &t.nodes[node];
    let lateral = (x - n.p.x) * n.nrm.x * CARPARK_SIDE + (z - n.p.z) * n.nrm.z * CARPARK_SIDE;
    let edge = carpark_edge(&t, n.s);
    assert!(lateral > 15.0 && lateral < edge - 2.0, "parked {lateral:.1} m out, wall at {edge:.1}");
    assert!((y - carpark_surface(&t, n.s, lateral).y).abs() < 0.2);
    // Facing 45° off the road's direction, toward the car park's side.
    let road = n.dir.x.atan2(n.dir.z);
    let off = (yaw - road + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    assert!((off.abs() - std::f32::consts::FRAC_PI_4).abs() < 0.05, "parked {off:.2} rad off the road");
}
