//! What cars exist on the memory stick, as against which one is in memory.
//!
//! The two used to be the same thing: every `.azcar` was read into an arena at boot, so the number
//! of cars the game offered was the number that fitted in six megabytes. That made adding a car a
//! question about `.bss` — seven of them came to 5.7 MB and the eighth did not fit — when nothing
//! in a run needs more than the one car on screen.
//!
//! So residency is somebody else's problem now, and this holds the cheap half: a filename per car,
//! sorted, with no file opened. Boot cost does not grow with how many cars are on the stick, and
//! neither does memory in any way a person would notice — an entry is 65 bytes, so the whole table
//! is 8 KB.
//!
//! There is a cap, and it is deliberately far past anything reachable: 128 cars is eighteen times
//! what this repo compiles, and a stick holding that many is holding 100 MB of them. It exists
//! because a fixed table is the only kind there can be without an allocator, not because 128 is a
//! number anybody should have to think about.
//!
//! Nothing here does IO. The console's directory walk lives in `psp::car`, which calls `insert`
//! with what it found — which is what lets the ordering, the naming and the limits be tested on the
//! host with no PSP and no memory stick.

/// The file extension a car is expected to carry.
pub const EXTENSION: &str = ".azcar";

/// How many cars the table can hold. See the module note: reachable only in theory.
pub const MAX_ENTRIES: usize = 128;

/// Longest filename an entry can carry, without its directory.
///
/// Sixty-four rather than FAT's 255, because the path this is joined onto is assembled in a
/// fixed buffer on the console and a name that cannot be turned into a path is not a car that can
/// be loaded. Refusing it here, by name, beats truncating it into a file that is not found.
pub const NAME_MAX: usize = 64;

/// Why a car could not be catalogued. Neither of these is fatal to the others.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The filename is longer than `NAME_MAX`.
    NameTooLong,
    /// The table already holds `MAX_ENTRIES`.
    Full,
}

/// One car on the stick: what it is called, and nothing else.
///
/// Not how big it is, though the directory entry carries that for free. The loader measures the
/// file on the handle it is about to read from, which is the only measurement that can be acted on
/// — a size taken from a directory walk minutes earlier is a second answer to a question that has
/// one, and keeping it would mean deciding which of the two to believe.
#[derive(Clone, Copy)]
pub struct Entry {
    name: [u8; NAME_MAX],
    len: u8,
}

impl Entry {
    const EMPTY: Entry = Entry {
        name: [0; NAME_MAX],
        len: 0,
    };

    /// The filename, as it is on the stick.
    pub fn name(&self) -> &[u8] {
        &self.name[..self.len as usize]
    }
}

/// A name worked out from a filename, for a screen that needs one before the file is open.
///
/// Fixed-size and `Copy` rather than borrowed, because it is built rather than found: there is
/// nowhere to build it that outlives the call.
#[derive(Clone, Copy)]
pub struct DisplayName {
    text: [u8; NAME_MAX],
    len: usize,
}

impl DisplayName {
    pub fn as_bytes(&self) -> &[u8] {
        &self.text[..self.len]
    }
}

pub struct Catalogue {
    entries: [Entry; MAX_ENTRIES],
    count: usize,
}

impl Default for Catalogue {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl Catalogue {
    pub const EMPTY: Catalogue = Catalogue {
        entries: [Entry::EMPTY; MAX_ENTRIES],
        count: 0,
    };

    /// Files them in sorted order as they arrive.
    ///
    /// Sorted here rather than after the walk because `sceIoDread` hands entries back in the order
    /// the filesystem stored them, which on FAT is the order they were copied. Two people with the
    /// same seven cars would otherwise get two different lists, and a car added later would appear
    /// at the end rather than where its name says it belongs. An insertion sort over at most 128
    /// entries costs nothing measurable and needs no scratch space, which a table this size and an
    /// allocator this absent both want.
    pub fn insert(&mut self, name: &[u8]) -> Result<(), Error> {
        if name.len() > NAME_MAX {
            return Err(Error::NameTooLong);
        }
        if self.count >= MAX_ENTRIES {
            return Err(Error::Full);
        }

        let mut at = self.count;
        while at > 0 && less(name, self.entries[at - 1].name()) {
            self.entries[at] = self.entries[at - 1];
            at -= 1;
        }

        let mut entry = Entry {
            name: [0; NAME_MAX],
            len: name.len() as u8,
        };
        entry.name[..name.len()].copy_from_slice(name);
        self.entries[at] = entry;
        self.count += 1;
        Ok(())
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn get(&self, index: usize) -> Option<&Entry> {
        self.entries.get(index).filter(|_| index < self.count)
    }

    /// What to call car `index` before its file has been read.
    ///
    /// A car names itself — the compiled asset carries the name its author wrote, and that is what
    /// the title screen shows once the car is loaded. But a car now takes a moment to arrive, and a
    /// blank line where the name goes is worse than a good guess, so until then the filename
    /// stands in: `nissan_s15.azcar` reads as `NISSAN S15`.
    ///
    /// Upper-cased because the font is: `psp::text` has one case, and the asset compiler
    /// upper-cases the authored name for the same reason.
    pub fn display_name(&self, index: usize) -> DisplayName {
        let mut out = DisplayName {
            text: [0; NAME_MAX],
            len: 0,
        };
        let Some(entry) = self.get(index) else {
            return out;
        };
        let name = entry.name();
        let stem = if ends_with_ignoring_case(name, EXTENSION.as_bytes()) {
            &name[..name.len() - EXTENSION.len()]
        } else {
            name
        };
        for (i, &c) in stem.iter().enumerate() {
            out.text[i] = match c {
                b'_' | b'-' => b' ',
                c => c.to_ascii_uppercase(),
            };
        }
        out.len = stem.len();
        out
    }
}

/// Whether a directory entry is a car this build should offer.
///
/// Case-insensitive: memory sticks are FAT, FAT is not fussy about case, and a car copied from a
/// machine that upper-cased it is still a car.
pub fn is_car_file(name: &[u8]) -> bool {
    // A file called exactly `.azcar` is an extension with no name, not a car.
    name.len() > EXTENSION.len() && ends_with_ignoring_case(name, EXTENSION.as_bytes())
}

fn ends_with_ignoring_case(name: &[u8], suffix: &[u8]) -> bool {
    if name.len() < suffix.len() {
        return false;
    }
    name[name.len() - suffix.len()..]
        .iter()
        .zip(suffix)
        .all(|(a, b)| a.to_ascii_lowercase() == b.to_ascii_lowercase())
}

/// Case-insensitive byte order, so `AE86.AZCAR` and `ae86.azcar` sort where a reader expects and
/// not in two different places.
fn less(a: &[u8], b: &[u8]) -> bool {
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (x.to_ascii_lowercase(), y.to_ascii_lowercase());
        if x != y {
            return x < y;
        }
    }
    a.len() < b.len()
}
