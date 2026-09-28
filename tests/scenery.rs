//! The far scenery is drawn without depth: the sky pass paints ridges back to front, and the valley
//! lights after them. That only works if the geometry keeps to an order, and these tests pin it.

use angle_zero::math::{Vec3, TAU};
use angle_zero::scenery::{
    nearer_than_ridges, ridge_farthest, ridge_height, rim_light, valley_lights, ValleyLight,
    DRAW_DISTANCE, RIDGE_BANDS, VALLEY_CLEARANCE, VALLEY_LIGHTS,
};
use angle_zero::track::Track;

fn track() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

#[test]
fn ridge_ring_closes_without_a_seam() {
    for band in RIDGE_BANDS.iter() {
        let a = ridge_height(band, 0.0);
        let b = ridge_height(band, TAU);
        assert!((a - b).abs() < 0.5, "seam of {} m", (a - b).abs());
    }
}

#[test]
fn ridge_heights_stay_in_their_band() {
    for band in RIDGE_BANDS.iter() {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for k in 0..3600 {
            let h = ridge_height(band, k as f32 / 3600.0 * TAU);
            lo = lo.min(h);
            hi = hi.max(h);
        }
        assert!(lo >= band.low - 0.01 && hi <= band.high + 0.01);
        // And it actually uses the range: a flat band is a failure of the profile, not a pass.
        assert!(hi - lo > (band.high - band.low) * 0.6, "band spans only {} m", hi - lo);
    }
}

#[test]
fn bands_are_drawn_far_to_near() {
    for pair in RIDGE_BANDS.windows(2) {
        assert!(pair[0].radius > pair[1].radius);
    }
}

#[test]
fn no_band_reaches_the_far_plane() {
    let t = track();
    for band in RIDGE_BANDS.iter() {
        let far = ridge_farthest(&t, band);
        assert!(far < DRAW_DISTANCE * 0.95, "band at {} reaches {far} m", band.radius);
    }
}

#[test]
fn rim_is_brightest_toward_the_moon() {
    let toward = rim_light(angle_zero::scenery::MOON_YAW);
    let away = rim_light(angle_zero::scenery::MOON_YAW + TAU / 2.0);
    assert!(toward > 0.95 && away < 0.25 && away > 0.0);
}

#[test]
fn valley_lights_fill_most_of_their_budget() {
    let t = track();
    let mut out = [ValleyLight::ZERO; VALLEY_LIGHTS];
    let n = valley_lights(&t, &mut out);
    assert!(n > VALLEY_LIGHTS * 3 / 4, "only {n} placed");
}

#[test]
fn valley_lights_stay_off_the_hillside() {
    let t = track();
    let mut out = [ValleyLight::ZERO; VALLEY_LIGHTS];
    let n = valley_lights(&t, &mut out);
    for l in &out[..n] {
        for node in t.nodes.iter() {
            assert!(node.p.horizontal_distance(l.p) >= VALLEY_CLEARANCE - 12.0);
        }
        // Below the top of the pass: the valley is under it, never level with where it starts.
        let top = t.nodes.iter().map(|n| n.p.y).fold(f32::MIN, f32::max);
        assert!(l.p.y < top - 60.0, "light at {:?} is level with the summit", l.p);
    }
}

#[test]
fn valley_lights_are_always_in_front_of_the_ridges() {
    let t = track();
    let mut out = [ValleyLight::ZERO; VALLEY_LIGHTS];
    let n = valley_lights(&t, &mut out);
    for l in &out[..n] {
        assert!(nearer_than_ridges(&t, l.p), "light at {:?} can fall behind a ridge", l.p);
    }
}

#[test]
fn valley_is_deterministic() {
    let t = track();
    let mut a = [ValleyLight::ZERO; VALLEY_LIGHTS];
    let mut b = [ValleyLight::ZERO; VALLEY_LIGHTS];
    let na = valley_lights(&t, &mut a);
    let nb = valley_lights(&t, &mut b);
    assert_eq!(na, nb);
    for (x, y) in a[..na].iter().zip(b[..nb].iter()) {
        assert_eq!(x.p, y.p);
        let _: Vec3 = x.p;
    }
}


#[test]
fn trees_stand_clear_of_every_road() {
    use angle_zero::scenery::{tree_sites, TreeSite, TREE_ROAD_CLEARANCE};
    let t = track();
    let mut out = vec![TreeSite::ZERO; 4000];
    let n = tree_sites(&t, &mut out, |_, _| false, |_, _| None);
    assert!(n > 1000, "only {n} trees");
    for tree in &out[..n] {
        for node in t.nodes.iter() {
            assert!(node.p.horizontal_distance(tree.base) > TREE_ROAD_CLEARANCE - 1.5);
        }
    }
}

#[test]
fn trees_come_in_stands() {
    // Not one every few nodes: somewhere along the road there is a clearing and somewhere a stand.
    use angle_zero::scenery::forest_density;
    let d: Vec<f32> = (0..3500).step_by(10).map(|s| forest_density(s as f32, 1.0)).collect();
    assert!(d.iter().any(|&x| x < 0.15) && d.iter().any(|&x| x > 0.85));
}

#[test]
fn terrain_profile_meets_the_road_and_falls_away() {
    use angle_zero::scenery::{terrain_drop, TERRAIN_PROFILE};
    assert!(terrain_drop(0.0) > -0.3);
    for w in TERRAIN_PROFILE.windows(2) {
        assert!(w[1].0 > w[0].0 && w[1].1 < w[0].1);
    }
    assert!((terrain_drop(-30.0) - terrain_drop(30.0)).abs() < 1e-6);
}

mod signs {
    use super::track;
    use angle_zero::math::Vec3;
    use angle_zero::scenery::{road_signs, sign_gain, Sign, SignKind, REFLECT_FAR};

    fn reflector_at(x: f32, z: f32) -> Sign {
        // Facing -Z: toward a car coming up from below on +Z... i.e. for a car heading +Z at it.
        Sign { kind: SignKind::Reflector, at: Vec3::new(x, 0.8, z), face: (0.0, -1.0), node: 0 }
    }

    #[test]
    fn a_reflector_ahead_in_the_beams_lights_up() {
        let g = sign_gain(&reflector_at(0.5, 30.0), Vec3::ZERO, (0.0, 1.0));
        assert!(g > 0.7, "{g}");
    }

    #[test]
    fn behind_off_axis_or_far_stays_dark() {
        let car = Vec3::ZERO;
        assert_eq!(sign_gain(&reflector_at(0.0, -30.0), car, (0.0, 1.0)), 0.0);
        assert_eq!(sign_gain(&reflector_at(30.0, 5.0), car, (0.0, 1.0)), 0.0);
        assert_eq!(sign_gain(&reflector_at(0.0, REFLECT_FAR + 5.0), car, (0.0, 1.0)), 0.0);
    }

    #[test]
    fn a_face_turned_away_stays_dark() {
        let mut s = reflector_at(0.0, 30.0);
        s.face = (0.0, 1.0);
        assert_eq!(sign_gain(&s, Vec3::ZERO, (0.0, 1.0)), 0.0);
    }

    #[test]
    fn nearer_is_brighter() {
        let near = sign_gain(&reflector_at(0.0, 20.0), Vec3::ZERO, (0.0, 1.0));
        let far = sign_gain(&reflector_at(0.0, 90.0), Vec3::ZERO, (0.0, 1.0));
        assert!(near > far);
    }

    #[test]
    fn signs_face_the_traffic_coming_down_the_pass() {
        let t = track();
        let mut out = vec![Sign::ZERO; 2000];
        let n = road_signs(&t, &mut out, |_, _| false);
        assert!(n > 600, "{n}");
        for s in &out[..n] {
            let d = t.nodes[s.node as usize].dir;
            // Facing back up the road, within the turn-in the chevrons allow.
            assert!(-(d.x * s.face.0 + d.z * s.face.1) > 0.5, "{:?}", s.kind);
        }
        assert!(out[..n].iter().any(|s| s.kind == SignKind::Chevron));
        assert!(out[..n].iter().any(|s| s.kind == SignKind::Mirror));
    }

    #[test]
    fn chevrons_stand_on_the_outside_of_the_bend() {
        let t = track();
        let mut out = vec![Sign::ZERO; 2000];
        let n = road_signs(&t, &mut out, |_, _| false);
        for s in out[..n].iter().filter(|s| s.kind == SignKind::Chevron) {
            let i = s.node as usize;
            // Outside means farther from the bend's centre: step along the road and the sign's
            // distance to the road should stay put or grow, never shrink toward the inside.
            let node = &t.nodes[i];
            let side = (s.at.x - node.p.x) * node.nrm.x + (s.at.z - node.p.z) * node.nrm.z;
            // The centre of the bend lies on the other side.
            let (a, b) = (&t.nodes[i.saturating_sub(6)], &t.nodes[(i + 6).min(t.nodes.len() - 1)]);
            let chord_mid = Vec3::new((a.p.x + b.p.x) * 0.5, 0.0, (a.p.z + b.p.z) * 0.5);
            let inward = (chord_mid.x - node.p.x) * node.nrm.x + (chord_mid.z - node.p.z) * node.nrm.z;
            assert!(inward * side <= 0.05, "chevron at node {i} is on the inside");
        }
    }
}

mod banks {
    use super::track;
    use angle_zero::scenery::{bank_surface, cut_banks, outside_of_bend, BankSpan, BANK_CLEARANCE};

    #[test]
    fn some_bends_get_a_bank() {
        let t = track();
        let mut out = [BankSpan::ZERO; 64];
        let n = cut_banks(&t, &mut out, |_, _| false);
        assert!(n >= 6, "only {n} banks");
    }

    #[test]
    fn banks_stand_on_the_inside() {
        let t = track();
        let mut out = [BankSpan::ZERO; 64];
        let n = cut_banks(&t, &mut out, |_, _| false);
        for b in &out[..n] {
            let mid = ((b.from + b.to) / 2) as usize;
            assert_eq!(b.side, -outside_of_bend(&t, mid), "bank {b:?}");
        }
    }

    #[test]
    fn no_bank_buries_another_part_of_the_road() {
        let t = track();
        let mut out = [BankSpan::ZERO; 64];
        let n = cut_banks(&t, &mut out, |_, _| false);
        for b in &out[..n] {
            for node in b.from..=b.to {
                let nd = &t.nodes[node as usize];
                for lateral in [9.0f32, 14.0, 20.0] {
                    if bank_surface(b, node, lateral * b.side).is_none() {
                        continue;
                    }
                    let p = angle_zero::math::Vec3::new(
                        nd.p.x + nd.nrm.x * lateral * b.side,
                        0.0,
                        nd.p.z + nd.nrm.z * lateral * b.side,
                    );
                    for (j, other) in t.nodes.iter().enumerate() {
                        if (j as i64 - node as i64).abs() > 30 {
                            assert!(
                                other.p.horizontal_distance(p) > BANK_CLEARANCE - 3.0,
                                "bank at node {node} reaches node {j}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_bank_meets_the_ground_at_both_ends() {
        let t = track();
        let mut out = [BankSpan::ZERO; 64];
        let n = cut_banks(&t, &mut out, |_, _| false);
        for b in &out[..n] {
            assert!(bank_surface(b, b.from, 12.0 * b.side).is_none());
            assert!(bank_surface(b, b.to, 12.0 * b.side).is_none());
            let mid = (b.from + b.to) / 2;
            assert!(bank_surface(b, mid, 12.0 * b.side).unwrap() > 2.0);
        }
    }
}
