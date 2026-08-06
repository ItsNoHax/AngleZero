//! Scalar and small-vector helpers.
//!
//! `no_std` has no float transcendentals, so these come from `libm`. Keeping them behind
//! this module means the rest of the core reads like normal float code.

pub const PI: f32 = core::f32::consts::PI;
pub const TAU: f32 = PI * 2.0;

#[inline]
pub fn sin(x: f32) -> f32 {
    libm::sinf(x)
}

#[inline]
pub fn cos(x: f32) -> f32 {
    libm::cosf(x)
}

#[inline]
pub fn atan2(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}

#[inline]
pub fn asin(x: f32) -> f32 {
    libm::asinf(x)
}

#[inline]
pub fn sqrt(x: f32) -> f32 {
    libm::sqrtf(x)
}

#[inline]
pub fn abs(x: f32) -> f32 {
    libm::fabsf(x)
}

#[inline]
pub fn floor(x: f32) -> f32 {
    libm::floorf(x)
}

#[inline]
pub fn hypot(x: f32, y: f32) -> f32 {
    sqrt(x * x + y * y)
}

#[inline]
pub fn radians(degrees: f32) -> f32 {
    degrees * (PI / 180.0)
}

#[inline]
pub fn min(a: f32, b: f32) -> f32 {
    if a < b {
        a
    } else {
        b
    }
}

#[inline]
pub fn max(a: f32, b: f32) -> f32 {
    if a > b {
        a
    } else {
        b
    }
}

/// `min` for indices — `core::cmp::min` needs `Ord` imports the callers do not otherwise want.
#[inline]
pub fn umin(a: usize, b: usize) -> usize {
    if a < b {
        a
    } else {
        b
    }
}

#[inline]
pub fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    min(max(x, lo), hi)
}

/// `+1.0` for positive input, `-1.0` for negative, `0.0` for exactly zero.
#[inline]
pub fn signum(x: f32) -> f32 {
    if x > 0.0 {
        1.0
    } else if x < 0.0 {
        -1.0
    } else {
        0.0
    }
}

#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Wraps an angle into `(-PI, PI]`, so heading differences stay meaningful across the seam.
pub fn wrap_pi(mut a: f32) -> f32 {
    while a > PI {
        a -= TAU;
    }
    while a <= -PI {
        a += TAU;
    }
    a
}

/// Wraps a continuously accumulating angle into `[0, TAU)`.
///
/// Anything that grows without bound and is later used as a rotation angle must go through this:
/// f32 loses meaningful angular resolution long before such a value stops being "valid".
pub fn wrap_tau(mut a: f32) -> f32 {
    while a >= TAU {
        a -= TAU;
    }
    while a < 0.0 {
        a += TAU;
    }
    a
}

/// Moves `current` toward `target` by at most `max_delta`.
#[inline]
pub fn approach(current: f32, target: f32, max_delta: f32) -> f32 {
    let d = target - current;
    if abs(d) <= max_delta {
        target
    } else {
        current + signum(d) * max_delta
    }
}

/// A 3D point in world space. `+Y` is up; the track descends in `-Y`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    #[inline]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Vec3 { x, y, z }
    }

    #[inline]
    pub fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    #[inline]
    pub fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }

    #[inline]
    pub fn scale(self, k: f32) -> Vec3 {
        Vec3::new(self.x * k, self.y * k, self.z * k)
    }

    #[inline]
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    #[inline]
    pub fn length(self) -> f32 {
        sqrt(self.dot(self))
    }

    #[inline]
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn normalized(self) -> Vec3 {
        let len = self.length();
        if len > 1e-6 {
            self.scale(1.0 / len)
        } else {
            Vec3::ZERO
        }
    }

    /// Horizontal (XZ-plane) distance, ignoring height.
    #[inline]
    pub fn horizontal_distance(self, o: Vec3) -> f32 {
        hypot(self.x - o.x, self.z - o.z)
    }
}

/// A horizontal direction in the XZ plane. Used for tangents and normals along the track.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub z: f32,
}

impl Vec2 {
    #[inline]
    pub const fn new(x: f32, z: f32) -> Self {
        Vec2 { x, z }
    }

    #[inline]
    pub fn dot(self, o: Vec2) -> f32 {
        self.x * o.x + self.z * o.z
    }

    /// 2D cross product (the Y component of the 3D cross), signed by turn direction.
    #[inline]
    pub fn cross(self, o: Vec2) -> f32 {
        self.x * o.z - self.z * o.x
    }

    #[inline]
    pub fn length(self) -> f32 {
        hypot(self.x, self.z)
    }

    pub fn normalized(self) -> Vec2 {
        let len = self.length();
        if len > 1e-6 {
            Vec2::new(self.x / len, self.z / len)
        } else {
            Vec2::new(0.0, 1.0)
        }
    }

    /// The lateral normal `(-z, x)`.
    ///
    /// The design calls this the "left normal", but in a right-handed, +Y-up
    /// frame it points to the car's **right** when travelling forward. The formula is kept
    /// exactly as it was — only the name is corrected — because every sign
    /// convention downstream (`lat`, the bay side, rail pushes) is built on it.
    #[inline]
    pub fn lateral_normal(self) -> Vec2 {
        Vec2::new(-self.z, self.x)
    }
}

/// A 4×4 matrix in column-major order, matching `ScePspFMatrix4` byte for byte so it can be
/// handed to `sceGumLoadMatrix` directly.
///
/// Only the view matrix is built here. `sceGumLookAt` in rust-psp 0.3.13 does nothing at all —
/// its helper shadows its own `&mut` output parameter, so the caller's matrix is never written
/// and the view stays identity. Constructing it here keeps it testable on the host.
///
/// Aligned to 16 bytes because the PSP's VFPU loads matrices with `lv.q`, which faults on
/// anything less.
#[derive(Clone, Copy, Debug)]
#[repr(C, align(16))]
pub struct Mat4 {
    /// Column-major: element `[row][col]` lives at `m[col * 4 + row]`.
    m: [f32; 16],
}

impl Mat4 {
    pub const IDENTITY: Mat4 = Mat4 {
        m: [
            1.0, 0.0, 0.0, 0.0, //
            0.0, 1.0, 0.0, 0.0, //
            0.0, 0.0, 1.0, 0.0, //
            0.0, 0.0, 0.0, 1.0,
        ],
    };

    #[inline]
    pub fn columns(&self) -> &[f32; 16] {
        &self.m
    }

    /// Right-handed view matrix: the camera ends up at the origin looking down `-Z`.
    pub fn look_at(eye: Vec3, center: Vec3, up: Vec3) -> Mat4 {
        let forward = center.sub(eye).normalized();
        // `normalized` returns zero rather than NaN for a degenerate input, so guard the cases
        // that would otherwise leave the basis unusable.
        let forward = if forward.length() < 0.5 {
            Vec3::new(0.0, 0.0, -1.0)
        } else {
            forward
        };
        let mut side = forward.cross(up).normalized();
        if side.length() < 0.5 {
            // Looking straight up or down: any perpendicular will do.
            side = forward.cross(Vec3::new(1.0, 0.0, 0.0)).normalized();
            if side.length() < 0.5 {
                side = Vec3::new(1.0, 0.0, 0.0);
            }
        }
        let true_up = side.cross(forward);

        Mat4 {
            m: [
                side.x,
                true_up.x,
                -forward.x,
                0.0,
                side.y,
                true_up.y,
                -forward.y,
                0.0,
                side.z,
                true_up.z,
                -forward.z,
                0.0,
                -side.dot(eye),
                -true_up.dot(eye),
                forward.dot(eye),
                1.0,
            ],
        }
    }

    /// Applies the matrix to a point (implicit `w = 1`).
    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        Vec3::new(
            self.m[0] * p.x + self.m[4] * p.y + self.m[8] * p.z + self.m[12],
            self.m[1] * p.x + self.m[5] * p.y + self.m[9] * p.z + self.m[13],
            self.m[2] * p.x + self.m[6] * p.y + self.m[10] * p.z + self.m[14],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        abs(a - b) < 1e-4
    }

    #[test]
    fn wrap_pi_folds_angles_into_a_single_turn() {
        assert!(close(wrap_pi(0.0), 0.0));
        assert!(close(wrap_pi(PI * 0.5), PI * 0.5));
        assert!(close(wrap_pi(TAU + 0.3), 0.3));
        assert!(close(wrap_pi(-TAU - 0.3), -0.3));
        // Crossing the seam must give the short way round, not the long way.
        assert!(close(wrap_pi(PI + 0.1), -PI + 0.1));
    }

    #[test]
    fn wrap_pi_result_is_always_within_range() {
        let mut a = -20.0f32;
        while a < 20.0 {
            let w = wrap_pi(a);
            assert!(w > -PI - 1e-5 && w <= PI + 1e-5, "wrap_pi({a}) = {w}");
            a += 0.37;
        }
    }

    #[test]
    fn approach_moves_by_at_most_the_step_and_never_overshoots() {
        assert!(close(approach(0.0, 1.0, 0.25), 0.25));
        assert!(close(approach(0.0, -1.0, 0.25), -0.25));
        // Within one step, it lands exactly on the target rather than oscillating.
        assert!(close(approach(0.9, 1.0, 0.25), 1.0));
        assert!(close(approach(1.0, 1.0, 0.25), 1.0));
    }

    #[test]
    fn clamp_and_signum_behave() {
        assert!(close(clamp(5.0, -1.0, 1.0), 1.0));
        assert!(close(clamp(-5.0, -1.0, 1.0), -1.0));
        assert!(close(clamp(0.5, -1.0, 1.0), 0.5));
        assert!(close(signum(-3.0), -1.0));
        assert!(close(signum(0.0), 0.0));
    }

    #[test]
    fn lateral_normal_is_perpendicular_and_points_to_the_cars_right() {
        // The track starts facing -Z. With +Y up and a right-handed frame, right is
        // `forward × up` = +X, which is what the normal must give.
        let dir = Vec2::new(0.0, -1.0);
        let n = dir.lateral_normal();
        assert!(close(n.dot(dir), 0.0));
        assert!(close(n.x, 1.0));
        assert!(close(n.z, 0.0));

        // Facing +Z instead, right is -X.
        let n = Vec2::new(0.0, 1.0).lateral_normal();
        assert!(close(n.x, -1.0));
    }

    #[test]
    fn vec3_basics() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(4.0, 5.0, 6.0);
        assert_eq!(a.add(b), Vec3::new(5.0, 7.0, 9.0));
        assert_eq!(a.sub(b), Vec3::new(-3.0, -3.0, -3.0));
        assert!(close(a.dot(b), 32.0));
        assert!(close(Vec3::new(3.0, 4.0, 0.0).length(), 5.0));
        assert!(close(Vec3::new(3.0, 4.0, 0.0).normalized().length(), 1.0));
        assert!(close(Vec3::ZERO.normalized().length(), 0.0));
    }
}
