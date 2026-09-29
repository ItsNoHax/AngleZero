//! Encrypts an `EBOOT.PBP` so it boots on official firmware, and proves the result before writing it.
//!
//! ```text
//! cargo run --release -p anglezero-eboot -- <plain EBOOT.PBP> <encrypted EBOOT.PBP>
//! ```
//!
//! Only `DATA.PSP` changes. The SFO, icon, background and music are carried over as they are.

use std::path::PathBuf;
use std::process::ExitCode;

use pspbuild::pbp::Pbp;
use pspbuild::{decrypt_prx, encrypt_prx, verify_prx, EncryptOptions};

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [input, output] = args.as_slice() else {
        eprintln!("usage: anglezero-eboot <plain EBOOT.PBP> <encrypted EBOOT.PBP>");
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
    let plain = std::fs::read(input).map_err(|e| format!("{}: {e}", input.display()))?;
    if !Pbp::is_pbp(&plain) {
        return Err(format!("{} is not an EBOOT.PBP", input.display()));
    }
    let module = Pbp::parse(&plain).map_err(|e| e.to_string())?.data_psp().to_vec();

    let encrypted = encrypt_prx(&plain, &EncryptOptions::default())
        .map_err(|e| format!("encrypting: {e}"))?
        .data;

    // verify_prx checks the header hash, both CMACs and the sizes, and decrypts the payload. It
    // does not insist the payload is the module that went in, or that it parses, so check both:
    // an EBOOT that decrypts to something else would pass and then fail on the console.
    let verified = verify_prx(&encrypted).map_err(|e| format!("verifying: {e}"))?;
    let Some(info) = &verified.module else {
        return Err("the encrypted EBOOT does not decrypt to a PSP module".into());
    };
    let recovered = decrypt_prx(&encrypted).map_err(|e| format!("decrypting: {e}"))?;
    if recovered != module {
        return Err("the encrypted EBOOT decrypts to a different module".into());
    }

    std::fs::write(output, &encrypted).map_err(|e| format!("{}: {e}", output.display()))?;

    for check in &verified.checks {
        println!("VALID: {check}");
    }
    println!("VALID: decrypts to the same {} bytes of {}", recovered.len(), info.name);
    if let Some(report) = &verified.snd0 {
        for warning in report.warnings() {
            println!("WARNING: SND0.AT3: {}", warning.message);
        }
    }
    println!("{} -> {} ({} -> {} bytes)", input.display(), output.display(), plain.len(), encrypted.len());
    Ok(())
}
