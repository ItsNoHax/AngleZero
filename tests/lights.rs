//! Vehicle lighting, from the side that is not the renderer.
//!
//! Every one of these is a statement about what the car *does* — the brake is on, so the tail lamps
//! are hard on; the car is going backwards, so the reverse lamps are lit — and none of them needs a
//! PSP to answer. That is the point of keeping the state out of `psp/render.rs`: a lamp that comes
//! on at the wrong moment is a bug a screenshot cannot settle and a test can.

use angle_zero::azcar::{LightDef, LightKind, LIGHT_STEERS};
use angle_zero::lights::{
    beam, dim, fade, intensity, lamp, push_beam, Signals, BEAM_FAR, BEAM_VERTS, LAMP_FAR, TAIL_IDLE,
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
    assert!(lamp(&light(LightKind::Head, [0.6, 0.7, 2.0]), &parked(), &dark, 5.0).is_none());
}

// --- where the lamps end up -----------------------------------------------------------

#[test]
fn a_lamp_is_carried_by_the_car_that_owns_it() {
    let def = light(LightKind::Tail, [0.64, 1.0, -2.0]);
    let mut st = parked();
    st.x = 30.0;
    st.z = -12.0;
    st.y = 4.0;

    let l = lamp(&def, &st, &LIT, 5.0).expect("a lit tail lamp");
    assert!((l.at[0] - 30.64).abs() < 1e-4, "{:?}", l.at);
    assert!((l.at[1] - 5.0).abs() < 1e-4, "the lamp rides at the car's height plus its own");
    assert!((l.at[2] + 14.0).abs() < 1e-4, "{:?}", l.at);
    assert!(!l.forward, "a lamp behind the origin faces backwards");
}

/// Turn the car and the lamps go round with it — the lamp that was on the left is now ahead.
#[test]
fn turning_the_car_swings_its_lamps_around_it() {
    let def = light(LightKind::Head, [1.0, 0.7, 0.0]);
    let mut st = parked();
    st.yaw = core::f32::consts::FRAC_PI_2;

    let l = lamp(&def, &st, &LIT, 5.0).unwrap();
    // Yawed a quarter turn, the car's +X points along world -Z... whichever way round it is, the
    // lamp must have left the axis it started on entirely.
    assert!(l.at[0].abs() < 1e-3, "the lamp did not follow the car's heading: {:?}", l.at);
    assert!(l.at[2].abs() > 0.9, "{:?}", l.at);
}

#[test]
fn a_headlight_is_seen_from_in_front_and_a_tail_lamp_from_behind() {
    let head = lamp(&headlight([0.6, 0.7, 2.0]), &parked(), &LIT, 5.0).unwrap();
    let tail = lamp(&light(LightKind::Tail, [0.6, 1.0, -2.0]), &parked(), &LIT, 5.0).unwrap();
    assert!(head.forward);
    assert!(!tail.forward);
}

#[test]
fn braking_blooms_the_lens_as_well_as_brightening_it() {
    let def = light(LightKind::Tail, [0.6, 1.0, -2.0]);
    let idle = lamp(&def, &parked(), &LIT, 5.0).unwrap();
    let hard = lamp(
        &def,
        &parked(),
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
    assert!(beam(&headlight([0.6, 0.7, 2.0]), &parked(), &LIT, 5.0).is_some());
    assert!(beam(&light(LightKind::Tail, [0.6, 1.0, -2.0]), &parked(), &LIT, 5.0).is_none());
}

#[test]
fn a_beam_widens_with_distance_and_lies_on_the_road() {
    let b = beam(&headlight([0.6, 0.7, 2.0]), &parked(), &LIT, 5.0).unwrap();
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

    let fixed = beam(&headlight([0.6, 0.7, 2.0]), &st, &LIT, 5.0).unwrap();
    assert_eq!(fixed.yaw, st.yaw, "a fixed lamp is bolted to the body");

    let mut def = headlight([0.6, 0.7, 2.0]);
    def.flags |= LIGHT_STEERS;
    let steered = beam(&def, &st, &LIT, 5.0).unwrap();
    assert!(
        (steered.yaw - (st.yaw + st.steer)).abs() < 1e-6,
        "a steering lamp follows the road wheels"
    );
}

#[test]
fn a_beam_is_the_handful_of_triangles_it_was_budgeted() {
    let b = beam(&headlight([0.6, 0.7, 2.0]), &parked(), &LIT, 5.0).unwrap();
    let mut out = [Vertex::ZERO; BEAM_VERTS];
    let mut w = 0;
    let written = push_beam(&mut out, &mut w, &b);

    assert_eq!(written, BEAM_VERTS);
    assert_eq!(written % 3, 0);
    assert!(written / 3 <= 20, "the plan's ceiling is 20 triangles a beam");

    // Every vertex is on the road, and the light fades to nothing at the far end and at the edges
    // rather than stopping at a line.
    assert!(out.iter().all(|v| (v.y - b.at[1]).abs() < 1e-6), "the patch is flat");
    assert!(
        out.iter().any(|v| v.color >> 24 == 0),
        "a beam with no transparent edge is a visible polygon"
    );
    assert!(out.iter().any(|v| v.color >> 24 > 0x40), "and it is lit somewhere");
}

/// Writing into a buffer that has run out must stop, not run off the end. The renderer's scratch
/// arena is finite and a field of cars is what fills it.
#[test]
fn a_beam_written_into_a_full_buffer_stops_short() {
    let b = beam(&headlight([0.6, 0.7, 2.0]), &parked(), &LIT, 5.0).unwrap();
    let mut out = [Vertex::ZERO; 8];
    let mut w = 0;
    assert_eq!(push_beam(&mut out, &mut w, &b), 8);
    assert_eq!(w, 8);
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
    assert!(beam(&def, &parked(), &LIT, far).is_none());
    assert!(lamp(&def, &parked(), &LIT, far).is_some());
    // And past the fog, nothing at all.
    assert!(lamp(&def, &parked(), &LIT, LAMP_FAR + 1.0).is_none());
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
