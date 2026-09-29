//! Cameras.
//!
//! The chase camera follows the car's *velocity* heading rather than where its nose points. That
//! is the whole reason slides read properly: the car visibly rotates within the frame instead of
//! the world swinging around a car that always faces away from you.

use crate::math::{atan2, cos, hypot, lerp, min, sin, wrap_pi, Vec3, PI};
use crate::vehicle::CarState;

/// The title shot, relative to the car parked in the summit car park: back from it, out to its
/// right and up, then turned left of the way the car faces and pitched down. From there the first
/// hairpin sweeps across the left of the frame, the city fills the right, and the car stands in
/// the middle against the rail.
pub const TITLE_BACK: f32 = 8.5;
pub const TITLE_RIGHT: f32 = 1.5;
pub const TITLE_UP: f32 = 5.0;
pub const TITLE_TURN: f32 = 18.0 * PI / 180.0;
pub const TITLE_PITCH: f32 = -14.0 * PI / 180.0;
/// A slow push-in over the first seconds on the title screen: this much closer, over this long.
const TITLE_PUSH: f32 = 1.5;
const TITLE_PUSH_TIME: f32 = 20.0;
pub const TITLE_FOV: f32 = 54.0;
const RUN_FOV_BASE: f32 = 60.0;

pub struct Camera {
    pub pos: Vec3,
    pub look_at: Vec3,
    /// Heading the camera is orbiting from, in the run chase.
    pub yaw: f32,
    pub fov: f32,
    /// Decaying impact shake.
    pub shake: f32,
    /// Seconds on the title screen, which drives its slow push-in.
    pub title_time: f32,
    /// Swing the chase round to look at the front of the car. This is the pose the camera already
    /// takes by itself when the car is reversing — the velocity heading points backwards, so the
    /// camera ends up ahead of the bonnet looking back down the road — and it is reached the same
    /// way, by swinging rather than cutting, so it reads as the same camera and not another one.
    pub front_view: bool,
    /// Cheap deterministic noise for the shake — no RNG in `no_std`, and determinism keeps
    /// headless captures reproducible.
    rng: u32,
}

impl Default for Camera {
    fn default() -> Self {
        Self::new()
    }
}

impl Camera {
    pub const fn new() -> Self {
        Camera {
            pos: Vec3::ZERO,
            look_at: Vec3::ZERO,
            yaw: 0.0,
            fov: RUN_FOV_BASE,
            shake: 0.0,
            title_time: 0.0,
            front_view: false,
            rng: 0x1234_5678,
        }
    }

    pub fn add_shake(&mut self, amount: f32) {
        if amount > self.shake {
            self.shake = amount;
        }
    }

    fn next_noise(&mut self) -> f32 {
        // xorshift32, mapped to -0.5..0.5
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1 << 24) as f32 - 0.5
    }

    /// Places the camera straight behind the car, for the first frame of a run.
    pub fn snap_behind(&mut self, car: &CarState) {
        self.yaw = car.yaw;
        self.pos = Vec3::new(
            car.x - sin(car.yaw) * 7.4,
            car.y + 3.3,
            car.z - cos(car.yaw) * 7.4,
        );
    }

    /// The title shot over the parked car, easing slowly in.
    pub fn update_title(&mut self, car: &CarState, dt: f32) {
        self.title_time = min(self.title_time + dt, TITLE_PUSH_TIME);
        let t = self.title_time / TITLE_PUSH_TIME;
        // Eased out, so the move settles rather than stopping dead.
        let push = TITLE_PUSH * (1.0 - (1.0 - t) * (1.0 - t));
        let (fx, fz) = (sin(car.yaw), cos(car.yaw));
        let (rx, rz) = (-fz, fx);
        let back = TITLE_BACK - push;
        self.pos = Vec3::new(
            car.x - fx * back + rx * TITLE_RIGHT,
            car.y + TITLE_UP,
            car.z - fz * back + rz * TITLE_RIGHT,
        );
        let look = car.yaw + TITLE_TURN;
        let (cp, sp) = (cos(TITLE_PITCH), sin(TITLE_PITCH));
        self.look_at = Vec3::new(
            self.pos.x + sin(look) * cp * 10.0,
            self.pos.y + sp * 10.0,
            self.pos.z + cos(look) * cp * 10.0,
        );
        self.fov = lerp(self.fov, TITLE_FOV, min(1.0, 3.0 * dt));
    }

    /// Chase camera during a run.
    pub fn update_run(&mut self, car: &CarState, dt: f32) {
        let speed = hypot(car.vx, car.vy);

        // Heading the car is actually travelling in, which differs from `yaw` in a slide.
        let vel_yaw = atan2(
            car.vx * sin(car.yaw) + car.vy * cos(car.yaw),
            car.vx * cos(car.yaw) - car.vy * sin(car.yaw),
        );
        // Below walking pace the velocity heading is just noise.
        let target = if self.front_view {
            // The nose, whichever way the car is travelling: a glance forward has to keep meaning
            // the same thing when the car is going backwards or sideways.
            car.yaw + PI
        } else if speed > 4.0 {
            vel_yaw
        } else {
            car.yaw
        };
        self.yaw += wrap_pi(target - self.yaw) * min(1.0, 3.2 * dt);

        let dist = 7.4 + min(3.2, speed * 0.075);
        let want = Vec3::new(
            car.x - sin(self.yaw) * dist,
            car.y + 3.3 + min(1.2, speed * 0.02),
            car.z - cos(self.yaw) * dist,
        );

        let t = min(1.0, 7.0 * dt);
        self.pos = Vec3::new(
            lerp(self.pos.x, want.x, t),
            lerp(self.pos.y, want.y, t),
            lerp(self.pos.z, want.z, t),
        );

        if self.shake > 0.0 {
            self.shake -= dt * 2.0;
            if self.shake < 0.0 {
                self.shake = 0.0;
            }
            let (nx, ny) = (self.next_noise(), self.next_noise());
            self.pos.x += nx * self.shake * 0.7;
            self.pos.y += ny * self.shake * 0.5;
        }

        // Look ahead of the car along the chase heading, not at the car itself.
        self.look_at = Vec3::new(
            car.x + sin(self.yaw) * 9.0,
            car.y + 1.6,
            car.z + cos(self.yaw) * 9.0,
        );

        let want_fov = RUN_FOV_BASE + min(12.0, speed * 0.28);
        self.fov = lerp(self.fov, want_fov, min(1.0, 3.0 * dt));
    }
}
