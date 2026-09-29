//! The summit: the course drops off it steeply, so the title camera in the car park looks down on
//! the first hairpin.

use angle_zero::track::{Track, NODE_COUNT};

fn track() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

#[test]
fn the_first_hairpin_lies_well_below_the_summit() {
    let t = track();
    // The apex of the first right-hander, 275 m down the road.
    let i = (0..NODE_COUNT).find(|&i| t.nodes[i].s > 275.0).unwrap();
    let below = t.nodes[0].p.y - t.nodes[i].p.y;
    assert!((26.0..34.0).contains(&below), "first hairpin {below:.1} m below the start");
}

#[test]
fn the_steep_opening_stops_at_324_m() {
    let t = track();
    // Past the opening the road falls at the curvature rule's rate again, never more than 7.7 %.
    let a = (0..NODE_COUNT).find(|&i| t.nodes[i].s > 420.0).unwrap();
    let b = (0..NODE_COUNT).find(|&i| t.nodes[i].s > 1400.0).unwrap();
    let grade = (t.nodes[a].p.y - t.nodes[b].p.y) / (t.nodes[b].s - t.nodes[a].s);
    assert!(grade < 0.077, "grade {grade:.3} after the opening");
}
