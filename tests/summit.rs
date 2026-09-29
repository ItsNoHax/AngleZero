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

mod camera {
    include!("common/summit_camera.rs");
}
use camera::{project, title_camera};

#[test]
fn the_title_shot_frames_the_car_and_the_first_hairpin() {
    let t = track();
    let cam = title_camera(&t);
    let (x, y, z, _, _) = angle_zero::track::carpark_parking(&t);
    let (cx, cy) = project(&cam, angle_zero::math::Vec3::new(x, y + 0.7, z)).expect("car behind the camera");
    assert!((180.0..330.0).contains(&cx) && (130.0..250.0).contains(&cy), "car at {cx:.0}, {cy:.0}");
    // The first hairpin, 180-300 m down the road, sweeps across the left half of the frame.
    let mut seen = 0;
    for i in 0..NODE_COUNT {
        let n = &t.nodes[i];
        if n.s < 180.0 || n.s > 300.0 {
            continue;
        }
        if let Some((sx, sy)) = project(&cam, n.p) {
            if (0.0..260.0).contains(&sx) && (60.0..200.0).contains(&sy) {
                seen += 1;
            }
        }
    }
    assert!(seen > 20, "only {seen} nodes of the first hairpin are on the left of the title shot");
}

/// The whole car stays in the title shot once the camera has eased in, with room to spare above
/// the name in the bottom corner — for a car as long and wide as any in the catalogue.
#[test]
fn the_title_shot_keeps_the_whole_car_in_frame() {
    let t = track();
    let (x, y, z, yaw, _) = angle_zero::track::carpark_parking(&t);
    let car = angle_zero::vehicle::CarState { x, y, z, yaw, ..Default::default() };
    let mut cam = angle_zero::camera::Camera::new();
    cam.fov = angle_zero::camera::TITLE_FOV;
    for _ in 0..400 {
        cam.update_title(&car, 0.1);
    }
    let (fx, fz) = (yaw.sin(), yaw.cos());
    let (rx, rz) = (-fz, fx);
    for (a, c) in [(2.5f32, 1.0f32), (2.5, -1.0), (-2.5, 1.0), (-2.5, -1.0)] {
        for h in [0.0f32, 1.4] {
            let p = angle_zero::math::Vec3::new(x + fx * a + rx * c, y + h, z + fz * a + rz * c);
            let (sx, sy) = project(&cam, p).expect("car corner behind the camera");
            assert!((20.0..460.0).contains(&sx) && (20.0..238.0).contains(&sy), "car corner at {sx:.0}, {sy:.0}");
        }
    }
}
