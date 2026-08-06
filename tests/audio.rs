//! Engine and tyre synthesis.
//!
//! There is no synthesiser to lean on here, so the waveform is generated a sample at a time.
//! Testing it on the host is the only practical way to know
//! it is right: an emulator screenshot says nothing about sound, and clipped or runaway samples
//! are painful rather than merely wrong.

use angle_zero::audio::{params_for, Params, Synth, FRAMES_PER_BUFFER, SAMPLE_RATE};

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

fn idle() -> Params {
    params_for(0.0, 0.1, 0.0, true, false, 0.0)
}

// ------------------------------------------------------------------ parameters

#[test]
fn engine_pitch_rises_with_speed_and_tops_out() {
    let slow = params_for(0.0, 0.1, 0.0, true, false, 0.0);
    let fast = params_for(30.0, 0.5, 1.0, true, false, 0.0);
    assert!(fast.engine_freq > slow.engine_freq);
    // f = min(420, 52 + |vx|*6.2 + rpm*40)
    assert!((slow.engine_freq - (52.0 + 0.1 * 40.0)).abs() < 1e-3);
    assert!(
        params_for(200.0, 1.0, 1.0, true, false, 0.0).engine_freq <= 420.0,
        "engine pitch must be capped"
    );
}

#[test]
fn the_engine_is_quieter_when_the_game_is_not_running() {
    let running = params_for(10.0, 0.5, 0.0, true, false, 0.0);
    let idling = params_for(10.0, 0.5, 0.0, false, false, 0.0);
    assert!((running.engine_gain - 0.026).abs() < 1e-4);
    assert!((idling.engine_gain - 0.012).abs() < 1e-4);
}

#[test]
fn opening_the_throttle_makes_it_louder() {
    let off = params_for(10.0, 0.5, 0.0, true, false, 0.0);
    let on = params_for(10.0, 0.5, 1.0, true, false, 0.0);
    assert!((on.engine_gain - (0.026 + 0.05)).abs() < 1e-4);
    assert!(on.engine_gain > off.engine_gain);
}

#[test]
fn the_filter_opens_up_with_speed() {
    let slow = params_for(0.0, 0.1, 0.0, true, false, 0.0);
    let fast = params_for(30.0, 0.5, 1.0, true, false, 0.0);
    assert!((slow.cutoff - 700.0).abs() < 1e-3);
    assert!((fast.cutoff - (700.0 + 30.0 * 40.0)).abs() < 1e-3);
    assert!(fast.cutoff > slow.cutoff);
}

#[test]
fn the_tyres_only_squeal_while_drifting() {
    assert_eq!(params_for(20.0, 0.5, 1.0, true, false, 0.5).squeal_gain, 0.0);
    let sliding = params_for(20.0, 0.5, 1.0, true, true, 0.5);
    assert!(sliding.squeal_gain > 0.0);
}

#[test]
fn squeal_grows_with_slip_and_is_capped() {
    let gentle = params_for(20.0, 0.5, 1.0, true, true, 0.20);
    let lurid = params_for(20.0, 0.5, 1.0, true, true, 0.60);
    assert!(lurid.squeal_gain > gentle.squeal_gain);
    // min(0.09, (slip - 0.14) * 0.24)
    assert!((gentle.squeal_gain - (0.20 - 0.14) * 0.24).abs() < 1e-4);
    assert!(params_for(20.0, 0.5, 1.0, true, true, 5.0).squeal_gain <= 0.09 + 1e-6);
}

#[test]
fn slip_below_the_threshold_never_produces_negative_gain() {
    // (slip - 0.14) goes negative just under the threshold, which would invert the noise.
    let p = params_for(20.0, 0.5, 1.0, true, true, 0.05);
    assert!(p.squeal_gain >= 0.0, "negative squeal gain {}", p.squeal_gain);
}

// ------------------------------------------------------------------ synthesis

#[test]
fn the_buffer_is_filled_completely() {
    let mut s = Synth::new();
    let mut buf = vec![7i16; FRAMES_PER_BUFFER * 2];
    s.render(&idle(), &mut buf);
    assert!(
        buf.iter().any(|&v| v != 7),
        "render left the buffer untouched"
    );
}

#[test]
fn output_never_clips_even_at_full_tilt() {
    // Everything at once: loudest engine, loudest squeal.
    let p = params_for(60.0, 1.0, 1.0, true, true, 2.0);
    let mut s = Synth::new();
    let samples = render(&mut s, &p, 20);
    let peak = samples.iter().map(|&v| v.unsigned_abs()).max().unwrap();
    assert!(
        peak < 32_700,
        "peak sample {peak} is at or beyond the i16 rail — this would audibly clip"
    );
    assert!(peak > 1_000, "at full tilt something should actually be audible");
}

#[test]
fn silence_in_gives_silence_out() {
    let p = Params {
        engine_freq: 200.0,
        engine_gain: 0.0,
        cutoff: 1_000.0,
        squeal_gain: 0.0,
    };
    let mut s = Synth::new();
    let samples = render(&mut s, &p, 4);
    assert!(rms(&samples) < 1.0, "expected silence, got RMS {}", rms(&samples));
}

#[test]
fn a_louder_engine_is_measurably_louder() {
    let quiet = params_for(10.0, 0.5, 0.0, true, false, 0.0);
    let loud = params_for(10.0, 0.5, 1.0, true, false, 0.0);
    let a = rms(&render(&mut Synth::new(), &quiet, 8));
    let b = rms(&render(&mut Synth::new(), &loud, 8));
    assert!(b > a * 1.3, "throttle barely changed the level: {a} -> {b}");
}

#[test]
fn the_engine_note_matches_the_requested_frequency() {
    // Count zero crossings of the low-passed tone and compare against the fundamental. The sub
    // is an octave down, so the combined waveform crosses at the sub's rate.
    let p = Params {
        engine_freq: 200.0,
        engine_gain: 0.08,
        cutoff: 6_000.0,
        squeal_gain: 0.0,
    };
    let mut s = Synth::new();
    let samples = render(&mut s, &p, 16);
    let mono: Vec<i16> = samples.iter().step_by(2).copied().collect();

    let mut crossings = 0;
    for w in mono.windows(2) {
        if (w[0] < 0) != (w[1] < 0) {
            crossings += 1;
        }
    }
    let seconds = mono.len() as f32 / SAMPLE_RATE as f32;
    let measured = crossings as f32 / 2.0 / seconds;
    // The sawtooth is the fundamental and dominates the crossings; the octave-down square
    // shifts the waveform without adding crossings of its own.
    assert!(
        (measured - 200.0).abs() < 20.0,
        "measured {measured} Hz, expected roughly the 200 Hz fundamental"
    );
}

#[test]
fn the_engine_carries_an_octave_down_layer() {
    // The design asks for a sub an octave below the fundamental. With one present the waveform
    // repeats every *two* cycles of the fundamental, not every one.
    let freq = 200.0;
    let p = Params {
        engine_freq: freq,
        engine_gain: 0.08,
        cutoff: 8_000.0,
        squeal_gain: 0.0,
    };
    let mut s = Synth::new();
    let samples = render(&mut s, &p, 8);
    let mono: Vec<i16> = samples.iter().step_by(2).copied().collect();

    let period = (SAMPLE_RATE as f32 / freq) as usize;
    let start = mono.len() / 2; // past any filter settling
    let diff = |lag: usize| -> f64 {
        (0..period)
            .map(|i| (mono[start + i] as f64 - mono[start + i + lag] as f64).abs())
            .sum()
    };
    // One period apart the square has flipped, so the signal differs; two periods apart it
    // lines back up.
    assert!(
        diff(period) > diff(period * 2) * 2.0,
        "no octave-down layer: one-period difference {} vs two-period {}",
        diff(period),
        diff(period * 2)
    );
}

#[test]
fn stereo_frames_are_written_to_both_ears() {
    let mut s = Synth::new();
    let samples = render(&mut s, &params_for(20.0, 0.5, 1.0, true, false, 0.0), 4);
    let left: Vec<i16> = samples.iter().step_by(2).copied().collect();
    let right: Vec<i16> = samples.iter().skip(1).step_by(2).copied().collect();
    assert_eq!(left, right, "the engine is centred, so both channels match");
    assert!(rms(&right) > 1.0);
}

#[test]
fn the_waveform_is_continuous_across_buffers() {
    // Phase has to carry over, or every buffer boundary is an audible click.
    let p = params_for(20.0, 0.5, 1.0, true, false, 0.0);
    let mut s = Synth::new();
    let samples = render(&mut s, &p, 8);

    let mut worst_step = 0i32;
    let mut worst_at = 0usize;
    for i in 1..samples.len() / 2 {
        let step = (samples[i * 2] as i32 - samples[(i - 1) * 2] as i32).abs();
        if step > worst_step {
            worst_step = step;
            worst_at = i;
        }
    }
    // A sawtooth resets once per cycle, so large steps are expected — but not specifically at
    // the buffer joins, which is what a phase reset would produce.
    for join in 1..8 {
        let i = join * FRAMES_PER_BUFFER;
        let step = (samples[i * 2] as i32 - samples[(i - 1) * 2] as i32).abs();
        assert!(
            step <= worst_step,
            "buffer join {join} steps by {step}, worse than anything inside a buffer \
             ({worst_step} at {worst_at}) — phase is being reset"
        );
    }
}

#[test]
fn synthesis_is_deterministic() {
    let p = params_for(20.0, 0.5, 1.0, true, true, 0.4);
    let a = render(&mut Synth::new(), &p, 4);
    let b = render(&mut Synth::new(), &p, 4);
    assert_eq!(a, b);
}

// ------------------------------------------------------------------ rail impacts

#[test]
fn a_rail_hit_makes_a_noise() {
    let silent = Params {
        engine_freq: 200.0,
        engine_gain: 0.0,
        cutoff: 1_000.0,
        squeal_gain: 0.0,
    };
    let mut s = Synth::new();
    // Nothing but the impact, so this measures the thud alone.
    s.trigger_impact(1.0);
    let hit = rms(&render(&mut s, &silent, 2));
    assert!(hit > 50.0, "the impact was inaudible (RMS {hit})");
}

#[test]
fn the_thud_dies_away() {
    let silent = Params {
        engine_freq: 200.0,
        engine_gain: 0.0,
        cutoff: 1_000.0,
        squeal_gain: 0.0,
    };
    let mut s = Synth::new();
    s.trigger_impact(1.0);
    // Equal-length windows, or the later one is just dominated by its own opening samples.
    let early = rms(&render(&mut s, &silent, 2));
    let _ = render(&mut s, &silent, 10);
    let late = rms(&render(&mut s, &silent, 2));
    assert!(late < early * 0.1, "impact rang on: {early} -> {late}");
    // And it settles to actual silence rather than to a floor.
    let _ = render(&mut s, &silent, 20);
    let eventually = rms(&render(&mut s, &silent, 2));
    assert!(eventually < 1.0, "impact never fell silent (RMS {eventually})");
}

#[test]
fn a_harder_hit_is_louder() {
    let silent = Params {
        engine_freq: 200.0,
        engine_gain: 0.0,
        cutoff: 1_000.0,
        squeal_gain: 0.0,
    };
    let mut soft = Synth::new();
    soft.trigger_impact(0.25);
    let a = rms(&render(&mut soft, &silent, 2));

    let mut hard = Synth::new();
    hard.trigger_impact(1.0);
    let b = rms(&render(&mut hard, &silent, 2));
    assert!(b > a * 2.0, "hit strength barely mattered: {a} vs {b}");
}

#[test]
fn grinding_along_a_rail_cannot_stack_into_a_roar() {
    // Wall taps arrive repeatedly while scraping. Retriggering must not accumulate.
    let p = params_for(40.0, 1.0, 1.0, true, true, 2.0);
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

#[test]
fn an_untriggered_synth_stays_silent() {
    let silent = Params {
        engine_freq: 200.0,
        engine_gain: 0.0,
        cutoff: 1_000.0,
        squeal_gain: 0.0,
    };
    let mut s = Synth::new();
    assert!(rms(&render(&mut s, &silent, 4)) < 1.0);
}
