//! Skid decals and tyre smoke.
//!
//! Both are fixed-size ring buffers that must never allocate: the PSP has no allocator here, and
//! a slide can emit for the whole descent.

use angle_zero::effects::{Effects, MAX_PUFFS, MAX_SKIDS, PUFF_LIFE, SKID_ALPHA};
use angle_zero::math::hypot;
use angle_zero::vehicle::{CarShape, CarState};

/// The shape marks are laid against. A car with an unmistakable track and wheelbase, so that a
/// mark laid from the wrong number cannot land in roughly the right place by luck.
fn shape() -> CarShape {
    CarShape {
        wheel_radius: 0.29,
        rear_hub_x: 0.73,
        rear_hub_z: -1.29,
        ..CarShape::DEFAULT
    }
}

fn car() -> CarState {
    CarState {
        x: 100.0,
        y: -20.0,
        z: -300.0,
        yaw: 0.7,
        vx: 25.0,
        ..CarState::default()
    }
}

#[test]
fn a_slide_lays_one_mark_per_rear_wheel() {
    let mut fx = Effects::new();
    assert_eq!(fx.live_skids(), 0);
    fx.emit_skids(&car(), &shape(), 25.0);
    assert_eq!(fx.live_skids(), 2);
}

#[test]
fn the_marks_land_under_the_rear_wheels() {
    let c = car();
    let mut fx = Effects::new();
    fx.emit_skids(&c, &shape(), 25.0);

    // Both marks must be exactly as far from the car's centre as its rear hubs are. The numbers
    // come from the shape rather than being written out again, because that is the whole claim:
    // the marks follow whichever car is loaded.
    let shape = shape();
    let expected = hypot(shape.rear_hub_x, shape.rear_hub_z);
    for s in fx.skids().iter().filter(|s| s.active) {
        let d = hypot(s.x - c.x, s.z - c.z);
        assert!(
            (d - expected).abs() < 1e-3,
            "mark {d} m from the car, rear hubs are at {expected}"
        );
        // Laid on the road, just above it.
        assert!((s.y - (c.y + 0.05)).abs() < 1e-4);
    }
    // The two marks are on opposite sides, so they are a rear track apart.
    let live: Vec<_> = fx.skids().iter().filter(|s| s.active).collect();
    assert_eq!(live.len(), 2);
    let apart = hypot(live[0].x - live[1].x, live[0].z - live[1].z);
    assert!(
        (apart - 2.0 * shape.rear_hub_x).abs() < 1e-3,
        "marks were {apart} m apart"
    );
}

#[test]
fn faster_slides_leave_longer_marks() {
    let mut slow = Effects::new();
    slow.emit_skids(&car(), &shape(), 10.0);
    let mut fast = Effects::new();
    fast.emit_skids(&car(), &shape(), 40.0);

    let stretch = |fx: &Effects| fx.skids().iter().find(|s| s.active).unwrap().stretch;
    // max(1, speed * 0.06): 10 m/s is below the floor, 40 m/s is not.
    assert!((stretch(&slow) - 1.0).abs() < 1e-4);
    assert!((stretch(&fast) - 2.4).abs() < 1e-4);
}

#[test]
fn the_skid_pool_wraps_instead_of_growing() {
    let mut fx = Effects::new();
    for _ in 0..MAX_SKIDS * 3 {
        fx.emit_skids(&car(), &shape(), 25.0);
    }
    assert_eq!(fx.live_skids(), MAX_SKIDS);
    assert_eq!(fx.skids().len(), MAX_SKIDS);
}

#[test]
fn smoke_puffs_are_emitted_behind_the_car() {
    let c = car();
    let mut fx = Effects::new();
    // Emission is probabilistic, so ask enough times to be sure of at least one.
    for _ in 0..80 {
        fx.emit_smoke(&c);
    }
    assert!(fx.live_puffs() > 0);

    for p in fx.puffs().iter().filter(|p| p.life > 0.0) {
        // 1.4 m behind the car, with up to half a metre of scatter.
        let back_x = c.x - 1.4 * c.yaw.sin();
        let back_z = c.z - 1.4 * c.yaw.cos();
        assert!(
            hypot(p.x - back_x, p.z - back_z) < 0.8,
            "puff was not behind the car"
        );
        assert!((p.y - (c.y + 0.4)).abs() < 1e-4);
    }
}

#[test]
fn smoke_emission_is_intermittent_rather_than_every_call() {
    let c = car();
    let mut fx = Effects::new();
    let mut emitted = 0;
    let mut last = 0;
    for _ in 0..400 {
        fx.emit_smoke(&c);
        let n = fx.live_puffs();
        if n != last {
            emitted += 1;
        }
        last = n;
        fx.update(1.0 / 120.0);
    }
    // Roughly 55% of calls should emit; anything near 0% or 100% means the gate is broken.
    assert!(emitted > 20, "smoke barely emitted ({emitted})");
}

#[test]
fn the_puff_pool_wraps_instead_of_growing() {
    let mut fx = Effects::new();
    for _ in 0..MAX_PUFFS * 8 {
        fx.emit_smoke(&car());
    }
    assert!(fx.live_puffs() <= MAX_PUFFS);
    assert_eq!(fx.puffs().len(), MAX_PUFFS);
}

#[test]
fn puffs_rise_fade_and_expire() {
    let c = car();
    let mut fx = Effects::new();
    for _ in 0..40 {
        fx.emit_smoke(&c);
    }
    assert!(fx.live_puffs() > 0);

    let first = *fx.puffs().iter().find(|p| p.life > 0.0).unwrap();
    let start_alpha = first.alpha();
    let start_scale = first.scale();
    let start_y = first.y;
    assert!((start_alpha - SKID_ALPHA).abs() < 0.02);

    fx.update(PUFF_LIFE * 0.5);
    let mid = fx
        .puffs()
        .iter()
        .find(|p| p.life > 0.0)
        .copied()
        .expect("puffs should still be alive halfway through");
    assert!(mid.alpha() < start_alpha, "puff did not fade");
    assert!(mid.scale() > start_scale, "puff did not grow");
    assert!(mid.y > start_y, "puff did not rise");

    // Everything is gone once the life window closes.
    fx.update(PUFF_LIFE);
    assert_eq!(fx.live_puffs(), 0);
}

#[test]
fn resetting_clears_the_marks_for_a_new_run() {
    let mut fx = Effects::new();
    for _ in 0..50 {
        fx.emit_skids(&car(), &shape(), 25.0);
        fx.emit_smoke(&car());
    }
    assert!(fx.live_skids() > 0);
    fx.reset();
    assert_eq!(fx.live_skids(), 0);
    assert_eq!(fx.live_puffs(), 0);
}

#[test]
fn effects_are_deterministic_so_captures_reproduce() {
    // No OS entropy on the PSP, and headless screenshots need to be comparable run to run.
    let run = || {
        let mut fx = Effects::new();
        for _ in 0..200 {
            fx.emit_smoke(&car());
            fx.update(1.0 / 120.0);
        }
        fx.live_puffs()
    };
    assert_eq!(run(), run());
}
