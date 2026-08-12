//! Vehicle simulation, plus the behavioural items from the acceptance
//! checklist that are testable without a renderer.

use angle_zero::math::{atan2, hypot, wrap_pi};
use angle_zero::track::{Locator, Track, BAY_FROM, BAY_TO, NODE_COUNT, RAIL_LIMIT};
use angle_zero::vehicle::{CarHandling, CarShape, Input, Vehicle, FIXED_DT};

fn track() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

fn at_start(t: &Track) -> Vehicle {
    let mut v = Vehicle::new();
    v.place_at_node(t, 2);
    v
}

/// Steers toward a point ~24 m down the centreline, so speed tests are not really rail tests.
fn autopilot(t: &Track, v: &Vehicle) -> f32 {
    let ahead = (v.locator.last_idx + 18).min(NODE_COUNT - 1);
    let p = t.nodes[ahead].p;
    let want = atan2(p.x - v.state.x, p.z - v.state.z);
    (wrap_pi(want - v.state.yaw) * 2.5).clamp(-1.0, 1.0)
}

fn run(t: &Track, v: &mut Vehicle, seconds: f32, mut input: impl FnMut(&Vehicle) -> Input) {
    let steps = (seconds / FIXED_DT) as usize;
    for _ in 0..steps {
        let i = input(v);
        v.step(t, i, FIXED_DT);
    }
}

fn speed(v: &Vehicle) -> f32 {
    hypot(v.state.vx, v.state.vy)
}

// ---------------------------------------------------------------- steering

#[test]
fn steering_lock_shrinks_as_speed_rises() {
    // 0.60 rad at rest, falling to 0.60 * 0.45 by 55 m/s and no further. The lock itself is the
    // car's now, so this asks the default one — which is the car the game was tuned around.
    let car = CarHandling::DEFAULT;
    assert!((car.steer_max(0.0) - 0.60).abs() < 1e-4);
    assert!((car.steer_max(55.0) - 0.60 * 0.45).abs() < 1e-4);
    assert!((car.steer_max(200.0) - 0.60 * 0.45).abs() < 1e-4);
    assert!(car.steer_max(20.0) < car.steer_max(5.0));
}

/// A car that says nothing about how it drives must drive exactly like the game always has.
///
/// This is the guarantee that made it safe to turn five constants into data: the descent is
/// balanced around these numbers, and the default is not a plausible set of values, it is the set
/// the game was tuned with.
#[test]
fn the_default_car_is_the_one_the_game_was_tuned_with() {
    let d = CarHandling::DEFAULT;
    assert_eq!(d.mass, 1420.0);
    assert_eq!(d.inertia, 1950.0);
    assert_eq!(d.front_axle, 1.18);
    assert_eq!(d.rear_axle, 1.42);
    assert_eq!(d.engine, 8200.0);
    assert_eq!(d.top_speed, 58.0);
    assert_eq!(d.brake, 11000.0);
    assert_eq!(d.steer_lock, 0.60);
    assert_eq!(d.grip, 1.0);
    assert_eq!(Vehicle::new().handling, d);
    assert!(d.is_sane());
}

#[test]
fn a_car_with_numbers_that_would_break_the_simulation_is_not_sane() {
    for bad in [
        CarHandling {
            mass: 0.0,
            ..CarHandling::DEFAULT
        },
        CarHandling {
            inertia: 0.0,
            ..CarHandling::DEFAULT
        },
        CarHandling {
            rear_axle: -1.42,
            ..CarHandling::DEFAULT
        },
        CarHandling {
            top_speed: 0.0,
            ..CarHandling::DEFAULT
        },
        CarHandling {
            steer_lock: 0.0,
            ..CarHandling::DEFAULT
        },
        CarHandling {
            grip: 0.0,
            ..CarHandling::DEFAULT
        },
    ] {
        assert!(!bad.is_sane(), "{bad:?} should have been refused");
    }
}

/// The point of the feature: a lighter car with more grip corners harder.
#[test]
fn a_lighter_grippier_car_holds_a_tighter_line() {
    let track = track();
    let corner = |handling| {
        let mut v = Vehicle::new();
        v.handling = handling;
        v.place_at_node(&track, 200);
        v.state.vx = 22.0;
        for _ in 0..240 {
            v.step(
                &track,
                Input {
                    throttle: 0.5,
                    steer_in: 1.0,
                    ..Default::default()
                },
                FIXED_DT,
            );
        }
        v.state.yaw_rate.abs()
    };

    let heavy = corner(CarHandling::DEFAULT);
    let light = corner(CarHandling {
        mass: 950.0,
        inertia: 1200.0,
        grip: 1.15,
        ..CarHandling::DEFAULT
    });
    assert!(
        light > heavy,
        "the lighter car turned at {light} rad/s against {heavy}"
    );
}

#[test]
fn steering_winds_on_slower_than_it_unwinds() {
    let t = track();

    // Winding on toward full lock happens at 6 rad/s.
    let mut v = at_start(&t);
    let held = Input {
        steer_in: 1.0,
        ..Input::default()
    };
    v.step(&t, held, FIXED_DT);
    let wind_on = v.state.steer;
    assert!((wind_on - 6.0 * FIXED_DT).abs() < 1e-5, "wound on {wind_on}");

    // Returning to centre happens at the faster 9 rad/s.
    let mut v = at_start(&t);
    run(&t, &mut v, 0.2, |_| held);
    let before = v.state.steer;
    v.step(&t, Input::default(), FIXED_DT);
    let unwound = before - v.state.steer;
    assert!(
        (unwound - 9.0 * FIXED_DT).abs() < 1e-5,
        "unwound {unwound} from {before}"
    );
}

#[test]
fn positive_steer_input_turns_the_car_left() {
    let t = track();
    let mut v = at_start(&t);
    v.state.vx = 15.0;
    let yaw0 = v.state.yaw;
    run(&t, &mut v, 0.6, |_| Input {
        steer_in: 1.0,
        throttle: 1.0,
        ..Input::default()
    });
    // Left is increasing yaw in this frame, and lateral offset moves off the right-hand normal.
    assert!(
        wrap_pi(v.state.yaw - yaw0) > 0.05,
        "yaw changed by {}",
        wrap_pi(v.state.yaw - yaw0)
    );
    assert!(v.query.lat < 0.0, "lat was {}", v.query.lat);
}

// ---------------------------------------------------------------- longitudinal

#[test]
fn gravity_alone_accelerates_the_car_downhill() {
    let t = track();
    let mut v = Vehicle::new();
    // Node 300 is on the descent proper. Nodes near the start sit on the flat lead-in, where
    // the pitch is ~0 and gravity genuinely has nothing to pull against.
    v.place_at_node(&t, 300);
    assert_eq!(speed(&v), 0.0);

    // Around here the road drops 0.64 m per 12 m step, so gravity contributes ~0.44 m/s² and
    // rolling resistance takes back ~0.15. Speed builds steadily rather than dramatically.
    let coast = |v: &Vehicle| {
        let steer_in = autopilot(&t, v);
        Input {
            steer_in,
            ..Input::default()
        }
    };
    run(&t, &mut v, 10.0, coast);
    let at_10s = speed(&v);
    run(&t, &mut v, 10.0, coast);
    let at_20s = speed(&v);

    // No throttle at any point — the slope did all of this.
    assert!(at_10s > 2.0, "after 10s of coasting: {at_10s} m/s");
    assert!(
        at_20s > at_10s + 1.0,
        "coasting is not still building speed: {at_10s} -> {at_20s} m/s"
    );
}

#[test]
fn the_lead_in_really_is_flat_so_the_start_needs_throttle() {
    let t = track();
    let mut v = at_start(&t);
    run(&t, &mut v, 5.0, |_| Input::default());
    assert!(
        speed(&v) < 1.0,
        "the flat lead-in should not roll away on its own, but reached {} m/s",
        speed(&v)
    );
}

/// The design says ~200 km/h, which the force constants cannot produce — engine force floors
/// long before aero drag stops growing, giving a terminal velocity of ~168 km/h even on the
/// steepest sustained slope here. What matters is that a full-throttle descent is quick.
#[test]
fn full_throttle_gets_the_car_up_to_a_proper_pace() {
    let t = track();
    let mut v = at_start(&t);

    let mut peak: f32 = 0.0;
    let steps = (70.0 / FIXED_DT) as usize;
    for _ in 0..steps {
        let steer_in = autopilot(&t, &v);
        v.step(
            &t,
            Input {
                throttle: 1.0,
                steer_in,
                ..Input::default()
            },
            FIXED_DT,
        );
        peak = peak.max(v.state.vx * 3.6);
    }

    assert!(
        (110.0..=175.0).contains(&peak),
        "peak speed was {peak} km/h"
    );
}

#[test]
fn braking_stops_a_moving_car() {
    let t = track();
    let mut v = at_start(&t);
    v.state.vx = 25.0;
    run(&t, &mut v, 3.0, |_| Input {
        brake: true,
        ..Input::default()
    });
    assert!(v.state.vx < 5.0, "still doing {} m/s", v.state.vx);
}

// ---------------------------------------------------------------- drifting

#[test]
fn the_handbrake_breaks_the_rear_away() {
    let t = track();

    // Same entry speed and steering, with and without the handbrake.
    let mut grip = at_start(&t);
    grip.state.vx = 28.0;
    run(&t, &mut grip, 1.0, |_| Input {
        throttle: 1.0,
        steer_in: 1.0,
        ..Input::default()
    });

    let mut slide = at_start(&t);
    slide.state.vx = 28.0;
    run(&t, &mut slide, 1.0, |_| Input {
        throttle: 1.0,
        steer_in: 1.0,
        handbrake: true,
        ..Input::default()
    });

    assert!(
        slide.slip_angle > 0.16,
        "handbrake turn only reached {} rad of slip",
        slide.slip_angle
    );
    assert!(
        slide.slip_angle > grip.slip_angle * 1.5,
        "handbrake slip {} was not meaningfully more than gripped slip {}",
        slide.slip_angle,
        grip.slip_angle
    );
}

#[test]
fn lifting_off_mid_slide_recovers_grip() {
    let t = track();
    let mut v = at_start(&t);
    v.state.vx = 28.0;
    run(&t, &mut v, 1.0, |_| Input {
        throttle: 1.0,
        steer_in: 1.0,
        handbrake: true,
        ..Input::default()
    });
    let sliding = v.slip_angle;
    assert!(sliding > 0.16);

    // Straighten up and come off everything.
    run(&t, &mut v, 1.5, |_| Input::default());
    assert!(
        v.slip_angle < sliding * 0.5,
        "slip only fell from {sliding} to {}",
        v.slip_angle
    );
}

// ---------------------------------------------------------------- containment

#[test]
fn the_car_cannot_be_pushed_through_the_guard_rails() {
    let t = track();
    let mut v = at_start(&t);
    // Aim hard at the rail with plenty of speed, well clear of the bay gap.
    v.place_at_node(&t, 600);
    v.state.vx = 30.0;

    for _ in 0..(6.0 / FIXED_DT) as usize {
        v.step(
            &t,
            Input {
                throttle: 1.0,
                steer_in: 1.0,
                ..Input::default()
            },
            FIXED_DT,
        );
        assert!(
            v.query.lat.abs() <= RAIL_LIMIT + 0.01,
            "car escaped to lat {}",
            v.query.lat
        );
    }
}

#[test]
fn the_emergency_bay_is_the_one_place_the_car_can_leave_the_road() {
    let t = track();
    let mut v = at_start(&t);
    v.place_at_node(&t, (BAY_FROM + BAY_TO) / 2);

    // Positive lat is the bay side, and inside the bay the limit opens up to 16.5 m.
    let mid = (BAY_FROM + BAY_TO) / 2;
    assert!(v.containment_limit(mid, 10.0) > 16.0);
    // The other side of the same nodes is still railed.
    assert!((v.containment_limit(mid, -10.0) - RAIL_LIMIT).abs() < 1e-4);
    // And so is the road either side of the bay.
    assert!((v.containment_limit(BAY_TO + 5, 10.0) - RAIL_LIMIT).abs() < 1e-4);
    assert!((v.containment_limit(BAY_FROM - 5, 10.0) - RAIL_LIMIT).abs() < 1e-4);
}

#[test]
fn the_car_cannot_drive_back_past_the_start_line() {
    let t = track();
    let mut v = at_start(&t);
    // Face back up the hill and try to reverse out.
    v.state.yaw += core::f32::consts::PI;
    run(&t, &mut v, 6.0, |_| Input {
        throttle: 1.0,
        ..Input::default()
    });

    // It is held at the line: never more than a metre behind node 0.
    let n0 = t.nodes[0];
    let behind = (v.state.x - n0.p.x) * n0.dir.x + (v.state.z - n0.p.z) * n0.dir.z;
    assert!(behind > -1.5, "car got {behind} m behind the start line");
}

#[test]
fn sustained_rail_contact_respawns_the_car() {
    let t = track();
    let mut v = at_start(&t);
    v.place_at_node(&t, 600);
    v.state.vx = 30.0;

    let mut respawned = false;
    for _ in 0..(8.0 / FIXED_DT) as usize {
        let out = v.step(
            &t,
            Input {
                throttle: 1.0,
                steer_in: 1.0,
                ..Input::default()
            },
            FIXED_DT,
        );
        if out.respawned {
            respawned = true;
            break;
        }
    }
    assert!(respawned, "grinding the rail never triggered a respawn");
    // Respawn puts it back on the centreline, pointing down the hill, rolling gently.
    assert!(v.query.lat.abs() < 0.5, "respawned at lat {}", v.query.lat);
    assert!((v.state.vx - 3.0).abs() < 0.01);
    assert_eq!(v.state.vy, 0.0);
    assert_eq!(v.state.yaw_rate, 0.0);
}

#[test]
fn driving_the_wrong_way_respawns_after_three_and_a_half_seconds() {
    let t = track();
    let mut v = at_start(&t);
    v.place_at_node(&t, 900);
    // Turn around completely and drive back up the hill.
    v.state.yaw += core::f32::consts::PI;
    v.state.vx = 12.0;

    let mut elapsed = 0.0;
    let mut respawned = false;
    for _ in 0..(6.0 / FIXED_DT) as usize {
        let out = v.step(
            &t,
            Input {
                throttle: 1.0,
                ..Input::default()
            },
            FIXED_DT,
        );
        elapsed += FIXED_DT;
        if out.respawned {
            respawned = true;
            break;
        }
    }
    assert!(respawned, "wrong-way driving never respawned");
    assert!(
        (3.0..4.2).contains(&elapsed),
        "respawned after {elapsed}s, expected ~3.5"
    );
}

#[test]
fn a_wall_tap_scrubs_speed_and_then_goes_quiet_for_half_a_second() {
    let t = track();
    let mut v = at_start(&t);
    v.place_at_node(&t, 600);
    v.state.vx = 30.0;

    // Point the car across the road so it reaches the rail quickly. 0.5 rad is nowhere near the
    // 2.0 rad that would count as driving the wrong way.
    v.state.yaw += 0.5;

    let mut first: Option<f32> = None;
    let mut second: Option<f32> = None;
    let mut speed_before = 0.0;
    let mut speed_after = 0.0;
    let mut elapsed = 0.0;

    for _ in 0..(3.0 / FIXED_DT) as usize {
        let before = v.state.vx;
        let out = v.step(&t, Input::default(), FIXED_DT);
        elapsed += FIXED_DT;
        if out.wall_tap {
            if first.is_none() {
                first = Some(elapsed);
                speed_before = before;
                speed_after = v.state.vx;
            } else if second.is_none() {
                second = Some(elapsed);
                break;
            }
        }
        if out.respawned {
            break;
        }
    }

    let first = first.expect("driving into the rail never registered a wall tap");
    assert!(
        speed_after < speed_before * 0.9,
        "a wall tap should scrub speed, but vx went {speed_before} -> {speed_after}"
    );
    // Continuous contact must not re-fire every substep — the cooldown gates it to 0.5 s.
    if let Some(second) = second {
        assert!(
            second - first >= 0.49,
            "two wall taps only {}s apart, expected the 0.5s cooldown",
            second - first
        );
    }
}

// ---------------------------------------------------------------- ride

#[test]
fn ride_height_follows_the_road_surface() {
    let t = track();
    let mut v = at_start(&t);
    // Start the car well above the road and let it settle.
    v.state.y += 8.0;
    run(&t, &mut v, 2.0, |_| Input::default());
    let road_y = t.nodes[v.locator.last_idx].p.y;
    assert!(
        (v.state.y - road_y).abs() < 0.2,
        "car sat at y {} with the road at {road_y}",
        v.state.y
    );
}

#[test]
fn a_full_descent_completes_without_getting_stuck() {
    let t = track();
    let mut v = at_start(&t);

    let mut finished = false;
    for _ in 0..(240.0 / FIXED_DT) as usize {
        let steer_in = autopilot(&t, &v);
        v.step(
            &t,
            Input {
                throttle: 1.0,
                steer_in,
                ..Input::default()
            },
            FIXED_DT,
        );
        if t.progress(v.locator.last_idx) > 0.985 {
            finished = true;
            break;
        }
    }
    assert!(
        finished,
        "autopilot only reached {:.1}% of the descent",
        t.progress(v.locator.last_idx) * 100.0
    );
}

#[test]
fn a_fresh_locator_starts_where_the_car_was_placed() {
    let t = track();
    let v = at_start(&t);
    let mut loc = Locator::new();
    loc.reset_to(2);
    assert_eq!(v.locator.last_idx, 2);
    assert!((v.state.y - t.nodes[2].p.y).abs() < 1e-4);
}

/// The wheels turn at the rate the loaded car's tyres are the size they are.
///
/// Purely visual, and visibly wrong when it is wrong: a car whose wheels turn at the wrong rate
/// looks like it is skating rather than driving, which is a thing the eye picks up long before it
/// can say why. The number used to be a constant matching a car built out of boxes; it now comes
/// off whichever model is loaded, so this checks that swapping the car swaps the rate.
#[test]
fn wheel_spin_follows_the_loaded_cars_rolling_radius() {
    let t = track();

    let mut small = at_start(&t);
    small.shape = CarShape {
        wheel_radius: 0.25,
        ..CarShape::DEFAULT
    };
    let mut large = at_start(&t);
    large.shape = CarShape {
        wheel_radius: 0.50,
        ..CarShape::DEFAULT
    };

    let small_turn = spun(&t, &mut small, 1.5);
    let large_turn = spun(&t, &mut large, 1.5);

    // Same car, same physics, same distance — the spin is the only thing that differs, and a wheel
    // half the size turns twice as far for it.
    assert!(
        (small.state.x - large.state.x).abs() < 1e-3,
        "the wheel radius must not reach the physics"
    );
    let ratio = small_turn / large_turn;
    assert!(
        (ratio - 2.0).abs() < 0.05,
        "a 0.25 m wheel turned {ratio:.2} times as far as a 0.50 m one, not twice"
    );
}

/// How far the wheels turned in total, undoing the wrap the simulation applies to keep the angle
/// small. A step's worth of rotation is a fraction of a turn, so the difference recovers exactly.
fn spun(t: &Track, v: &mut Vehicle, seconds: f32) -> f32 {
    let mut total = 0.0;
    let steps = (seconds / FIXED_DT) as usize;
    for _ in 0..steps {
        let before = v.state.wheel_spin;
        let i = Input {
            throttle: 1.0,
            steer_in: autopilot(t, v),
            ..Input::default()
        };
        v.step(t, i, FIXED_DT);
        total += wrap_pi(v.state.wheel_spin - before);
    }
    total
}
