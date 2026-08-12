//! Loading compiled cars off the memory stick.
//!
//! A car is a file, not code. That is the whole point of the pipeline: the model is a build
//! artifact that a person can recompile with a different triangle budget and copy onto the stick,
//! and the console binary does not change. So this reads a `.azcar` into a static arena, hands it
//! to `azcar::Car` to be checked over once, and keeps the result — after which drawing the car is
//! pointing the GE at bytes that are already in the layout it wants.
//!
//! There is no allocator here and there is not going to be one, so the arena is a fixed static and
//! cars are bump-allocated out of it 16-byte aligned. That alignment is not a nicety: the vertex
//! arrays are read in place by the GE, which fetches them by DMA.
//!
//! Every failure is reported rather than worked around. A missing or stale car is a car that
//! cannot be drawn, and the alternative to saying so is a race that starts with an invisible
//! vehicle.

use angle_zero::azcar::{self, Car};
use psp::sys::{self, IoOpenFlags, IoWhence};

/// Where compiled cars live on the stick.
///
/// One absolute path for every build rather than one relative to whichever slot is running, so the
/// release build and the devtools build in `AngleZeroDev` read the same files and there is only
/// ever one copy of a car on the stick. It is under the release slot because that is where
/// unzipping the release archive puts it, and `ms0:/ANGLEZERO/` is already spoken for: that is the
/// diagnostics dump, and the release check refuses to package a build that mentions it.
pub const DIR: &str = "ms0:/PSP/GAME/AngleZero/CARS/";

/// Room for every car that can be resident at once. The E36 is 239 KB at 15k triangles; four cars
/// at the 10k target is comfortably inside this, and it is a fifth of one per cent of the machine.
const ARENA_BYTES: usize = 768 * 1024;
const MAX_CARS: usize = 4;
/// Longest path this will assemble, including the NUL.
const PATH_MAX: usize = 96;

#[repr(C, align(16))]
struct Arena([u8; ARENA_BYTES]);

static mut ARENA: Arena = Arena([0; ARENA_BYTES]);
static mut USED: usize = 0;
static mut CARS: [Option<Car<'static>>; MAX_CARS] = [None, None, None, None];
static mut COUNT: usize = 0;

/// Why a car could not be loaded. All of these end with no car rather than a wrong one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// No such file. Almost always a car that was never copied onto the stick.
    Missing,
    /// The file is larger than the arena, or the arena is full.
    NoRoom,
    /// The read stopped early.
    Short,
    /// More cars than there are slots.
    TooMany,
    /// The path does not fit the fixed-size buffer this assembles it in.
    NameTooLong,
    /// It is a file, but not one this build can draw.
    Format(azcar::Error),
}

impl LoadError {
    /// One line, for a screen with room for one line.
    pub fn message(self) -> &'static str {
        match self {
            LoadError::Missing => "car asset not found on the memory stick",
            LoadError::NoRoom => "car asset is too large to load",
            LoadError::Short => "car asset could not be read in full",
            LoadError::TooMany => "too many cars loaded",
            LoadError::NameTooLong => "car asset name is too long",
            LoadError::Format(e) => e.message(),
        }
    }
}

/// Loads `<DIR><name>` and returns the slot it went into.
///
/// `name` is the file's name only — the directory is this module's business, so that no caller has
/// to know where cars live, and a car can be named by a vehicle definition rather than by a path.
pub fn load(name: &str) -> Result<usize, LoadError> {
    let mut path = [0u8; PATH_MAX];
    let written = join(&mut path, DIR, name)?;

    unsafe {
        if COUNT >= MAX_CARS {
            return Err(LoadError::TooMany);
        }

        let fd = sys::sceIoOpen(path[..written].as_ptr(), IoOpenFlags::RD_ONLY, 0o777);
        if fd.0 < 0 {
            return Err(LoadError::Missing);
        }
        let size = sys::sceIoLseek(fd, 0, IoWhence::End);
        sys::sceIoLseek(fd, 0, IoWhence::Set);
        if size <= 0 {
            sys::sceIoClose(fd);
            return Err(LoadError::Short);
        }
        let size = size as usize;

        // Bump to the next 16-byte line before reserving, so every car's sections keep the
        // alignment the format promised them.
        let start = (USED + 15) & !15;
        if start + size > ARENA_BYTES {
            sys::sceIoClose(fd);
            return Err(LoadError::NoRoom);
        }

        let base = &raw mut ARENA as *mut u8;
        let read = sys::sceIoRead(fd, base.add(start) as *mut _, size as u32);
        sys::sceIoClose(fd);
        if read < size as i32 {
            return Err(LoadError::Short);
        }

        // The GE reads the vertex array straight out of here by DMA, and it does not go through
        // the data cache. Without this the first frames draw whatever was in memory before.
        sys::sceKernelDcacheWritebackInvalidateRange(base.add(start) as *const _, size as u32);

        let bytes: &'static [u8] = core::slice::from_raw_parts(base.add(start), size);
        let car = Car::parse(bytes).map_err(LoadError::Format)?;

        // Only now is the arena actually spent: a file that failed to parse leaves nothing behind,
        // so a stale car can be replaced by a good one without a reboot.
        USED = start + size;
        let slot = COUNT;
        CARS[slot] = Some(car);
        COUNT += 1;
        Ok(slot)
    }
}

/// A loaded car, or `None` for a slot nothing was loaded into.
pub fn get(slot: usize) -> Option<&'static Car<'static>> {
    unsafe { (*(&raw const CARS)).get(slot).and_then(|c| c.as_ref()) }
}

/// How many cars are resident.
pub fn count() -> usize {
    unsafe { COUNT }
}

/// How much of the arena is spent, for the diagnostics overlay.
#[cfg_attr(not(feature = "devtools"), allow(dead_code))]
pub fn used_bytes() -> usize {
    unsafe { USED }
}

#[cfg_attr(not(feature = "devtools"), allow(dead_code))]
pub const fn arena_bytes() -> usize {
    ARENA_BYTES
}

/// Joins the directory and a name into a NUL-terminated path, refusing rather than truncating.
///
/// A truncated path is a file that is not found, reported as a missing car — which sends whoever
/// is holding it looking on the memory stick for a file that is sitting right there.
fn join(out: &mut [u8; PATH_MAX], dir: &str, name: &str) -> Result<usize, LoadError> {
    let need = dir.len() + name.len() + 1;
    if need > PATH_MAX {
        return Err(LoadError::NameTooLong);
    }
    out[..dir.len()].copy_from_slice(dir.as_bytes());
    out[dir.len()..dir.len() + name.len()].copy_from_slice(name.as_bytes());
    out[need - 1] = 0;
    Ok(need)
}
