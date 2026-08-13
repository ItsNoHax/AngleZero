//! Vehicle lighting, from the side that is not the renderer.
//!
//! Every one of these is a statement about what the car *does* — the brake is on, so the tail lamps
//! are hard on; the car is going backwards, so the reverse lamps are lit — and none of them needs a
//! PSP to answer. That is the point of keeping the state out of `psp/render.rs`: a lamp that comes
//! on at the wrong moment is a bug a screenshot cannot settle and a test can.

use angle_zero::azcar::{LightDef, LightKind, LIGHT_STEERS};
use angle_zero::lights::{
    beam, dim, fade, intensity, lamp, push_beam, seen_from, Pose, Signals, BEAM_FAR, BEAM_VERTS,
    LAMP_FAR, TAIL_IDLE,
};
use angle_zero::mesh::Vertex;
use angle_zero::vehicle::CarState;

/// A lamp of a kind, at a place, with everything else at a workable default.
fn light(kind: LightKind, at: [f32; 3]) -> LightDef {
    LightDef {
        kind,
        flags: 0,
        name: 0,
        at,
        color: 0xFF00_00FF, // opaque, and red in the PSP's ABGR
        radius: 0.3,
        range: 0.0,
        spread: 0.0,
    }
}

fn headlight(at: [f32; 3]) -> LightDef {
    LightDef {
        range: 24.0,
        spread: 2.6,
        color: 0x80D2_F3FF,
        ..light(LightKind::Head, at)
    }
}

fn parked() -> CarState {
    CarState::default()
}

/// A car standing on level ground, facing +Z. Pitch and roll are the renderer's, so a test that is
/// not about the car's attitude says so by passing zero for both.
fn level() -> Pose {
    Pose::of(&parked(), 0.0, 0.0)
}

fn poised(st: &CarState) -> Pose {
    Pose::of(st, 0.0, 0.0)
}

const LIT: Signals = Signals {
    braking: false,
    reversing: false,
    lit: true,
};

fn alpha(color: u32) -> u32 {
    color >> 24
}

// --- what the driver's actions do to the lamps ----------------------------------------

#[test]
fn not_braking_leaves_the_tail_lamps_burning_low() {
    let i = intensity(LightKind::Tail, &LIT);
    assert_eq!(i, TAIL_IDLE);
    assert!(i > 0.0, "a tail lamp is lit whenever the car is");
    assert!(i < 0.5, "and is not to be confusable with a brake light");
}

#[test]
fn braking_takes_the_tail_lamps_to_full() {
    let braking = Signals {
        braking: true,
        ..LIT
    };
    assert_eq!(intensity(LightKind::Tail, &braking), 1.0);
    // The gap is what makes it read as braking rather than as a lamp being switched on.
    assert!(
        intensity(LightKind::Tail, &braking) > intensity(LightKind::Tail, &LIT) * 2.0,
        "braking must be substantially brighter than idling, not merely brighter"
    );
}

/// A separate brake lens, on a car that has one, is dark until it is asked for.
#[test]
fn a_dedicated_brake_lens_is_dark_until_the_brake_is_applied() {
    assert_eq!(intensity(LightKind::Brake, &LIT), 0.0);
    assert_eq!(
        intensity(
            LightKind::Brake,
            &Signals {
                braking: true,
                ..LIT
            }
        ),
        1.0
    );
}

#[test]
fn reverse_lamps_burn_only_while_the_car_is_going_backwards() {
    let mut st = parked();
    st.vx = 4.0;
    assert!(!Signals::of(&st, false, true).reversing);

    // Stopped is not reversing: the car creeps against a rail by a few centimetres a second, and
    // lamps that flicker at a standstill are worse than none.
    st.vx = -0.05;
    assert!(!Signals::of(&st, false, true).reversing);

    st.vx = -3.0;
    let sig = Signals::of(&st, false, true);
    assert!(sig.reversing);
    assert_eq!(intensity(LightKind::Reverse, &sig), 1.0);
    // And nothing else changes because the car happens to be going backwards.
    assert_eq!(intensity(LightKind::Tail, &sig), TAIL_IDLE);
}

/// Braking is read off whatever the game says braking is, so the handbrake lights the lamps too.
#[test]
fn any_braking_the_game_reports_lights_the_lamps() {
    let st = parked();
    for braking in [true, false] {
        let sig = Signals::of(&st, braking, true);
        assert_eq!(sig.braking, braking);
        assert_eq!(intensity(LightKind::Tail, &sig) == 1.0, braking);
    }
}

#[test]
fn lights_off_parks_every_lamp_on_the_car() {
    let dark = Signals {
        braking: true,
        reversing: true,
        lit: false,
    };
    for kind in [
        LightKind::Head,
        LightKind::Tail,
        LightKind::Brake,
        LightKind::Reverse,
    ] {
        assert_eq!(intensity(kind, &dark), 0.0, "{}", kind.name());
    }
    assert!(lamp(&light(LightKind::Head, [0.6, 0.7, 2.0]), &level(), &dark, 5.0).is_none());
}

// --- where the lamps end up -----------------------------------------------------------

#[test]
fn a_lamp_is_carried_by_the_car_that_owns_it() {
    let def = light(LightKind::Tail, [0.64, 1.0, -2.0]);
    let mut st = parked();
    st.x = 30.0;
    st.z = -12.0;
    st.y = 4.0;

    let l = lamp(&def, &poised(&st), &LIT, 5.0).expect("a lit tail lamp");
    assert!((l.at[0] - 30.64).abs() < 1e-4, "{:?}", l.at);
    assert!((l.at[1] - 5.0).abs() < 1e-4, "the lamp rides at the car's height plus its own");
    // The car's z, the lens's own z, and the standoff that keeps the glow out of the bodywork.
    let standoff = def.radius * angle_zero::lights::GLOW_PROUD;
    assert!((l.at[2] - (-12.0 - 2.0 - standoff)).abs() < 1e-4, "{:?}", l.at);
    assert!(!l.forward, "a lamp behind the origin faces backwards");
}

/// Turn the car and the lamps go round with it — the lamp that was on the left is now ahead.
#[test]
fn turning_the_car_swings_its_lamps_around_it() {
    let def = light(LightKind::Head, [1.0, 0.7, 0.0]);
    let mut st = parked();
    st.yaw = core::f32::consts::FRAC_PI_2;

    let l = lamp(&def, &poised(&st), &LIT, 5.0).unwrap();
    // Yawed a quarter turn, a lamp a metre out on the car's left is a metre along the world's z.
    // The only x it keeps is the standoff that holds the glow off its own lens.
    let standoff = def.radius * angle_zero::lights::GLOW_PROUD;
    assert!(l.at[2].abs() > 0.9, "the lamp did not follow the car's heading: {:?}", l.at);
    assert!(l.at[0].abs() <= standoff + 1e-4, "{:?}", l.at);
}

/// The fault that made the brake lights sit too low and the headlights too high on every car, all
/// the way down the hill: lamps were placed by the car's heading alone, while the car itself is
/// drawn pitched onto the slope. The two ends of a car move in opposite directions when it pitches,
/// which is why it read as two separate problems.
#[test]
fn a_pitched_car_carries_its_lamps_with_its_body() {
    let head = headlight([0.6, 0.7, 1.9]);
    let tail = light(LightKind::Tail, [0.6, 0.9, -2.0]);
    // Nose down, as the car is for the whole descent: about 4 degrees on this pass.
    let downhill = Pose {
        pitch: 0.074,
        ..level()
    };

    let flat_head = lamp(&head, &level(), &LIT, 5.0).unwrap();
    let flat_tail = lamp(&tail, &level(), &LIT, 5.0).unwrap();
    let down_head = lamp(&head, &downhill, &LIT, 5.0).unwrap();
    let down_tail = lamp(&tail, &downhill, &LIT, 5.0).unwrap();

    assert!(
        down_head.at[1] < flat_head.at[1] - 0.1,
        "a nose-down car's headlight has to drop with its nose: {} against {}",
        down_head.at[1],
        flat_head.at[1]
    );
    assert!(
        down_tail.at[1] > flat_tail.at[1] + 0.1,
        "and its tail lamp has to rise with its tail: {} against {}",
        down_tail.at[1],
        flat_tail.at[1]
    );
}

/// Roll does the same thing across the car rather than along it.
#[test]
fn a_rolling_car_carries_its_lamps_with_its_body() {
    let left = light(LightKind::Tail, [0.64, 0.9, -2.0]);
    let right = light(LightKind::Tail, [-0.64, 0.9, -2.0]);
    let leaning = Pose {
        roll: 0.09,
        ..level()
    };

    let l = lamp(&left, &leaning, &LIT, 5.0).unwrap();
    let r = lamp(&right, &leaning, &LIT, 5.0).unwrap();
    assert!(
        l.at[1] > r.at[1] + 0.05,
        "one side of a rolling car is higher than the other: {} against {}",
        l.at[1],
        r.at[1]
    );
    // And the pair still straddles the car rather than sliding off one side of it.
    assert!(l.at[0] > 0.0 && r.at[0] < 0.0, "{:?} {:?}", l.at, r.at);
}

#[test]
fn a_headlight_is_seen_from_in_front_and_a_tail_lamp_from_behind() {
    let head = lamp(&headlight([0.6, 0.7, 2.0]), &level(), &LIT, 5.0).unwrap();
    let tail = lamp(&light(LightKind::Tail, [0.6, 1.0, -2.0]), &level(), &LIT, 5.0).unwrap();
    assert!(head.forward);
    assert!(!tail.forward);
}

/// A lamp fades with the angle it is seen at rather than switching off at the wing mirror. The
/// chase camera spends its life at the one angle where this matters: the tail lamps have to be
/// full and the headlamps must not shine through the car.
#[test]
fn a_lamp_fades_with_the_angle_it_is_seen_from() {
    // Dead astern: tail lamps whole, headlamps gone.
    assert_eq!(seen_from(false, -1.0), 1.0);
    assert_eq!(seen_from(true, -1.0), 0.0);
    // Dead ahead, the other way round.
    assert_eq!(seen_from(true, 1.0), 1.0);
    assert_eq!(seen_from(false, 1.0), 0.0);

    // Alongside, both ends of the car show something, and neither shows everything.
    for forward in [true, false] {
        let side_on = seen_from(forward, 0.0);
        assert!(side_on > 0.0 && side_on < 1.0, "{side_on}");
    }

    // And it is monotone as the camera comes round the nose — no step anywhere in it.
    let mut last = 0.0;
    for i in 0..=20 {
        let facing = -1.0 + i as f32 * 0.1;
        let seen = seen_from(true, facing);
        assert!(seen >= last - 1e-6, "a headlamp dimmed as the eye came round to it: {facing}");
        last = seen;
    }
}

#[test]
fn braking_blooms_the_lens_as_well_as_brightening_it() {
    let def = light(LightKind::Tail, [0.6, 1.0, -2.0]);
    let idle = lamp(&def, &level(), &LIT, 5.0).unwrap();
    let hard = lamp(
        &def,
        &level(),
        &Signals {
            braking: true,
            ..LIT
        },
        5.0,
    )
    .unwrap();

    assert!(alpha(hard.color) > alpha(idle.color) * 2, "brighter");
    assert!(hard.radius > idle.radius, "and visibly larger");
}

// --- the beam -------------------------------------------------------------------------

#[test]
fn only_a_lamp_with_a_range_throws_a_beam() {
    assert!(beam(&headlight([0.6, 0.7, 2.0]), &level(), &LIT, 5.0).is_some());
    assert!(beam(&light(LightKind::Tail, [0.6, 1.0, -2.0]), &level(), &LIT, 5.0).is_none());
}

#[test]
fn a_beam_widens_with_distance_and_lies_on_the_road() {
    let b = beam(&headlight([0.6, 0.7, 2.0]), &level(), &LIT, 5.0).unwrap();
    assert!(b.far_half > b.near_half * 2.0, "narrow at the lamp, wide where it lands");
    assert!(b.far > b.near);
    assert!(
        b.at[1] > 0.0 && b.at[1] < 0.2,
        "the beam lies on the tarmac, not at the height of the lens: {}",
        b.at[1]
    );
}

/// A fixed headlight points where the nose points, however the front wheels are turned; a lamp
/// configured to steer follows them. Both are per-car, and neither is in the renderer.
#[test]
fn steering_swings_only_the_lamps_configured_to_follow_it() {
    let mut st = parked();
    st.steer = 0.4;

    let fixed = beam(&headlight([0.6, 0.7, 2.0]), &poised(&st), &LIT, 5.0).unwrap();
    assert_eq!(fixed.yaw, st.yaw, "a fixed lamp is bolted to the body");

    let mut def = headlight([0.6, 0.7, 2.0]);
    def.flags |= LIGHT_STEERS;
    let steered = beam(&def, &poised(&st), &LIT, 5.0).unwrap();
    assert!(
        (steered.yaw - (st.yaw + st.steer)).abs() < 1e-6,
        "a steering lamp follows the road wheels"
    );
}

/// Flat ground, for the tests that are not about the road's shape.
fn flat(_x: f32, _z: f32) -> f32 {
    0.0
}

#[test]
fn a_beam_is_the_handful_of_triangles_it_was_budgeted() {
    let b = beam(&headlight([0.6, 0.7, 2.0]), &level(), &LIT, 5.0).unwrap();
    // Twice the room it should need, so that what this measures is how much a beam costs rather
    // than how much it was given — which is the mistake this test made when it was first written.
    let mut out = [Vertex::ZERO; BEAM_VERTS * 2];
    let mut w = 0;
    let written = push_beam(&mut out, &mut w, &b, flat);

    assert_eq!(written, BEAM_VERTS, "a beam costs what the constant says it costs");
    assert_eq!(written % 3, 0);
    assert!(written / 3 <= 20, "the plan's ceiling is 20 triangles a beam");

    // The light fades to nothing at the far end and at the edges rather than stopping at a line.
    let drawn = &out[..written];
    assert!(
        drawn.iter().any(|v| v.color >> 24 == 0),
        "a beam with no transparent edge is a visible polygon"
    );
    assert!(drawn.iter().any(|v| v.color >> 24 > 0x40), "and it is lit somewhere");
}

/// The fault that made the beam stop two metres in front of the bumper: the road falls away, and a
/// patch that stays at one height is under the tarmac as soon as the road climbs relative to it.
#[test]
fn a_beam_takes_its_height_from_the_road_it_lands_on() {
    let b = beam(&headlight([0.6, 0.7, 2.0]), &level(), &LIT, 5.0).unwrap();
    let mut out = [Vertex::ZERO; BEAM_VERTS];
    let mut w = 0;

    // A road climbing at 7% away from the car, which is this track's gradient.
    push_beam(&mut out, &mut w, &b, |_x, z| z * 0.07);
    let near = out[..6].iter().map(|v| v.y).fold(f32::MAX, f32::min);
    let far = out[BEAM_VERTS - 6..].iter().map(|v| v.y).fold(f32::MIN, f32::max);
    assert!(
        far > near + 1.0,
        "the far end did not climb with the road: {near} to {far}"
    );

    // On the flat it is flat, and it clears the tarmac rather than lying in it.
    let mut out = [Vertex::ZERO; BEAM_VERTS];
    let mut w = 0;
    push_beam(&mut out, &mut w, &b, flat);
    assert!(out.iter().all(|v| (v.y - angle_zero::lights::BEAM_LIFT).abs() < 1e-6));
}

/// Writing into a buffer that has run out must stop, not run off the end. The renderer's scratch
/// arena is finite and a field of cars is what fills it.
#[test]
fn a_beam_written_into_a_full_buffer_stops_short() {
    let b = beam(&headlight([0.6, 0.7, 2.0]), &level(), &LIT, 5.0).unwrap();
    let mut out = [Vertex::ZERO; 8];
    let mut w = 0;
    assert_eq!(push_beam(&mut out, &mut w, &b, flat), 8);
    assert_eq!(w, 8);
}

/// A glow centred exactly on its lens is half buried in the bodywork, and the depth test throws
/// that half away — two tail lamps came to forty-nine pixels between them.
#[test]
fn a_lamp_glow_stands_off_the_lens_it_comes_from() {
    let tail = light(LightKind::Tail, [0.64, 1.0, -2.0]);
    let head = headlight([0.62, 0.7, 2.0]);
    let l = lamp(&tail, &level(), &LIT, 5.0).unwrap();
    let h = lamp(&head, &level(), &LIT, 5.0).unwrap();

    assert!(l.at[2] < tail.at[2], "a tail lamp's glow is behind its lens");
    assert!(h.at[2] > head.at[2], "a headlamp's glow is in front of its lens");
    // Along the car's axis only: a lamp does not wander sideways off its own lens.
    assert!((l.at[0] - tail.at[0]).abs() < 1e-6);
}

// --- distance -------------------------------------------------------------------------

#[test]
fn lamps_fade_into_the_same_fog_the_world_does() {
    assert_eq!(fade(0.0), 1.0);
    assert_eq!(fade(20.0), 1.0);
    assert!(fade(200.0) < 1.0 && fade(200.0) > 0.0);
    assert_eq!(fade(LAMP_FAR), 0.0);
    assert_eq!(fade(LAMP_FAR * 2.0), 0.0);
}

/// The expensive effect is the one that stops first. A car far enough away keeps the lamps that
/// cost six triangles and loses the beam that costs eighteen.
#[test]
fn a_distant_car_keeps_its_lamps_and_loses_its_beams() {
    let def = headlight([0.6, 0.7, 2.0]);
    let far = BEAM_FAR + 10.0;
    assert!(beam(&def, &level(), &LIT, far).is_none());
    assert!(lamp(&def, &level(), &LIT, far).is_some());
    // And past the fog, nothing at all.
    assert!(lamp(&def, &level(), &LIT, LAMP_FAR + 1.0).is_none());
}

#[test]
fn dimming_a_colour_touches_its_brightness_and_not_its_hue() {
    let red = 0xFF00_00FFu32;
    assert_eq!(dim(red, 1.0), red);
    assert_eq!(dim(red, 0.0) & 0x00ff_ffff, red & 0x00ff_ffff);
    assert_eq!(alpha(dim(red, 0.0)), 0);
    assert_eq!(alpha(dim(red, 0.5)), 0x7F);
    // Out of range is clamped rather than wrapped: an alpha that overflows into the blue channel
    // would turn a brake light into whatever colour 0x1xx happens to be.
    assert_eq!(alpha(dim(red, 4.0)), 0xFF);
}
