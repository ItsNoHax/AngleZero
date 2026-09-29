//! The engine model behind the sound and the rev counter: a gearbox over the car's speed.

use angle_zero::engine::{Engine, GEAR_RPM_PER_MS, IDLE_RPM, REDLINE_RPM};
use angle_zero::hud::Gear;

const DT: f32 = 1.0 / 60.0;

/// Runs the engine for `seconds` at a fixed speed and input.
fn hold(e: &mut Engine, vx: f32, throttle: f32, brake: bool, slip: f32, seconds: f32) {
    for _ in 0..(seconds / DT) as usize {
        e.update(vx, throttle, brake, slip, DT);
    }
}

/// Accelerates steadily from `from` to `to` m/s at full throttle, returning the rpm each frame.
fn pull(e: &mut Engine, from: f32, to: f32, seconds: f32) -> Vec<f32> {
    let n = (seconds / DT) as usize;
    (0..n)
        .map(|i| {
            e.update(from + (to - from) * i as f32 / n as f32, 1.0, false, 0.0, DT);
            e.rpm
        })
        .collect()
}

#[test]
fn it_ticks_over_at_idle_when_parked() {
    let mut e = Engine::new();
    hold(&mut e, 0.0, 0.0, false, 0.0, 3.0);
    assert!((e.rpm - IDLE_RPM).abs() < 20.0, "idle at {}", e.rpm);
    assert_eq!(e.gear(), Gear::Forward(1));
    assert!(e.load < 0.01);
}

#[test]
fn it_revs_freely_at_a_standstill() {
    let mut e = Engine::new();
    hold(&mut e, 0.0, 1.0, false, 0.0, 1.5);
    assert!(e.rpm > 5000.0, "only reached {}", e.rpm);
    assert!(e.rpm <= REDLINE_RPM + 200.0);
}

#[test]
fn it_climbs_through_the_gears_under_full_throttle() {
    let mut e = Engine::new();
    let revs = pull(&mut e, 0.0, 44.0, 14.0);
    // Third runs out at about 44 m/s, so it has not quite needed fourth.
    assert_eq!(e.gear(), Gear::Forward(3), "ended in {:?}", e.gear());
    // Every upshift is a drop in the revs of well over a thousand: that is the whole point.
    let drops = revs.windows(12).filter(|w| w[0] - w[11] > 1200.0).count();
    assert!(drops >= 2, "only {drops} frames showed a shift drop");
    assert!(revs.iter().all(|&r| r <= REDLINE_RPM + 300.0), "over-revved");
}

#[test]
fn the_revs_follow_road_speed_in_gear() {
    let mut e = Engine::new();
    // Into second, then hold 25 m/s on a light throttle.
    pull(&mut e, 0.0, 25.0, 6.0);
    hold(&mut e, 25.0, 0.2, false, 0.0, 2.0);
    assert_eq!(e.gear(), Gear::Forward(2));
    let want = 25.0 * GEAR_RPM_PER_MS[1];
    assert!((e.rpm - want).abs() < 100.0, "{} rpm, expected {want}", e.rpm);
}

#[test]
fn it_changes_down_under_braking() {
    let mut e = Engine::new();
    pull(&mut e, 0.0, 40.0, 12.0);
    let top = e.gear();
    // Braking from 40 down to 15 m/s.
    for i in 0..120 {
        e.update(40.0 - 25.0 * i as f32 / 120.0, 0.0, true, 0.0, DT);
    }
    assert!(matches!((top, e.gear()), (Gear::Forward(a), Gear::Forward(b)) if b < a), "{top:?} -> {:?}", e.gear());
    assert!(e.rpm > 3000.0, "downshifts should keep the revs up, got {}", e.rpm);
}

#[test]
fn the_throttle_is_cut_during_a_shift() {
    let mut e = Engine::new();
    let mut cut = false;
    let n = (8.0 / DT) as usize;
    for i in 0..n {
        e.update(25.0 * i as f32 / n as f32, 1.0, false, 0.0, DT);
        if e.shifting() && e.load < 0.5 {
            cut = true;
        }
    }
    assert!(cut, "load never dropped across an upshift");
}

#[test]
fn a_drift_on_the_throttle_spins_the_revs_up() {
    let mut grip = Engine::new();
    let mut slide = Engine::new();
    pull(&mut grip, 0.0, 18.0, 5.0);
    pull(&mut slide, 0.0, 18.0, 5.0);
    hold(&mut grip, 18.0, 1.0, false, 0.0, 1.0);
    hold(&mut slide, 18.0, 1.0, false, 0.5, 1.0);
    assert!(slide.rpm > grip.rpm * 1.1, "{} vs {}", slide.rpm, grip.rpm);
}

#[test]
fn the_limiter_cuts_in_bursts() {
    // The gearbox changes up before the limiter on grip, so it takes a slide near the top of
    // first, with the rear wheels spinning the revs past road speed.
    let mut e = Engine::new();
    let mut cut = 0;
    for _ in 0..240 {
        e.update(16.0, 1.0, false, 0.6, DT);
        if e.rpm > REDLINE_RPM && e.load < 0.5 {
            cut += 1;
        }
    }
    assert!(cut > 0, "the limiter never cut the fuel");
    assert!(e.rpm < REDLINE_RPM + 400.0, "ran away to {}", e.rpm);
}

#[test]
fn boost_builds_under_load_and_dumps_on_a_lift() {
    let mut e = Engine::new();
    pull(&mut e, 0.0, 25.0, 6.0);
    hold(&mut e, 25.0, 1.0, false, 0.0, 2.0);
    let built = e.boost;
    assert!(built > 0.5, "boost only reached {built}");
    hold(&mut e, 25.0, 0.0, false, 0.0, 0.5);
    assert!(e.boost < built * 0.2, "boost held at {} after a lift", e.boost);
}

#[test]
fn rolling_backwards_is_reverse() {
    let mut e = Engine::new();
    hold(&mut e, -2.0, 0.0, false, 0.0, 0.2);
    assert_eq!(e.gear(), Gear::Reverse);
    hold(&mut e, -0.3, 0.0, false, 0.0, 0.2);
    assert_eq!(e.gear(), Gear::Forward(1), "just rolling back is not reverse yet");
}

#[test]
fn the_rev_counter_stays_in_range() {
    let mut e = Engine::new();
    for i in 0..1200 {
        e.update(i as f32 * 0.05, 1.0, false, (i % 7) as f32 * 0.1, DT);
        let r = e.rev_fraction();
        assert!((0.0..=1.0).contains(&r), "needle at {r}");
    }
}
