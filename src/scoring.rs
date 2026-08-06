//! Drift scoring.
//!
//! Points accrue only while the car is genuinely sideways and moving. Sustained sliding builds a
//! multiplier; touching a rail throws it away, which is what makes the guard rails matter.

use crate::math::{max, min};

/// How long a combo survives after the slide stops.
pub const COMBO_HOLD: f32 = 1.15;
/// Drift points needed to step the multiplier up.
pub const CHUNK_PER_COMBO: f32 = 380.0;
/// Slip angle at which a slide starts counting. Mirrors `vehicle::DRIFT_SLIP`, re-exported
/// here because scoring is where the threshold is meaningful to a reader.
pub const DRIFT_SLIP_THRESHOLD: f32 = crate::vehicle::DRIFT_SLIP;
/// The multiplier stops here.
pub const COMBO_MAX: u32 = 9;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScoreEvent {
    /// The multiplier stepped up this update.
    pub combo_up: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct Scoring {
    pub score: f32,
    pub combo: u32,
    /// Time left before the combo lapses.
    pub combo_timer: f32,
    /// Progress toward the next multiplier step.
    pub chunk: f32,
    pub best_combo: u32,
}

impl Default for Scoring {
    fn default() -> Self {
        Self::new()
    }
}

impl Scoring {
    pub const fn new() -> Self {
        Scoring {
            score: 0.0,
            combo: 1,
            combo_timer: 0.0,
            chunk: 0.0,
            best_combo: 1,
        }
    }

    pub fn reset(&mut self) {
        *self = Scoring::new();
    }

    /// One substep of scoring. `drifting` comes from the vehicle, which already applies the
    /// slip-angle, speed and lateral-offset thresholds.
    pub fn update(&mut self, drifting: bool, speed: f32, slip_angle: f32, dt: f32) -> ScoreEvent {
        let mut event = ScoreEvent::default();

        if drifting {
            let gain = speed * (slip_angle * 3.0) * 8.0 * dt;
            self.chunk += gain;
            self.score += gain * self.combo as f32;
            self.combo_timer = COMBO_HOLD;

            if self.chunk > CHUNK_PER_COMBO {
                self.chunk = 0.0;
                self.combo = min_u32(COMBO_MAX, self.combo + 1);
                event.combo_up = true;
            }
            self.best_combo = max_u32(self.best_combo, self.combo);
        } else {
            self.combo_timer -= dt;
            if self.combo_timer <= 0.0 {
                self.combo_timer = 0.0;
                if self.combo > 1 {
                    self.combo = 1;
                    self.chunk = 0.0;
                }
            }
        }

        event
    }

    /// Hitting a rail drops the multiplier at once. Returns whether it is worth telling the
    /// player — announcing a lost combo the player never had is just noise.
    pub fn on_wall_tap(&mut self) -> bool {
        let announce = self.combo_timer > 0.0;
        self.combo_timer = 0.0;
        self.combo = 1;
        self.chunk = 0.0;
        announce
    }

    /// The combo timer bar, 0.0–1.0.
    pub fn combo_fraction(&self) -> f32 {
        max(0.0, min(1.0, self.combo_timer / COMBO_HOLD))
    }
}

#[inline]
fn min_u32(a: u32, b: u32) -> u32 {
    if a < b {
        a
    } else {
        b
    }
}

#[inline]
fn max_u32(a: u32, b: u32) -> u32 {
    if a > b {
        a
    } else {
        b
    }
}
