//! Engine, tyre, road and impact synthesis.
//!
//! There is no synthesiser on the PSP and no room to ship long samples, so the waveform is
//! generated a buffer at a time.
//!
//! The engine is built the way an exhaust makes its sound: a train of pressure pulses, one per
//! cylinder firing, rung through a few fixed resonances like those of the pipe. Its pitch is the
//! firing rate, which `crate::engine` works out through a gearbox, so it climbs through each gear
//! and drops at every shift rather than droning upward with speed. The four cylinders fire at
//! slightly different strengths, which is where the burble at low revs comes from, and pulses
//! under load are sharper and brighter than on the overrun. None of it is a raw oscillator: the
//! first version was a sawtooth, whose aliasing put a harsh buzz across the whole spectrum.
//!
//! Around it: a faint turbo whistle, tyre squeal as a narrow band of noise whose pitch wanders,
//! road rumble and wind that grow with speed, and a thud for a guard-rail hit.
//!
//! Kept in the core rather than the PSP shell so it can actually be checked. A screenshot says
//! nothing about sound, and clipped or runaway samples are painful rather than merely wrong.
//!
//! Nothing here calls a transcendental per sample: on the PSP one `sinf` in the inner loop would
//! cost more than the rest of the synthesiser. Filter coefficients are worked out once a buffer
//! and the whistle is a rotating phasor.

use crate::engine::{Engine, IDLE_RPM};
use crate::math::{clamp, cos, exp, min, sin, sqrt, PI, TAU};

pub const SAMPLE_RATE: u32 = 44_100;
/// Stereo frames per buffer. The PSP wants a multiple of 64.
pub const FRAMES_PER_BUFFER: usize = 1024;

const DT: f32 = 1.0 / SAMPLE_RATE as f32;

/// Everything the synthesiser needs for one buffer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Params {
    pub rpm: f32,
    /// 0–1, see `Engine::load`.
    pub load: f32,
    /// 0–1, see `Engine::boost`.
    pub boost: f32,
    /// Road speed, m/s, for the rumble and wind.
    pub speed: f32,
    /// Slip angle, radians. Zero unless the car is drifting.
    pub slip: f32,
    /// Scales the engine alone, 0–1. Zero silences it, which the tests use to hear the rest.
    pub engine_gain: f32,
}

impl Params {
    /// The engine ticking over, the car standing still.
    pub const IDLE: Params =
        Params { rpm: IDLE_RPM, load: 0.0, boost: 0.0, speed: 0.0, slip: 0.0, engine_gain: 1.0 };
}

impl Default for Params {
    fn default() -> Self {
        Params::IDLE
    }
}

/// Maps the engine and the car's motion onto synthesis parameters.
pub fn params_for(engine: &Engine, speed: f32, drifting: bool, slip_angle: f32) -> Params {
    Params {
        rpm: engine.rpm,
        load: engine.load,
        boost: engine.boost,
        speed,
        slip: if drifting { slip_angle } else { 0.0 },
        engine_gain: 1.0,
    }
}

/// Per-sample decay of the impact envelope: down to a thousandth after about 0.45 s.
const IMPACT_DECAY: f32 = 0.99965;
/// Slip angle where the squeal starts, and how much more it takes to reach full voice.
const SQUEAL_FROM: f32 = 0.14;
const SQUEAL_SPAN: f32 = 0.4;

/// The exhaust resonances: centre Hz, Q, and gain off and on load.
const RESONANCES: [(f32, f32, f32, f32); 3] =
    [(87.5, 1.3, 0.85, 1.05), (380.0, 2.35, 0.30, 0.725), (1375.0, 2.75, 0.045, 0.315)];
/// How much each cylinder's firing differs from the first. Uneven on purpose: that is the
/// burble.
const CYLINDERS: [f32; 4] = [1.0, 0.84, 0.94, 0.89];
/// How much noise roughens each pulse.
const RASP: f32 = 0.85;
/// The engine gets quieter toward idle, by this much of its level, and louder toward the
/// redline: otherwise the boom of the low revs outweighs the top end.
const LOW_TRIM: f32 = 0.4;
/// Revs over idle at which the engine reaches full voice.
const REV_SPAN: f32 = 6000.0;
/// Overall engine level before the master stage.
const ENGINE_LEVEL: f32 = 0.05;
/// Turbo whistle level at full boost.
const WHISTLE: f32 = 0.0108;

/// Drive into the soft limiter, and the level after it.
const MASTER_DRIVE: f32 = 0.45;
const MASTER_OUT: f32 = 0.66;

/// A one-pole low-pass coefficient for a corner at `fc` Hz.
fn one_pole(fc: f32) -> f32 {
    1.0 - exp(-TAU * fc * DT)
}

/// Two-pole resonator, peak gain about one.
#[derive(Clone, Copy)]
struct Resonator {
    a1: f32,
    a2: f32,
    b0: f32,
    y1: f32,
    y2: f32,
}

impl Resonator {
    const ZERO: Resonator = Resonator { a1: 0.0, a2: 0.0, b0: 0.0, y1: 0.0, y2: 0.0 };

    fn tune(&mut self, f: f32, q: f32) {
        let w = TAU * f * DT;
        let r = exp(-w / q * 0.5);
        self.a1 = 2.0 * r * cos(w);
        self.a2 = r * r;
        self.b0 = (1.0 - r * r) * 0.5;
    }

    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.a1 * self.y1 - self.a2 * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// State-variable band-pass. `f` is set by `tune`, so changing pitch costs a `sin` then rather
/// than every sample.
#[derive(Clone, Copy)]
struct BandPass {
    f: f32,
    damp: f32,
    lo: f32,
    band: f32,
}

impl BandPass {
    const ZERO: BandPass = BandPass { f: 0.0, damp: 1.0, lo: 0.0, band: 0.0 };

    fn tune(&mut self, fc: f32, q: f32) {
        self.f = 2.0 * sin(PI * fc * DT);
        self.damp = 1.0 / q;
    }

    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        self.lo += self.f * self.band;
        let hi = x - self.lo - self.band * self.damp;
        self.band += self.f * hi;
        self.band
    }
}

/// Stateful synthesiser. Everything carries across buffers: resetting a phase or a filter would
/// click audibly at every boundary, roughly 43 times a second.
pub struct Synth {
    noise: [u32; 3],
    /// rpm at the end of the last buffer, so the pitch glides across this one.
    rpm: f32,

    // The engine.
    firing_phase: f32,
    cylinder: usize,
    pulse: f32,
    pulse_rough: f32,
    resonators: [Resonator; 3],
    thump: f32,
    tone: f32,
    dc_x: f32,
    dc_y: f32,
    whistle: (f32, f32),

    // Tyres.
    squeal: [BandPass; 3],
    squeal_tone: [f32; 2],
    squeal_gain: f32,
    wobble: f32,
    wobble_left: f32,
    scrub: f32,

    // Road and wind, one of each per ear.
    road: [(f32, f32); 2],
    wind: [BandPass; 2],

    impact_env: f32,
    impact: [Resonator; 2],

    master: [f32; 2],
}

impl Default for Synth {
    fn default() -> Self {
        Self::new()
    }
}

impl Synth {
    pub const fn new() -> Self {
        Synth {
            noise: [0xBEEF_1234, 0x1234_5677, 0x7654_3211],
            rpm: IDLE_RPM,
            firing_phase: 0.0,
            cylinder: 0,
            pulse: 0.0,
            pulse_rough: 0.0,
            resonators: [Resonator::ZERO; 3],
            thump: 0.0,
            tone: 0.0,
            dc_x: 0.0,
            dc_y: 0.0,
            whistle: (1.0, 0.0),
            squeal: [BandPass::ZERO; 3],
            squeal_tone: [0.0; 2],
            squeal_gain: 0.0,
            wobble: 0.0,
            wobble_left: 0.0,
            scrub: 0.0,
            road: [(0.0, 0.0); 2],
            wind: [BandPass::ZERO; 2],
            impact_env: 0.0,
            impact: [Resonator::ZERO; 2],
            master: [0.0; 2],
        }
    }

    /// Thumps once — a guard-rail hit. `strength` is 0.0–1.0.
    ///
    /// Retriggering takes the louder of the two rather than adding, so grinding along a rail
    /// cannot stack impacts into a roar.
    pub fn trigger_impact(&mut self, strength: f32) {
        let s = clamp(strength, 0.0, 1.0);
        if s > self.impact_env {
            self.impact_env = s;
        }
    }

    #[inline]
    fn white(&mut self, k: usize) -> f32 {
        let mut n = self.noise[k];
        n ^= n << 13;
        n ^= n >> 17;
        n ^= n << 5;
        self.noise[k] = n;
        (n >> 8) as f32 / (1 << 23) as f32 - 1.0
    }

    /// Fills `out` with interleaved stereo samples.
    pub fn render(&mut self, p: &Params, out: &mut [i16]) {
        let frames = out.len() / 2;
        let load = clamp(p.load, 0.0, 1.0);

        // --- worked out once a buffer ---------------------------------------------------
        let mut gains = [0.0f32; 3];
        for (k, &(f, q, off, on)) in RESONANCES.iter().enumerate() {
            self.resonators[k].tune(f, q);
            gains[k] = (off + (on - off) * load) * 6.0;
        }
        // Sharper pulses under load, softer on the overrun.
        let pulse_decay = exp(-DT / (0.0045 - 0.0025 * load));
        let a_thump = one_pole(160.0);
        let a_tone = one_pole(1150.0 + 2500.0 * load);

        let whistle_hz = 2400.0 + p.boost * 2800.0;
        let (wc, ws) = (cos(TAU * whistle_hz * DT), sin(TAU * whistle_hz * DT));
        let whistle_gain = p.boost * p.boost * WHISTLE;
        // Keep the phasor on the unit circle, or rounding slowly grows or shrinks it.
        let m = sqrt(self.whistle.0 * self.whistle.0 + self.whistle.1 * self.whistle.1);
        self.whistle = (self.whistle.0 / m, self.whistle.1 / m);

        let squeal_want = clamp((p.slip - SQUEAL_FROM) / SQUEAL_SPAN, 0.0, 1.0) * min(p.speed / 12.0, 1.0);
        let squeal_rate = if squeal_want > self.squeal_gain { 18.0 } else { 7.0 };
        let squeal_step = 1.0 - exp(-squeal_rate * DT);
        let squeal_base = 880.0 + p.slip * 380.0;
        let a_squeal = one_pole(2600.0);
        let mut squeal_tuned = false;

        let road_gain = min(p.speed / 40.0, 1.0) * 0.3;
        let wind_gain = (p.speed / 45.0) * (p.speed / 45.0) * 0.05;
        let (a_road, a_road2) = (one_pole(140.0), one_pole(60.0));
        self.wind[0].tune(650.0, 0.6);
        self.wind[1].tune(720.0, 0.6);

        self.impact[0].tune(62.0, 3.0);
        self.impact[1].tune(410.0, 6.0);
        let a_master = one_pole(7000.0);

        let rpm0 = self.rpm;
        let rpm_step = (p.rpm - rpm0) / frames as f32;

        for (i, frame) in out.chunks_exact_mut(2).enumerate() {
            // --- engine -------------------------------------------------------------------
            let rpm = rpm0 + rpm_step * i as f32;
            // An inline four fires twice a revolution.
            self.firing_phase += rpm * (2.0 / 60.0) * DT;
            if self.firing_phase >= 1.0 {
                self.firing_phase -= 1.0;
                self.cylinder = (self.cylinder + 1) & 3;
                let jitter = 1.0 + self.white(0) * 0.07;
                self.pulse = (0.35 + 0.65 * load) * CYLINDERS[self.cylinder] * jitter;
                self.pulse_rough = self.white(0);
            }
            let x = self.pulse * (1.0 + (self.white(0) * 0.5 + self.pulse_rough * 0.3) * RASP);
            self.pulse *= pulse_decay;

            let rev = clamp((rpm - IDLE_RPM) / REV_SPAN, 0.0, 1.0);
            // The lowest resonance and the thump are what boom at idle, so they are trimmed most.
            let low = 1.0 - LOW_TRIM * (1.0 - rev);
            self.thump += a_thump * (x - self.thump);
            let mut y = self.thump * 1.3 * low;
            y += self.resonators[0].run(x) * gains[0] * low;
            y += self.resonators[1].run(x) * gains[1];
            y += self.resonators[2].run(x) * gains[2];
            self.tone += a_tone * (y - self.tone);
            // DC blocker: the pulses are all positive.
            let dc = self.tone - self.dc_x + 0.995 * self.dc_y;
            self.dc_x = self.tone;
            self.dc_y = dc;
            let mut engine = dc * ENGINE_LEVEL * (1.0 - LOW_TRIM * 0.7 + LOW_TRIM * 1.2 * rev);

            self.whistle = (
                self.whistle.0 * wc - self.whistle.1 * ws,
                self.whistle.0 * ws + self.whistle.1 * wc,
            );
            engine += self.whistle.1 * whistle_gain;
            engine *= p.engine_gain;

            // --- tyres --------------------------------------------------------------------
            let (l, r) = (self.white(1), self.white(2));
            self.squeal_gain += (squeal_want - self.squeal_gain) * squeal_step;
            let mut squeal = (0.0, 0.0);
            if self.squeal_gain > 0.001 {
                // The pitch wanders in small jumps, like a tyre on the edge of grip.
                self.wobble_left -= DT;
                if self.wobble_left <= 0.0 || !squeal_tuned {
                    if self.wobble_left <= 0.0 {
                        self.wobble_left = 0.06 + (self.white(1) + 1.0) * 0.06;
                        self.wobble = self.white(1) * 70.0;
                    }
                    let fc = squeal_base + self.wobble;
                    self.squeal[0].tune(fc, 14.0);
                    self.squeal[1].tune(fc * 1.012, 14.0);
                    self.squeal[2].tune(fc * 2.02, 10.0);
                    squeal_tuned = true;
                }
                let a = self.squeal[0].run(l);
                let b = self.squeal[1].run(r);
                let c = self.squeal[2].run((l + r) * 0.5) * 0.35;
                self.squeal_tone[0] += a_squeal * (a + c - self.squeal_tone[0]);
                self.squeal_tone[1] += a_squeal * (b + c - self.squeal_tone[1]);
                self.scrub += 0.05 * (l - self.scrub);
                let g = self.squeal_gain * self.squeal_gain * 0.55;
                let scrub = self.scrub * 0.24;
                squeal = ((self.squeal_tone[0] + scrub) * g, (self.squeal_tone[1] + scrub) * g);
            }

            // --- road and wind, separate noise each side so they sound wide ---------------
            self.road[0].0 += a_road * (l - self.road[0].0);
            self.road[0].1 += a_road2 * (self.road[0].0 - self.road[0].1);
            self.road[1].0 += a_road * (r - self.road[1].0);
            self.road[1].1 += a_road2 * (self.road[1].0 - self.road[1].1);
            let wind = (self.wind[0].run(l) * wind_gain, self.wind[1].run(r) * wind_gain);

            // --- a rail hit: a body thud and a shorter metallic edge ----------------------
            let mut impact = 0.0;
            if self.impact_env > 0.0001 {
                let n = self.white(0) * self.impact_env;
                impact = self.impact[0].run(n) * 3.5 + self.impact[1].run(n) * 0.8;
                self.impact_env *= IMPACT_DECAY;
            } else {
                self.impact_env = 0.0;
            }

            // --- master: a gentle low-pass and a soft limiter that only meets the peaks ---
            let left = engine + squeal.0 + self.road[0].1 * road_gain + wind.0 + impact;
            let right = engine + squeal.1 + self.road[1].1 * road_gain + wind.1 + impact;
            self.master[0] += a_master * (left * MASTER_DRIVE - self.master[0]);
            self.master[1] += a_master * (right * MASTER_DRIVE - self.master[1]);
            let soft = |x: f32| x / (1.0 + if x < 0.0 { -x } else { x });
            frame[0] = (soft(self.master[0]) * MASTER_OUT * 32_767.0) as i16;
            frame[1] = (soft(self.master[1]) * MASTER_OUT * 32_767.0) as i16;
        }
        self.rpm = p.rpm;

        // Let the squeal filters settle while silent, so the next slide does not start with a
        // thump of stale state.
        if self.squeal_gain <= 0.001 {
            for s in self.squeal.iter_mut() {
                s.lo *= 0.5;
                s.band *= 0.5;
            }
        }
    }
}
