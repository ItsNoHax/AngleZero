//! Encodes the XMB background music as an `SND0.AT3` that loops, and refuses to write one that
//! pspbuild's own validator finds anything wrong with.
//!
//! ```text
//! cargo run --release -p anglezero-eboot --bin anglezero-snd0 -- <source.wav> <SND0.AT3>
//! ```
//!
//! pspbuild does the rest of what the XMB needs: stereo at 44.1 kHz, a low-pass at 15.5 kHz,
//! ATRAC3 LP4 in joint stereo with three QMF bands per frame, and RIFF with `fact` and `smpl`
//! chunks so the XMB repeats the track instead of stopping after one pass.

use std::path::PathBuf;
use std::process::ExitCode;

use pspbuild::audio::{make_snd0, Snd0Options};

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [input, output] = args.as_slice() else {
        eprintln!("usage: anglezero-snd0 <source audio> <SND0.AT3>");
        return ExitCode::from(2);
    };
    match run(input, output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("REFUSING: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(input: &PathBuf, output: &PathBuf) -> Result<(), String> {
    let source = std::fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let snd0 = make_snd0(&source, &Snd0Options::default()).map_err(|e| format!("encoding: {e}"))?;

    // A notice means pspbuild changed the music to fit, such as cutting a long input short. The
    // source is written as a seamless loop, so any change to its length breaks the seam.
    if !snd0.notices.is_empty() {
        return Err(snd0.notices.join("; "));
    }
    // Strict: warnings too. A missing loop point is only a warning, and it is the fault this
    // exists to fix.
    if let Some(problems) = snd0.report.failure_summary(true) {
        return Err(problems);
    }
    let Some(looped) = snd0.report.loop_points.filter(|l| l.play_count == 0) else {
        return Err("the encoded file does not loop forever".into());
    };

    std::fs::write(output, &snd0.data).map_err(|e| format!("{}: {e}", output.display()))?;
    println!(
        "{} -> {}: {} frames, {:.2} s, loop samples {} to {}, {} bytes",
        input.display(),
        output.display(),
        snd0.report.frames,
        snd0.report.duration_seconds(),
        looped.start,
        looped.end,
        snd0.data.len()
    );
    Ok(())
}
