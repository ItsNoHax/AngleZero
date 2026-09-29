//! The city below the summit: on the right of the title shot, clear of the course, and never
//! hidden by the course's own hillside from the car park.

use angle_zero::city::{
    bearing_of, city_lights, city_towers, downtown, origin, Tower, CITY_CLEARANCE, CITY_LIGHTS,
    CITY_TOWERS,
};
use angle_zero::math::Vec3;
use angle_zero::scenery::{terrain_drop, ValleyLight, DRAW_DISTANCE};
use angle_zero::track::Track;

mod summit {
    include!("common/summit_camera.rs");
}

fn track() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

fn lights(t: &Track) -> Vec<ValleyLight> {
    let mut out = vec![ValleyLight::ZERO; CITY_LIGHTS];
    let n = city_lights(t, &mut out);
    out.truncate(n);
    out
}

#[test]
fn the_city_is_a_city_but_not_a_blanket_of_lights() {
    let t = track();
    let n = lights(&t).len();
    assert!((1000..2600).contains(&n), "{n} city lights");
    let mut towers = [Tower::ZERO; CITY_TOWERS];
    let k = city_towers(&t, &mut towers);
    assert!(k >= 36, "only {k} towers");
    assert!(towers[..k].iter().filter(|t| t.beacon()).count() > 5);
}

#[test]
fn no_city_light_is_near_the_road_or_left_of_the_view() {
    let t = track();
    for l in lights(&t) {
        let near = t.nodes.iter().step_by(2).map(|n| n.p.horizontal_distance(l.p)).fold(f32::MAX, f32::min);
        assert!(near > CITY_CLEARANCE - 10.0, "a light {near:.0} m from the road");
        let (b, d) = bearing_of(&t, l.p);
        assert!(b > -6.0, "a light {b:.1}° left of the car park's facing");
        assert!(d < DRAW_DISTANCE, "a light {d:.0} m out, past the far plane");
    }
}

#[test]
fn towers_are_drawn_farthest_first() {
    let t = track();
    let mut towers = [Tower::ZERO; CITY_TOWERS];
    let k = city_towers(&t, &mut towers);
    let (x, z, _) = origin(&t);
    let far = |t: &Tower| (t.base.x - x).hypot(t.base.z - z);
    for w in towers[..k].windows(2) {
        assert!(far(&w[0]) >= far(&w[1]));
    }
}

/// The highest the course's own hillside stands at each degree of bearing from `eye`, as an
/// elevation angle, the way the terrain ribbon lays it out.
fn hillside_horizon(t: &Track, eye: Vec3) -> std::collections::HashMap<i32, f32> {
    let mut h = std::collections::HashMap::new();
    for n in t.nodes.iter().step_by(2) {
        for l in [-190.0f32, -150.0, -96.0, -70.0, -48.0, -22.0, 22.0, 48.0, 70.0, 96.0, 150.0, 190.0] {
            let (x, z) = (n.p.x + n.nrm.x * l, n.p.z + n.nrm.z * l);
            let y = n.p.y + terrain_drop(l);
            let d = (x - eye.x).hypot(z - eye.z);
            if d < 60.0 {
                continue;
            }
            let b = (z - eye.z).atan2(x - eye.x).to_degrees().round() as i32;
            let e = (y - eye.y).atan2(d);
            let v = h.entry(b).or_insert(-10.0f32);
            *v = v.max(e);
        }
    }
    h
}

#[test]
fn from_the_title_camera_the_city_stands_clear_of_the_hillside() {
    let t = track();
    let cam = summit::title_camera(&t);
    let h = hillside_horizon(&t, cam.pos);
    let mut hidden = 0;
    let all = lights(&t);
    for l in all.iter() {
        let d = (l.p.x - cam.pos.x).hypot(l.p.z - cam.pos.z);
        let b = (l.p.z - cam.pos.z).atan2(l.p.x - cam.pos.x).to_degrees().round() as i32;
        let e = (l.p.y - cam.pos.y).atan2(d);
        if e < *h.get(&b).unwrap_or(&-10.0) {
            hidden += 1;
        }
    }
    // The near left edge of the city lies behind the hillside under the first hairpin from here,
    // though not from lower down the course; the rest must be in plain view.
    assert!(hidden * 4 < all.len(), "{hidden} of {} city lights are behind the hillside", all.len());
}

#[test]
fn downtown_is_on_the_right_of_the_title_shot() {
    let t = track();
    let cam = summit::title_camera(&t);
    let (sx, sy) = summit::project(&cam, downtown(&t)).expect("downtown behind the camera");
    assert!((300.0..470.0).contains(&sx) && (70.0..200.0).contains(&sy), "downtown at {sx:.0}, {sy:.0}");
}
