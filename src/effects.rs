//! Skid decals and tyre smoke.
//!
//! Both are fixed-size ring buffers. Nothing here allocates: a long slide would otherwise emit
//! without bound, and there is no allocator on the PSP side anyway. When a pool is full the
//! oldest entry is simply overwritten, which is what gives the marks their limited trail length.

use crate::math::{cos, sin};
use crate::vehicle::{CarShape, CarState};

/// 260 marks at two per substep gives roughly a second of trail.
pub const MAX_SKIDS: usize = 260;
pub const MAX_PUFFS: usize = 34;

/// How long a puff lives.
pub const PUFF_LIFE: f32 = 0.9;
/// Peak opacity of a puff, and of a fresh skid mark.
pub const SKID_ALPHA: f32 = 0.34;
// The rear hub offsets used to be constants here, matching the car the renderer built out of
// boxes. They are measured off whichever car is loaded now and arrive with the call: a mark laid
// where the tyre is not shows up the moment the car slides.

/// A tyre mark laid on the road.
#[derive(Clone, Copy, Debug, Default)]
pub struct Skid {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Heading of the mark, matching the car at the moment it was laid.
    pub yaw: f32,
    /// Lengthwise stretch, so faster slides leave longer marks.
    pub stretch: f32,
    pub active: bool,
}

/// A puff of tyre smoke.
#[derive(Clone, Copy, Debug, Default)]
pub struct Puff {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Counts down to zero; the puff is dead at or below it.
    pub life: f32,
}

impl Puff {
    /// Fades linearly over the puff's life.
    #[inline]
    pub fn alpha(&self) -> f32 {
        let k = if self.life > 0.0 {
            self.life / PUFF_LIFE
        } else {
            0.0
        };
        SKID_ALPHA * k
    }

    /// Expands from 1.2 to 4.6 as it fades.
    #[inline]
    pub fn scale(&self) -> f32 {
        let k = if self.life > 0.0 {
            self.life / PUFF_LIFE
        } else {
            0.0
        };
        1.2 + (1.0 - k) * 3.4
    }
}

pub struct Effects {
    skids: [Skid; MAX_SKIDS],
    skid_next: usize,
    puffs: [Puff; MAX_PUFFS],
    puff_next: usize,
    /// Deterministic, so headless captures stay comparable between runs.
    rng: u32,
}

impl Default for Effects {
    fn default() -> Self {
        Self::new()
    }
}

impl Effects {
    pub const fn new() -> Self {
        Effects {
            skids: [Skid {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                yaw: 0.0,
                stretch: 1.0,
                active: false,
            }; MAX_SKIDS],
            skid_next: 0,
            puffs: [Puff {
                x: 0.0,
                y: 0.0,
                z: 0.0,
                life: 0.0,
            }; MAX_PUFFS],
            puff_next: 0,
            rng: 0x9E37_79B9,
        }
    }

    pub fn reset(&mut self) {
        for s in self.skids.iter_mut() {
            s.active = false;
        }
        for p in self.puffs.iter_mut() {
            p.life = 0.0;
        }
        self.skid_next = 0;
        self.puff_next = 0;
    }

    #[inline]
    pub fn skids(&self) -> &[Skid; MAX_SKIDS] {
        &self.skids
    }

    #[inline]
    pub fn puffs(&self) -> &[Puff; MAX_PUFFS] {
        &self.puffs
    }

    pub fn live_skids(&self) -> usize {
        self.skids.iter().filter(|s| s.active).count()
    }

    pub fn live_puffs(&self) -> usize {
        self.puffs.iter().filter(|p| p.life > 0.0).count()
    }

    /// 0.0–1.0, xorshift so it behaves the same on every run and every machine.
    fn next_unit(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng >> 8) as f32 / (1 << 24) as f32
    }

    /// Lays a mark under each rear wheel. Called per substep while the car is sliding.
    pub fn emit_skids(&mut self, car: &CarState, shape: &CarShape, speed: f32) {
        let (s, c) = (sin(car.yaw), cos(car.yaw));
        let stretch = crate::math::max(1.0, speed * 0.06);

        for side in [shape.rear_hub_x, -shape.rear_hub_x] {
            let mark = &mut self.skids[self.skid_next % MAX_SKIDS];
            self.skid_next = self.skid_next.wrapping_add(1);
            *mark = Skid {
                x: car.x + side * c + shape.rear_hub_z * s,
                y: car.y + 0.05,
                z: car.z - side * s + shape.rear_hub_z * c,
                yaw: car.yaw,
                stretch,
                active: true,
            };
        }
    }

    /// Puffs smoke from just behind the car, most but not all of the time.
    pub fn emit_smoke(&mut self, car: &CarState) {
        if self.next_unit() > 0.55 {
            return;
        }
        let (s, c) = (sin(car.yaw), cos(car.yaw));
        let jitter_x = self.next_unit() - 0.5;
        let jitter_z = self.next_unit() - 0.5;

        let puff = &mut self.puffs[self.puff_next % MAX_PUFFS];
        self.puff_next = self.puff_next.wrapping_add(1);
        *puff = Puff {
            x: car.x - 1.4 * s + jitter_x,
            y: car.y + 0.4,
            z: car.z - 1.4 * c + jitter_z,
            life: PUFF_LIFE,
        };
    }

    /// Ages the smoke. Called once per rendered frame, not per substep.
    pub fn update(&mut self, dt: f32) {
        for p in self.puffs.iter_mut() {
            if p.life <= 0.0 {
                continue;
            }
            p.life -= dt;
            if p.life <= 0.0 {
                p.life = 0.0;
                continue;
            }
            p.y += dt * 1.4;
        }
    }
}
