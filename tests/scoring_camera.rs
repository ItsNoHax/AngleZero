//! Drift scoring, the chase camera, and the derived HUD numbers.

use angle_zero::camera::Camera;
use angle_zero::hud;
use angle_zero::math::{hypot, wrap_pi};
use angle_zero::scoring::{Scoring, COMBO_HOLD, COMBO_MAX, CHUNK_PER_COMBO};
use angle_zero::vehicle::CarState;

// ------------------------------------------------------------------ scoring

/// Feeds a steady slide in for `seconds`.
fn slide(s: &mut Scoring, speed: f32, slip: f32, seconds: f32) {
    let dt = 1.0 / 120.0;
    for _ in 0..(seconds / dt) as usize {
        s.update(true, speed, slip, dt);
    }
}

fn coast(s: &mut Scoring, seconds: f32) {
    let dt = 1.0 / 120.0;
    for _ in 0..(seconds / dt) as usize {
        s.update(false, 30.0, 0.0, dt);
    }
}

#[test]
fn nothing_scores_when_the_car_is_not_drifting() {
    let mut s = Scoring::new();
    coast(&mut s, 5.0);
    assert_eq!(s.score, 0.0);
    assert_eq!(s.combo, 1);
}

#[test]
fn score_accrues_while_drifting_and_scales_with_speed_and_slip() {
    let mut slow = Scoring::new();
    slide(&mut slow, 12.0, 0.3, 0.5);

    let mut fast = Scoring::new();
    slide(&mut fast, 30.0, 0.3, 0.5);

    let mut sideways = Scoring::new();
    slide(&mut sideways, 12.0, 0.6, 0.5);

    assert!(slow.score > 0.0);
    assert!(
        fast.score > slow.score * 2.0,
        "2.5x the speed should score much more: {} vs {}",
        fast.score,
        slow.score
    );
    assert!(
        sideways.score > slow.score * 1.8,
        "twice the slip should score about twice: {} vs {}",
        sideways.score,
        slow.score
    );
}

#[test]
fn the_combo_climbs_one_step_per_chunk_of_drift_points() {
    let mut s = Scoring::new();
    assert_eq!(s.combo, 1);

    // Drive just past one chunk's worth.
    let dt = 1.0 / 120.0;
    let gain_per_step = 20.0 * (0.4 * 3.0) * 8.0 * dt;
    let steps_for_one_chunk = (CHUNK_PER_COMBO / gain_per_step).ceil() as usize + 1;
    for _ in 0..steps_for_one_chunk {
        s.update(true, 20.0, 0.4, dt);
    }
    assert_eq!(s.combo, 2, "combo did not step up after one chunk");
}

#[test]
fn the_combo_is_capped() {
    let mut s = Scoring::new();
    slide(&mut s, 40.0, 0.9, 30.0);
    assert_eq!(s.combo, COMBO_MAX);
    assert_eq!(s.best_combo, COMBO_MAX);
}

#[test]
fn the_combo_multiplies_the_score() {
    // Same slide, but one starts at combo 4.
    let dt = 1.0 / 120.0;
    let mut plain = Scoring::new();
    plain.update(true, 20.0, 0.3, dt);

    let mut multiplied = Scoring::new();
    multiplied.combo = 4;
    multiplied.update(true, 20.0, 0.3, dt);

    assert!((multiplied.score - plain.score * 4.0).abs() < 1e-3);
}

#[test]
fn the_combo_survives_a_brief_pause_then_resets() {
    let mut s = Scoring::new();
    slide(&mut s, 30.0, 0.5, 2.0);
    let combo = s.combo;
    assert!(combo > 1, "expected a combo to have built up");

    // A short interruption keeps it alive.
    coast(&mut s, COMBO_HOLD * 0.5);
    assert_eq!(s.combo, combo, "combo dropped too early");

    // Past the hold window it goes back to 1.
    coast(&mut s, COMBO_HOLD);
    assert_eq!(s.combo, 1);
    assert_eq!(s.chunk, 0.0);
}

#[test]
fn a_wall_tap_kills_the_combo_immediately() {
    let mut s = Scoring::new();
    slide(&mut s, 30.0, 0.5, 2.0);
    assert!(s.combo > 1);
    let score = s.score;

    let announced = s.on_wall_tap();
    assert!(announced, "a wall tap during an active combo should be announced");
    assert_eq!(s.combo, 1);
    assert_eq!(s.chunk, 0.0);
    assert_eq!(s.combo_timer, 0.0);
    // The points already banked are kept.
    assert_eq!(s.score, score);
}

#[test]
fn a_wall_tap_outside_a_combo_is_not_announced() {
    let mut s = Scoring::new();
    assert!(!s.on_wall_tap());
}

#[test]
fn the_best_combo_is_remembered_after_it_resets() {
    let mut s = Scoring::new();
    slide(&mut s, 30.0, 0.5, 3.0);
    let peak = s.combo;
    assert!(peak > 1);
    coast(&mut s, 3.0);
    assert_eq!(s.combo, 1);
    assert_eq!(s.best_combo, peak);
}

#[test]
fn the_combo_bar_reads_as_a_fraction_of_the_hold_window() {
    let mut s = Scoring::new();
    slide(&mut s, 30.0, 0.5, 1.0);
    assert!((s.combo_fraction() - 1.0).abs() < 1e-3);
    coast(&mut s, COMBO_HOLD * 0.5);
    assert!((s.combo_fraction() - 0.5).abs() < 0.05);
    coast(&mut s, COMBO_HOLD);
    assert_eq!(s.combo_fraction(), 0.0);
}

// ------------------------------------------------------------------ camera

fn car_at(x: f32, z: f32, yaw: f32, vx: f32) -> CarState {
    CarState {
        x,
        y: 0.0,
        z,
        yaw,
        vx,
        ..CarState::default()
    }
}

#[test]
fn the_title_camera_orbits_the_parked_car() {
    let mut cam = Camera::new();
    let car = car_at(10.0, -20.0, 0.5, 0.0);

    cam.update_title(&car, 0.0);
    let start_angle = cam.orbit_angle;
    // 0.16 rad/s.
    cam.update_title(&car, 1.0);
    assert!((cam.orbit_angle - start_angle - 0.16).abs() < 1e-4);

    // It stays a fixed distance out, circling the car.
    for _ in 0..40 {
        cam.update_title(&car, 0.1);
        let d = hypot(cam.pos.x - car.x, cam.pos.z - car.z);
        assert!((d - 10.5).abs() < 1e-3, "orbit radius drifted to {d}");
        assert!(cam.pos.y > car.y);
    }
}

#[test]
fn the_chase_camera_settles_behind_the_car() {
    let mut cam = Camera::new();
    let car = car_at(0.0, 0.0, 0.0, 20.0);
    cam.snap_behind(&car);
    for _ in 0..240 {
        cam.update_run(&car, 1.0 / 60.0);
    }

    // Behind means on the opposite side to the heading, which at yaw 0 is +Z.
    assert!(cam.pos.z < -6.0, "camera sat at z {}", cam.pos.z);
    let dist = hypot(cam.pos.x - car.x, cam.pos.z - car.z);
    assert!(
        (7.4..=10.7).contains(&dist),
        "chase distance was {dist}, expected 7.4 + speed term"
    );
    assert!(cam.pos.y > car.y + 3.0);
}

#[test]
fn the_chase_camera_backs_off_and_widens_as_speed_rises() {
    let settle = |vx: f32| {
        let mut cam = Camera::new();
        let car = car_at(0.0, 0.0, 0.0, vx);
        cam.snap_behind(&car);
        for _ in 0..600 {
            cam.update_run(&car, 1.0 / 60.0);
        }
        (hypot(cam.pos.x, cam.pos.z), cam.fov)
    };

    let (slow_d, slow_fov) = settle(5.0);
    let (fast_d, fast_fov) = settle(45.0);
    assert!(fast_d > slow_d + 2.0, "{slow_d} -> {fast_d}");
    assert!(fast_fov > slow_fov + 5.0, "{slow_fov} -> {fast_fov}");
    // Both terms are capped.
    assert!(fast_d <= 10.7);
    assert!(fast_fov <= 72.1);
}

#[test]
fn the_chase_camera_follows_the_direction_of_travel_not_the_nose() {
    // A car sliding sideways: body yaw 0, but travelling off to one side.
    let mut cam = Camera::new();
    let sliding = CarState {
        yaw: 0.0,
        vx: 20.0,
        vy: 20.0,
        ..CarState::default()
    };
    cam.snap_behind(&sliding);
    for _ in 0..300 {
        cam.update_run(&sliding, 1.0 / 60.0);
    }
    // Velocity heading is 45° off the nose, and the camera should have swung to it.
    assert!(
        wrap_pi(cam.yaw - 0.785).abs() < 0.1,
        "camera yaw {} did not follow the velocity heading",
        cam.yaw
    );
}

#[test]
fn a_slow_car_is_framed_on_its_nose_instead() {
    let mut cam = Camera::new();
    // Below 4 m/s the velocity heading is noise, so the body yaw is used.
    let crawling = CarState {
        yaw: 1.0,
        vx: 0.5,
        vy: 0.5,
        ..CarState::default()
    };
    cam.snap_behind(&crawling);
    for _ in 0..300 {
        cam.update_run(&crawling, 1.0 / 60.0);
    }
    assert!(wrap_pi(cam.yaw - 1.0).abs() < 0.05, "camera yaw {}", cam.yaw);
}

#[test]
fn camera_shake_decays_to_nothing() {
    let mut cam = Camera::new();
    let car = car_at(0.0, 0.0, 0.0, 10.0);
    cam.add_shake(0.5);
    assert!(cam.shake > 0.0);
    for _ in 0..120 {
        cam.update_run(&car, 1.0 / 60.0);
    }
    assert_eq!(cam.shake, 0.0);
}

// ------------------------------------------------------------------ hud

#[test]
fn the_run_clock_formats_as_minutes_seconds_hundredths() {
    assert_eq!(hud::split_time(0.0), (0, 0, 0));
    assert_eq!(hud::split_time(9.87), (0, 9, 87));
    assert_eq!(hud::split_time(65.5), (1, 5, 50));
    assert_eq!(hud::split_time(3599.99), (59, 59, 99));
}

#[test]
fn the_gearbox_has_six_gears_and_a_reverse() {
    assert_eq!(hud::gear(0.0), hud::Gear::Forward(1));
    assert_eq!(hud::gear(9.0), hud::Gear::Forward(1));
    assert_eq!(hud::gear(9.6), hud::Gear::Forward(2));
    assert_eq!(hud::gear(100.0), hud::Gear::Forward(6));
    assert_eq!(hud::gear(-2.0), hud::Gear::Reverse);
    // Just rolling back is not reverse yet.
    assert_eq!(hud::gear(-0.3), hud::Gear::Forward(1));
}

#[test]
fn the_rev_counter_sweeps_within_each_gear_and_stays_in_range() {
    let mut v = 0.0;
    while v < 80.0 {
        let r = hud::rpm(v, 1.0);
        assert!((0.0..=1.0).contains(&r), "rpm {r} at {v} m/s");
        v += 0.13;
    }
    // Throttle lifts the needle.
    assert!(hud::rpm(20.0, 1.0) > hud::rpm(20.0, 0.0));
}

#[test]
fn the_rev_bar_changes_colour_at_the_documented_thresholds() {
    assert_eq!(hud::rev_zone(0.5), hud::RevZone::Green);
    assert_eq!(hud::rev_zone(0.8), hud::RevZone::Amber);
    assert_eq!(hud::rev_zone(0.95), hud::RevZone::Red);
}
