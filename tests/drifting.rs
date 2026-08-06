//! Drifting end to end: "handbrake breaks the rear away and the car holds an angle
//! with counter-steer".
//!
//! Worth testing through `Game` rather than the vehicle alone, because it is the combination that
//! matters: the slide has to last long enough, at enough speed, to actually score and to lay
//! marks. Pinning full lock and holding the handbrake does *not* do this — it spins the car, and
//! speed collapses before the slip angle is ever high enough to count. That looks like broken
//! scoring and is really just bad driving, so the driver below models the real technique.

use angle_zero::game::{Buttons, Game};
use angle_zero::math::{atan2, clamp, hypot, wrap_pi};
use angle_zero::scoring::DRIFT_SLIP_THRESHOLD;
use angle_zero::track::{Track, NODE_COUNT};

const NONE: Buttons = Buttons {
    cross: false,
    circle: false,
    square: false,
    triangle: false,
    up: false,
    down: false,
    left: false,
    right: false,
    analog_x: 0.0,
};

struct Run {
    drift_frames: u32,
    score: f32,
    best_combo: u32,
    max_skids: usize,
    saw_smoke: bool,
}

/// Builds speed, then repeatedly stabs the handbrake and catches the slide on opposite lock.
fn drive_a_drifting_descent(seconds: f32) -> (Box<Game>, Run) {
    let mut track = Box::new(Track::EMPTY);
    Track::generate(&mut track);
    let mut g = Box::new(Game::new());
    g.enter_title(&track);

    let dt = 1.0 / 60.0;
    let mut out = Run {
        drift_frames: 0,
        score: 0.0,
        best_combo: 1,
        max_skids: 0,
        saw_smoke: false,
    };

    for i in 0..(seconds / dt) as usize {
        let secs = i as f32 * dt;
        let st = g.vehicle.state;

        // Aim at a point down the road, so the car stays roughly on the track.
        let ahead = (g.vehicle.locator.last_idx + 18).min(NODE_COUNT - 1);
        let p = track.nodes[ahead].p;
        let want = atan2(p.x - st.x, p.z - st.z);
        let lane = clamp(wrap_pi(want - st.yaw) * 2.5, -1.0, 1.0);

        // Positive `vy` is toward the car's right, so catching a slide means steering right.
        let counter = clamp(-st.vy * 0.18, -1.0, 1.0);

        let buttons = if secs < 18.0 {
            // Get up to a speed where a slide can score at all.
            Buttons {
                cross: true,
                analog_x: lane,
                ..NONE
            }
        } else {
            // A short stab every few seconds, held on opposite lock in between.
            let stab = ((secs - 18.0) % 6.0) < 0.5;
            Buttons {
                cross: true,
                circle: stab,
                analog_x: clamp(lane * 0.35 + counter, -1.0, 1.0),
                ..NONE
            }
        };

        g.update(&track, buttons, dt);

        if g.vehicle.drifting {
            out.drift_frames += 1;
        }
        out.max_skids = out.max_skids.max(g.effects.live_skids());
        if g.effects.live_puffs() > 0 {
            out.saw_smoke = true;
        }
    }

    out.score = g.scoring.score;
    out.best_combo = g.scoring.best_combo;
    (g, out)
}

#[test]
fn counter_steering_out_of_a_handbrake_stab_sustains_a_scoring_drift() {
    let (_, run) = drive_a_drifting_descent(120.0);

    assert!(
        run.drift_frames > 300,
        "only {} frames of drifting in two minutes — the slides are not being held",
        run.drift_frames
    );
    assert!(run.score > 500.0, "drift score was only {}", run.score);
    assert!(
        run.best_combo > 1,
        "the multiplier never climbed, so no single slide lasted a chunk's worth"
    );
}

#[test]
fn holding_full_lock_and_the_handbrake_spins_rather_than_drifts() {
    // The counterpart to the test above, and the reason it exists: this input looks like it
    // should drift and does not, because speed collapses before the slip angle counts.
    let mut track = Box::new(Track::EMPTY);
    Track::generate(&mut track);
    let mut g = Box::new(Game::new());
    g.enter_title(&track);

    let dt = 1.0 / 60.0;
    let mut scoring_frames = 0;
    for i in 0..(90.0 / dt) as usize {
        let secs = i as f32 * dt;
        let b = if secs < 22.0 {
            Buttons { cross: true, ..NONE }
        } else {
            Buttons {
                cross: true,
                circle: true,
                left: true,
                ..NONE
            }
        };
        g.update(&track, b, dt);
        let sp = hypot(g.vehicle.state.vx, g.vehicle.state.vy);
        if g.vehicle.slip_angle > DRIFT_SLIP_THRESHOLD && sp > 9.0 {
            scoring_frames += 1;
        }
    }
    assert_eq!(
        scoring_frames, 0,
        "pinned lock plus handbrake unexpectedly scored — the drift thresholds may have moved"
    );
}

#[test]
fn a_long_slide_fills_the_skid_pool_without_overrunning_it() {
    let (g, run) = drive_a_drifting_descent(120.0);
    assert_eq!(
        run.max_skids,
        angle_zero::effects::MAX_SKIDS,
        "a two-minute drifting run should saturate the mark pool"
    );
    // And it is still a fixed pool afterwards, not something that grew.
    assert_eq!(g.effects.skids().len(), angle_zero::effects::MAX_SKIDS);
}

#[test]
fn drifting_puffs_smoke() {
    let (_, run) = drive_a_drifting_descent(120.0);
    assert!(run.saw_smoke, "no tyre smoke was emitted during the slides");
}

#[test]
fn starting_a_new_run_clears_the_previous_runs_marks() {
    let (mut g, _) = drive_a_drifting_descent(60.0);
    let mut track = Box::new(Track::EMPTY);
    Track::generate(&mut track);
    assert!(g.effects.live_skids() > 0);
    g.start_run(&track);
    assert_eq!(g.effects.live_skids(), 0);
    assert_eq!(g.effects.live_puffs(), 0);
}
