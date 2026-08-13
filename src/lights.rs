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
/// the only thing that says the driver has lifted.
///
/// Both ends of this were measured on screen rather than chosen. At 0.30 a tail lamp added about
/// seventy to the red channel over bodywork that is already dark red, which came out as no glow at
/// all — a car that only had lights when it was braking. What keeps the two apart at 0.42 is that
/// braking also blooms the lens: see [`BRAKE_BLOOM`], which is the half of the difference that
/// survives being looked at from a hundred metres.
pub const TAIL_IDLE: f32 = 0.42;
/// How much wider a lamp's glow gets at full brightness than at rest. A brake light does not only
/// get brighter, it visibly blooms, and on a 480-pixel screen the size change carries further than
/// the brightness change does.
pub const BRAKE_BLOOM: f32 = 1.55;
/// How far a lamp's glow stands off its lens, as a fraction of the glow's own radius.
pub const GLOW_PROUD: f32 = 0.8;
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
    /// Where the light starts: on the road under the lens rather than at the lens itself, since the
    /// patch lies on the tarmac. Its height is the road under the *car*, which is the one station's
    /// height that needs no looking up — see [`push_beam`].
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
    let radius = def.radius * bloom;
    // The glow stands off the lens rather than on it, and this is not decoration.
    //
    // A glow is a camera-facing disc, and a disc centred exactly on a lens is half buried in the
    // bodywork the lens is set into — the depth test then throws away everything behind the boot
    // lid, which on a car seen from behind is most of it. Two tail lamps came to forty-nine pixels
    // between them. Standing the glow proud of the glass is also what a real lamp looks like: the
    // light is in the air in front of the lens, not painted on it.
    let forward = def.at[2] >= 0.0;
    let out = radius * GLOW_PROUD * if forward { 1.0 } else { -1.0 };
    let (s, c) = (sin(st.yaw), cos(st.yaw));
    Some(Lamp {
        kind: def.kind,
        at: [x + out * s, y, z + out * c],
        color: dim(def.color, scale),
        radius,
        forward,
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
    let far_half = max(def.spread, max(def.radius, 0.2));
    // How wide the light already is by the time it reaches the road.
    //
    // Not the width of the lens, which is what this started as and which is wrong twice over. A
    // dipped beam has spread to most of a lane within a few metres — and, more to the point, a
    // patch narrower than the car sits in the one part of the screen the car itself is covering.
    // From a chase camera that is the whole of it: two beams a lens wide lit 709 pixels, all of
    // them in the sliver of road visible over the roof.
    let near_half = max(def.radius, far_half * 0.3);
    Some(Beam {
        at: [x, st.y + BEAM_LIFT, z],
        yaw,
        // The light lands a little ahead of the lens rather than under it. A patch that starts at
        // the bumper puts a bright pool beneath the car, where no headlight has ever put one.
        near: max(def.radius, 0.2) * 2.0,
        far: max(def.range, far_half * 2.0),
        near_half,
        far_half,
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

/// Where the beam is measured along its length, and how bright it is there.
///
/// Three stations rather than two, and the light does not simply ramp from full to nothing: a
/// headlight's hot spot is a few metres in front of the car rather than at the lens, which is where
/// the road is actually brightest and where the eye expects it. It fades to nothing by the far end,
/// so the patch has no edge across it.
const BEAM_STATIONS: [(f32, f32); 3] = [(0.0, 0.85), (0.30, 1.0), (1.0, 0.0)];

/// How much of the beam's width is at full brightness before it starts fading to its edge.
const BEAM_CORE: f32 = 0.45;

/// Vertices one beam costs: two lengthwise segments by three crosswise strips, six each.
///
/// Three strips across — a bright core and a fading one either side — is what gives the beam a soft
/// edge without a texture, the same trick the lamp glows use. Twelve triangles, against the plan's
/// ceiling of twenty.
pub const BEAM_VERTS: usize = (BEAM_STATIONS.len() - 1) * 3 * 6;

/// Writes one beam's triangles, in world space, lying on the road.
///
/// `ground` gives the height of the road at a world (x, z), and the beam takes its height from that
/// at every station past the first rather than lying flat. That is not a refinement: the pass falls seven
/// centimetres a metre, so a horizontal patch 24 m long is a metre out by its far end — floating in
/// the air where the road drops away, and buried under the tarmac where it climbs. Buried is what it
/// actually did, and a beam that reached two metres in front of the bumper and stopped dead was the
/// result. `push_bay_pool` lays the lay-by's light pool on its paving for exactly this reason.
///
/// Sampled once per station and not once per vertex: the height is a property of how far up the
/// road the light has reached, and asking the track eighteen times for a beam that has three
/// answers would make the cheap half of vehicle lighting the expensive one.
pub fn push_beam(
    out: &mut [Vertex],
    w: &mut usize,
    beam: &Beam,
    mut ground: impl FnMut(f32, f32) -> f32,
) -> usize {
    let (s, c) = (sin(beam.yaw), cos(beam.yaw));
    let start = *w;

    // Each station: how far along, how wide, how bright, and how high the road is there.
    let mut station = [(0.0f32, 0.0f32, 0.0f32, 0.0f32); BEAM_STATIONS.len()];
    for (i, (along, intensity)) in BEAM_STATIONS.iter().enumerate() {
        let d = beam.near + (beam.far - beam.near) * along;
        let (cx, cz) = (beam.at[0] + d * s, beam.at[2] + d * c);
        station[i] = (
            d,
            beam.near_half + (beam.far_half - beam.near_half) * along,
            *intensity,
            // The nearest station is a metre in front of the bumper, and the height of the road
            // there is one the caller already knows exactly: it is the car's own, which the
            // simulation settles onto the surface every substep. Asking the track for it would be
            // a third of this pass's cost spent re-deriving a number that arrived with the beam.
            if i == 0 {
                beam.at[1]
            } else {
                ground(cx, cz) + BEAM_LIFT
            },
        );
    }

    // A point on the beam: which station, and `across` from -1 to 1.
    let at = |i: usize, across: f32, alpha: f32| {
        let (d, half, _, y) = station[i];
        let off = across * half;
        Vertex::new(
            beam.at[0] + d * s + off * c,
            y,
            beam.at[2] + d * c - off * s,
            dim(beam.color, alpha),
        )
    };

    for i in 0..BEAM_STATIONS.len() - 1 {
        let (i0, i1) = (station[i].2, station[i + 1].2);
        for (x0, x1, e0, e1) in [
            (-1.0f32, -BEAM_CORE, 0.0f32, 1.0f32),
            (-BEAM_CORE, BEAM_CORE, 1.0, 1.0),
            (BEAM_CORE, 1.0, 1.0, 0.0),
        ] {
            let quad = [
                at(i, x0, i0 * e0),
                at(i, x1, i0 * e1),
                at(i + 1, x1, i1 * e1),
                at(i, x0, i0 * e0),
                at(i + 1, x1, i1 * e1),
                at(i + 1, x0, i1 * e0),
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
