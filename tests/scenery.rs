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

