//! Engine and tyre synthesis.
//!
//! These are built from oscillators through a low-pass for the
//! engine, band-passed white noise for the squeal. There is no equivalent on the PSP and no room
//! to ship long samples, so the waveform is generated a buffer at a time.
//!
//! Kept in the core rather than the PSP shell so it can actually be checked. A screenshot says
//! nothing about sound, and clipped or runaway samples are painful rather than merely wrong.

use crate::math::{abs, min};

pub const SAMPLE_RATE: u32 = 44_100;
/// Stereo frames per buffer. The PSP wants a multiple of 64.
pub const FRAMES_PER_BUFFER: usize = 1024;

/// Everything the synthesiser needs for one buffer.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Params {
    /// Fundamental of the engine note, in Hz.
    pub engine_freq: f32,
    /// 0.0–1.0-ish; the gains are small, around 0.01–0.08.
    pub engine_gain: f32,
    /// Low-pass corner, in Hz.
    pub cutoff: f32,
    /// Tyre squeal level, zero unless sliding.
    pub squeal_gain: f32,
}

/// Maps car state onto synthesis parameters.
pub fn params_for(
    vx: f32,
    rpm: f32,
    throttle: f32,
    running: bool,
    drifting: bool,
    slip_angle: f32,
) -> Params {
    let speed = abs(vx);
    Params {
        engine_freq: min(420.0, 52.0 + speed * 6.2 + rpm * 40.0),
        engine_gain: if running {
            0.026 + throttle * 0.05
        } else {
            0.012
        },
        cutoff: 700.0 + speed * 40.0,
        squeal_gain: if drifting {
            // Clamped at zero as well as at the ceiling: just under the threshold this term
            // goes negative, which would invert the noise rather than silence it.
            let g = (slip_angle - 0.14) * 0.24;
            min(0.09, if g > 0.0 { g } else { 0.0 })
        } else {
            0.0
        },
    }
}

/// Per-sample decay of the impact envelope: down to a thousandth after about 0.3 s, which is
/// roughly how long a guard rail takes to stop ringing.
const IMPACT_DECAY: f32 = 0.99948;
/// Fundamental of the impact thud, in Hz. Low enough to feel like a body panel rather than a beep.
const IMPACT_FREQ: f32 = 78.0;

/// Stateful oscillator bank. Phase carries across buffers — resetting it would click audibly at
/// every boundary, roughly 43 times a second.
pub struct Synth {
    saw_phase: f32,
    sub_phase: f32,
    low_pass: f32,
    noise: u32,
    band_lo: f32,
    band_mid: f32,
    impact_env: f32,
    impact_phase: f32,
}

impl Default for Synth {
    fn default() -> Self {
        Self::new()
    }
}

impl Synth {
    pub const fn new() -> Self {
        Synth {
            saw_phase: 0.0,
            sub_phase: 0.0,
            low_pass: 0.0,
            noise: 0x1357_9BDF,
            band_lo: 0.0,
            band_mid: 0.0,
            impact_env: 0.0,
            impact_phase: 0.0,
        }
    }

    /// Thumps once — a guard-rail hit. `strength` is 0.0–1.0.
    ///
    /// Retriggering takes the louder of the two rather than adding, so grinding along a rail
    /// cannot stack impacts into a roar.
    pub fn trigger_impact(&mut self, strength: f32) {
        let s = crate::math::clamp(strength, 0.0, 1.0);
        if s > self.impact_env {
            self.impact_env = s;
            self.impact_phase = 0.0;
        }
    }

    fn white(&mut self) -> f32 {
        self.noise ^= self.noise << 13;
        self.noise ^= self.noise >> 17;
        self.noise ^= self.noise << 5;
        (self.noise >> 8) as f32 / (1 << 23) as f32 - 1.0
    }

    /// Fills `out` with interleaved stereo samples. The engine is centred, so both channels
    /// carry the same signal.
    pub fn render(&mut self, p: &Params, out: &mut [i16]) {
        let dt = 1.0 / SAMPLE_RATE as f32;
        let saw_step = p.engine_freq * dt;
        // The sub sits an octave down, as the design asks: a square an octave down.
        let sub_step = p.engine_freq * 0.5 * dt;

        // One-pole low-pass; the coefficient is the usual RC approximation.
        let rc = 1.0 / (crate::math::TAU * p.cutoff.max(20.0));
        let alpha = dt / (rc + dt);

        // Band-pass for the squeal, centred at 2100 Hz with a modest Q, as a state-variable
        // filter — cheap and stable at this sample rate.
        let f = 2.0 * crate::math::sin(crate::math::PI * 2100.0 * dt);
        let q = 1.0 / 1.4;

        for frame in out.chunks_exact_mut(2) {
            self.saw_phase += saw_step;
            if self.saw_phase >= 1.0 {
                self.saw_phase -= 1.0;
            }
            self.sub_phase += sub_step;
            if self.sub_phase >= 1.0 {
                self.sub_phase -= 1.0;
            }

            let saw = self.saw_phase * 2.0 - 1.0;
            let square = if self.sub_phase < 0.5 { -1.0 } else { 1.0 };
            let raw = saw * 0.6 + square * 0.4;

            self.low_pass += alpha * (raw - self.low_pass);
            let engine = self.low_pass * p.engine_gain;

            let squeal = if p.squeal_gain > 0.0 {
                let input = self.white();
                self.band_lo += f * self.band_mid;
                let high = input - self.band_lo - q * self.band_mid;
                self.band_mid += f * high;
                self.band_mid * p.squeal_gain
            } else {
                // Let the filter settle so it does not thump when the next slide starts.
                self.band_lo *= 0.9;
                self.band_mid *= 0.9;
                0.0
            };

            // A rail hit: a low tone with a noise edge, decaying fast.
            let impact = if self.impact_env > 0.0001 {
                self.impact_phase += IMPACT_FREQ * dt;
                if self.impact_phase >= 1.0 {
                    self.impact_phase -= 1.0;
                }
                let tone = crate::math::sin(self.impact_phase * crate::math::TAU);
                let edge = self.white();
                let hit = (tone * 0.75 + edge * 0.25) * self.impact_env * 0.16;
                self.impact_env *= IMPACT_DECAY;
                hit
            } else {
                self.impact_env = 0.0;
                0.0
            };

            // Headroom is deliberate: the gains sum well under 1.0, and the limiter is
            // only here so a future tweak cannot wrap the sample and produce a full-scale click.
            let mixed = (engine + squeal + impact) * 4.0;
            let clamped = crate::math::clamp(mixed, -0.98, 0.98);
            let sample = (clamped * 32_767.0) as i16;

            frame[0] = sample;
            frame[1] = sample;
        }
    }
}
