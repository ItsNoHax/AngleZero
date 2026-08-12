//! Vehicle simulation.
//!
//! A single-track (bicycle) model with linear tyres and a friction ceiling, integrated at a fixed
//! 1/120 s so behaviour does not change with frame rate. Gravity along the road's pitch is what
//! actually drives the game: the car accelerates downhill with no throttle at all.
//!
//! Signs: `+lat` is to the car's right (see `Vec2::lateral_normal`), `+steer` turns left,
//! `vx` is forward and `vy` lateral in the body frame.

use crate::math::{abs, atan2, clamp, cos, hypot, min, sin, signum, wrap_pi, wrap_tau};
use crate::track::{
    Locator, Query, Track, BAY_FROM, BAY_LIMIT, BAY_SIDE, BAY_TO, RAIL_LIMIT, TARMAC_HALF_WIDTH,
};

/// Physics runs at this fixed step regardless of frame rate.
pub const FIXED_DT: f32 = 1.0 / 120.0;
/// Never take more than this many substeps for one frame, so a stall cannot spiral.
pub const MAX_SUBSTEPS: u32 = 40;
/// A frame longer than this is treated as a hitch and clamped.
pub const MAX_FRAME_DT: f32 = 0.25;

/// Rolling radius used to turn forward speed into wheel rotation.
pub const WHEEL_RADIUS: f32 = 0.36;

const MASS: f32 = 1420.0;
const LF: f32 = 1.18;
const LR: f32 = 1.42;
const IZ: f32 = 1950.0;
const G: f32 = 9.81;

/// Slip angle beyond which the car counts as drifting.
pub const DRIFT_SLIP: f32 = 0.16;
/// Minimum speed for a slide to score.
pub const DRIFT_SPEED: f32 = 9.0;
/// Beyond this lateral offset a slide stops scoring.
pub const DRIFT_LAT: f32 = 7.0;

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

    // --- derived each substep, for the renderer, HUD and scoring ---
    pub on_road: bool,
    pub slip_angle: f32,
    pub drifting: bool,
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
            on_road: false,
            slip_angle: 0.0,
            drifting: false,
        }
    }

    /// Steering lock available at a given speed — less lock the faster you go.
    #[inline]
    pub fn steer_max(speed: f32) -> f32 {
        0.60 * (1.0 - 0.55 * min(1.0, speed / 55.0))
    }

    /// Lateral limit at a node. The bay side of the pull-off has no rail, so it opens up.
    pub fn containment_limit(&self, index: usize, lat: f32) -> f32 {
        let in_bay = index > BAY_FROM && index < BAY_TO && signum(lat) == BAY_SIDE;
        if in_bay {
            BAY_LIMIT
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
        self.query = self.locator.nearest(track, self.state.x, self.state.z);
    }

    /// Parks the car at an arbitrary pose (used for the title screen's pull-off).
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
        self.query = self.locator.nearest(track, self.state.x, self.state.z);
    }

    /// One fixed substep of the simulation.
    pub fn step(&mut self, track: &Track, input: Input, dt: f32) -> StepOutcome {
        let mut out = StepOutcome::default();
        // Integrated on a local copy so the borrow checker still lets us query the track and
        // the bay limits mid-step; written back before containment runs.
        let mut st = self.state;

        // --- steering ------------------------------------------------------------------
        let speed = hypot(st.vx, st.vy);
        let target = input.steer_in * Self::steer_max(speed);
        // Winding on is slower than letting go; the tyres load up either way.
        let rate = if abs(target) > abs(st.steer) { 6.0 } else { 9.0 };
        st.steer += clamp(target - st.steer, -rate * dt, rate * dt);

        // --- grip ----------------------------------------------------------------------
        let near = self.locator.nearest(track, st.x, st.z);
        let on_road = abs(near.lat) < TARMAC_HALF_WIDTH;
        let surface = if on_road { 1.0 } else { 0.62 };
        let grip = surface * (0.92 + 0.22 * self.grip_assist);
        self.on_road = on_road;

        let cf = 105_000.0 * grip;
        let cr = 118_000.0 * grip * if input.handbrake { 0.34 } else { 1.0 };
        let max_ff = 0.5 * MASS * G * 1.42 * grip;
        let max_fr = max_ff * if input.handbrake { 0.30 } else { 1.0 };

        // --- tyre forces ---------------------------------------------------------------
        // Floor the longitudinal speed used for slip so the model does not blow up at rest.
        let vxs = crate::math::max(2.2, abs(st.vx));
        let dir_sign = if st.vx == 0.0 { 1.0 } else { signum(st.vx) };
        let slip_f = atan2(st.vy + st.yaw_rate * LF, vxs) - st.steer * dir_sign;
        let slip_r = atan2(st.vy - st.yaw_rate * LR, vxs);
        let ff = clamp(-cf * slip_f, -max_ff, max_ff);
        let fr = clamp(-cr * slip_r, -max_fr, max_fr);

        // --- longitudinal forces -------------------------------------------------------
        // Engine, tailing off toward ~58 m/s.
        let mut fx = input.throttle * 8200.0 * crate::math::max(0.08, 1.0 - abs(st.vx) / 58.0);
        if input.handbrake {
            fx *= 0.35;
        }
        if input.brake {
            fx -= if st.vx > 0.5 { 11000.0 } else { 4200.0 };
        }
        fx -= 1.05 * st.vx * abs(st.vx); // aero drag
        fx -= if on_road { 210.0 } else { 900.0 } * signum(st.vx) * min(1.0, abs(st.vx) / 3.0);
        // The downhill pull. This is what makes the game a descent rather than a track day.
        fx += -G * sin(near.node_pitch(track)) * MASS * 0.85;

        // --- integrate -----------------------------------------------------------------
        let ax = fx / MASS - ff * sin(st.steer) / MASS + st.yaw_rate * st.vy;
        let ay = (ff * cos(st.steer) + fr) / MASS - st.yaw_rate * st.vx;
        st.vx += ax * dt;
        st.vy += ay * dt;
        st.yaw_rate += ((LF * ff * cos(st.steer) - LR * fr) / IZ) * dt;
        st.yaw_rate *= 1.0 - 1.6 * dt;

        st.yaw += st.yaw_rate * dt;
        let (s, c) = (sin(st.yaw), cos(st.yaw));
        st.x += (st.vx * s + st.vy * c) * dt;
        st.z += (st.vx * c - st.vy * s) * dt;
        self.state = st;
        let st = &mut self.state;

        // --- containment ---------------------------------------------------------------
        let q = self.locator.nearest(track, st.x, st.z);
        let node = track.nodes[q.index];
        let in_bay = q.index > BAY_FROM && q.index < BAY_TO && signum(q.lat) == BAY_SIDE;
        let lim = if in_bay { BAY_LIMIT } else { RAIL_LIMIT };

        if q.over < -1.0 {
            // Driven back off the start line: hold it at the line.
            let push = -q.over - 1.0;
            st.x += node.dir.x * push;
            st.z += node.dir.z * push;
            if st.vx < 0.0 {
                st.vx *= 0.2;
            }
            self.wall_timer += dt;
        } else if abs(q.lat) > lim {
            let sgn = signum(q.lat);
            let push = abs(q.lat) - lim;
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
            wrap_tau(self.state.wheel_spin + self.state.vx * dt / WHEEL_RADIUS);
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
