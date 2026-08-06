//! Chunk culling.
//!
//! Culling was the first suspect when geometry was reported vanishing behind the car, and these
//! tests are what ruled it out: the chunk holding the car sits roughly 47 m clear of the culling
//! threshold at every speed sampled, so it is never a near miss. The cause of that report is still
//! open.
//!
//! What did come out of the investigation is that the behind-camera test has to be measured along
//! the camera's real view direction. The chase camera looks down at the road from several metres
//! up, so its near plane is pitched, and testing against the flat chase heading is not a frustum
//! test at all — it can reject geometry that is plainly in shot.

use angle_zero::camera::Camera;
use angle_zero::math::Vec3;
use angle_zero::mesh::{chunk_visible, ribbon_capacity, Ribbon, Station};
use angle_zero::track::Track;
use angle_zero::vehicle::{Input, Vehicle, FIXED_DT};

const FOG_FAR: f32 = 330.0;

const ROAD: [Station; 5] = [
    Station::new(-6.4, 0.0, 0xff20_1c1a),
    Station::new(-5.2, 0.02, 0xff20_1c1a),
    Station::new(0.0, 0.03, 0xff20_1c1a),
    Station::new(5.2, 0.02, 0xff20_1c1a),
    Station::new(6.4, 0.0, 0xff20_1c1a),
];

fn track() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

/// The camera state after driving a while, which is where the fault showed up.
fn driven_camera(t: &Track, seconds: f32) -> (Camera, Vehicle) {
    let mut v = Vehicle::new();
    v.place_at_node(t, 2);
    let mut cam = Camera::new();
    cam.snap_behind(&v.state);

    let steps = (seconds / FIXED_DT) as usize;
    for _ in 0..steps {
        // Follow the centreline so the run stays on the road.
        let ahead = (v.locator.last_idx + 18).min(angle_zero::track::NODE_COUNT - 1);
        let p = t.nodes[ahead].p;
        let want = angle_zero::math::atan2(p.x - v.state.x, p.z - v.state.z);
        let steer_in = (angle_zero::math::wrap_pi(want - v.state.yaw) * 2.5).clamp(-1.0, 1.0);
        v.step(
            t,
            Input {
                throttle: 1.0,
                steer_in,
                ..Input::default()
            },
            FIXED_DT,
        );
        cam.update_run(&v.state, FIXED_DT);
    }
    (cam, v)
}

/// What it uses now.
fn true_forward(cam: &Camera) -> Vec3 {
    cam.look_at.sub(cam.pos).normalized()
}

#[test]
fn the_camera_really_does_look_downwards() {
    // If it did not, the two forward vectors would agree and none of this would matter.
    let t = track();
    let (cam, _) = driven_camera(&t, 20.0);
    let f = true_forward(&cam);
    assert!(
        f.y < -0.05,
        "expected the chase camera to be pitched down, got forward.y = {}",
        f.y
    );
}

#[test]
fn the_chunk_under_the_car_is_never_culled() {
    let t = track();
    let mut road = Box::new(Ribbon::<{ ribbon_capacity(5) }>::EMPTY);
    road.build(&t, &ROAD);

    // Sample the whole descent rather than one moment, since the fault depended on where the
    // camera happened to sit relative to a chunk boundary.
    for seconds in [5.0f32, 10.0, 20.0, 30.0, 45.0, 60.0] {
        let (cam, v) = driven_camera(&t, seconds);
        let forward = true_forward(&cam);

        // The chunk holding the car must always be drawn — it is the ground under the wheels.
        let car = Vec3::new(v.state.x, v.state.y, v.state.z);
        let mut found = false;
        for chunk in road.chunks.iter() {
            if chunk.center.sub(car).length() <= chunk.radius {
                found = true;
                assert!(
                    chunk_visible(chunk, cam.pos, forward, FOG_FAR),
                    "at {seconds}s the chunk containing the car was culled"
                );
            }
        }
        assert!(found, "at {seconds}s no chunk contained the car");
    }
}

/// A sphere entirely behind the near plane is invisible; one straddling it is not. Synthetic, so
/// the boundary is exact rather than whatever the track happens to produce.
#[test]
fn the_near_plane_test_is_measured_along_the_view_direction() {
    let eye = Vec3::new(0.0, 3.6, 0.0);
    // Looking along +Z and tilted down onto the road, as the chase camera is.
    let forward = Vec3::new(0.0, -0.11, 0.99).normalized();

    let sphere = |cx: f32, cy: f32, cz: f32, r: f32| angle_zero::mesh::Chunk {
        start: 0,
        count: 6,
        center: Vec3::new(cx, cy, cz),
        radius: r,
    };

    // Straight ahead: plainly visible.
    assert!(chunk_visible(&sphere(0.0, 0.0, 60.0, 20.0), eye, forward, FOG_FAR));
    // Underneath the camera: behind along the flat heading, but well inside a pitched frustum.
    assert!(chunk_visible(&sphere(0.0, 0.0, -4.0, 20.0), eye, forward, FOG_FAR));
    // Far enough back that the whole sphere clears the near plane.
    assert!(!chunk_visible(
        &sphere(0.0, 0.0, -400.0, 20.0),
        eye,
        forward,
        FOG_FAR
    ));
    // Beyond the fog, so nothing of it would survive being drawn.
    assert!(!chunk_visible(
        &sphere(0.0, 0.0, 900.0, 20.0),
        eye,
        forward,
        FOG_FAR
    ));
    // Nothing in it to draw.
    let mut empty = sphere(0.0, 0.0, 60.0, 20.0);
    empty.count = 0;
    assert!(!chunk_visible(&empty, eye, forward, FOG_FAR));
}
