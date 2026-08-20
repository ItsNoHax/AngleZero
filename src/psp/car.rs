//! Loading compiled cars off the memory stick, one at a time.
//!
//! A car is a file, not code. That is the whole point of the pipeline: the model is a build
//! artifact that a person can recompile with a different triangle budget and copy onto the stick,
//! and the console binary does not change. So this reads a `.azcar` into a static slot, hands it
//! to `azcar::Car` to be checked over once, and keeps the result — after which drawing the car is
//! pointing the GE at bytes that are already in the layout it wants.
//!
//! What changed is how many of them are held at once. Every car on the stick used to be read at
//! boot, which made the number of cars the game offered a question about `.bss`: seven of them came
//! to 5.7 MB of a 6 MB arena and the eighth did not fit. Nothing in a run needs more than the one
//! car on screen, so now the stick holds the fleet and this holds the car — two slots of it, the
//! one being driven and the one arriving, so a stale or oversized file never evicts a good car.
//!
//! Arriving is the word: a car is the better part of a megabyte, and read whole it stops the title
//! screen for as long as the stick takes. Instead `begin` opens the file and `step` reads a chunk
//! per frame, so the screen keeps running while the car comes in. `hurry` finishes the job when the
//! player presses X and is done waiting.
//!
//! There is no allocator here and there is not going to be one, so the slots are fixed statics,
//! 16-byte aligned. That alignment is not a nicety: the vertex arrays are read in place by the GE,
//! which fetches them by DMA.
//!
//! Every failure is reported rather than worked around. A missing or stale car is a car that
//! cannot be drawn, and the alternative to saying so is a race that starts with an invisible
//! vehicle.

use angle_zero::azcar::{self, Car, Silhouette};
use angle_zero::catalogue::{self, Catalogue, DisplayName};
use angle_zero::stream::Progress;
use psp::sys::{self, IoOpenFlags, IoStatMode, IoWhence, SceUid};

/// Where compiled cars live on the stick.
///
/// One absolute path for every build rather than one relative to whichever slot is running, so the
/// release build and the devtools build in `AngleZeroDev` read the same files and there is only
/// ever one copy of a car on the stick. It is under the release slot because that is where
/// unzipping the release archive puts it, and `ms0:/ANGLEZERO/` is already spoken for: that is the
/// diagnostics dump, and the release check refuses to package a build that mentions it.
pub const DIR: &str = "ms0:/PSP/GAME/AngleZero/CARS/";

/// The most one car may be.
///
/// This is the limit now, and it is a much better one than the arena was: it is a property of a
/// single file that the asset compiler can check before anybody copies it anywhere, rather than a
/// property of the whole stick that only shows up as the eighth car refusing to appear.
///
/// 1.25 MB against the largest car this repo compiles at 920 KB — a third again of headroom, which
/// is a car with a bigger texture or a raised triangle budget, not a car that was never thought
/// about. `azcar::MAX_CAR_BYTES` is the same number, so the compiler and the console agree.
pub const SLOT_BYTES: usize = azcar::MAX_CAR_BYTES;

/// How many cars can be resident at once.
///
/// Two in a shipping build: the car being driven, and the one being read. That second slot is what
/// makes a failed load harmless — the bytes arriving never touch the car on screen, so a car that
/// turns out to be truncated leaves the player exactly where they were, with a message.
///
/// A devtools build keeps five, because `--mode 13` and `--mode 14` put four and eight cars on
/// screen to price what a field costs and a field of one repeated model prices nothing about
/// switching textures. Five slots is 6.25 MB, near enough what the old arena reserved, and it is
/// reserved only in a build that is never shipped.
const SLOTS: usize = if cfg!(feature = "devtools") { 5 } else { 2 };

/// How much to read per frame.
///
/// Measured on hardware, which is the only place it can be: under the headless emulator a "memory
/// stick" is a directory on an SSD and every chunk size looks free. On the console this reads
/// **consistently in 4,438 µs**, which is 7.4 MB/s and about 27% of a 60 Hz frame, leaving 12.2 ms
/// for the title screen to draw in. Consistent is the load-bearing word: a figure that wandered
/// would mean seeks behind it, and a worst case hiding behind an average.
///
/// So a car lands in about 29 frames, just under half a second for the largest in this repo. It
/// could be halved by doubling this — and should not be. Two chunks a frame would put 8.9 ms of
/// blocking read in front of a frame that has 16.7 ms to spend, and a stutter on the one screen
/// with a camera orbiting smoothly is far more noticeable than a shadow standing in for an extra
/// fifth of a second. That trade only got worse for the chunk when the silhouette got cheap: at
/// 5 KB it arrives inside the first read whatever this is, so what is being bought by reading
/// faster is no longer *when the player sees the car's shape* but only *when the paint arrives*.
const CHUNK_BYTES: usize = 32 * 1024;

/// Longest path this will assemble, including the NUL. `DIR` plus `catalogue::NAME_MAX` plus one.
const PATH_MAX: usize = 96;

#[repr(C, align(16))]
struct Arena([u8; SLOT_BYTES * SLOTS]);

/// A car that has been read and checked, and which slot's bytes it is looking at.
struct Resident {
    /// Which entry in the catalogue it came from.
    index: usize,
    car: Car<'static>,
}

/// A load in flight.
struct Load {
    fd: SceUid,
    slot: usize,
    index: usize,
    /// Whether finishing this load puts the car on screen. False only for the benchmark, which
    /// fills the spare slots with cars nobody selected so that a field of them is a field of
    /// different models.
    show: bool,
    progress: Progress,
    /// The car's shape, once enough of the file has landed to hold it — which is the first chunk,
    /// because the compiler writes it in front of everything else. Kept rather than looked for
    /// every frame: finding it checks every one of its indices, and that answer does not change.
    silhouette: Option<Silhouette<'static>>,
}

static mut ARENA: Arena = Arena([0; SLOT_BYTES * SLOTS]);
static mut RESIDENT: [Option<Resident>; SLOTS] = [const { None }; SLOTS];
/// Which slot holds the car the game is drawing, if any has been loaded yet.
static mut CURRENT: Option<usize> = None;
static mut CATALOGUE: Catalogue = Catalogue::EMPTY;
static mut LOADING: Option<Load> = None;
/// The last thing that went wrong, cleared by the next load that goes right.
static mut FAULT: Option<LoadError> = None;
/// What was wrong with the stick itself, which no later load can put right and none should clear.
static mut SCAN_FAULT: Option<LoadError> = None;
/// The longest a single chunk read has taken, in microseconds.
///
/// The one number `CHUNK_BYTES` has to be chosen against, and the one that cannot be got anywhere
/// but a real memory stick: under the headless emulator this is a read from a host filesystem and
/// comes back as a few microseconds whatever the chunk size. A peak past about 16,000 is a chunk
/// that costs a frame.
///
/// Only where it is read. A harness run leaves the overlay out so that its frames can be compared
/// pixel by pixel, and nothing else asks.
#[cfg(all(feature = "devtools", not(feature = "harness")))]
static mut PEAK_READ_US: u32 = 0;

/// Why a car could not be loaded. All of these end with no car rather than a wrong one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadError {
    /// No such file, or no `CARS/` directory at all. Almost always a car that was never copied
    /// onto the stick.
    Missing,
    /// The file is larger than a slot.
    NoRoom,
    /// The read stopped early.
    Short,
    /// More cars on the stick than the list can hold.
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
            // Per car now, rather than per stick: the fleet can be any size, but one file cannot
            // be bigger than the slot it is read into. Recompiling that car to a smaller budget is
            // the fix, and it is a fix that affects nothing else on the stick.
            LoadError::NoRoom => "car asset is too large for this build",
            LoadError::Short => "car asset could not be read in full",
            LoadError::TooMany => "more cars on the stick than the list holds",
            LoadError::NameTooLong => "car asset name is too long",
            LoadError::Format(e) => e.message(),
        }
    }
}

/// Reads what cars are on the stick, without opening one.
///
/// This is what makes a car a file rather than a build: dropping one onto the memory stick adds it
/// to the game. Nothing here knows what cars exist, and boot does not get slower for finding more
/// of them — a directory walk is all it is, so a stick with fifty cars costs what a stick with
/// seven does.
///
/// Anything wrong with the stick is left in `fault` rather than returned, because it is not news
/// that goes stale: an empty `CARS/` at boot is an empty `CARS/` all session, and the title screen
/// is the only thing that ever wanted to know.
pub fn scan() {
    let mut dir = [0u8; PATH_MAX];
    // sceIoDopen wants the directory without its trailing slash.
    let trimmed = &DIR[..DIR.len() - 1];
    dir[..trimmed.len()].copy_from_slice(trimmed.as_bytes());

    let mut fault = None;
    unsafe {
        let cat = &mut *(&raw mut CATALOGUE);
        let fd = sys::sceIoDopen(dir.as_ptr());
        if fd.0 < 0 {
            SCAN_FAULT = Some(LoadError::Missing);
            return;
        }
        // Zeroed rather than default: `SceIoDirent` is a C struct with a raw pointer in it and no
        // `Default`, and the kernel fills every field this reads.
        let mut entry: sys::SceIoDirent = core::mem::zeroed();
        while sys::sceIoDread(fd, &mut entry) > 0 {
            // A directory called `something.azcar` is not a car, and opening one would read as an
            // empty file rather than as the mistake it is.
            if entry.d_stat.st_mode.contains(IoStatMode::IFDIR) {
                continue;
            }
            let name = &entry.d_name;
            let len = name.iter().position(|c| *c == 0).unwrap_or(name.len());
            let name = &name[..len];
            if !catalogue::is_car_file(name) {
                continue;
            }
            if let Err(e) = cat.insert(name) {
                fault = fault.or(Some(match e {
                    catalogue::Error::NameTooLong => LoadError::NameTooLong,
                    catalogue::Error::Full => LoadError::TooMany,
                }));
            }
        }
        sys::sceIoDclose(fd);

        if cat.is_empty() {
            fault = fault.or(Some(LoadError::Missing));
        }
        SCAN_FAULT = fault;
    }
}

/// How many cars the stick is offering.
pub fn count() -> usize {
    unsafe { (*(&raw const CATALOGUE)).len() }
}

/// What to call car `index` before its file has been read.
pub fn display_name(index: usize) -> DisplayName {
    unsafe { (*(&raw const CATALOGUE)).display_name(index) }
}

/// The car the game is drawing, or `None` before the first one has finished loading.
pub fn current() -> Option<&'static Car<'static>> {
    unsafe { slot_car(CURRENT?) }
}

/// A car in a residency slot. Only the benchmark asks for cars by slot.
#[cfg_attr(not(feature = "devtools"), allow(dead_code))]
pub fn get(slot: usize) -> Option<&'static Car<'static>> {
    unsafe { slot_car(slot) }
}

/// How many residency slots there are, which is how many different cars the benchmark can field.
#[cfg_attr(not(feature = "devtools"), allow(dead_code))]
pub const fn slot_count() -> usize {
    SLOTS
}

unsafe fn slot_car(slot: usize) -> Option<&'static Car<'static>> {
    (*(&raw const RESIDENT)).get(slot)?.as_ref().map(|r| &r.car)
}

/// Whether a car is on its way in.
pub fn is_loading() -> bool {
    unsafe { (*(&raw const LOADING)).is_some() }
}

/// Which car is on its way in, and how far along it is.
pub fn loading() -> Option<(usize, Progress)> {
    unsafe {
        (*(&raw const LOADING))
            .as_ref()
            .map(|l| (l.index, l.progress))
    }
}

/// The shape of the car that is arriving, to stand in for it until it does.
pub fn loading_silhouette() -> Option<Silhouette<'static>> {
    unsafe { (*(&raw const LOADING)).as_ref()?.silhouette }
}

/// What to say about cars, if anything needs saying.
///
/// The load's fault first, because it is about the car in front of the player and it is the newer
/// news; then the scan's, which is about the stick and stays true all session — a `CARS/` directory
/// that was not there at boot is not there now, and no successful load makes it so.
pub fn fault() -> Option<LoadError> {
    unsafe { FAULT.or(SCAN_FAULT) }
}

/// Starts loading car `index`, cancelling whatever was being loaded before.
///
/// A car that is still resident from earlier is not read again — it is simply made current, which
/// is why flicking back and forth between two cars costs nothing after the first pass.
///
/// Returns `Ok(true)` when the car is on screen by the time this returns — either because it was
/// already resident, or because the whole of it fitted in the first chunk — and `Ok(false)` when it
/// is on its way and will take a few more frames.
pub fn begin(index: usize) -> Result<bool, LoadError> {
    start(index, true)
}

fn start(index: usize, show: bool) -> Result<bool, LoadError> {
    unsafe {
        cancel();

        if index >= count() {
            return fail(LoadError::Missing);
        }

        // Already in a slot: nothing to read.
        if let Some(slot) = resident_slot(index) {
            if show {
                CURRENT = Some(slot);
            }
            FAULT = None;
            return Ok(true);
        }

        let mut path = [0u8; PATH_MAX];
        let name = match (*(&raw const CATALOGUE)).get(index) {
            Some(entry) => entry.name(),
            None => return fail(LoadError::Missing),
        };
        let written = match join(&mut path, DIR, name) {
            Ok(n) => n,
            Err(e) => return fail(e),
        };

        let fd = sys::sceIoOpen(path[..written].as_ptr(), IoOpenFlags::RD_ONLY, 0o777);
        if fd.0 < 0 {
            return fail(LoadError::Missing);
        }
        // Measured here rather than taken from the directory entry, because this is the number the
        // reads are counted against and it has to come from the handle they are counted on.
        let size = sys::sceIoLseek(fd, 0, IoWhence::End);
        sys::sceIoLseek(fd, 0, IoWhence::Set);
        if size <= 0 {
            sys::sceIoClose(fd);
            return fail(LoadError::Short);
        }
        let size = size as usize;
        if size > SLOT_BYTES {
            sys::sceIoClose(fd);
            return fail(LoadError::NoRoom);
        }

        let slot = staging_slot();
        // Whatever was in that slot is about to be written over, so the car that was looking at
        // those bytes stops existing first.
        RESIDENT[slot] = None;
        LOADING = Some(Load {
            fd,
            slot,
            index,
            show,
            progress: Progress::new(size),
            silhouette: None,
        });
        FAULT = None;

        // The first chunk is read here rather than left to the next frame, and that one frame is
        // the whole difference between a car that is replaced by its own shadow and a car that
        // blinks out of the lay-by for a sixtieth of a second on its way to being one. The
        // silhouette lives at the front of the file, so this read is what puts it in memory.
        Ok(read(CHUNK_BYTES))
    }
}

/// Reads the next chunk. Call once a frame while `is_loading`.
///
/// Returns whether the car finished arriving on this call, so the caller can take its handling and
/// its proportions the moment there is a car to take them from.
pub fn step() -> bool {
    read(CHUNK_BYTES)
}

/// Reads all of what is left, for a player who has stopped browsing and pressed X.
///
/// The wait is the same wait the whole-file read used to impose, but it is now paid once, at the
/// moment the player asked for the run to start, rather than on every press of L or R.
pub fn hurry() -> bool {
    let rest = match loading() {
        Some((_, progress)) => progress.rest(),
        None => return false,
    };
    read(rest)
}

/// Reads up to `want` bytes into the staging slot, finishing the load if that was the last of it.
fn read(want: usize) -> bool {
    unsafe {
        let Some(load) = (*(&raw mut LOADING)).as_mut() else {
            return false;
        };
        let want = load.progress.next_read(want);
        let at = load.slot * SLOT_BYTES + load.progress.done();
        let base = (&raw mut ARENA) as *mut u8;

        if want > 0 {
            #[cfg(all(feature = "devtools", not(feature = "harness")))]
            let started = sys::sceKernelGetSystemTimeLow();
            let got = sys::sceIoRead(load.fd, base.add(at) as *mut _, want as u32);
            #[cfg(all(feature = "devtools", not(feature = "harness")))]
            {
                let took = sys::sceKernelGetSystemTimeLow().wrapping_sub(started);
                if took > PEAK_READ_US {
                    PEAK_READ_US = took;
                }
            }
            // The request never overruns the file, so anything short is a fault rather than the
            // end of it.
            if got < want as i32 {
                cancel();
                FAULT = Some(LoadError::Short);
                return false;
            }
            // The GE reads the vertex array straight out of here by DMA, and it does not go through
            // the data cache. Without this the first frames draw whatever was in memory before.
            sys::sceKernelDcacheWritebackInvalidateRange(base.add(at) as *const _, got as u32);
            load.progress.advance(got as usize);
        }

        // Look for the car's shape in what has landed so far. It is at the very front of the file,
        // so this succeeds on the first chunk and the title screen has something to draw a frame
        // after the button was pressed rather than half a second after.
        if load.silhouette.is_none() {
            let prefix: &'static [u8] =
                core::slice::from_raw_parts(base.add(load.slot * SLOT_BYTES), load.progress.done());
            load.silhouette = Silhouette::parse(prefix);
        }

        if !load.progress.is_complete() {
            return false;
        }

        let (slot, index, size, show) = (load.slot, load.index, load.progress.size(), load.show);
        sys::sceIoClose(load.fd);
        LOADING = None;

        let bytes: &'static [u8] = core::slice::from_raw_parts(base.add(slot * SLOT_BYTES), size);
        match Car::parse(bytes) {
            Ok(car) => {
                RESIDENT[slot] = Some(Resident { index, car });
                if show {
                    CURRENT = Some(slot);
                }
                FAULT = None;
                true
            }
            Err(e) => {
                // The slot is left empty and the car that was on screen is untouched, which is the
                // whole reason the incoming car reads into a slot of its own.
                FAULT = Some(LoadError::Format(e));
                false
            }
        }
    }
}

/// Fills every spare slot with a car nobody has selected, for the benchmark to field.
///
/// Blocking, and unapologetically so: this is what a diagnostic mode costs to enter, it happens on
/// a button press in a build that is never shipped, and a chunk-per-frame version of it would take
/// two seconds to do the same thing while pretending not to.
///
/// The cars are read but not shown. `--mode 13` and `--mode 14` exist to price what a field of
/// *different* models costs — the texture bind and the vertex-buffer switch between them — and one
/// car drawn eight times prices none of that.
#[cfg(feature = "devtools")]
pub fn fill_spare_slots() {
    unsafe {
        while (*(&raw const RESIDENT)).iter().any(|r| r.is_none()) {
            // The first car that is not already in a slot. Nothing to fill with when every car on
            // the stick is resident, which is a stick with fewer cars than there are slots.
            let Some(index) = (0..count()).find(|i| resident_slot(*i).is_none()) else {
                return;
            };
            if start(index, false).is_err() {
                return;
            }
            while is_loading() {
                hurry();
            }
            // A car that failed to load leaves its slot empty, and asking for the same one again
            // would never end.
            if resident_slot(index).is_none() {
                return;
            }
        }
    }
}

/// Stops a load in flight, leaving the resident car alone.
fn cancel() {
    unsafe {
        if let Some(load) = (*(&raw mut LOADING)).take() {
            sys::sceIoClose(load.fd);
        }
    }
}

/// Records a fault and hands it back, so `begin` can report and remember in one line.
unsafe fn fail(e: LoadError) -> Result<bool, LoadError> {
    FAULT = Some(e);
    Err(e)
}

/// Which slot already holds car `index`, if any.
unsafe fn resident_slot(index: usize) -> Option<usize> {
    (*(&raw const RESIDENT))
        .iter()
        .position(|r| r.as_ref().is_some_and(|r| r.index == index))
}

/// Which slot the next car should be read into.
///
/// Never the current one — that is the car on screen, and overwriting it would be the very thing
/// the second slot exists to prevent. An empty slot first, then the oldest thing that is not
/// current, which with two slots is the same choice made twice.
unsafe fn staging_slot() -> usize {
    let current = CURRENT.unwrap_or(usize::MAX);
    let empty = (*(&raw const RESIDENT))
        .iter()
        .position(|r| r.is_none())
        .filter(|s| *s != current);
    empty.unwrap_or_else(|| (0..SLOTS).find(|s| *s != current).unwrap_or(0))
}

/// The longest a chunk read has taken, in microseconds. What `CHUNK_BYTES` is chosen against.
#[cfg(all(feature = "devtools", not(feature = "harness")))]
pub fn peak_read_us() -> u32 {
    unsafe { PEAK_READ_US }
}

/// How much of the arena holds a car, for the diagnostics overlay.
#[cfg(all(feature = "devtools", not(feature = "harness")))]
pub fn used_bytes() -> usize {
    unsafe {
        (*(&raw const RESIDENT))
            .iter()
            .filter_map(|r| r.as_ref())
            .map(|r| r.car.byte_len())
            .sum()
    }
}

#[cfg(all(feature = "devtools", not(feature = "harness")))]
pub const fn arena_bytes() -> usize {
    SLOT_BYTES * SLOTS
}

/// Joins the directory and a name into a NUL-terminated path, refusing rather than truncating.
///
/// A truncated path is a file that is not found, reported as a missing car — which sends whoever
/// is holding it looking on the memory stick for a file that is sitting right there.
fn join(out: &mut [u8; PATH_MAX], dir: &str, name: &[u8]) -> Result<usize, LoadError> {
    let need = dir.len() + name.len() + 1;
    if need > PATH_MAX {
        return Err(LoadError::NameTooLong);
    }
    out[..dir.len()].copy_from_slice(dir.as_bytes());
    out[dir.len()..dir.len() + name.len()].copy_from_slice(name);
    out[need - 1] = 0;
    Ok(need)
}
