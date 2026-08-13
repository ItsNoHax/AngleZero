//! Which lamps on a car are burning, how brightly, and where they are in the world.
//!
//! The expensive half of vehicle lighting is drawing it; this is the other half, and it is
//! deliberately on this side of the PSP boundary. What the tail lamps do when the driver brakes is
//! a rule about the game, not about the hardware — it is the kind of thing that is wrong in a way a
//! screenshot cannot settle, so it is answered here where a test can ask.
//!
//! Nothing here decides where a lamp is. That comes out of the car asset, one
//! [`LightDef`](crate::azcar::LightDef) per lens, exactly as the wheels do — so a car's lights are
//! a property of the file it was compiled from, and a car with none simply has none.
//!
//! The flow the renderer sees is:
//!
//! ```text
//! vehicle state + braking  ->  Signals  ->  Lamp / Beam (world space)  ->  additive triangles
//! ```
//!
//! Both outputs are in world space rather than car space, and that is a decision about cost: it
//! means every lamp on every car on screen goes into one buffer behind one matrix, so a field of
//! twenty cars is the same two draw calls as one car.

use crate::azcar::{LightDef, LightKind};
use crate::math::{cos, max, min, sin};
use crate::mesh::Vertex;
use crate::vehicle::CarState;

/// How much of its full brightness a tail lamp burns at when the driver is not braking.
///
/// The gap between this and 1.0 is the whole point of a tail lamp: at night, on a car ahead, it is
/// the only thing that says the driver has lifted. A third is about where the two stop being
/// confusable at the distance the chase camera puts a car at.
pub const TAIL_IDLE: f32 = 0.30;
/// How much wider a lamp's glow gets at full brightness than at rest. A brake light does not only
/// get brighter, it visibly blooms, and on a 480-pixel screen the size change carries further than
/// the brightness change does.
pub const BRAKE_BLOOM: f32 = 1.55;
/// Forward speed below which the car counts as reversing, m/s.
///
/// Not zero. The car idles against a rail with a few centimetres a second of numerical creep in it,
/// and reverse lamps that flicker on at a standstill are worse than none.
pub const REVERSE_SPEED: f32 = -0.5;

/// Beyond this many metres a car's lamps are not drawn at all.
///
/// The fog closes at 330 m and takes everything with it, lamps included, so this is where they stop
/// being visible rather than a budget somebody chose. It matches `render::FOG_FAR`.
pub const LAMP_FAR: f32 = 330.0;
/// Beyond this, a car keeps its lamps but loses its beams.
pub const BEAM_FAR: f32 = 70.0;
/// How high above the car's origin a beam lies, metres.
///
/// The road is crowned — `ROAD_STATIONS` puts its centre 3 cm above the centreline the car's `y`
/// follows, and the edge lines sit at 5 cm — so a beam laid any lower than this is under the paint
/// and fighting the tarmac for the depth buffer.
pub const BEAM_LIFT: f32 = 0.08;

/// What the car is doing, as its lamps see it.
///
/// Read off the vehicle rather than off the controller: the point is that the lamps work whatever
/// causes the car to slow, so this takes the state the simulation arrived at, not the button that
/// was pressed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Signals {
    /// The brake or the handbrake is applied.
    pub braking: bool,
    /// The car is actually going backwards, not merely stopped.
    pub reversing: bool,
    /// The car's lights are switched on at all. False parks every lamp on the car.
    pub lit: bool,
}

impl Signals {
    /// What the lamps should be doing, given the car and whether the driver is on the brakes.
    ///
    /// `braking` is passed in rather than derived because the simulation does not keep it: the
    /// brake is an input to a substep, and by the time a frame is drawn what is left of it is the
    /// deceleration it caused. The game layer already keeps the flag for the rev counter's sake.
    pub fn of(st: &CarState, braking: bool, lit: bool) -> Signals {
        Signals {
            braking,
            reversing: st.vx < REVERSE_SPEED,
            lit,
        }
    }
}

/// How much of its full brightness a lamp of this kind is burning at, 0.0 to 1.0.
///
/// This is the whole of the lighting logic, and every acceptance test in the plan is a statement
/// about this function.
pub fn intensity(kind: LightKind, signals: &Signals) -> f32 {
    if !signals.lit {
        return 0.0;
    }
    match kind {
        LightKind::Head => 1.0,
        // One lens doing two jobs, which is what a real car has: lit whenever the car is, and hard
        // on under braking. Not on-off — the step from a quarter to full is what reads as a brake
        // light rather than as a lamp being switched.
        LightKind::Tail => {
            if signals.braking {
                1.0
            } else {
                TAIL_IDLE
            }
        }
        // A separate high-level lens, dark until it is asked for.
        LightKind::Brake => {
            if signals.braking {
                1.0
            } else {
                0.0
            }
        }
        LightKind::Reverse => {
            if signals.reversing {
                1.0
            } else {
                0.0
            }
        }
    }
}

/// How much of a lamp survives the distance between it and the eye.
///
/// The same curve the world's fog follows, for the same reason: a lamp is in the fog with
/// everything else, and one that stayed at full brightness while the car carrying it faded into the
/// murk would be the brightest thing on screen at the point where it should be the dimmest.
///
/// This is also the quality tier. There is no separate distance test — a lamp that has faded to
/// nothing costs no triangles because it is never resolved.
pub fn fade(metres: f32) -> f32 {
    if metres <= 0.0 {
        return 1.0;
    }
    // Full out to a third of the way, then linear to nothing. Lamps are small and bright, so they
    // survive further into the fog than the surfaces around them do.
    let start = LAMP_FAR * 0.35;
    if metres <= start {
        1.0
    } else {
        max(0.0, 1.0 - (metres - start) / (LAMP_FAR - start))
    }
}

/// A lamp resolved into the world: where its glow goes, how big, and how bright.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lamp {
    pub kind: LightKind,
    pub at: [f32; 3],
    /// Colour with the lamp's current brightness already in the alpha.
    pub color: u32,
    pub radius: f32,
    /// Whether the lens faces the front of the car. A glow is a billboard with no depth of its own,
    /// so the renderer shows each lamp only from the side it actually points at — otherwise the
    /// headlamps shine straight through the car at a chase camera.
    pub forward: bool,
}

/// The patch of road a headlight throws light on.
///
/// This is the beam. There is no second volume of lit air above it, and that is deliberate: a
/// translucent wedge standing in the air is exactly the "obvious transparent polygon" the effect is
/// trying not to be, and from a chase camera sitting behind and above the car, what a real
/// headlight actually shows you is the tarmac.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Beam {
    /// Where the light starts, on the road under the lens rather than at the lens itself. The
    /// patch lies on the tarmac, so this is the only height any of it has.
    pub at: [f32; 3],
    /// Heading the beam runs along, world radians.
    pub yaw: f32,
    /// Where the light starts and stops, metres ahead of the lens.
    pub near: f32,
    pub far: f32,
    /// Half-width at each end. The near one is the lens; the far one is where it has spread to.
    pub near_half: f32,
    pub far_half: f32,
    /// Colour, with brightness at the near end in the alpha. It fades to nothing by `far`.
    pub color: u32,
}

/// Puts a lamp where the car has carried it, or `None` if it is dark or too far away to matter.
///
/// `metres` is how far the car is from the eye.
pub fn lamp(def: &LightDef, st: &CarState, signals: &Signals, metres: f32) -> Option<Lamp> {
    let scale = intensity(def.kind, signals) * fade(metres);
    if scale <= 0.0 {
        return None;
    }
    let (x, y, z) = place(st, def.at);
    // Braking blooms the lens as well as brightening it.
    let bloom = if def.kind == LightKind::Tail && signals.braking {
        BRAKE_BLOOM
    } else {
        1.0
    };
    Some(Lamp {
        kind: def.kind,
        at: [x, y, z],
        color: dim(def.color, scale),
        radius: def.radius * bloom,
        forward: def.at[2] >= 0.0,
    })
}

/// The road patch a lamp throws, or `None` if it throws none.
///
/// Only a lamp with a range has one, which in practice means the headlights: a tail lamp lights the
/// road behind it in life, and at the brightness it does so it is not worth a triangle.
pub fn beam(def: &LightDef, st: &CarState, signals: &Signals, metres: f32) -> Option<Beam> {
    if def.range <= 0.0 || metres > BEAM_FAR {
        return None;
    }
    let scale = intensity(def.kind, signals) * fade(metres);
    if scale <= 0.0 {
        return None;
    }
    let (x, _, z) = place(st, def.at);
    // A lamp that steers swings with the road wheels; one that does not is a fixed unit behind a
    // fixed grille, and points where the car's nose points however the car is sliding.
    let yaw = st.yaw + if def.steers() { st.steer } else { 0.0 };
    let half = max(def.radius, 0.2);
    Some(Beam {
        at: [x, st.y + BEAM_LIFT, z],
        yaw,
        // The light lands a little ahead of the lens rather than under it. A patch that starts at
        // the bumper puts a bright pool beneath the car, where no headlight has ever put one.
        near: half * 2.0,
        far: max(def.range, half * 4.0),
        near_half: half,
        far_half: max(def.spread, half),
        color: dim(def.color, scale),
    })
}

/// A point in car space, put where the car has carried it.
fn place(st: &CarState, at: [f32; 3]) -> (f32, f32, f32) {
    let (s, c) = (sin(st.yaw), cos(st.yaw));
    (
        st.x + at[0] * c + at[2] * s,
        st.y + at[1],
        st.z - at[0] * s + at[2] * c,
    )
}

/// Scales a colour's alpha, leaving the hue alone.
///
/// Brightness is the alpha channel throughout, because every use of one of these ends up
/// multiplied into an additive vertex colour, where alpha is exactly what "how much of this light
/// arrives" means.
pub fn dim(color: u32, scale: f32) -> u32 {
    let alpha = (color >> 24) as f32 * min(1.0, max(0.0, scale));
    (color & 0x00ff_ffff) | ((alpha as u32) << 24)
}

/// Triangles one beam costs.
///
/// Three quads across: a bright core and a fading strip either side of it. That is what gives the
/// beam a soft edge without a texture — the same trick the lamp glows use, where the falloff is in
/// the vertex colours rather than in an image.
pub const BEAM_VERTS: usize = 18;

/// Writes one beam's triangles, in world space, on the ground.
///
/// Four lengthwise stations, not two, and the light does not simply ramp from full to nothing:
/// a beam is at its brightest a few metres in front of the car rather than at the lens, which is
/// where the road is actually lit and where the eye expects the hot spot to be.
pub fn push_beam(out: &mut [Vertex], w: &mut usize, beam: &Beam) -> usize {
    let (s, c) = (sin(beam.yaw), cos(beam.yaw));
    let start = *w;

    // A point on the beam: `along` from 0 at the lens to 1 at the far end, `across` from -1 to 1.
    let at = |along: f32, across: f32, alpha: f32| {
        let d = beam.near + (beam.far - beam.near) * along;
        let half = beam.near_half + (beam.far_half - beam.near_half) * along;
        let off = across * half;
        Vertex::new(
            beam.at[0] + d * s + off * c,
            beam.at[1],
            beam.at[2] + d * c - off * s,
            dim(beam.color, alpha),
        )
    };

    // Lengthwise: bright close in, fading out. Crosswise: full through the middle, nothing at the
    // edges, so neither end of the patch has a line on it.
    let core = 0.45f32;
    for (a0, a1, i0, i1) in [(0.0f32, 0.30f32, 0.85f32, 1.0f32), (0.30, 1.0, 1.0, 0.0)] {
        for (x0, x1, e0, e1) in [
            (-1.0f32, -core, 0.0f32, 1.0f32),
            (-core, core, 1.0, 1.0),
            (core, 1.0, 1.0, 0.0),
        ] {
            let quad = [
                at(a0, x0, i0 * e0),
                at(a0, x1, i0 * e1),
                at(a1, x1, i1 * e1),
                at(a0, x0, i0 * e0),
                at(a1, x1, i1 * e1),
                at(a1, x0, i1 * e0),
            ];
            for v in quad {
                if *w >= out.len() {
                    return *w - start;
                }
                out[*w] = v;
                *w += 1;
            }
        }
    }
    *w - start
}
