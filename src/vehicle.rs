//! Vehicle simulation.
//!
//! A single-track (bicycle) model with linear tyres and a friction ceiling, integrated at a fixed
//! 1/120 s so behaviour does not change with frame rate. Gravity along the road's pitch is what
//! actually drives the game: the car accelerates downhill with no throttle at all.
//!
//! The car runs in one of two modes. In grip mode, traction control keeps the rear tyres from
//! spinning and stability control stops the car rotating faster than its grip can explain, the way
//! a road car's aids do. A handbrake stab, or a slide past `DRIFT_ENTRY_SLIP`, pops it into drift
//! mode. There both aids are off, so the throttle spins the rear wheels and a spinning tyre has
//! less sideways grip: the throttle holds the angle, and lifting off lets the car grip up again.
//!
//! A drift is unstable by nature. Left alone, the car either spins or snaps back into line, and a
//! real driver holds it with constant small corrections. The drift assist makes those corrections
//! only at the edges of a corridor of angles, so inside it the angle is the player's.
//!
//! Signs: `+lat` is to the car's right (see `Vec2::lateral_normal`), `+steer` turns left,
//! `vx` is forward and `vy` lateral in the body frame.

use crate::math::{abs, atan2, clamp, cos, hypot, min, sin, signum, wrap_pi, wrap_tau};
use crate::track::{
    Locator, Query, Track, CARPARK_SIDE, RAIL_LIMIT, TARMAC_HALF_WIDTH,
};

/// Physics runs at this fixed step regardless of frame rate.
pub const FIXED_DT: f32 = 1.0 / 120.0;
/// Never take more than this many substeps for one frame, so a stall cannot spiral.
pub const MAX_SUBSTEPS: u32 = 40;
/// A frame longer than this is treated as a hitch and clamped.
pub const MAX_FRAME_DT: f32 = 0.25;

/// The measurements the simulation takes off whatever car is loaded.
///
/// None of them affect handling: the physics is a bicycle model with its own wheelbase and mass,
/// and it drives every car the same. What they decide is whether what is drawn agrees with it —
/// wheels that roll at the speed the car is going rather than scrubbing, tyre marks laid under the
/// tyres rather than near them, and bodywork that stops at a guard rail rather than inside one.
///
/// Kept as data on the vehicle rather than as constants because the car is a file now. The
/// defaults are a mid-size saloon, used when no car loaded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarShape {
    /// Rolling radius, turning forward speed into wheel rotation.
    pub wheel_radius: f32,
    /// Half the rear track: how far out from the centreline the back wheels sit.
    pub rear_hub_x: f32,
    /// How far behind the origin the rear axle is. Negative.
    pub rear_hub_z: f32,
    /// Half the width of the body at its widest, arches and mirrors included.
    pub half_width: f32,
    /// Half the length of the body, nose to tail.
    pub half_length: f32,
    /// Where the middle of that length sits relative to the origin.
    ///
    /// Rarely quite zero. A model's origin sits between its axles, and a car with a long boot and
    /// a short nose has more of itself behind that point than in front of it.
    pub centre_z: f32,
}

impl CarShape {
    pub const DEFAULT: CarShape = CarShape {
        wheel_radius: 0.32,
        rear_hub_x: 0.78,
        rear_hub_z: -1.30,
        half_width: 0.97,
        half_length: 2.24,
        centre_z: 0.0,
    };

    /// Takes the shape off a compiled car's rear hubs and bounding box.
    ///
    /// The hubs are averaged over both of them, and the track is taken as the mean of their
    /// distances from the centreline rather than from one side, because a scanned model is rarely
    /// exactly symmetric and marks that are 2 cm out on one side and 2 cm in on the other look
    /// like the car is crabbing.
    ///
    /// The footprint comes off the box rather than off the wheels, because what a rail has to stop
    /// is the bodywork: an arch or a mirror stands out past the tyre under it, and it is the part
    /// the player watches disappear into the barrier. `bounds` is min xyz then max xyz, as the
    /// asset stores it; the width is taken as the wider side rather than as half the span, because
    /// everything else here treats the model's origin as the car's centreline.
    pub fn measure(wheel_radius: f32, rear_hubs: &[[f32; 3]], bounds: [f32; 6]) -> CarShape {
        if rear_hubs.is_empty() || !(wheel_radius > 0.0) {
            return CarShape::DEFAULT;
        }
        let n = rear_hubs.len() as f32;
        let x = rear_hubs.iter().map(|h| abs(h[0])).sum::<f32>() / n;
        let z = rear_hubs.iter().map(|h| h[2]).sum::<f32>() / n;
        let mut shape = CarShape {
            wheel_radius,
            rear_hub_x: x,
            rear_hub_z: z,
            ..CarShape::DEFAULT
        };

        let half_width = crate::math::max(abs(bounds[0]), abs(bounds[3]));
        let half_length = (bounds[5] - bounds[2]) * 0.5;
        // A box that is empty or inside out says nothing about the car, and the default outline is
        // a better answer than a car that either stops a car's width short of every rail or drives
        // through them as if it were a point.
        if half_width > 0.1 && half_length > 0.1 {
            shape.half_width = half_width;
            shape.half_length = half_length;
            shape.centre_z = (bounds[5] + bounds[2]) * 0.5;
        }
        shape
    }

    /// How far the bodywork reaches either side of the origin, measured along a road normal.
    ///
    /// `fwd` and `side` are that normal resolved onto the car's own axes: how much of it lies
    /// along the way the car points, and how much across. Returns `(bias, reach)` — the middle of
    /// the footprint sits `bias` metres out along the normal from the origin, and its corners
    /// reach `reach` metres either side of that.
    ///
    /// This is the exact projection of the footprint rectangle onto one axis, and it costs no
    /// trigonometry: the two dot products the caller already has are the whole of it. A bounding
    /// circle would be easier and wrong by most of a metre — half the length of a 4.5 m car is
    /// 2.25 m, so a circle would hold the car that far off a rail it is driving straight past, on
    /// a road only 14 m wide.
    #[inline]
    pub fn lateral_span(&self, fwd: f32, side: f32) -> (f32, f32) {
        (
            self.centre_z * fwd,
            self.half_width * abs(side) + self.half_length * abs(fwd),
        )
    }
}

impl Default for CarShape {
    fn default() -> Self {
        CarShape::DEFAULT
    }
}

/// What a car is like to drive, as data rather than as constants.
///
/// The shape of a car is measured off its model; how it drives cannot be, because nothing in a
/// mesh says what an engine produces. So these come from the car's config file, are compiled into
/// the asset beside the geometry, and are read back by the game — which means a car that handles
/// differently is a different file and not a different build.
///
/// Every field defaults to the numbers the game was tuned with, and an asset that carries none
/// gets exactly those. That is deliberate: the descent is balanced around this car, and a new
/// model should have to ask before it changes how the game plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CarHandling {
    /// Kerb mass, kg.
    pub mass: f32,
    /// Yaw inertia, kg·m². How reluctant the car is to start and stop rotating.
    pub inertia: f32,
    /// Front axle ahead of the centre of mass, m.
    pub front_axle: f32,
    /// Rear axle behind it, m.
    pub rear_axle: f32,
    /// Drive force at full throttle from rest, N, before `POWER` scales it.
    pub engine: f32,
    /// Where that force has tailed off to its floor, m/s. The top speed in all but name.
    pub top_speed: f32,
    /// Braking force, N.
    pub brake: f32,
    /// Steering lock at a standstill, radians.
    pub steer_lock: f32,
    /// Multiplies cornering stiffness and the friction limit. Below 1.0 slides earlier.
    pub grip: f32,
}

impl CarHandling {
    pub const DEFAULT: CarHandling = CarHandling {
        mass: 1420.0,
        inertia: 1950.0,
        front_axle: 1.18,
        rear_axle: 1.42,
        engine: 8200.0,
        top_speed: 58.0,
        brake: 11000.0,
        steer_lock: 0.60,
        grip: 1.0,
    };

    /// Steering lock available at a given speed — less lock the faster you go.
    #[inline]
    pub fn steer_max(&self, speed: f32) -> f32 {
        self.steer_lock * (1.0 - 0.55 * min(1.0, speed / 55.0))
    }

    /// Rejects anything that would make the simulation misbehave rather than merely drive oddly.
    ///
    /// A zero mass divides by zero on the first substep and puts the car at infinity; a negative
    /// wheelbase inverts the yaw response and the car spins on its own axis. Both are typos in a
    /// config file, and both are much easier to explain here than to diagnose on a handheld.
    pub fn is_sane(&self) -> bool {
        self.mass > 1.0
            && self.inertia > 1.0
            && self.front_axle > 0.0
            && self.rear_axle > 0.0
            && self.engine >= 0.0
            && self.top_speed > 1.0
            && self.brake >= 0.0
            && self.steer_lock > 0.0
            && self.grip > 0.0
    }
}

impl Default for CarHandling {
    fn default() -> Self {
        CarHandling::DEFAULT
    }
}

const G: f32 = 9.81;

/// Scales every car's authored `engine`. Applied here rather than to the numbers in each car's
/// config, so the cars keep their power relative to one another and nothing needs reconverting.
/// At 1.5 the default car does 0–100 km/h in about 4.7 s against 6.7 s at 1.0; below about
/// 60 km/h it is grip rather than power that limits it, and traction control is what holds it there.
pub const POWER: f32 = 1.5;

/// Slip angle beyond which the car counts as drifting.
pub const DRIFT_SLIP: f32 = 0.16;
/// Minimum speed for a slide to score.
pub const DRIFT_SPEED: f32 = 9.0;
/// Beyond this lateral offset a slide stops scoring.
pub const DRIFT_LAT: f32 = 7.0;

/// Sideslip that pops the car into drift mode without the handbrake, radians (about 11 degrees).
pub const DRIFT_ENTRY_SLIP: f32 = 0.20;
/// Sideslip the car must stay under for `DRIFT_EXIT_TIME` before the aids come back.
pub const DRIFT_EXIT_SLIP: f32 = 0.09;
/// How long the car has to be straight before a drift counts as over, s. Long enough that the
/// straight moment in a transition from one corner to the next does not end it.
pub const DRIFT_EXIT_TIME: f32 = 0.5;
/// Time for the aids to fade out on entering a drift, and back in on leaving one, s.
const AIDS_OFF_TIME: f32 = 0.08;
const AIDS_ON_TIME: f32 = 0.45;

/// The drift assist's corridor, radians of sideslip (about 14 to 43 degrees).
///
/// Below it, with the throttle down, the assist lifts the angle back up so the slide does not die
/// mid-corner. Above it the assist pulls the angle back down so the car does not spin. Inside it
/// the assist only damps how fast the angle changes.
pub const DRIFT_CORRIDOR: (f32, f32) = (0.25, 0.75);
/// Yaw acceleration per radian outside the corridor, 1/s².
const ASSIST_GAIN: f32 = 14.0;
/// Yaw acceleration per rad/s of sideslip change, 1/s. Damps the drift's natural instability.
const ASSIST_DAMPING: f32 = 2.2;

/// Sideslip stability control lets through in grip mode before it starts to correct, radians.
const ESC_SLIP: f32 = 0.10;
/// Stability control's yaw acceleration per radian of excess sideslip, 1/s².
const ESC_SLIP_GAIN: f32 = 5.0;
/// Stability control's yaw acceleration per rad/s of yaw rate past what grip can explain, 1/s.
const ESC_YAW_GAIN: f32 = 6.0;
/// How far past the front tyres' peak slip the steering limiter lets the wheels go.
const STEER_LIMIT_MARGIN: f32 = 1.6;
/// How much of the front's direction of travel a released wheel follows in a drift.
const CASTER: f32 = 0.9;
/// Share of the rear tyre's grip that traction control leaves for drive.
const TC_MARGIN: f32 = 0.92;
/// Drive traction control always lets through, so a car sliding in grip mode is not left dead.
const TC_FLOOR: f32 = 0.15;
/// Share of the rear's sideways grip a fully spinning tyre loses.
const SPIN_GRIP_LOSS: f32 = 0.35;
/// How far the rear's sideways force falls once the tyre is sliding: kinetic grip below static.
const SLIDE_FALLOFF: f32 = 0.12;

/// What the driver is asking for this substep.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    /// 0.0–1.0.
    pub throttle: f32,
    pub brake: bool,
    pub handbrake: bool,
    /// -1.0 (right) to +1.0 (left).
    pub steer_in: f32,
}

/// Everything integrated by the simulation.
#[derive(Clone, Copy, Debug, Default)]
pub struct CarState {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub yaw: f32,
    /// Forward velocity, body frame.
    pub vx: f32,
    /// Lateral velocity, body frame.
    pub vy: f32,
    pub yaw_rate: f32,
    /// Current road-wheel angle in radians.
    pub steer: f32,
    /// Accumulated wheel rotation, for rendering only.
    pub wheel_spin: f32,
}

/// Things that happened during one substep, for the game layer to react to.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StepOutcome {
    /// A fresh guard-rail impact (gated by the hit cooldown).
    pub wall_tap: bool,
    /// The car has been pointing the wrong way long enough to warn about.
    pub wrong_way: bool,
    /// The car was put back on the centreline this substep.
    pub respawned: bool,
}

pub struct Vehicle {
    pub state: CarState,
    pub locator: Locator,
    /// Result of the last nearest-node query, after containment was applied.
    pub query: Query,

    /// How long the car has been in continuous contact with something.
    pub wall_timer: f32,
    /// How long the car has been pointing backwards.
    pub wrong_timer: f32,
    /// Suppresses repeat wall-tap penalties.
    pub hit_cooldown: f32,

    /// Handling aid, 0.4–1.6. 1.0 is the default feel.
    pub grip_assist: f32,

    /// Which compiled car asset draws this vehicle.
    ///
    /// An index rather than the model itself, because the physics has no opinion about geometry:
    /// the same simulation drives whatever is in that slot, which is what lets a second car be a
    /// second file. Nothing in this module ever reads it.
    pub model: usize,
    /// What that asset measures, for the parts of the game that have to agree with it.
    pub shape: CarShape,
    /// What that asset says it drives like. `CarHandling::DEFAULT` until a car says otherwise.
    pub handling: CarHandling,

    // --- derived each substep, for the renderer, HUD and scoring ---
    pub on_road: bool,
    pub slip_angle: f32,
    pub drifting: bool,

    // --- driver aids ---
    /// Drift mode: the aids are off and the drift assist is on. See the module docs.
    pub drift_mode: bool,
    /// How much of the aids is in effect, 0.0–1.0. Fades rather than switching, so they do not
    /// snap on at the end of a drift.
    pub aids: f32,
    /// How far the rear wheels are spinning up, 0.0–1.0.
    pub wheelspin: f32,
    /// Share of the requested drive that traction control took away this substep.
    pub tc_cut: f32,
    /// How long the car has been straight while in drift mode.
    calm_timer: f32,
    /// Last substep's signed sideslip, for its rate of change.
    prev_beta: f32,
}

impl Default for Vehicle {
    fn default() -> Self {
        Self::new()
    }
}

impl Vehicle {
    pub const fn new() -> Self {
        Vehicle {
            state: CarState {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                yaw: 0.0,
                vx: 0.0,
                vy: 0.0,
                yaw_rate: 0.0,
                steer: 0.0,
                wheel_spin: 0.0,
            },
            locator: Locator::new(),
            query: Query {
                index: 0,
                lat: 0.0,
                along: 0.0,
                over: 0.0,
            },
            wall_timer: 0.0,
            wrong_timer: 0.0,
            hit_cooldown: 0.0,
            grip_assist: 1.0,
            model: 0,
            shape: CarShape::DEFAULT,
            handling: CarHandling::DEFAULT,
            on_road: false,
            slip_angle: 0.0,
            drifting: false,
            drift_mode: false,
            aids: 1.0,
            wheelspin: 0.0,
            tc_cut: 0.0,
            calm_timer: 0.0,
            prev_beta: 0.0,
        }
    }

    /// Steering lock available at a given speed — less lock the faster you go.
    #[inline]
    pub fn steer_max(&self, speed: f32) -> f32 {
        self.handling.steer_max(speed)
    }

    /// Lateral limit at a node. The car park's side has no rail beside it, so it opens up to the
    /// car park's walls.
    pub fn containment_limit(&self, track: &Track, index: usize, lat: f32) -> f32 {
        if signum(lat) == CARPARK_SIDE {
            crate::track::carpark_limit(track, index)
        } else {
            RAIL_LIMIT
        }
    }

    /// Parks the car on the centreline at `index`, facing down the hill and stationary.
    pub fn place_at_node(&mut self, track: &Track, index: usize) {
        let n = &track.nodes[index];
        self.state.x = n.p.x;
        self.state.y = n.p.y;
        self.state.z = n.p.z;
        self.state.yaw = atan2(n.dir.x, n.dir.z);
        self.state.vx = 0.0;
        self.state.vy = 0.0;
        self.state.yaw_rate = 0.0;
        self.state.steer = 0.0;
        self.locator.reset_to(index);
        self.wall_timer = 0.0;
        self.wrong_timer = 0.0;
        self.hit_cooldown = 0.0;
        self.reset_aids();
        self.query = self.locator.nearest(track, self.state.x, self.state.z);
    }

    /// Back to grip mode with the aids fully on, for a car that has just been put somewhere.
    fn reset_aids(&mut self) {
        self.drift_mode = false;
        self.aids = 1.0;
        self.wheelspin = 0.0;
        self.tc_cut = 0.0;
        self.calm_timer = 0.0;
        self.prev_beta = 0.0;
    }

    /// Parks the car at an arbitrary pose (used for the title screen's car park).
    pub fn place_at(&mut self, track: &Track, x: f32, y: f32, z: f32, yaw: f32, index: usize) {
        self.state.x = x;
        self.state.y = y;
        self.state.z = z;
        self.state.yaw = yaw;
        self.state.vx = 0.0;
        self.state.vy = 0.0;
        self.state.yaw_rate = 0.0;
        self.state.steer = 0.0;
        self.locator.reset_to(index);
        self.reset_aids();
        self.query = self.locator.nearest(track, x, z);
    }

    /// Puts the car back on the centreline at the node it was last near.
    fn respawn(&mut self, track: &Track) {
        let i = self.locator.last_idx;
        let n = &track.nodes[i];
        self.state.x = n.p.x;
        self.state.y = n.p.y;
        self.state.z = n.p.z;
        self.state.yaw = atan2(n.dir.x, n.dir.z);
        self.state.vx = 3.0;
        self.state.vy = 0.0;
        self.state.yaw_rate = 0.0;
        self.wall_timer = 0.0;
        self.wrong_timer = 0.0;
        self.hit_cooldown = 0.6;
        self.reset_aids();
        self.query = self.locator.nearest(track, self.state.x, self.state.z);
    }

    /// One fixed substep of the simulation.
    pub fn step(&mut self, track: &Track, input: Input, dt: f32) -> StepOutcome {
        let mut out = StepOutcome::default();
        // Integrated on a local copy so the borrow checker still lets us query the track and
        // the car park's limits mid-step; written back before containment runs.
        let mut st = self.state;

        // The car's own numbers, taken once: every force below is scaled by one of them.
        let car = self.handling;

        // --- steering ------------------------------------------------------------------
        let speed = hypot(st.vx, st.vy);
        let lock = car.steer_max(speed);
        let mut target = input.steer_in * lock;
        let travel = atan2(st.vy + st.yaw_rate * car.front_axle, crate::math::max(2.2, abs(st.vx)));
        if st.vx > 0.5 && self.aids > 0.0 && !input.handbrake {
            // Steering limiter. Past the slip angle where the front tyres peak, more lock turns
            // the car no harder and only scrubs speed, and a d-pad asks for full lock on every
            // tap. So in grip mode the wheels stop at the peak, the way a good driver's hands
            // would. It fades out with the other aids: a drift wants all the lock there is.
            let peak = 0.5 * car.mass * G * 1.42 / 105_000.0 * STEER_LIMIT_MARGIN;
            let limited = if target > 0.0 {
                min(target, crate::math::max(0.0, travel + peak))
            } else {
                crate::math::max(target, min(0.0, travel - peak))
            };
            target += (limited - target) * self.aids;
        }
        if self.drift_mode {
            // Caster: a front wheel left to itself trails the way the front of the car is moving,
            // which in a slide is opposite lock. It is how a real car catches a slide when the
            // driver lets the wheel go, and it is what a d-pad, which only knows full lock or
            // none, needs to hold one. Whatever the stick does not ask for goes to the caster.
            target += clamp(travel * CASTER, -lock, lock) * (1.0 - abs(input.steer_in));
        }
        // Winding on is slower than letting go; the tyres load up either way.
        let rate = if abs(target) > abs(st.steer) { 6.0 } else { 9.0 };
        st.steer += clamp(target - st.steer, -rate * dt, rate * dt);

        // --- grip ----------------------------------------------------------------------
        let near = self.locator.nearest(track, st.x, st.z);
        let on_road = abs(near.lat) < TARMAC_HALF_WIDTH;
        let surface = if on_road { 1.0 } else { 0.62 };
        let grip = surface * (0.92 + 0.22 * self.grip_assist) * car.grip;
        self.on_road = on_road;

        let cf = 105_000.0 * grip;
        let cr = 118_000.0 * grip * if input.handbrake { 0.34 } else { 1.0 };
        let max_ff = 0.5 * car.mass * G * 1.42 * grip;
        let max_fr = max_ff * if input.handbrake { 0.30 } else { 1.0 };

        // --- grip mode or drift mode -----------------------------------------------------
        // Floor the longitudinal speed used for slip so the model does not blow up at rest.
        let vxs = crate::math::max(2.2, abs(st.vx));
        let beta = atan2(st.vy, vxs);
        let beta_rate = (beta - self.prev_beta) / dt;
        self.prev_beta = beta;
        if !self.drift_mode {
            if speed > DRIFT_SPEED && (input.handbrake || abs(beta) > DRIFT_ENTRY_SLIP) {
                self.drift_mode = true;
                self.calm_timer = 0.0;
            }
        } else {
            let calm = abs(beta) < DRIFT_EXIT_SLIP || speed < DRIFT_SPEED * 0.6;
            self.calm_timer = if calm { self.calm_timer + dt } else { 0.0 };
            if self.calm_timer > DRIFT_EXIT_TIME && !input.handbrake {
                self.drift_mode = false;
            }
        }
        self.aids = if self.drift_mode {
            crate::math::max(0.0, self.aids - dt / AIDS_OFF_TIME)
        } else {
            min(1.0, self.aids + dt / AIDS_ON_TIME)
        };
        let aids = self.aids;

        // --- tyre forces ---------------------------------------------------------------
        let dir_sign = if st.vx == 0.0 { 1.0 } else { signum(st.vx) };
        let slip_f = atan2(st.vy + st.yaw_rate * car.front_axle, vxs) - st.steer * dir_sign;
        let slip_r = atan2(st.vy - st.yaw_rate * car.rear_axle, vxs);
        let ff = clamp(-cf * slip_f, -max_ff, max_ff);

        // Drive goes through the rear tyres, so it shares their grip with cornering: a friction
        // circle. Engine, tailing off toward ~58 m/s.
        let mut drive =
            input.throttle * car.engine * POWER * crate::math::max(0.08, 1.0 - abs(st.vx) / car.top_speed);
        if input.handbrake {
            drive *= 0.35;
        }
        let lat_demand = cr * abs(slip_r);
        // Traction control: only as much drive as the circle has room for once cornering has
        // taken its share. Blended by `aids`, so it is fully off in a drift.
        let lat_used = min(lat_demand, max_fr * TC_MARGIN);
        let room = crate::math::sqrt(crate::math::max(
            0.0,
            (max_fr * TC_MARGIN) * (max_fr * TC_MARGIN) - lat_used * lat_used,
        ));
        let allowed = min(drive, crate::math::max(room, drive * TC_FLOOR));
        let requested = drive;
        drive += (allowed - drive) * aids;
        self.tc_cut = if requested > 0.0 { 1.0 - drive / requested } else { 0.0 };
        // Asking the rear for more than it has spins the wheels up. The aids never let it.
        let overdriven = hypot(drive, lat_demand) > max_fr && input.throttle > 0.0;
        self.wheelspin = if overdriven && aids < 0.5 {
            min(1.0, self.wheelspin + 5.0 * dt)
        } else {
            crate::math::max(0.0, self.wheelspin - 3.0 * dt)
        };
        let fx_rear = min(drive, max_fr * 0.95);
        // What the circle leaves for cornering, and less again for a tyre that is spinning.
        let lat_cap = crate::math::sqrt(crate::math::max(0.0, max_fr * max_fr - fx_rear * fx_rear))
            * (1.0 - SPIN_GRIP_LOSS * self.wheelspin);
        // Past the limit the tyre is sliding, and sliding grip is a little below the peak. That
        // drop is what lets a slide, once started, keep going.
        let fr = if lat_demand > lat_cap {
            let over = min(1.0, (lat_demand / crate::math::max(lat_cap, 1.0) - 1.0) * 0.5);
            -signum(slip_r) * lat_cap * (1.0 - SLIDE_FALLOFF * over)
        } else {
            -cr * slip_r
        };

        // --- longitudinal forces -------------------------------------------------------
        let mut fx = drive;
        if input.brake {
            // Reverse is a fraction of forward braking, so that holding the brake at a standstill
            // backs the car off a rail rather than launching it.
            fx -= if st.vx > 0.5 {
                car.brake
            } else {
                car.brake * 0.38
            };
        }
        fx -= 1.05 * st.vx * abs(st.vx); // aero drag
        fx -= if on_road { 210.0 } else { 900.0 } * signum(st.vx) * min(1.0, abs(st.vx) / 3.0);
        // The downhill pull. This is what makes the game a descent rather than a track day.
        fx += -G * sin(near.node_pitch(track)) * car.mass * 0.85;

        // --- aids and assist -----------------------------------------------------------
        // Yaw acceleration from the aids, 1/s². Sideslip changes as `beta' = Fy/(m vx) - r`, so
        // yaw rate in the direction of `beta` straightens the car and against it adds angle.
        let mut yaw_aid = 0.0;
        if aids > 0.0 {
            // Stability control: no faster rotation than grip can explain at this speed, and no
            // more sideslip than a road car's aids would allow.
            let r_cap = 1.42 * grip * G / crate::math::max(speed, 6.0) * 1.1;
            let spin = abs(st.yaw_rate) - r_cap;
            if spin > 0.0 {
                yaw_aid -= signum(st.yaw_rate) * spin * ESC_YAW_GAIN;
            }
            let slide = abs(beta) - ESC_SLIP;
            if slide > 0.0 {
                yaw_aid += signum(beta) * slide * ESC_SLIP_GAIN;
            }
            yaw_aid *= aids;
        }
        if self.drift_mode && !input.handbrake && speed > DRIFT_SPEED * 0.6 {
            // The drift assist: hold the angle inside the corridor, and damp how fast it moves.
            // It only lifts the angle while the throttle is down, so lifting off still ends it.
            let (lo, hi) = DRIFT_CORRIDOR;
            let want_more = if abs(beta) < lo && input.throttle > 0.0 {
                lo - abs(beta)
            } else if abs(beta) > hi {
                hi - abs(beta)
            } else {
                0.0
            };
            let fade = min(1.0, speed / 15.0) * (1.0 - aids);
            yaw_aid += (-signum(beta) * want_more * ASSIST_GAIN + beta_rate * ASSIST_DAMPING) * fade;
        }

        // --- integrate -----------------------------------------------------------------
        let ax = fx / car.mass - ff * sin(st.steer) / car.mass + st.yaw_rate * st.vy;
        let ay = (ff * cos(st.steer) + fr) / car.mass - st.yaw_rate * st.vx;
        st.vx += ax * dt;
        st.vy += ay * dt;
        st.yaw_rate +=
            ((car.front_axle * ff * cos(st.steer) - car.rear_axle * fr) / car.inertia + yaw_aid)
                * dt;
        // Less damping in a drift, where the yaw rate is what holds the angle.
        st.yaw_rate *= 1.0 - (0.6 + 1.0 * aids) * dt;

        st.yaw += st.yaw_rate * dt;
        let (s, c) = (sin(st.yaw), cos(st.yaw));
        st.x += (st.vx * s + st.vy * c) * dt;
        st.z += (st.vx * c - st.vy * s) * dt;
        self.state = st;

        // --- containment ---------------------------------------------------------------
        let q = self.locator.nearest(track, self.state.x, self.state.z);
        let node = track.nodes[q.index];

        // What has to clear the rail is the car, not the point it is steered from. The road normal
        // resolved onto the car's own axes gives the footprint's projection onto `lat`: `(s, c)` is
        // the way the car points and `(c, -s)` its right-hand side, both already unit vectors.
        let fwd = s * node.nrm.x + c * node.nrm.z;
        let side = c * node.nrm.x - s * node.nrm.z;
        let (bias, reach) = self.shape.lateral_span(fwd, side);
        // Where the middle of the bodywork sits, and where its two flanks end.
        let middle = q.lat + bias;
        // Both limits, rather than the one the origin happens to be nearest: the car park leaves
        // one side of these nodes open and the other railed, and they are 24 m apart.
        let (lim_pos, lim_neg) = (
            self.containment_limit(track, q.index, 1.0),
            self.containment_limit(track, q.index, -1.0),
        );
        // Which flank is through a rail, and by how much. Only ever one of them: the longest car
        // here reaches 2.7 m and the rails are 15 m apart.
        let contact = if middle + reach > lim_pos {
            Some((1.0, middle + reach - lim_pos))
        } else if middle - reach < -lim_neg {
            Some((-1.0, -lim_neg - (middle - reach)))
        } else {
            None
        };

        let st = &mut self.state;
        if q.over < -1.0 {
            // Driven back off the start line: hold it at the line.
            let push = -q.over - 1.0;
            st.x += node.dir.x * push;
            st.z += node.dir.z * push;
            if st.vx < 0.0 {
                st.vx *= 0.2;
            }
            self.wall_timer += dt;
        } else if let Some((sgn, push)) = contact {
            st.x -= node.nrm.x * sgn * push;
            st.z -= node.nrm.z * sgn * push;
            st.vy *= -0.25;
            self.wall_timer += dt;
            if self.hit_cooldown <= 0.0 {
                st.vx *= 0.82;
                st.yaw_rate *= 0.35;
                self.hit_cooldown = 0.5;
                out.wall_tap = true;
            }
        } else {
            self.wall_timer = crate::math::max(0.0, self.wall_timer - dt * 2.0);
        }

        // --- wrong way and stuck recovery ----------------------------------------------
        let road_heading = atan2(node.dir.x, node.dir.z);
        let heading_delta = wrap_pi(st.yaw - road_heading);
        self.wrong_timer = if abs(heading_delta) > 2.0 {
            self.wrong_timer + dt
        } else {
            0.0
        };
        if self.wrong_timer > 0.6 {
            out.wrong_way = true;
        }
        if self.wall_timer > 1.5 || self.wrong_timer > 3.5 {
            self.respawn(track);
            out.respawned = true;
            return out;
        }

        self.hit_cooldown = crate::math::max(0.0, self.hit_cooldown - dt);

        // --- ride height and derived state ---------------------------------------------
        let q3 = self.locator.nearest(track, self.state.x, self.state.z);
        let road_y = track.nodes[q3.index].p.y;
        self.state.y += (road_y - self.state.y) * min(1.0, 12.0 * dt);
        // Wrapped rather than accumulated. Left to grow, this reaches thousands of radians in a
        // single descent, and f32 resolution at that magnitude is coarse enough to make the
        // wheels stutter — and it is fed straight to the VFPU as a rotation angle.
        self.state.wheel_spin =
            wrap_tau(self.state.wheel_spin + self.state.vx * dt / self.shape.wheel_radius);
        self.query = q3;

        let slip = abs(atan2(
            self.state.vy,
            crate::math::max(0.1, abs(self.state.vx)),
        ));
        let spd = hypot(self.state.vx, self.state.vy);
        self.slip_angle = slip;
        self.drifting = slip > DRIFT_SLIP && spd > DRIFT_SPEED && abs(q3.lat) < DRIFT_LAT;

        out
    }

    /// Body roll from cornering load, for the renderer.
    pub fn roll(&self) -> f32 {
        clamp(-self.state.yaw_rate * abs(self.state.vx) * 0.010, -0.09, 0.09)
    }

    /// Pitch of the body, following the road but only insofar as the car is aligned with it.
    pub fn body_pitch(&self, track: &Track) -> f32 {
        let n = &track.nodes[self.query.index];
        -n.pitch * cos(self.state.yaw - atan2(n.dir.x, n.dir.z))
    }

    /// Speed in km/h, as the HUD shows it.
    #[inline]
    pub fn speed_kph(&self) -> f32 {
        abs(self.state.vx) * 3.6
    }
}

impl Query {
    /// Pitch of the node this query resolved to.
    #[inline]
    fn node_pitch(&self, track: &Track) -> f32 {
        track.nodes[self.index].pitch
    }
}
