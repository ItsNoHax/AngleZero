//! The pull-off is cut into a hillside, and the hillside is not level.
//!
//! Its first version was laid out from [`BAY_NODE`] alone: one 38 m slab, one flat parapet base,
//! one flat edge line. The pass drops 2.8 m across that span, so the slab was buried over a metre
//! deep at its upper end and hanging a metre and a half in the air at its lower one — from the
//! car it looked like the pull-off simply was not there, because the only part of it above ground
//! was a thin band z-fighting with the terrain. These tests pin the paving to the hill.

use angle_zero::mesh::{ribbon_samples_within, ribbon_spacing};
use angle_zero::track::{
    bay_apron_offset, bay_node_at, bay_shelf_blend, bay_shelf_offset, bay_surface,
    node_at_arclength, Track, BAY_APRON_INNER, BAY_APRON_OUTER, BAY_HALF_LENGTH, BAY_NODE,
    BAY_SIDE, ROAD_SHOULDER,
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
    // The paving meets the road at the shoulder, at the shoulder's own height, so there is no
    // step across the joint.
    assert_eq!(bay_apron_offset(BAY_APRON_INNER), 0.0);
    assert_eq!(bay_apron_offset(ROAD_SHOULDER), 0.0);
    const {
        assert!(
            BAY_APRON_INNER == ROAD_SHOULDER,
            "the paving must meet the road ribbon edge to edge. It used to start 0.4 m inside \
             the shoulder so that no butt joint was needed, but two surfaces cut at different \
             intervals along the same falling centreline interpenetrate, and the console cannot \
             resolve the difference — that overlap is what made the title screen flicker"
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
            // The floor is not just a matter of which surface is on top: the console has a
            // 16-bit depth buffer and a 0.4 m near plane, so two surfaces this far apart stop
            // being distinguishable at
            //
            //     65535 * near * far * clearance / (dist^2 * (far - near))  <  1 unit
            //
            // The pull-off's first version laid its gravel 0.03 m over the shelf, which goes
            // unresolvable at 28 m — and the title camera orbits at 10.5 m looking across
            // 38 m of it, so the far half tore into grass and flickered with every frame.
            // 0.15 m holds out to 60 m, which is further than any of this is ever seen from.
            assert!(
                (0.15..=0.4).contains(&clearance),
                "at along {along:.1} lateral {lateral:.1} the paving clears the shelf by \
                 {clearance:+.3} m — under 0.15 m the depth buffer cannot keep them apart and \
                 the grass tears through the surface",
            );
        }
    }
}

/// The joint between the paving and the carriageway, which is the one the title camera stares at.
///
/// Both are piecewise-linear approximations of the same falling centreline. Cut at different
/// intervals they interpenetrate — measured across the pull-off, the old 0.4 m overlap held the
/// two within 18 mm of each other and let the paving break up through the road at a quarter of
/// the samples, against a depth buffer that resolves 8.6 mm at 15 m. Cut on the same nodes they
/// share their vertices exactly, and there is nothing left to fight over.
#[test]
fn the_paving_is_cut_on_the_same_nodes_as_the_road_ribbon() {
    let t = track();
    let s0 = t.nodes[BAY_NODE].s;
    let spacing = ribbon_spacing(&t);
    let (first, last) = ribbon_samples_within(&t, s0, BAY_HALF_LENGTH);
    assert!(last > first, "the pull-off is not even one ribbon sample long");

    for k in first..=last {
        // Where the road ribbon puts its outer edge, exactly as `mesh::station_vertex` does.
        let n = &t.nodes[node_at_arclength(&t, k as f32 * spacing)];
        let road_x = n.p.x + n.nrm.x * ROAD_SHOULDER * BAY_SIDE;
        let road_y = n.p.y;
        let road_z = n.p.z + n.nrm.z * ROAD_SHOULDER * BAY_SIDE;

        // And where the paving puts its inner edge.
        let p = bay_surface(&t, k as f32 * spacing - s0, BAY_APRON_INNER);

        let off = ((p.x - road_x).powi(2) + (p.y - road_y).powi(2) + (p.z - road_z).powi(2)).sqrt();
        assert!(
            off < 1e-4,
            "at sample {k} the paving's inner edge misses the road ribbon's outer edge by \
             {off:.4} m — the joint is a T-junction, not a shared edge, and it will crack",
        );
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
