//! View matrix construction.
//!
//! This exists because `sceGumLookAt` in rust-psp 0.3.13 silently does nothing: its helper
//! `gum_look_at` shadows its own `&mut` output parameter with a local, so the caller's matrix is
//! never written and the view matrix stays identity. The symptom on hardware is subtle — the
//! world still draws, because it is rendered with an identity model matrix, but everything is
//! positioned as though the camera sat at the world origin.
//!
//! Building the matrix here instead means it can be checked on the host.

use angle_zero::math::{Mat4, Vec3};

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

fn assert_vec_close(got: Vec3, want: Vec3, what: &str) {
    assert!(
        close(got.x, want.x) && close(got.y, want.y) && close(got.z, want.z),
        "{what}: got ({}, {}, {}), want ({}, {}, {})",
        got.x,
        got.y,
        got.z,
        want.x,
        want.y,
        want.z
    );
}

const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);

#[test]
fn the_eye_maps_to_the_view_space_origin() {
    let eye = Vec3::new(12.0, 4.0, -30.0);
    let center = Vec3::new(12.0, 3.0, -40.0);
    let m = Mat4::look_at(eye, center, UP);
    assert_vec_close(m.transform_point(eye), Vec3::ZERO, "eye");
}

#[test]
fn the_look_at_target_lands_on_the_negative_z_axis() {
    // Whatever the camera is aimed at must project to the centre of the screen, which in view
    // space means x = y = 0 and z negative (in front of the camera).
    let cases = [
        (
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.0, -10.0),
        ),
        (
            Vec3::new(12.0, 4.0, -30.0),
            Vec3::new(20.0, 1.0, -38.0),
        ),
        (
            Vec3::new(-100.0, -50.0, 300.0),
            Vec3::new(-90.0, -52.0, 280.0),
        ),
    ];
    for (eye, center) in cases {
        let m = Mat4::look_at(eye, center, UP);
        let v = m.transform_point(center);
        assert!(close(v.x, 0.0), "target x was {} (eye {:?})", v.x, eye);
        assert!(close(v.y, 0.0), "target y was {} (eye {:?})", v.y, eye);
        assert!(v.z < 0.0, "target should be in front, z was {}", v.z);
        // Distance is preserved: the view transform is a rigid motion.
        let d = center.sub(eye).length();
        assert!(close(-v.z, d), "target depth {} vs distance {d}", -v.z);
    }
}

#[test]
fn a_point_to_the_cameras_right_has_positive_view_x() {
    // Camera at the origin looking down -Z; +X is then to the right on screen.
    let m = Mat4::look_at(Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0), UP);
    let v = m.transform_point(Vec3::new(5.0, 0.0, -10.0));
    assert!(v.x > 0.0, "expected positive view x, got {}", v.x);
    assert!(close(v.y, 0.0));
}

#[test]
fn a_point_above_the_camera_has_positive_view_y() {
    let m = Mat4::look_at(Vec3::ZERO, Vec3::new(0.0, 0.0, -10.0), UP);
    let v = m.transform_point(Vec3::new(0.0, 5.0, -10.0));
    assert!(v.y > 0.0, "expected positive view y, got {}", v.y);
}

#[test]
fn the_rotation_part_stays_orthonormal() {
    let m = Mat4::look_at(
        Vec3::new(3.0, 9.0, -4.0),
        Vec3::new(-20.0, 2.0, -60.0),
        UP,
    );
    let c = m.columns();
    // Column-major: the upper-left 3x3 rows are the view basis vectors.
    let right = Vec3::new(c[0], c[4], c[8]);
    let up = Vec3::new(c[1], c[5], c[9]);
    let back = Vec3::new(c[2], c[6], c[10]);

    for (v, name) in [(right, "right"), (up, "up"), (back, "back")] {
        assert!(close(v.length(), 1.0), "{name} was not unit: {}", v.length());
    }
    assert!(close(right.dot(up), 0.0));
    assert!(close(right.dot(back), 0.0));
    assert!(close(up.dot(back), 0.0));
}

#[test]
fn the_bottom_row_is_the_affine_identity() {
    let m = Mat4::look_at(
        Vec3::new(1.0, 2.0, 3.0),
        Vec3::new(4.0, 5.0, 6.0),
        UP,
    );
    let c = m.columns();
    assert!(close(c[3], 0.0));
    assert!(close(c[7], 0.0));
    assert!(close(c[11], 0.0));
    assert!(close(c[15], 1.0));
}

#[test]
fn a_degenerate_look_direction_does_not_produce_nan() {
    // Eye and target coincident, and an up vector parallel to the view direction: both would
    // divide by zero if normalisation were unguarded. A NaN here wipes out the whole 3D scene.
    for m in [
        Mat4::look_at(Vec3::ZERO, Vec3::ZERO, UP),
        Mat4::look_at(Vec3::ZERO, Vec3::new(0.0, 10.0, 0.0), UP),
    ] {
        for v in m.columns().iter() {
            assert!(v.is_finite(), "degenerate look_at produced {v}");
        }
    }
}

#[test]
fn the_chase_camera_frames_the_car_it_is_following() {
    // The case that actually failed on hardware: a camera behind the car, far from the origin.
    use angle_zero::camera::Camera;
    use angle_zero::vehicle::CarState;

    let car = CarState {
        x: 170.0,
        y: -29.0,
        z: -342.0,
        yaw: 1.2,
        vx: 18.0,
        ..CarState::default()
    };
    let mut cam = Camera::new();
    cam.snap_behind(&car);
    for _ in 0..120 {
        cam.update_run(&car, 1.0 / 60.0);
    }

    let m = Mat4::look_at(cam.pos, cam.look_at, UP);
    let v = m.transform_point(Vec3::new(car.x, car.y + 0.8, car.z));
    // The car must be in front of the camera and close to the middle of the frame.
    assert!(v.z < 0.0, "car ended up behind the camera: z {}", v.z);
    let depth = -v.z;
    assert!(
        (5.0..15.0).contains(&depth),
        "car should be a chase distance away, was {depth}"
    );
    assert!(
        v.x.abs() / depth < 0.2,
        "car is {} off-axis at {depth} m — it would sit far from screen centre",
        v.x
    );
}
