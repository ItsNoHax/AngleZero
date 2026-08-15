//! Screen flow, PSP button mapping and the fixed-timestep loop.

use angle_zero::game::{Buttons, Game, PauseChoice, Phase, Toast};
use angle_zero::math::{cos, sin};
use angle_zero::track::{Track, BAY_NODE, NODE_COUNT};
use angle_zero::vehicle::FIXED_DT;

fn track() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

fn game(t: &Track) -> Box<Game> {
    let mut g = Box::new(Game::new());
    g.enter_title(t);
    g
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

const CROSS: Buttons = Buttons {
    cross: true,
    ..NONE
};

const SQUARE: Buttons = Buttons {
    square: true,
    ..NONE
};

const START: Buttons = Buttons {
    start: true,
    ..NONE
};

const UP: Buttons = Buttons { up: true, ..NONE };

const DOWN: Buttons = Buttons {
    down: true,
    ..NONE
};

fn hold(g: &mut Game, t: &Track, b: Buttons, seconds: f32) {
    let frames = (seconds / (1.0 / 60.0)) as usize;
    for _ in 0..frames {
        g.update(t, b, 1.0 / 60.0);
    }
}

// ------------------------------------------------------------------ screens

#[test]
fn the_game_opens_on_the_title_screen_with_the_car_parked_in_the_pull_off() {
    let t = track();
    let g = game(&t);
    assert_eq!(g.phase, Phase::Title);

    // Parked off to the bay side of the centreline, near the bay node.
    let bay = &t.nodes[BAY_NODE];
    let dx = g.vehicle.state.x - bay.p.x;
    let dz = g.vehicle.state.z - bay.p.z;
    let lat = dx * bay.nrm.x + dz * bay.nrm.z;
    assert!(
        lat > 8.0,
        "car should be parked out in the pull-off, but lat was {lat}"
    );
    assert_eq!(g.vehicle.state.vx, 0.0);
}

#[test]
fn the_title_camera_orbits_while_nothing_is_pressed() {
    let t = track();
    let mut g = game(&t);
    let start = g.camera.orbit_angle;
    hold(&mut g, &t, NONE, 2.0);
    assert!(g.camera.orbit_angle > start + 0.2);
    assert_eq!(g.phase, Phase::Title);
}

#[test]
fn cross_starts_the_run_and_says_go() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    assert_eq!(g.phase, Phase::Run);
    assert_eq!(g.toast, Some(Toast::Go));
    // The run starts on the road at the top, not in the pull-off.
    assert!(g.vehicle.locator.last_idx <= 4);
    assert_eq!(g.scoring.score, 0.0);
    assert_eq!(g.run_time, 0.0);
}

#[test]
fn a_held_button_does_not_re_trigger_a_screen_change() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    assert_eq!(g.phase, Phase::Run);

    // Keep holding X — it is now the throttle, and must not restart the run every frame.
    hold(&mut g, &t, CROSS, 2.0);
    assert_eq!(g.phase, Phase::Run);
    assert!(g.run_time > 1.9, "run clock did not advance: {}", g.run_time);
    assert!(g.vehicle.state.vx > 1.0, "X should be throttle during a run");
}

#[test]
fn reaching_the_bottom_ends_the_run() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    // Drop the car near the finish rather than driving the whole 3.5 km.
    g.vehicle.place_at_node(&t, NODE_COUNT - 30);
    g.vehicle.state.vx = 20.0;

    hold(&mut g, &t, CROSS, 6.0);
    assert_eq!(g.phase, Phase::Results, "run never finished");
    assert!(g.result.time > 0.0);
    assert!(g.result.best_combo >= 1);
}

#[test]
fn the_run_clock_stops_once_the_results_are_up() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    g.vehicle.place_at_node(&t, NODE_COUNT - 30);
    g.vehicle.state.vx = 20.0;
    hold(&mut g, &t, CROSS, 6.0);
    assert_eq!(g.phase, Phase::Results);

    let frozen = g.result.time;
    hold(&mut g, &t, NONE, 2.0);
    assert_eq!(g.result.time, frozen);
}

/// Drives to the finish and leaves the game on the results screen with nothing held, so the next
/// press is a clean edge.
fn finish_a_run(g: &mut Game, t: &Track) {
    g.update(t, CROSS, 1.0 / 60.0);
    g.vehicle.place_at_node(t, NODE_COUNT - 30);
    g.vehicle.state.vx = 20.0;
    hold(g, t, CROSS, 6.0);
    assert_eq!(g.phase, Phase::Results, "run never finished");
    g.update(t, NONE, 1.0 / 60.0);
}

#[test]
fn cross_restarts_from_the_results_screen() {
    let t = track();
    let mut g = game(&t);
    finish_a_run(&mut g, &t);

    g.update(&t, CROSS, 1.0 / 60.0);
    assert_eq!(g.phase, Phase::Run);
    assert_eq!(g.scoring.score, 0.0);
    assert!(g.vehicle.locator.last_idx <= 4, "restart should go back to the top");
}

#[test]
fn square_sends_the_results_screen_back_to_the_car_selection() {
    let t = track();
    let mut g = game(&t);
    finish_a_run(&mut g, &t);

    g.update(&t, SQUARE, 1.0 / 60.0);
    assert_eq!(g.phase, Phase::Title);
    // The title screen is where a car gets picked, so the car has to be parked in the pull-off
    // again rather than left wherever the run ended.
    let bay = &t.nodes[BAY_NODE];
    let dx = g.vehicle.state.x - bay.p.x;
    let dz = g.vehicle.state.z - bay.p.z;
    assert!((dx * bay.nrm.x + dz * bay.nrm.z) > 8.0);
}

#[test]
fn triangle_during_a_run_looks_at_the_front_of_the_car_and_lets_go_again() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    hold(&mut g, &t, CROSS, 3.0);

    // How far ahead of the car's nose the camera is standing: positive is in front of it, which is
    // the whole point of the view and is what a yaw comparison would only imply.
    let along_nose = |g: &Game| {
        let (car, cam) = (&g.vehicle.state, &g.camera);
        (cam.pos.x - car.x) * sin(car.yaw) + (cam.pos.z - car.z) * cos(car.yaw)
    };
    assert!(along_nose(&g) < -3.0, "the chase should start out behind");

    let ahead = Buttons {
        triangle: true,
        ..CROSS
    };
    hold(&mut g, &t, ahead, 2.0);

    // The run carries on — this is a camera, not a reset.
    assert_eq!(g.phase, Phase::Run);
    assert!(g.camera.front_view);
    assert!(
        along_nose(&g) > 3.0,
        "camera should be out in front of the car, was {}",
        along_nose(&g)
    );

    // And letting go swings it back behind.
    hold(&mut g, &t, CROSS, 2.0);
    assert!(!g.camera.front_view);
    assert!(
        along_nose(&g) < -3.0,
        "camera did not return behind the car, was {}",
        along_nose(&g)
    );
}

// ------------------------------------------------------------------ pause menu

#[test]
fn start_pauses_the_run_and_nothing_moves_while_it_is_up() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    hold(&mut g, &t, CROSS, 3.0);

    g.update(&t, START, 1.0 / 60.0);
    assert_eq!(g.phase, Phase::Paused);
    assert_eq!(g.pause_choice, PauseChoice::Continue);

    let (x, z, clock) = (g.vehicle.state.x, g.vehicle.state.z, g.run_time);
    // Two seconds parked on a 1-in-8 descent: rolling away is what an unpaused car would do.
    hold(&mut g, &t, NONE, 2.0);
    assert_eq!(g.phase, Phase::Paused);
    assert_eq!(g.run_time, clock, "the clock ran while paused");
    assert_eq!((g.vehicle.state.x, g.vehicle.state.z), (x, z));
}

#[test]
fn continue_resumes_the_same_run_where_it_stopped() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    hold(&mut g, &t, CROSS, 3.0);
    let (clock, node) = (g.run_time, g.vehicle.locator.last_idx);

    g.update(&t, START, 1.0 / 60.0);
    g.update(&t, NONE, 1.0 / 60.0);
    g.update(&t, CROSS, 1.0 / 60.0); // Continue is what it opens on

    assert_eq!(g.phase, Phase::Run);
    assert_eq!(g.vehicle.locator.last_idx, node);
    assert!(g.run_time >= clock, "resuming rewound the clock");
    assert!(g.run_time < clock + 0.1, "resuming cost the run a chunk of time");
}

#[test]
fn restart_puts_the_car_back_at_the_top_with_the_score_cleared() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    hold(&mut g, &t, CROSS, 3.0);
    g.scoring.score = 5000.0;

    g.update(&t, START, 1.0 / 60.0);
    g.update(&t, DOWN, 1.0 / 60.0);
    assert_eq!(g.pause_choice, PauseChoice::Restart);
    g.update(&t, CROSS, 1.0 / 60.0);

    assert_eq!(g.phase, Phase::Run);
    assert_eq!(g.scoring.score, 0.0);
    assert!(g.vehicle.locator.last_idx <= 4);
    assert_eq!(g.run_time, 0.0);
}

#[test]
fn select_another_car_goes_back_to_the_title_screen() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    hold(&mut g, &t, CROSS, 3.0);

    g.update(&t, START, 1.0 / 60.0);
    g.update(&t, DOWN, 1.0 / 60.0);
    g.update(&t, NONE, 1.0 / 60.0);
    g.update(&t, DOWN, 1.0 / 60.0);
    assert_eq!(g.pause_choice, PauseChoice::ChangeCar);
    g.update(&t, CROSS, 1.0 / 60.0);

    assert_eq!(g.phase, Phase::Title);
    assert_eq!(g.vehicle.state.vx, 0.0);
}

#[test]
fn the_menu_wraps_both_ways_and_start_closes_it_again() {
    // Three entries, so wrapping is what makes the last one reachable in one press.
    assert_eq!(PauseChoice::Continue.step(-1), PauseChoice::ChangeCar);
    assert_eq!(PauseChoice::ChangeCar.step(1), PauseChoice::Continue);

    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    hold(&mut g, &t, CROSS, 3.0);

    g.update(&t, START, 1.0 / 60.0);
    g.update(&t, UP, 1.0 / 60.0);
    assert_eq!(g.pause_choice, PauseChoice::ChangeCar);

    // START is its own way out, whatever is highlighted, and does not act on the entry.
    g.update(&t, NONE, 1.0 / 60.0);
    g.update(&t, START, 1.0 / 60.0);
    assert_eq!(g.phase, Phase::Run);
}

#[test]
fn a_held_start_does_not_flicker_between_paused_and_running() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    hold(&mut g, &t, CROSS, 3.0);

    hold(&mut g, &t, START, 1.0);
    assert_eq!(g.phase, Phase::Paused);
}

// ------------------------------------------------------------------ controls

#[test]
fn the_face_buttons_map_the_way_the_design_says() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0); // into the run

    let i = Game::drive_input(&CROSS);
    assert_eq!(i.throttle, 1.0);
    assert!(!i.brake && !i.handbrake);

    let i = Game::drive_input(&Buttons {
        circle: true,
        ..NONE
    });
    assert!(i.handbrake);

    // Brake is Square *or* Down.
    assert!(Game::drive_input(&Buttons { square: true, ..NONE }).brake);
    assert!(Game::drive_input(&Buttons { down: true, ..NONE }).brake);

    // Up is a throttle alias, matching the original's Up-arrow.
    assert_eq!(Game::drive_input(&Buttons { up: true, ..NONE }).throttle, 1.0);
}

#[test]
fn the_d_pad_steers_left_positive() {
    assert_eq!(Game::drive_input(&Buttons { left: true, ..NONE }).steer_in, 1.0);
    assert_eq!(
        Game::drive_input(&Buttons {
            right: true,
            ..NONE
        })
        .steer_in,
        -1.0
    );
    // Both at once cancels out.
    assert_eq!(
        Game::drive_input(&Buttons {
            left: true,
            right: true,
            ..NONE
        })
        .steer_in,
        0.0
    );
}

#[test]
fn the_analog_nub_steers_proportionally_when_the_d_pad_is_idle() {
    let i = Game::drive_input(&Buttons {
        analog_x: 0.5,
        ..NONE
    });
    assert!((i.steer_in - 0.5).abs() < 1e-6);

    // The d-pad wins when both are used, so a resting nub cannot dilute a deliberate press.
    let i = Game::drive_input(&Buttons {
        analog_x: 0.2,
        left: true,
        ..NONE
    });
    assert_eq!(i.steer_in, 1.0);
}

// ------------------------------------------------------------------ timing

#[test]
fn the_simulation_is_frame_rate_independent() {
    let t = track();

    let mut sixty = game(&t);
    sixty.update(&t, CROSS, 1.0 / 60.0);
    for _ in 0..120 {
        sixty.update(&t, CROSS, 1.0 / 60.0);
    }

    let mut thirty = game(&t);
    thirty.update(&t, CROSS, 1.0 / 60.0);
    for _ in 0..60 {
        thirty.update(&t, CROSS, 1.0 / 30.0);
    }

    // Both advanced the same 2 s of simulation, so they must agree exactly.
    assert!(
        (sixty.vehicle.state.x - thirty.vehicle.state.x).abs() < 1e-4,
        "{} vs {}",
        sixty.vehicle.state.x,
        thirty.vehicle.state.x
    );
    assert!((sixty.vehicle.state.vx - thirty.vehicle.state.vx).abs() < 1e-4);
    assert!((sixty.run_time - thirty.run_time).abs() < 1e-4);
}

#[test]
fn a_huge_frame_gap_cannot_blow_up_the_simulation() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);

    // A five-second stall: clamped to 0.25 s, and capped at 40 substeps regardless.
    g.update(&t, CROSS, 5.0);
    assert!(g.vehicle.state.x.is_finite());
    assert!(g.run_time <= 0.35, "run clock jumped to {}", g.run_time);
}

// ------------------------------------------------------------------ toasts

#[test]
fn toasts_expire_on_their_own() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    assert_eq!(g.toast, Some(Toast::Go));
    hold(&mut g, &t, CROSS, 2.0);
    assert_eq!(g.toast, None);
}

#[test]
fn the_toast_fades_over_its_last_moments() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    assert!((g.toast_opacity() - 1.0).abs() < 1e-6);
    hold(&mut g, &t, CROSS, 0.95);
    let fading = g.toast_opacity();
    assert!(
        fading > 0.0 && fading < 1.0,
        "expected a partial fade, got {fading}"
    );
}

#[test]
fn hitting_a_rail_during_a_combo_warns_and_drops_the_multiplier() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);

    // Build a combo by hand, then drive into the rail.
    g.vehicle.place_at_node(&t, 600);
    g.vehicle.state.vx = 30.0;
    g.vehicle.state.yaw += 0.5;
    g.scoring.combo = 5;
    g.scoring.combo_timer = 1.0;

    for _ in 0..(3.0 / FIXED_DT) as usize {
        g.update(&t, NONE, FIXED_DT);
        if g.toast == Some(Toast::WallTap) {
            break;
        }
    }
    assert_eq!(g.toast, Some(Toast::WallTap));
    assert_eq!(g.scoring.combo, 1);
}

#[test]
fn the_descent_percentage_tracks_progress_down_the_hill() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);
    assert_eq!(g.descent_percent(&t), 0);

    g.vehicle.place_at_node(&t, NODE_COUNT / 2);
    g.update(&t, NONE, 1.0 / 60.0);
    assert!((49..=51).contains(&g.descent_percent(&t)));
}

#[test]
fn hitting_a_rail_bumps_the_impact_count_once_per_hit() {
    let t = track();
    let mut g = game(&t);
    g.update(&t, CROSS, 1.0 / 60.0);

    // Aim across the road so the rail arrives quickly.
    g.vehicle.place_at_node(&t, 600);
    g.vehicle.state.vx = 30.0;
    g.vehicle.state.yaw += 0.5;
    let before = g.impact_count();

    let mut saw_first = None;
    for i in 0..(3.0 / FIXED_DT) as usize {
        g.update(&t, NONE, FIXED_DT);
        if saw_first.is_none() && g.impact_count() != before {
            saw_first = Some(i);
        }
    }
    let after = g.impact_count();
    assert!(saw_first.is_some(), "driving into the rail never registered an impact");
    assert!(after > before, "impact count did not advance");
    // The 0.5 s hit cooldown gates it, so a few seconds of contact is a handful of thuds and
    // not one per substep.
    assert!(
        after - before <= 6,
        "{} impacts in three seconds — the thud would machine-gun",
        after - before
    );
}
