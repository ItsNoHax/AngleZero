//! Cameras.
//!
//! The chase camera follows the car's *velocity* heading rather than where its nose points. That
//! is the whole reason slides read properly: the car visibly rotates within the frame instead of
//! the world swinging around a car that always faces away from you.

use crate::math::{atan2, cos, hypot, lerp, min, sin, wrap_pi, Vec3, PI};
use crate::vehicle::CarState;

/// Radius of the title screen's orbit.
const ORBIT_RADIUS: f32 = 10.5;
const ORBIT_SPEED: f32 = 0.16;
const TITLE_FOV: f32 = 54.0;
const RUN_FOV_BASE: f32 = 60.0;

pub struct Camera {
    pub pos: Vec3,
    pub look_at: Vec3,
    /// Heading the camera is orbiting from, in the run chase.
    pub yaw: f32,
    pub fov: f32,
    /// Decaying impact shake.
    pub shake: f32,
    /// Title-screen orbit phase.
    pub orbit_angle: f32,
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
            orbit_angle: 0.0,
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

    /// Slow orbit around the parked car on the title screen.
    pub fn update_title(&mut self, car: &CarState, dt: f32) {
        self.orbit_angle += dt * ORBIT_SPEED;
        let a = self.orbit_angle;
        self.pos = Vec3::new(
            car.x + sin(a) * ORBIT_RADIUS,
            car.y + 3.1 + sin(a * 0.7) * 0.5,
            car.z + cos(a) * ORBIT_RADIUS,
        );
        self.look_at = Vec3::new(car.x, car.y + 0.95, car.z);
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
