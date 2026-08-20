//! `convert` — source model to compiled car.

use std::path::{Path, PathBuf};

use angle_zero::azcar::Car;

use crate::compile;
use crate::config::CarConfig;
use crate::extract;
use crate::Result;

pub struct Options {
    pub input: PathBuf,
    pub output: PathBuf,
    pub config: Option<PathBuf>,
    /// Overrides whatever the config says.
    pub triangles: Option<usize>,
    pub quiet: bool,
    /// Where to also write the packed texture as a PNG. Nothing reads it — it exists because a
    /// texture that is wrong is quicker to recognise by looking at it than by anything else.
    pub atlas: Option<PathBuf>,
}

pub fn run(options: &Options) -> Result<()> {
    let config = match &options.config {
        Some(path) => CarConfig::load(path)?,
        None => match beside(&options.input, &options.output) {
            Some(path) => CarConfig::load(&path)?,
            None => CarConfig::unconfigured(&stem(&options.input)),
        },
    };
    let budget = options.triangles.unwrap_or(config.triangles);

    // A config that names a different model than the one being compiled is either the wrong config
    // or a line that has gone stale, and both produce a car that is not what anybody asked for.
    // Refused rather than warned about: the conversion takes a minute, and every check downstream
    // of here is about the model rather than about whether it is the right model.
    if let Some(named) = &config.source {
        let given = options
            .input
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if named != &given {
            return Err(format!(
                "the config says this car is built from `{named}`, but `{given}` was passed. \
                 Either the wrong config, or `source` needs updating."
            ));
        }
    }

    let mut model = extract::load(&options.input)?;
    let mut compiled = compile::compile(&mut model, &config, budget)?;
    if let Some(path) = &options.atlas {
        image::save_buffer(
            path,
            &compiled.atlas,
            crate::texture::ATLAS as u32,
            crate::texture::ATLAS as u32,
            image::ColorType::Rgba8,
        )
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    }
    compiled.report.check(budget);

    // Read the file back through the runtime's own reader before it is written. The converter and
    // the console disagreeing about the format is the failure this pipeline exists to make
    // impossible, and it costs a millisecond to rule out here rather than on hardware.
    let car = Car::parse(&compiled.bytes).map_err(|e| {
        format!(
            "the compiled car does not pass the runtime's own check: {} ({e:?})",
            e.message()
        )
    })?;
    if car.triangle_count() != compiled.report.out_triangles {
        return Err(format!(
            "the compiled car reads back as {} triangles, not {}",
            car.triangle_count(),
            compiled.report.out_triangles
        ));
    }

    if let Some(dir) = options.output.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    std::fs::write(&options.output, &compiled.bytes)
        .map_err(|e| format!("could not write {}: {e}", options.output.display()))?;

    if !options.quiet {
        compiled.report.print();
        println!();
        println!(
            "Wrote {} ({} KB)",
            options.output.display(),
            compiled.bytes.len() / 1024
        );
    }
    Ok(())
}

/// Finds the config without being told where it is.
///
/// A source model and its compiled car are usually named differently — `bmw_3-series_e36.glb`
/// becomes `bmw_e36.azcar`, because one name came from a download and the other is the game's —
/// so `assets/configs/` is searched under both names before giving up and looking beside the
/// model. Naming one with `--config` always works.
fn beside(input: &Path, output: &Path) -> Option<PathBuf> {
    let configs = input.parent()?.parent()?.join("configs");
    let candidates = [
        configs.join(output.file_stem()?).with_extension("toml"),
        configs.join(input.file_stem()?).with_extension("toml"),
        input.with_extension("toml"),
    ];
    candidates.into_iter().find(|p| p.is_file())
}

fn stem(input: &Path) -> String {
    input
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "car".to_string())
}
