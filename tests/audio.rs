//! Engine, tyre, road and impact synthesis.
//!
//! Testing it on the host is the only practical way to know it is right: an emulator screenshot
//! says nothing about sound, and clipped or runaway samples are painful rather than merely wrong.

use angle_zero::audio::{params_for, Params, Synth, FRAMES_PER_BUFFER, SAMPLE_RATE};
use angle_zero::engine::Engine;

fn render(synth: &mut Synth, p: &Params, buffers: usize) -> Vec<i16> {
    let mut all = Vec::new();
    let mut buf = vec![0i16; FRAMES_PER_BUFFER * 2];
    for _ in 0..buffers {
        synth.render(p, &mut buf);
        all.extend_from_slice(&buf);
    }
    all
}

fn rms(samples: &[i16]) -> f32 {
    let sum: f64 = samples.iter().map(|&s| (s as f64) * (s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

fn left(samples: &[i16]) -> Vec<f32> {
    samples.iter().step_by(2).map(|&s| s as f32).collect()
}

/// Energy at `freq` Hz, by Goertzel over `x`.
fn energy_at(x: &[f32], freq: f32) -> f32 {
    let w = 2.0 * std::f32::consts::PI * freq / SAMPLE_RATE as f32;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for &v in x {
        let s0 = v + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - c * s1 * s2) / x.len() as f32
}

fn engine_only(rpm: f32, load: f32) -> Params {
    Params { rpm, load, ..Params::IDLE }
}

/// Nothing but what is triggered: no engine, no motion.
const SILENT: Params = Params { engine_gain: 0.0, ..Params::IDLE };

// ------------------------------------------------------------------ parameters

#[test]
fn parameters_come_from_the_engine() {
    let mut e = Engine::new();
    for _ in 0..120 {
        e.update(20.0, 1.0, false, 0.0, 1.0 / 60.0);
    }
    let p = params_for(&e, 20.0, false, 0.0);
    assert_eq!((p.rpm, p.load, p.boost), (e.rpm, e.load, e.boost));
    assert_eq!(p.speed, 20.0);
    assert_eq!(p.engine_gain, 1.0);
}

#[test]
fn the_tyres_only_slip_while_drifting() {
    let e = Engine::new();
    assert_eq!(params_for(&e, 20.0, false, 0.5).slip, 0.0);
    assert_eq!(params_for(&e, 20.0, true, 0.5).slip, 0.5);
}

// ------------------------------------------------------------------ synthesis

#[test]
fn the_buffer_is_filled_completely() {
    let mut s = Synth::new();
    let mut buf = vec![7i16; FRAMES_PER_BUFFER * 2];
    s.render(&engine_only(3000.0, 1.0), &mut buf);
    assert!(buf.iter().any(|&v| v != 7), "render left the buffer untouched");
}

#[test]
fn output_never_clips_even_at_full_tilt() {
    // Everything at once: redline, full boost, a lurid slide at speed, and a rail hit.
    let p = Params { rpm: 7600.0, load: 1.0, boost: 1.0, speed: 58.0, slip: 1.2, engine_gain: 1.0 };
    let mut s = Synth::new();
    s.trigger_impact(1.0);
    let samples = render(&mut s, &p, 40);
    let peak = samples.iter().map(|&v| v.unsigned_abs()).max().unwrap();
    assert!(peak < 32_700, "peak sample {peak} is at or beyond the i16 rail — this would audibly clip");
    assert!(peak > 1_000, "at full tilt something should actually be audible");
}

#[test]
fn an_untriggered_silent_synth_stays_silent() {
    let mut s = Synth::new();
    assert!(rms(&render(&mut s, &SILENT, 4)) < 1.0);
}

#[test]
fn the_engine_idles_audibly() {
    let mut s = Synth::new();
    let r = rms(&render(&mut s, &Params::IDLE, 16));
    assert!(r > 30.0, "idle is inaudible (RMS {r})");
}

#[test]
fn the_engine_note_is_the_firing_rate() {
    // An inline four fires twice a revolution: 3000 rpm is 100 Hz.
    let mut s = Synth::new();
    let x = left(&render(&mut s, &engine_only(3000.0, 1.0), 24));
    let x = &x[x.len() / 3..];
    let on = energy_at(x, 100.0);
    for off in [70.0, 130.0, 160.0] {
        let e = energy_at(x, off);
        assert!(on > e * 10.0, "100 Hz carries {on}, {off} Hz carries {e}");
    }
}

#[test]
fn opening_the_throttle_makes_it_louder_and_brighter() {
    let quiet = rms(&render(&mut Synth::new(), &engine_only(4000.0, 0.0), 16));
    let loud = rms(&render(&mut Synth::new(), &engine_only(4000.0, 1.0), 16));
    assert!(loud > quiet * 1.4, "throttle barely changed the level: {quiet} -> {loud}");
}

#[test]
fn low_revs_do_not_drown_the_high_ones() {
    // The complaint that shaped the mix: the low revs sounded too loud against the top end. The
    // mix that fixed it by ear still has 2500 rpm a little ahead of 7000, so this holds it there
    // rather than letting it drift back.
    // Measured above the bass, which RMS overweights against the ear.
    let above_bass = |rpm: f32| {
        let x = left(&render(&mut Synth::new(), &engine_only(rpm, 1.0), 24));
        let (mut lp, mut sum) = (0.0f32, 0.0f64);
        let a = 1.0 - (-2.0 * std::f32::consts::PI * 250.0 / SAMPLE_RATE as f32).exp();
        for &v in &x[x.len() / 3..] {
            lp += a * (v - lp);
            sum += ((v - lp) as f64).powi(2);
        }
        (sum / (x.len() * 2 / 3) as f64).sqrt() as f32
    };
    let (low, high) = (above_bass(2500.0), above_bass(7000.0));
    assert!(high > low * 0.7, "2500 rpm at {low} drowns 7000 rpm at {high}");
}

#[test]
fn the_waveform_has_no_buzz_above_the_mix() {
    // The old sawtooth aliased, and put energy right up to the top of the spectrum.
    let mut s = Synth::new();
    let x = left(&render(&mut s, &engine_only(7000.0, 1.0), 24));
    let x = &x[x.len() / 3..];
    let body = energy_at(x, 233.3);
    for hf in [12_000.0, 15_000.0, 18_000.0] {
        let e = energy_at(x, hf);
        assert!(e < body * 1e-4, "{hf} Hz carries {e} against {body} in the note");
    }
}

#[test]
fn the_pitch_glides_across_a_buffer_rather_than_stepping() {
    // A jump in rpm between buffers must not produce a jump in the waveform at the join.
    let mut s = Synth::new();
    let mut buf = vec![0i16; FRAMES_PER_BUFFER * 2];
    s.render(&engine_only(3000.0, 1.0), &mut buf);
    let last = buf[buf.len() - 2] as i32;
    s.render(&engine_only(6000.0, 1.0), &mut buf);
    let first = buf[0] as i32;
    let inside = buf.chunks_exact(2).collect::<Vec<_>>().windows(2).map(|w| (w[1][0] as i32 - w[0][0] as i32).abs()).max().unwrap();
    assert!((first - last).abs() <= inside, "join steps {} against {inside} inside", (first - last).abs());
}

#[test]
fn road_and_wind_grow_with_speed_and_are_wide() {
    let still = Params { speed: 0.0, ..SILENT };
    let fast = Params { speed: 40.0, ..SILENT };
    let a = rms(&render(&mut Synth::new(), &still, 8));
    let samples = render(&mut Synth::new(), &fast, 8);
    assert!(rms(&samples) > a + 20.0, "no road noise at speed");
    let l: Vec<i16> = samples.iter().step_by(2).copied().collect();
    let r: Vec<i16> = samples.iter().skip(1).step_by(2).copied().collect();
    assert_ne!(l, r, "road noise should differ between the ears");
}

#[test]
fn the_engine_is_centred() {
    let mut s = Synth::new();
    let samples = render(&mut s, &engine_only(4000.0, 1.0), 4);
    let l: Vec<i16> = samples.iter().step_by(2).copied().collect();
    let r: Vec<i16> = samples.iter().skip(1).step_by(2).copied().collect();
    assert_eq!(l, r);
    assert!(rms(&r) > 1.0);
}

#[test]
fn synthesis_is_deterministic() {
    let p = Params { rpm: 5000.0, load: 1.0, boost: 0.6, speed: 20.0, slip: 0.4, engine_gain: 1.0 };
    let a = render(&mut Synth::new(), &p, 4);
    let b = render(&mut Synth::new(), &p, 4);
    assert_eq!(a, b);
}

// ------------------------------------------------------------------ tyres

#[test]
fn the_tyres_squeal_only_past_the_slip_threshold() {
    let grip = Params { speed: 20.0, slip: 0.1, ..SILENT };
    let slide = Params { speed: 20.0, slip: 0.5, ..SILENT };
    let a = rms(&render(&mut Synth::new(), &grip, 12));
    let b = rms(&render(&mut Synth::new(), &slide, 12));
    assert!(b > a * 2.0, "slide at {b} barely louder than grip at {a}");
}

#[test]
fn the_squeal_is_a_narrow_tone_not_a_hiss() {
    let p = Params { speed: 20.0, slip: 0.5, ..SILENT };
    let mut s = Synth::new();
    let x = left(&render(&mut s, &p, 24));
    let x = &x[x.len() / 3..];
    // Centred near 880 + 0.5 * 380 Hz, give or take its wander.
    let tone = energy_at(x, 1070.0).max(energy_at(x, 1040.0)).max(energy_at(x, 1100.0));
    let hiss = energy_at(x, 6000.0);
    assert!(tone > hiss * 100.0, "tone {tone} against {hiss} at 6 kHz");
}

// ------------------------------------------------------------------ rail impacts

#[test]
fn a_rail_hit_makes_a_noise() {
    let mut s = Synth::new();
    s.trigger_impact(1.0);
    let hit = rms(&render(&mut s, &SILENT, 2));
    assert!(hit > 50.0, "the impact was inaudible (RMS {hit})");
}

#[test]
fn the_thud_dies_away() {
    let mut s = Synth::new();
    s.trigger_impact(1.0);
    let early = rms(&render(&mut s, &SILENT, 2));
    let _ = render(&mut s, &SILENT, 10);
    let late = rms(&render(&mut s, &SILENT, 2));
    assert!(late < early * 0.1, "impact rang on: {early} -> {late}");
    let _ = render(&mut s, &SILENT, 30);
    let eventually = rms(&render(&mut s, &SILENT, 2));
    assert!(eventually < 1.0, "impact never fell silent (RMS {eventually})");
}

#[test]
fn a_harder_hit_is_louder() {
    let mut soft = Synth::new();
    soft.trigger_impact(0.25);
    let a = rms(&render(&mut soft, &SILENT, 2));
    let mut hard = Synth::new();
    hard.trigger_impact(1.0);
    let b = rms(&render(&mut hard, &SILENT, 2));
    assert!(b > a * 2.0, "hit strength barely mattered: {a} vs {b}");
}

#[test]
fn grinding_along_a_rail_cannot_stack_into_a_roar() {
    let p = Params { rpm: 7600.0, load: 1.0, boost: 1.0, speed: 40.0, slip: 1.0, engine_gain: 1.0 };
    let mut s = Synth::new();
    let mut peak = 0u16;
    for _ in 0..60 {
        s.trigger_impact(1.0);
        let mut buf = vec![0i16; FRAMES_PER_BUFFER * 2];
        s.render(&p, &mut buf);
        peak = peak.max(buf.iter().map(|&v| v.unsigned_abs()).max().unwrap());
    }
    assert!(peak < 32_700, "repeated impacts clipped at {peak}");
}
