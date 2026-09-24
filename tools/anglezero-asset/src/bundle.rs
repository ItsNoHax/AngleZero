//! Packing the compiled cars into a release EBOOT.
//!
//! The format is `angle_zero::bundle`'s; this is only the half that needs a filesystem. It appends
//! the bundle as the EBOOT's `DATA.PSAR`, which `cargo psp` leaves empty and points at the end of
//! the file — so the PBP header is left exactly as it was, and the firmware loads the same PRX
//! from the same place.
//!
//! Every car is checked before it goes in, with the console's own parser, and the finished bundle
//! is read back with the console's own index reader. A release is the one build nobody recompiles
//! a car for, so a stale or oversized car is refused here, by name, rather than on a player's PSP.

use std::path::{Path, PathBuf};

use angle_zero::azcar::{self, Car};
use angle_zero::bundle::{self, Index, Span};
use angle_zero::catalogue::{self, MAX_ENTRIES};

use crate::Result;

pub struct Options {
    pub eboot: PathBuf,
    pub output: PathBuf,
    pub cars: Vec<PathBuf>,
    pub quiet: bool,
}

pub fn run(opts: &Options) -> Result<()> {
    let pbp = std::fs::read(&opts.eboot)
        .map_err(|e| format!("could not read {}: {e}", opts.eboot.display()))?;
    let psar = bundle::psar_offset(&pbp)
        .map_err(|_| format!("{} is not an EBOOT.PBP", opts.eboot.display()))?;
    if psar != pbp.len() {
        return Err(format!(
            "{} already carries a DATA.PSAR ({} bytes); bundle the EBOOT `cargo psp` wrote",
            opts.eboot.display(),
            pbp.len().saturating_sub(psar)
        ));
    }

    let cars = read_cars(&opts.cars)?;
    let packed = pack(&cars)?;

    // Read back what is about to be written, through the same parser the console uses, and check
    // every car comes out as it went in.
    let index = Index::parse(&packed, packed.len())
        .map_err(|e| format!("the bundle did not read back: {e:?}"))?;
    for (name, bytes) in &cars {
        let span = index
            .find(name.as_bytes())
            .ok_or_else(|| format!("{name} is missing from the bundle it was packed into"))?;
        if &packed[span.offset..span.offset + span.size] != bytes.as_slice() {
            return Err(format!("{name} did not read back from the bundle intact"));
        }
    }

    let mut out = pbp;
    out.extend_from_slice(&packed);
    std::fs::write(&opts.output, &out)
        .map_err(|e| format!("could not write {}: {e}", opts.output.display()))?;

    if !opts.quiet {
        println!(
            "{}: {} cars, {:.1} MB packed after {} KB of game",
            opts.output.display(),
            cars.len(),
            packed.len() as f64 / (1024.0 * 1024.0),
            psar / 1024
        );
    }
    Ok(())
}

/// Reads each car and refuses any the console would.
fn read_cars(paths: &[PathBuf]) -> Result<Vec<(String, Vec<u8>)>> {
    if paths.is_empty() {
        return Err("no cars to bundle".into());
    }
    if paths.len() > MAX_ENTRIES {
        return Err(format!(
            "{} cars, and the title screen lists at most {MAX_ENTRIES}",
            paths.len()
        ));
    }

    let mut cars: Vec<(String, Vec<u8>)> = Vec::new();
    for path in paths {
        let name = file_name(path)?;
        if !catalogue::is_car_file(name.as_bytes()) {
            return Err(format!("{name} is not a .azcar"));
        }
        if name.len() > catalogue::NAME_MAX {
            return Err(format!(
                "{name} is longer than the {} characters a car's filename may be",
                catalogue::NAME_MAX
            ));
        }
        // FAT would not hold both, and the list would show one car twice.
        if let Some((other, _)) = cars.iter().find(|(n, _)| n.eq_ignore_ascii_case(&name)) {
            return Err(format!("{name} and {other} are the same name to the console"));
        }
        let bytes =
            std::fs::read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
        if bytes.len() > azcar::MAX_CAR_BYTES {
            return Err(format!(
                "{name} is {} KB, over the {} KB a car may be; recompile it",
                bytes.len() / 1024,
                azcar::MAX_CAR_BYTES / 1024
            ));
        }
        Car::parse(&bytes)
            .map_err(|e| format!("{name} would not load: {}; recompile it", e.message()))?;
        cars.push((name, bytes));
    }
    Ok(cars)
}

/// Lays the bundle out: index, then each car on an `ALIGN` boundary.
fn pack(cars: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut spans = Vec::with_capacity(cars.len());
    let mut at = bundle::index_bytes(cars.len());
    for (_, bytes) in cars {
        at = at.next_multiple_of(bundle::ALIGN);
        spans.push(Span {
            offset: at,
            size: bytes.len(),
        });
        at += bytes.len();
    }

    let mut out = Vec::with_capacity(at);
    out.extend_from_slice(&bundle::encode_header(cars.len()));
    for ((name, _), span) in cars.iter().zip(&spans) {
        let entry = bundle::encode_entry(name.as_bytes(), *span)
            .map_err(|_| format!("{name} cannot be written into the index"))?;
        out.extend_from_slice(&entry);
    }
    for ((_, bytes), span) in cars.iter().zip(&spans) {
        out.resize(span.offset, 0);
        out.extend_from_slice(bytes);
    }
    Ok(out)
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("{} has no usable filename", path.display()))
}
