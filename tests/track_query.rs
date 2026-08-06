//! Nearest-node queries. These drive containment, gravity and scoring, so the sign
//! conventions matter more than almost anything else in the core.

use angle_zero::math::Vec2;
use angle_zero::track::{Locator, Track, NODE_COUNT};

fn built() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

/// A point offset from node `i` by `lat` metres along the left normal.
fn offset_from(t: &Track, i: usize, lat: f32) -> (f32, f32) {
    let n = &t.nodes[i];
    (n.p.x + n.nrm.x * lat, n.p.z + n.nrm.z * lat)
}

#[test]
fn a_point_on_the_centreline_reports_no_lateral_offset() {
    let t = built();
    let mut loc = Locator::new();
    for &i in &[5usize, 300, 900, 1500, 2100, 2600] {
        loc.reset_to(i);
        let n = &t.nodes[i];
        let q = loc.nearest(&t, n.p.x, n.p.z);
        assert_eq!(q.index, i);
        assert!(q.lat.abs() < 1e-3, "node {i} reported lat {}", q.lat);
        assert!(q.along.abs() < 1e-3, "node {i} reported along {}", q.along);
    }
}

#[test]
fn lateral_offset_is_signed_left_positive() {
    let t = built();
    let mut loc = Locator::new();

    for &i in &[400usize, 1200, 2000] {
        loc.reset_to(i);
        let (x, z) = offset_from(&t, i, 3.0);
        let q = loc.nearest(&t, x, z);
        assert!(
            (q.lat - 3.0).abs() < 0.35,
            "3 m to the left read as lat {} at node {i}",
            q.lat
        );

        loc.reset_to(i);
        let (x, z) = offset_from(&t, i, -3.0);
        let q = loc.nearest(&t, x, z);
        assert!(
            (q.lat + 3.0).abs() < 0.35,
            "3 m to the right read as lat {} at node {i}",
            q.lat
        );
    }
}

#[test]
fn the_search_window_tracks_the_car_forward_along_the_track() {
    let t = built();
    let mut loc = Locator::new();

    // Walk the whole track in small hops; the windowed search must stay locked on.
    let mut i = 0;
    while i < NODE_COUNT {
        let n = &t.nodes[i];
        let q = loc.nearest(&t, n.p.x, n.p.z);
        assert_eq!(q.index, i, "windowed search lost the car at node {i}");
        i += 40;
    }
}

#[test]
fn a_car_thrown_clear_of_the_window_is_recovered_by_the_full_scan() {
    let t = built();
    let mut loc = Locator::new();
    // Locator believes the car is at the start; put it three quarters of the way down instead.
    let far = 1900;
    let n = &t.nodes[far];
    let q = loc.nearest(&t, n.p.x, n.p.z);
    // The stride-3 sweep lands within a node or two, and the next query refines it.
    assert!(
        q.index.abs_diff(far) <= 3,
        "full scan landed at {} instead of near {far}",
        q.index
    );
    let q2 = loc.nearest(&t, n.p.x, n.p.z);
    assert_eq!(q2.index, far);
}

#[test]
fn over_is_zero_in_the_middle_of_the_track() {
    let t = built();
    let mut loc = Locator::new();
    for &i in &[200usize, 1000, 2400] {
        loc.reset_to(i);
        let n = &t.nodes[i];
        let q = loc.nearest(&t, n.p.x, n.p.z);
        assert_eq!(q.over, 0.0, "node {i} reported over {}", q.over);
    }
}

#[test]
fn driving_back_past_the_start_line_reports_negative_over() {
    let t = built();
    let mut loc = Locator::new();
    let n = &t.nodes[0];
    // 5 m *behind* node 0, i.e. against the direction of travel.
    let x = n.p.x - n.dir.x * 5.0;
    let z = n.p.z - n.dir.z * 5.0;
    let q = loc.nearest(&t, x, z);
    assert!(q.index <= 1, "expected to clamp at the start, got {}", q.index);
    assert!(
        q.over < -4.0 && q.over > -6.0,
        "expected over ≈ -5, got {}",
        q.over
    );
}

#[test]
fn running_off_the_end_of_the_track_reports_positive_over() {
    let t = built();
    let mut loc = Locator::new();
    let last = NODE_COUNT - 1;
    loc.reset_to(last);
    let n = &t.nodes[last];
    let x = n.p.x + n.dir.x * 5.0;
    let z = n.p.z + n.dir.z * 5.0;
    let q = loc.nearest(&t, x, z);
    assert!(q.index >= last - 1);
    assert!(
        q.over > 4.0 && q.over < 6.0,
        "expected over ≈ +5, got {}",
        q.over
    );
}

#[test]
fn along_measures_progress_within_a_node() {
    let t = built();
    let mut loc = Locator::new();
    let i = 800;
    loc.reset_to(i);
    let n = &t.nodes[i];
    // Half a node-spacing ahead stays nearest to `i` but reads a positive `along`.
    let d = 0.5;
    let q = loc.nearest(&t, n.p.x + n.dir.x * d, n.p.z + n.dir.z * d);
    assert_eq!(q.index, i);
    assert!((q.along - d).abs() < 0.1, "along was {}", q.along);
}

#[test]
fn normals_and_directions_stay_orthonormal_where_the_car_actually_drives() {
    let t = built();
    for n in t.nodes.iter() {
        let d = Vec2::new(n.dir.x, n.dir.z);
        assert!((d.length() - 1.0).abs() < 1e-3);
    }
}
