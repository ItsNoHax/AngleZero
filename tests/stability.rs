//! Long-run numerical stability.
//!
//! A NaN anywhere in the car or camera state is invisible on the host but catastrophic on the
//! PSP: `sceGumLookAt` produces a NaN view matrix and the entire 3D scene silently disappears,
//! leaving only the 2D HUD. These runs use deliberately awful driving, because that is what
//! finds the corner cases.

use angle_zero::game::{Buttons, Game, Phase};
use angle_zero::track::Track;

fn track() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

const NONE: Buttons = Buttons {
    cross: false,
    circle: false,
    square: false,
    triangle: false,
    up: false,
    down: false,
    left: false,
    right: false,
    start: false,
    analog_x: 0.0,
};

fn assert_finite(g: &Game, when: f32) {
    let s = &g.vehicle.state;
    for (name, v) in [
        ("x", s.x),
        ("y", s.y),
        ("z", s.z),
        ("yaw", s.yaw),
        ("vx", s.vx),
        ("vy", s.vy),
        ("yaw_rate", s.yaw_rate),
        ("steer", s.steer),
        ("wheel_spin", s.wheel_spin),
    ] {
        assert!(v.is_finite(), "car {name} became {v} after {when:.1}s");
    }
    // Angles handed to the VFPU as rotations must stay in a range where f32 still has useful
    // resolution, so nothing here may accumulate without bound.
    assert!(
        s.wheel_spin.abs() <= core::f32::consts::PI * 2.0 + 1e-3,
        "wheel_spin grew to {} after {when:.1}s",
        s.wheel_spin
    );
    assert!(
        s.yaw.abs() < 1.0e4,
        "yaw grew to {} after {when:.1}s",
        s.yaw
    );
    for (name, v) in [
        ("cam.x", g.camera.pos.x),
        ("cam.y", g.camera.pos.y),
        ("cam.z", g.camera.pos.z),
        ("cam.yaw", g.camera.yaw),
        ("cam.fov", g.camera.fov),
        ("look.x", g.camera.look_at.x),
        ("look.y", g.camera.look_at.y),
        ("look.z", g.camera.look_at.z),
    ] {
        assert!(v.is_finite(), "camera {name} became {v} after {when:.1}s");
    }
    // A camera sitting exactly on its own look-at target makes lookAt degenerate too.
    let d = ((g.camera.pos.x - g.camera.look_at.x).powi(2)
        + (g.camera.pos.y - g.camera.look_at.y).powi(2)
        + (g.camera.pos.z - g.camera.look_at.z).powi(2))
    .sqrt();
    assert!(
        d > 0.01,
        "camera collapsed onto its look-at target after {when:.1}s"
    );
}

/// Exactly what the headless capture does: hold X and never steer.
#[test]
fn holding_throttle_with_no_steering_stays_finite_for_minutes() {
    let t = track();
    let mut g = Box::new(Game::new());
    g.enter_title(&t);

    let held = Buttons { cross: true, ..NONE };
    let dt = 1.0 / 60.0;
    let mut elapsed = 0.0;

    for _ in 0..(240.0 / dt) as usize {
        g.update(&t, held, dt);
        elapsed += dt;
        assert_finite(&g, elapsed);
        if g.phase == Phase::Results {
            break;
        }
    }
}

#[test]
fn permanent_handbrake_and_full_lock_stays_finite() {
    let t = track();
    let mut g = Box::new(Game::new());
    g.enter_title(&t);
    g.update(&t, Buttons { cross: true, ..NONE }, 1.0 / 60.0);

    let chaos = Buttons {
        cross: true,
        circle: true,
        left: true,
        ..NONE
    };
    let dt = 1.0 / 60.0;
    let mut elapsed = 0.0;
    for _ in 0..(180.0 / dt) as usize {
        g.update(&t, chaos, dt);
        elapsed += dt;
        assert_finite(&g, elapsed);
    }
}

#[test]
fn slamming_between_opposite_inputs_stays_finite() {
    let t = track();
    let mut g = Box::new(Game::new());
    g.enter_title(&t);
    g.update(&t, Buttons { cross: true, ..NONE }, 1.0 / 60.0);

    let dt = 1.0 / 60.0;
    let mut elapsed = 0.0;
    for i in 0..(180.0 / dt) as usize {
        // Flip everything every few frames.
        let flip = (i / 7) % 2 == 0;
        let b = Buttons {
            cross: flip,
            square: !flip,
            circle: i % 3 == 0,
            left: flip,
            right: !flip,
            ..NONE
        };
        g.update(&t, b, dt);
        elapsed += dt;
        assert_finite(&g, elapsed);
    }
}

#[test]
fn the_car_never_leaves_the_mountain() {
    let t = track();
    let mut g = Box::new(Game::new());
    g.enter_title(&t);

    let held = Buttons { cross: true, ..NONE };
    let dt = 1.0 / 60.0;
    for _ in 0..(240.0 / dt) as usize {
        g.update(&t, held, dt);
        if g.phase == Phase::Results {
            break;
        }
        // The track's bounding box is roughly 1.7 km across and 180 m tall; nothing legitimate
        // puts the car far outside it.
        let s = &g.vehicle.state;
        assert!(
            s.x.abs() < 4000.0 && s.z.abs() < 4000.0 && s.y > -1000.0 && s.y < 400.0,
            "car left the world at ({}, {}, {})",
            s.x,
            s.y,
            s.z
        );
    }
}
