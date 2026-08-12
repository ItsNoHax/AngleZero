//! The AngleZero car asset compiler.
//!
//! Takes a glTF binary — a scanned or modelled car, typically a few hundred thousand triangles —
//! and compiles it into the `.azcar` the game loads. Every expensive or fiddly step lives here:
//! parsing glTF, flattening the node hierarchy, simplifying meshes, converting textures. The PSP
//! side never sees any of it, and never parses glTF.
//!
//! ```text
//! anglezero-asset inspect assets/source/bmw_3-series_e36.glb
//! anglezero-asset convert assets/source/bmw_3-series_e36.glb assets/compiled/bmw_e36.azcar
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

mod categorise;
mod compile;
mod config;
mod convert;
mod extract;
mod inspect;
mod mat;
mod model;
mod report;
mod simplify;
mod visibility;
mod wheels;

/// Anything the tool refuses to do, phrased for the person running it. Section 25 of the plan is
/// the rule here: fail loudly rather than write a broken asset.
pub type Error = String;
pub type Result<T> = std::result::Result<T, Error>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();

    let outcome = match refs.split_first() {
        Some((&"inspect", rest)) => run_inspect(rest),
        Some((&"convert", rest)) => run_convert(rest),
        Some((&"help", _)) | Some((&"--help", _)) | Some((&"-h", _)) | None => {
            usage();
            return ExitCode::SUCCESS;
        }
        Some((other, _)) => Err(format!("unknown command `{other}`")),
    };

    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ERROR: {e}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    println!("AngleZero car asset compiler");
    println!();
    println!("  anglezero-asset inspect [--deep] <model.glb>");
    println!("      Report what is inside a source model, without converting anything.");
    println!("      --deep also reads every vertex, and reports what only the data can say.");
    println!();
    println!("  anglezero-asset convert <model.glb> <car.azcar> [options]");
    println!("      Compile a source model into the car the game loads.");
    println!("      --config <car.toml>   which config to use, if not the one beside the model");
    println!("      --triangles <n>       triangle budget, overriding the config's");
    println!("      --quiet               write the file and print nothing");
}

fn run_convert(args: &[&str]) -> Result<()> {
    let mut paths: Vec<&str> = Vec::new();
    let mut config = None;
    let mut triangles = None;
    let mut quiet = false;

    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match *arg {
            "--config" => {
                config = Some(PathBuf::from(
                    *it.next().ok_or("--config needs a path")?,
                ))
            }
            "--triangles" => {
                let v = *it.next().ok_or("--triangles needs a number")?;
                triangles = Some(
                    v.replace([',', '_'], "")
                        .parse::<usize>()
                        .map_err(|_| format!("`{v}` is not a triangle count"))?,
                );
            }
            "--quiet" => quiet = true,
            other if other.starts_with("--") => return Err(format!("unknown option `{other}`")),
            other => paths.push(other),
        }
    }

    let [input, output] = paths[..] else {
        return Err("convert needs a source model and an output path".into());
    };
    convert::run(&convert::Options {
        input: PathBuf::from(input),
        output: PathBuf::from(output),
        config,
        triangles,
        quiet,
    })
}

fn run_inspect(args: &[&str]) -> Result<()> {
    let deep = args.contains(&"--deep");
    let paths: Vec<&&str> = args.iter().filter(|a| !a.starts_with("--")).collect();
    match paths[..] {
        [path] => inspect::run(std::path::Path::new(path), deep),
        [] => Err("inspect needs a path to a .glb".into()),
        _ => Err("inspect takes exactly one path".into()),
    }
}
