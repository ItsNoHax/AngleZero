//! Cars packed into the EBOOT, for a release that is one folder and nothing else to copy.
//!
//! A development build reads loose `.azcar` files out of `CARS/`, and should: recompiling a car is
//! then copying one file, with no rebuild. But a release is for players, and a player who moves the
//! game into a category folder, or copies the EBOOT on its own, should not be left with a game and
//! no cars. So `scripts/release.sh` packs every car into the EBOOT's `DATA.PSAR` section, and a
//! build that finds cars there reads them from there and nowhere else.
//!
//! `DATA.PSAR` is the one part of a PBP that nothing loads. The firmware reads the PRX out of
//! `DATA.PSP` and leaves whatever follows it on the stick, so 20 MB of cars cost the game no memory:
//! they are read in a chunk at a time, exactly as a loose file is, from an offset into the EBOOT.
//! And because `cargo psp` writes the PSAR offset as the end of the file, packing is appending —
//! the PBP header does not change.
//!
//! The layout, all little-endian, offsets counted from the start of the bundle:
//!
//! ```text
//! magic    "AZPK"
//! version  u32
//! count    u32
//! entries  count × { name: [u8; NAME_MAX] NUL-padded, offset: u32, size: u32 }
//! cars     each at its offset, 16-byte aligned
//! ```
//!
//! The index carries names rather than numbers because a car is still known by its filename: the
//! title screen sorts and labels the list from it before the car is read, and the catalogue that
//! does that should not care which of the two places the names came from.
//!
//! Nothing here does IO, so the format is tested on the host, and the asset tool that writes a
//! bundle uses these same encoders and runs this same parser over what it wrote.

use crate::catalogue::{self, MAX_ENTRIES, NAME_MAX};

/// Leads every bundle.
pub const MAGIC: [u8; 4] = *b"AZPK";
/// The only version this build reads.
pub const VERSION: u32 = 1;

/// Magic, version, count.
pub const HEADER_BYTES: usize = 12;
/// A name and two `u32`s.
pub const ENTRY_BYTES: usize = NAME_MAX + 8;
/// The largest index there can be, which is what the console reserves to read one into.
pub const INDEX_MAX: usize = HEADER_BYTES + MAX_ENTRIES * ENTRY_BYTES;
/// Where each car starts, in bytes. Not needed for correctness — every car is copied into an
/// aligned slot before anything looks at it — but it costs at most 15 bytes a car and keeps every
/// read starting on a boundary the memory stick driver likes.
pub const ALIGN: usize = 16;

/// A PBP's fixed header: magic, version, and eight section offsets.
pub const PBP_HEADER_BYTES: usize = 40;
const PBP_MAGIC: [u8; 4] = *b"\0PBP";
/// Where the last of the eight offsets, `DATA.PSAR`'s, sits in the header.
const PBP_PSAR_AT: usize = 36;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Not a PBP, where a PBP was expected.
    NotAPbp,
    /// No bundle here at all. Not a fault: a build straight out of `cargo psp` has none.
    Absent,
    /// A bundle written by a different version of the format.
    Version,
    /// The bundle claims more than it holds.
    Truncated,
    /// More cars than the catalogue can list.
    TooMany,
    /// An entry whose name is not a car's, or whose bytes lie outside the bundle.
    BadEntry,
}

/// Where `DATA.PSAR` starts in a PBP, from its first `PBP_HEADER_BYTES`.
pub fn psar_offset(pbp_header: &[u8]) -> Result<usize, Error> {
    if pbp_header.len() < PBP_HEADER_BYTES || pbp_header[..4] != PBP_MAGIC {
        return Err(Error::NotAPbp);
    }
    Ok(le_u32(pbp_header, PBP_PSAR_AT) as usize)
}

/// Where one car sits in the bundle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub offset: usize,
    pub size: usize,
}

/// A bundle's index, checked over once.
pub struct Index<'a> {
    bytes: &'a [u8],
    count: usize,
}

impl<'a> Index<'a> {
    /// Reads an index out of the front of a bundle.
    ///
    /// `bytes` need only hold the index, which is all the console reads up front; `bundle_len` is
    /// how long the whole bundle is, so that an entry pointing past its end is refused here rather
    /// than turning up later as a short read.
    pub fn parse(bytes: &'a [u8], bundle_len: usize) -> Result<Index<'a>, Error> {
        if bytes.len() < HEADER_BYTES || bytes[..4] != MAGIC {
            return Err(Error::Absent);
        }
        if le_u32(bytes, 4) != VERSION {
            return Err(Error::Version);
        }
        let count = le_u32(bytes, 8) as usize;
        if count > MAX_ENTRIES {
            return Err(Error::TooMany);
        }
        let index_end = HEADER_BYTES + count * ENTRY_BYTES;
        if bytes.len() < index_end || bundle_len < index_end {
            return Err(Error::Truncated);
        }

        let index = Index { bytes, count };
        for i in 0..count {
            let name = index.name(i);
            if name.len() > catalogue::NAME_MAX || !catalogue::is_car_file(name) {
                return Err(Error::BadEntry);
            }
            let span = index.span(i);
            let end = span.offset.checked_add(span.size).ok_or(Error::BadEntry)?;
            if span.offset < index_end || span.size == 0 {
                return Err(Error::BadEntry);
            }
            if end > bundle_len {
                return Err(Error::Truncated);
            }
        }
        Ok(index)
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// The filename car `i` was packed under.
    pub fn name(&self, i: usize) -> &'a [u8] {
        let at = HEADER_BYTES + i * ENTRY_BYTES;
        let field = &self.bytes[at..at + NAME_MAX];
        let len = field.iter().position(|c| *c == 0).unwrap_or(NAME_MAX);
        &field[..len]
    }

    fn span(&self, i: usize) -> Span {
        let at = HEADER_BYTES + i * ENTRY_BYTES + NAME_MAX;
        Span {
            offset: le_u32(self.bytes, at) as usize,
            size: le_u32(self.bytes, at + 4) as usize,
        }
    }

    /// Where the car packed as `name` is. Exact match: the names come from this same index, by way
    /// of the catalogue, so there is no second spelling to allow for.
    pub fn find(&self, name: &[u8]) -> Option<Span> {
        (0..self.count)
            .find(|i| self.name(*i) == name)
            .map(|i| self.span(i))
    }
}

/// How long the index of a bundle of `count` cars is, which is where its first car can start.
pub const fn index_bytes(count: usize) -> usize {
    HEADER_BYTES + count * ENTRY_BYTES
}

/// The header of a bundle of `count` cars.
pub fn encode_header(count: usize) -> [u8; HEADER_BYTES] {
    let mut out = [0u8; HEADER_BYTES];
    out[..4].copy_from_slice(&MAGIC);
    out[4..8].copy_from_slice(&VERSION.to_le_bytes());
    out[8..12].copy_from_slice(&(count as u32).to_le_bytes());
    out
}

/// One index entry. Refuses a name that could not be read back as a car.
pub fn encode_entry(name: &[u8], span: Span) -> Result<[u8; ENTRY_BYTES], Error> {
    if name.len() > NAME_MAX || !catalogue::is_car_file(name) || name.contains(&0) {
        return Err(Error::BadEntry);
    }
    let (Ok(offset), Ok(size)) = (u32::try_from(span.offset), u32::try_from(span.size)) else {
        return Err(Error::BadEntry);
    };
    let mut out = [0u8; ENTRY_BYTES];
    out[..name.len()].copy_from_slice(name);
    out[NAME_MAX..NAME_MAX + 4].copy_from_slice(&offset.to_le_bytes());
    out[NAME_MAX + 4..].copy_from_slice(&size.to_le_bytes());
    Ok(out)
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}
