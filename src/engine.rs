//! The engine the player hears and the HUD shows: revs, gear and boost, worked out from how fast
//! the car is going and what the driver is doing.
//!
//! The vehicle model has no drivetrain. It pushes the car with a force that tails off toward top
//! speed, and that is all the handling needs. The sound needs more: an engine whose pitch only
//! climbs with speed is one long drone, which is what the first synthesiser was. So this puts a
//! six-speed gearbox between the road and the engine, shifted the way a driver would, and the rev
//! counter reads the same gearbox so the needle drops when the note does.
//!
//! None of it feeds back into the car. It is a model of what the engine would be doing, not of
//! what moves the car.

use crate::hud::Gear;
use crate::math::{abs, clamp, exp, max, min, sin};

/// Engine rpm per m/s of road speed in each gear: a Silvia S15's six-speed box on a 4.1 final
/// drive and 0.32 m tyres. First runs out at about 17 m/s, second at 30, third at 44.
pub const GEAR_RPM_PER_MS: [f32; 6] = [406.0, 233.0, 160.0, 122.0, 93.0, 79.0];
pub const IDLE_RPM: f32 = 850.0;
/// Where the limiter cuts in.
pub const REDLINE_RPM: f32 = 7600.0;

/// Upshift above this under throttle.
const SHIFT_UP_RPM: f32 = 7100.0;
/// Under braking, change down while the lower gear would stay below this.
const BRAKE_DOWN_RPM: f32 = 6300.0;
/// Change down anyway once the revs fall this low.
const LUG_RPM: f32 = 2200.0;
/// Revs held against a slipping clutch pulling away.
const LAUNCH_RPM: f32 = 3400.0;
/// Revs the engine reaches revving freely at a standstill.
const FREE_REV_RPM: f32 = 5500.0;
/// Slip angle where the tyres are sliding rather than gripping, as for the squeal.
const SLIDE_SLIP: f32 = 0.14;

/// What the engine is doing this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Engine {
    /// 0 is first gear. Meaningless while `reverse`.
    gear: usize,
    reverse: bool,
    pub rpm: f32,
    /// How hard the engine is working, 0–1: the throttle, less the moments a gear change or the
    /// limiter cuts it.
    pub load: f32,
    /// Turbo boost, 0–1. Builds slowly under load and dumps quickly on a lift.
    pub boost: f32,
    shift_left: f32,
    limiter_phase: f32,
    /// Drives the slow wander of the idle.
    idle_phase: f32,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// Moves `cur` toward `target` as a first-order lag with the given rate, per second.
fn lag(cur: f32, target: f32, rate: f32, dt: f32) -> f32 {
    cur + (target - cur) * (1.0 - exp(-rate * dt))
}

impl Engine {
    pub const fn new() -> Self {
        Engine {
            gear: 0,
            reverse: false,
            rpm: IDLE_RPM,
            load: 0.0,
            boost: 0.0,
            shift_left: 0.0,
            limiter_phase: 0.0,
            idle_phase: 0.0,
        }
    }

    /// The gear the HUD shows.
    pub fn gear(&self) -> Gear {
        if self.reverse {
            Gear::Reverse
        } else {
            Gear::Forward(self.gear as u8 + 1)
        }
    }

    /// The rev counter's needle, 0 at a standstill and 1 at the limiter.
    pub fn rev_fraction(&self) -> f32 {
        clamp(self.rpm / REDLINE_RPM, 0.0, 1.0)
    }

    /// Whether a gear change is under way, when the throttle is cut.
    pub fn shifting(&self) -> bool {
        self.shift_left > 0.0
    }

    /// Advances one frame. `vx` is forward speed, negative going backwards; `slip` is the slip
    /// angle, which should be zero unless the car is actually drifting.
    pub fn update(&mut self, vx: f32, throttle: f32, brake: bool, slip: f32, dt: f32) {
        let v = abs(vx);
        // Reverse only once the car is really rolling backwards, as the gear readout always did.
        self.reverse = vx < -0.6;
        let ratio = if self.reverse { GEAR_RPM_PER_MS[0] } else { GEAR_RPM_PER_MS[self.gear] };
        let road = |g: usize| v * GEAR_RPM_PER_MS[g];

        if !self.reverse && self.shift_left <= 0.0 {
            if self.gear < 5 && road(self.gear) > SHIFT_UP_RPM && throttle > 0.3 {
                self.gear += 1;
                self.shift_left = 0.16;
            } else if self.gear > 0 && brake && road(self.gear - 1) < BRAKE_DOWN_RPM {
                self.gear -= 1;
                self.shift_left = 0.14;
            } else if self.gear > 0 && road(self.gear) < LUG_RPM {
                self.gear -= 1;
                self.shift_left = 0.2;
            }
        }
        let shifting = self.shift_left > 0.0;
        self.shift_left -= dt;

        let mut target = v * ratio;
        if self.gear == 0 || self.reverse {
            // Pulling away the clutch slips, and holds the revs up until the road catches up.
            target = max(target, IDLE_RPM + throttle * LAUNCH_RPM);
        }
        if slip > SLIDE_SLIP {
            // Sliding, the driven wheels spin up past road speed and take the revs with them.
            target *= 1.0 + (slip - 0.1) * 0.55 * throttle;
        }
        if v < 0.5 {
            // At a standstill the clutch is in and the engine revs freely.
            target = IDLE_RPM + throttle * FREE_REV_RPM;
        }
        self.idle_phase += dt * 1.3;
        if self.idle_phase > 100.0 {
            self.idle_phase -= 100.0;
        }
        target = max(target, IDLE_RPM + sin(self.idle_phase) * 12.0);
        // Past the limiter the fuel is cut, so the revs bounce against it rather than climbing on.
        target = min(target, REDLINE_RPM + 150.0);

        let rate = if shifting {
            18.0
        } else if v < 0.5 {
            6.0
        } else {
            12.0
        };
        self.rpm = lag(self.rpm, target, rate, dt);

        // The limiter cuts the fuel in bursts, about fourteen a second.
        let mut drive = throttle;
        if self.rpm > REDLINE_RPM {
            self.limiter_phase += dt * 14.0;
            if self.limiter_phase > 1.0 {
                self.limiter_phase -= 1.0;
            }
            if self.limiter_phase < 0.5 {
                drive = 0.0;
            }
        }
        if shifting {
            drive = 0.0;
        }
        self.load = lag(self.load, drive, 25.0, dt);

        let want = clamp(drive * (self.rpm - 2800.0) / 3500.0, 0.0, 1.0);
        let rate = if want < self.boost { 5.0 } else { 1.1 };
        self.boost = lag(self.boost, want, rate, dt);
    }
}
