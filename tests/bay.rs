//! The pull-off is cut into a hillside, and the hillside is not level.
//!
//! Its first version was laid out from [`BAY_NODE`] alone: one 38 m slab, one flat parapet base,
//! one flat edge line. The pass drops 2.8 m across that span, so the slab was buried over a metre
//! deep at its upper end and hanging a metre and a half in the air at its lower one — from the
//! car it looked like the pull-off simply was not there, because the only part of it above ground
//! was a thin band z-fighting with the terrain. These tests pin the paving to the hill.

use angle_zero::track::{
    bay_apron_offset, bay_node_at, bay_shelf_blend, bay_shelf_offset, bay_surface,
    node_at_arclength, Track, BAY_APRON_INNER, BAY_APRON_OUTER, BAY_HALF_LENGTH, BAY_NODE,
    ROAD_SHOULDER,
};

fn track() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

/// Samples spanning the pull-off, as anything built on it must.
fn alongs() -> impl Iterator<Item = f32> {
    (0..=16).map(|i| (i as f32 / 8.0 - 1.0) * BAY_HALF_LENGTH)
}

fn laterals() -> impl Iterator<Item = f32> {
    (0..=8).map(|i| BAY_APRON_INNER + (BAY_APRON_OUTER - BAY_APRON_INNER) * i as f32 / 8.0)
}

#[test]
fn the_paving_follows_the_pass_downhill() {
    let t = track();
    let top = bay_surface(&t, -BAY_HALF_LENGTH, 12.0).y;
    let bottom = bay_surface(&t, BAY_HALF_LENGTH, 12.0).y;
    let drop = top - bottom;
    assert!(
        drop > 2.0,
        "the pull-off spans {:.1} m of a descending pass but its paving only drops {drop:.2} m — \
         it is being laid out flat again",
        BAY_HALF_LENGTH * 2.0
    );

    // Monotonic: no part of it climbs back up.
    let mut prev = f32::INFINITY;
    for along in alongs() {
        let y = bay_surface(&t, along, 12.0).y;
        assert!(y <= prev + 0.001, "the paving rises again at along {along:.1}");
        prev = y;
    }
}

#[test]
fn the_paving_runs_out_of_the_carriageway_without_a_step() {
    // Anything inside the shoulder is at the shoulder's own height, so there is no seam to see.
    assert_eq!(bay_apron_offset(BAY_APRON_INNER), 0.0);
    assert_eq!(bay_apron_offset(ROAD_SHOULDER), 0.0);
    const {
        assert!(
            BAY_APRON_INNER < ROAD_SHOULDER,
            "the paving must start under the road ribbon, not alongside it — a butt joint \
             between two separately built surfaces is exactly the seam this is meant to avoid"
        )
    };

    // And it drains away from the road rather than towards it.
    let outer = bay_apron_offset(BAY_APRON_OUTER);
    assert!(
        (-0.4..-0.1).contains(&outer),
        "the crossfall drops {outer:.3} m across the paving, which is either a cliff or a puddle"
    );
}

#[test]
fn the_paving_sits_above_the_shelf_it_is_laid_on_everywhere() {
    let t = track();
    for along in alongs() {
        let n = bay_node_at(&t, along);
        for lateral in laterals() {
            // What the terrain mesh puts here, and what the paving puts on top of it.
            let terrain = n.p.y + bay_shelf_offset(lateral);
            let paving = bay_surface(&t, along, lateral).y;
            let clearance = paving - terrain;
            assert!(
                (0.02..=0.4).contains(&clearance),
                "at along {along:.1} lateral {lateral:.1} the paving clears the shelf by \
                 {clearance:+.3} m — it is either buried in it or floating over it",
            );
        }
    }
}

#[test]
fn the_paving_stays_within_the_fully_shelved_run_of_nodes() {
    let t = track();
    for along in [-BAY_HALF_LENGTH, BAY_HALF_LENGTH] {
        let s = t.nodes[BAY_NODE].s + along;
        let index = node_at_arclength(&t, s);
        assert_eq!(
            bay_shelf_blend(index),
            1.0,
            "the paving reaches node {index} at along {along:.1}, where the shelf has already \
             begun easing back into the hillside — the paving would overhang it",
        );
    }
}
