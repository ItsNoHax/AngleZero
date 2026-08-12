//! The `.azcar` car asset: what the console loads instead of a 3D model file.
//!
//! A compiled car is a single file that the runtime opens, checks, and draws straight out of. It
//! is not a document to be parsed — everything expensive was decided on a development machine by
//! `tools/anglezero-asset`, and what is left here is a header, a few dozen fixed-size records, and
//! two arrays that go to the hardware untouched.
//!
//! Three things shape the layout:
//!
//! * **Vertices and indices are read in place.** Every section starts on a 16-byte boundary, so
//!   when the file is loaded into a 16-byte-aligned buffer the vertex array is already where the
//!   GE wants it, in `GU_COLOR_8888 | GU_VERTEX_32BITF` order. Nothing is copied or converted at
//!   load time, which is what makes loading a car a file read and a bounds check.
//! * **Records are decoded, not cast.** The handful of mesh, material and wheel records are
//!   little-endian byte layouts with explicit `decode`, like `save::Record`. Casting structs out
//!   of a file means trusting a compiler's padding across two targets; there are only a few dozen
//!   records, and decoding them costs nothing measurable.
//! * **Unknown sections are skipped, not rejected.** The section table has room for extra offsets,
//!   so LOD meshes and textures can be added later without moving anything a version-1 reader
//!   already knows how to find.
//!
//! The format is versioned and refuses anything it does not understand. A car that half-loads
//! would draw as scattered triangles at the origin, which is a much worse way to learn that an
//! asset is stale than an error at boot.

use crate::mesh::Vertex;

/// Stable magic. Present in every version.
pub const MAGIC: [u8; 4] = *b"AZCR";
/// The only version this build can read.
pub const VERSION: u16 = 1;
/// Bytes before the first section. A multiple of 16, so section offsets stay aligned.
pub const HEADER_BYTES: usize = 112;

pub const MESH_BYTES: usize = 32;
pub const MATERIAL_BYTES: usize = 16;
pub const WHEEL_BYTES: usize = 32;
/// Nine `f32`, padded to the section alignment.
pub const HANDLING_BYTES: usize = 48;

/// Vertex layout tag written into the header. One value today; the field exists so that a compact
/// vertex format can be introduced later and refused cleanly by an older build rather than drawn
/// as noise.
pub const VERTEX_COLOR_8888_F32: u32 = 1;

/// Which corner a wheel is, in the order the writer emits them.
pub const WHEEL_FRONT_LEFT: u8 = 0;
pub const WHEEL_FRONT_RIGHT: u8 = 1;
pub const WHEEL_REAR_LEFT: u8 = 2;
pub const WHEEL_REAR_RIGHT: u8 = 3;

/// Mesh belongs to the body rather than to a wheel.
pub const NO_WHEEL: u16 = 0xFFFF;
/// Material has no texture.
pub const NO_TEXTURE: u16 = 0xFFFF;

/// What a surface is, for the handful of decisions the renderer makes per material.
///
/// Six categories rather than a material system: the PSP does not get a shader per part, it gets
/// blending on or off and culling on or off, and these are the groups that differ in that answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Category {
    Body = 0,
    Window = 1,
    Tyre = 2,
    Interior = 3,
    Light = 4,
    Chrome = 5,
}

impl Category {
    pub fn from_u8(v: u8) -> Option<Category> {
        Some(match v {
            0 => Category::Body,
            1 => Category::Window,
            2 => Category::Tyre,
            3 => Category::Interior,
            4 => Category::Light,
            5 => Category::Chrome,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Category::Body => "body",
            Category::Window => "window",
            Category::Tyre => "tyre",
            Category::Interior => "interior",
            Category::Light => "light",
            Category::Chrome => "chrome",
        }
    }
}

/// Material flags.
/// Drawn with alpha blending.
pub const MATERIAL_BLEND: u8 = 1 << 0;
/// Drawn with back-face culling off, for surfaces modelled as single sheets.
pub const MATERIAL_TWO_SIDED: u8 = 1 << 1;

/// Why a file was refused. Every one of these is a reason not to draw anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// Shorter than a header.
    TooShort,
    /// Not an .azcar at all.
    BadMagic,
    /// An .azcar from a newer or older format than this build knows.
    UnsupportedVersion(u16),
    /// Vertices in a layout this build cannot hand to the hardware.
    UnsupportedVertexFormat(u32),
    /// The header's own length field disagrees with how many bytes there are.
    LengthMismatch,
    /// A section runs off the end of the file, or overlaps the header.
    BadSection,
    /// A section that must be 16-byte aligned is not.
    Misaligned,
    /// An index, material or wheel reference points at something that is not there.
    DanglingReference,
}

impl Error {
    /// A short line for the on-screen error, which has no room for anything longer.
    pub fn message(self) -> &'static str {
        match self {
            Error::TooShort => "car asset is truncated",
            Error::BadMagic => "not a car asset",
            Error::UnsupportedVersion(_) => "car asset version not supported",
            Error::UnsupportedVertexFormat(_) => "car vertex format not supported",
            Error::LengthMismatch => "car asset length is wrong",
            Error::BadSection => "car asset section is out of range",
            Error::Misaligned => "car asset is misaligned",
            Error::DanglingReference => "car asset reference is dangling",
        }
    }
}

/// One drawable run of the index buffer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mesh {
    pub first_index: u32,
    pub index_count: u32,
    pub material: u16,
    /// Which wheel this belongs to, or `NO_WHEEL` for the body. Wheel meshes are stored about
    /// their own hub, so they can be spun and steered without moving the body.
    pub wheel: u16,
    pub name: u16,
    pub flags: u16,
    /// Centre and radius of the mesh in the space it is stored in, for sorting and culling.
    pub center: [f32; 3],
    pub radius: f32,
}

impl Mesh {
    fn decode(b: &[u8]) -> Mesh {
        Mesh {
            first_index: le_u32(b, 0),
            index_count: le_u32(b, 4),
            material: le_u16(b, 8),
            wheel: le_u16(b, 10),
            name: le_u16(b, 12),
            flags: le_u16(b, 14),
            center: [le_f32(b, 16), le_f32(b, 20), le_f32(b, 24)],
            radius: le_f32(b, 28),
        }
    }

    pub fn encode(&self) -> [u8; MESH_BYTES] {
        let mut o = [0u8; MESH_BYTES];
        o[0..4].copy_from_slice(&self.first_index.to_le_bytes());
        o[4..8].copy_from_slice(&self.index_count.to_le_bytes());
        o[8..10].copy_from_slice(&self.material.to_le_bytes());
        o[10..12].copy_from_slice(&self.wheel.to_le_bytes());
        o[12..14].copy_from_slice(&self.name.to_le_bytes());
        o[14..16].copy_from_slice(&self.flags.to_le_bytes());
        for (i, c) in self.center.iter().enumerate() {
            o[16 + i * 4..20 + i * 4].copy_from_slice(&c.to_le_bytes());
        }
        o[28..32].copy_from_slice(&self.radius.to_le_bytes());
        o
    }
}

/// How a surface is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialDef {
    /// Baked base colour, `GU_COLOR_8888`. Vertex colours carry the lighting; this is what the
    /// report and any untextured fallback use.
    pub color: u32,
    pub texture: u16,
    pub name: u16,
    pub category: Category,
    pub flags: u8,
}

impl MaterialDef {
    fn decode(b: &[u8]) -> Result<MaterialDef, Error> {
        Ok(MaterialDef {
            color: le_u32(b, 0),
            texture: le_u16(b, 4),
            name: le_u16(b, 6),
            category: Category::from_u8(b[8]).ok_or(Error::DanglingReference)?,
            flags: b[9],
        })
    }

    pub fn encode(&self) -> [u8; MATERIAL_BYTES] {
        let mut o = [0u8; MATERIAL_BYTES];
        o[0..4].copy_from_slice(&self.color.to_le_bytes());
        o[4..6].copy_from_slice(&self.texture.to_le_bytes());
        o[6..8].copy_from_slice(&self.name.to_le_bytes());
        o[8] = self.category as u8;
        o[9] = self.flags;
        o
    }

    pub fn blended(&self) -> bool {
        self.flags & MATERIAL_BLEND != 0
    }

    pub fn two_sided(&self) -> bool {
        self.flags & MATERIAL_TWO_SIDED != 0
    }
}

/// Where a wheel goes and how big it is. The runtime needs no more than this to place, steer and
/// spin one, which is the whole point of keeping wheels out of the body mesh.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WheelDef {
    pub corner: u8,
    /// Whether steering input turns it.
    pub steers: bool,
    pub name: u16,
    /// Hub centre in car space: where the wheel's own origin sits on the body.
    pub hub: [f32; 3],
    pub radius: f32,
    pub width: f32,
}

impl WheelDef {
    fn decode(b: &[u8]) -> WheelDef {
        WheelDef {
            corner: b[0],
            steers: b[1] != 0,
            name: le_u16(b, 2),
            hub: [le_f32(b, 4), le_f32(b, 8), le_f32(b, 12)],
            radius: le_f32(b, 16),
            width: le_f32(b, 20),
        }
    }

    pub fn encode(&self) -> [u8; WHEEL_BYTES] {
        let mut o = [0u8; WHEEL_BYTES];
        o[0] = self.corner;
        o[1] = self.steers as u8;
        o[2..4].copy_from_slice(&self.name.to_le_bytes());
        for (i, c) in self.hub.iter().enumerate() {
            o[4 + i * 4..8 + i * 4].copy_from_slice(&c.to_le_bytes());
        }
        o[16..20].copy_from_slice(&self.radius.to_le_bytes());
        o[20..24].copy_from_slice(&self.width.to_le_bytes());
        o
    }
}

/// Byte offsets of the header's fields, shared with the writer so the two cannot drift.
pub mod field {
    pub const MAGIC: usize = 0;
    pub const VERSION: usize = 4;
    pub const FLAGS: usize = 6;
    pub const LENGTH: usize = 8;
    pub const VERTEX_FORMAT: usize = 12;
    pub const VERTEX_COUNT: usize = 16;
    pub const INDEX_COUNT: usize = 20;
    pub const MESH_COUNT: usize = 24;
    pub const MATERIAL_COUNT: usize = 26;
    pub const TEXTURE_COUNT: usize = 28;
    pub const WHEEL_COUNT: usize = 30;
    /// min xyz then max xyz, in car space.
    pub const BOUNDS: usize = 32;
    pub const MESHES_AT: usize = 56;
    pub const MATERIALS_AT: usize = 60;
    pub const TEXTURES_AT: usize = 64;
    pub const WHEELS_AT: usize = 68;
    pub const VERTICES_AT: usize = 72;
    pub const INDICES_AT: usize = 76;
    pub const STRINGS_AT: usize = 80;
    pub const STRINGS_BYTES: usize = 84;
    /// Where LOD meshes will go. Zero in version 1, and readers must tolerate that.
    pub const LODS_AT: usize = 88;
    /// Offset into the string table of the attribution line, or `NO_CREDIT`.
    pub const CREDIT: usize = 92;
    /// Offset into the string table of the car's name, as a person would say it.
    pub const NAME: usize = 96;
    /// Where the handling record is, or zero for "this car does not say".
    ///
    /// Added after version 1 shipped, in the space the header already reserved, which is exactly
    /// what that space was for: a build that predates it reads zero and drives the car with the
    /// default numbers, which is what it did anyway.
    pub const HANDLING_AT: usize = 100;
}

/// The car carries no attribution line.
pub const NO_CREDIT: u32 = 0xFFFF_FFFF;

/// A validated car, borrowing the bytes it was loaded from.
///
/// Holding the buffer rather than copying out of it is the point: on the console the buffer is a
/// static arena the file was read into, and the vertex array inside it is handed to the GE as is.
pub struct Car<'a> {
    bytes: &'a [u8],
    vertex_count: usize,
    index_count: usize,
    mesh_count: usize,
    material_count: usize,
    wheel_count: usize,
    meshes_at: usize,
    materials_at: usize,
    wheels_at: usize,
    vertices_at: usize,
    indices_at: usize,
    strings: (usize, usize),
    handling_at: usize,
    bounds: [f32; 6],
}

impl<'a> Car<'a> {
    /// Checks a buffer over and returns a car that can be drawn, or the reason it cannot.
    ///
    /// Everything reachable is checked here, once, so that drawing needs no bounds tests: every
    /// section fits, every index is inside the vertex array, and every material and wheel a mesh
    /// names exists.
    pub fn parse(bytes: &'a [u8]) -> Result<Car<'a>, Error> {
        if bytes.len() < HEADER_BYTES {
            return Err(Error::TooShort);
        }
        if bytes[0..4] != MAGIC {
            return Err(Error::BadMagic);
        }
        let version = le_u16(bytes, field::VERSION);
        if version != VERSION {
            return Err(Error::UnsupportedVersion(version));
        }
        let format = le_u32(bytes, field::VERTEX_FORMAT);
        if format != VERTEX_COLOR_8888_F32 {
            return Err(Error::UnsupportedVertexFormat(format));
        }
        if le_u32(bytes, field::LENGTH) as usize != bytes.len() {
            return Err(Error::LengthMismatch);
        }

        let vertex_count = le_u32(bytes, field::VERTEX_COUNT) as usize;
        let index_count = le_u32(bytes, field::INDEX_COUNT) as usize;
        let mesh_count = le_u16(bytes, field::MESH_COUNT) as usize;
        let material_count = le_u16(bytes, field::MATERIAL_COUNT) as usize;
        let wheel_count = le_u16(bytes, field::WHEEL_COUNT) as usize;

        let meshes_at = le_u32(bytes, field::MESHES_AT) as usize;
        let materials_at = le_u32(bytes, field::MATERIALS_AT) as usize;
        let wheels_at = le_u32(bytes, field::WHEELS_AT) as usize;
        let vertices_at = le_u32(bytes, field::VERTICES_AT) as usize;
        let indices_at = le_u32(bytes, field::INDICES_AT) as usize;
        let strings_at = le_u32(bytes, field::STRINGS_AT) as usize;
        let strings_bytes = le_u32(bytes, field::STRINGS_BYTES) as usize;
        let handling_at = le_u32(bytes, field::HANDLING_AT) as usize;

        let car = Car {
            bytes,
            vertex_count,
            index_count,
            mesh_count,
            material_count,
            wheel_count,
            meshes_at,
            materials_at,
            wheels_at,
            vertices_at,
            indices_at,
            strings: (strings_at, strings_bytes),
            handling_at,
            bounds: [
                le_f32(bytes, field::BOUNDS),
                le_f32(bytes, field::BOUNDS + 4),
                le_f32(bytes, field::BOUNDS + 8),
                le_f32(bytes, field::BOUNDS + 12),
                le_f32(bytes, field::BOUNDS + 16),
                le_f32(bytes, field::BOUNDS + 20),
            ],
        };

        // Every section must lie past the header and inside the file.
        for (at, len, aligned) in [
            (meshes_at, mesh_count * MESH_BYTES, true),
            (materials_at, material_count * MATERIAL_BYTES, true),
            (wheels_at, wheel_count * WHEEL_BYTES, true),
            (
                vertices_at,
                vertex_count * core::mem::size_of::<Vertex>(),
                true,
            ),
            (indices_at, index_count * 2, true),
            (strings_at, strings_bytes, false),
            // Zero means the car does not carry one, which is the only optional section so far —
            // hence the length rather than the offset saying so.
            (
                handling_at,
                if handling_at == 0 { 0 } else { HANDLING_BYTES },
                true,
            ),
        ] {
            if len == 0 {
                continue;
            }
            if at < HEADER_BYTES || at > bytes.len() || bytes.len() - at < len {
                return Err(Error::BadSection);
            }
            if aligned && at % 16 != 0 {
                return Err(Error::Misaligned);
            }
        }

        if vertex_count == 0 || index_count == 0 || mesh_count == 0 {
            return Err(Error::BadSection);
        }
        // Indices are 16-bit and relative to the start of the vertex array.
        if vertex_count > u16::MAX as usize + 1 {
            return Err(Error::BadSection);
        }

        for i in 0..mesh_count {
            let m = car.mesh(i);
            let end = m.first_index as usize + m.index_count as usize;
            if end > index_count || m.index_count % 3 != 0 {
                return Err(Error::DanglingReference);
            }
            if m.material as usize >= material_count {
                return Err(Error::DanglingReference);
            }
            if m.wheel != NO_WHEEL && m.wheel as usize >= wheel_count {
                return Err(Error::DanglingReference);
            }
        }
        for i in 0..material_count {
            // Decoding is what validates the category byte.
            car.material_checked(i)?;
        }
        for index in car.indices() {
            if *index as usize >= vertex_count {
                return Err(Error::DanglingReference);
            }
        }

        Ok(car)
    }

    pub fn vertex_count(&self) -> usize {
        self.vertex_count
    }

    pub fn index_count(&self) -> usize {
        self.index_count
    }

    pub fn triangle_count(&self) -> usize {
        self.index_count / 3
    }

    pub fn mesh_count(&self) -> usize {
        self.mesh_count
    }

    pub fn material_count(&self) -> usize {
        self.material_count
    }

    pub fn wheel_count(&self) -> usize {
        self.wheel_count
    }

    /// min xyz, max xyz, in car space with the wheels on the ground at y = 0.
    pub fn bounds(&self) -> [f32; 6] {
        self.bounds
    }

    /// The vertex array, laid out exactly as the GU consumes it.
    pub fn vertices(&self) -> &'a [Vertex] {
        // Safety: `parse` checked that the section is 16-byte aligned inside a buffer the caller
        // aligned, and that `vertex_count` vertices fit. `Vertex` is `repr(C)` over four 4-byte
        // fields with no padding and no invalid bit patterns.
        unsafe {
            core::slice::from_raw_parts(
                self.bytes.as_ptr().add(self.vertices_at) as *const Vertex,
                self.vertex_count,
            )
        }
    }

    pub fn indices(&self) -> &'a [u16] {
        // Safety: as above; `u16` has no invalid bit patterns and the section is 16-byte aligned.
        unsafe {
            core::slice::from_raw_parts(
                self.bytes.as_ptr().add(self.indices_at) as *const u16,
                self.index_count,
            )
        }
    }

    /// Raw pointer to the vertex array, for handing to the GE.
    pub fn vertices_ptr(&self) -> *const u8 {
        // Safety: the section was checked to lie inside the buffer.
        unsafe { self.bytes.as_ptr().add(self.vertices_at) }
    }

    pub fn indices_ptr(&self) -> *const u8 {
        unsafe { self.bytes.as_ptr().add(self.indices_at) }
    }

    pub fn mesh(&self, i: usize) -> Mesh {
        Mesh::decode(&self.bytes[self.meshes_at + i * MESH_BYTES..])
    }

    pub fn material(&self, i: usize) -> MaterialDef {
        // `parse` decoded every material once already, so this cannot fail.
        self.material_checked(i).unwrap_or(MaterialDef {
            color: 0xFFFF_FFFF,
            texture: NO_TEXTURE,
            name: 0,
            category: Category::Body,
            flags: 0,
        })
    }

    fn material_checked(&self, i: usize) -> Result<MaterialDef, Error> {
        MaterialDef::decode(&self.bytes[self.materials_at + i * MATERIAL_BYTES..])
    }

    pub fn wheel(&self, i: usize) -> WheelDef {
        WheelDef::decode(&self.bytes[self.wheels_at + i * WHEEL_BYTES..])
    }

    /// What the simulation needs to know about this car's proportions.
    ///
    /// Presenting the asset to the rest of the game is this module's job, and this is the only
    /// part of it the physics and the effects care about: how fast the wheels should turn, and
    /// where the back ones are. A car with no wheels in it falls back to a default rather than to
    /// zeroes, which would stop the wheels dead and pile every tyre mark on the car's origin.
    pub fn shape(&self) -> crate::vehicle::CarShape {
        let mut rear = [[0.0f32; 3]; 4];
        let mut rear_count = 0;
        let mut radius = 0.0;
        let mut wheels = 0;

        for i in 0..self.wheel_count() {
            let w = self.wheel(i);
            radius += w.radius;
            wheels += 1;
            if (w.corner == WHEEL_REAR_LEFT || w.corner == WHEEL_REAR_RIGHT)
                && rear_count < rear.len()
            {
                rear[rear_count] = w.hub;
                rear_count += 1;
            }
        }
        if wheels == 0 {
            return crate::vehicle::CarShape::DEFAULT;
        }
        crate::vehicle::CarShape::measure(radius / wheels as f32, &rear[..rear_count])
    }

    /// What this car drives like, or the default if it does not say.
    ///
    /// Refused rather than trusted: a car whose config had a typo in it — a zero mass, a negative
    /// wheelbase — would not drive oddly, it would put the vehicle at infinity on the first
    /// substep. A car that fails this check drives like the default one, which is a car that
    /// drives.
    pub fn handling(&self) -> crate::vehicle::CarHandling {
        use crate::vehicle::CarHandling;
        if self.handling_at == 0 {
            return CarHandling::DEFAULT;
        }
        let at = self.handling_at;
        let f = |i: usize| le_f32(self.bytes, at + i * 4);
        let h = CarHandling {
            mass: f(0),
            inertia: f(1),
            front_axle: f(2),
            rear_axle: f(3),
            engine: f(4),
            top_speed: f(5),
            brake: f(6),
            steer_lock: f(7),
            grip: f(8),
        };
        if h.is_sane() {
            h
        } else {
            CarHandling::DEFAULT
        }
    }

    /// What the car is called. `BMW E36`, not `bmw_e36.azcar`.
    ///
    /// In the asset rather than in a table in the game, so that a car dropped onto the memory
    /// stick can name itself. Nothing about drawing depends on it.
    pub fn name_of_car(&self) -> &'a [u8] {
        self.string_at(field::NAME)
    }

    /// Who made the source model, and under what licence.
    ///
    /// Not a nicety. Car models come from scanning and modelling sites under licences that require
    /// attribution, so the obligation travels with the asset rather than with a line in a readme
    /// that a rebuild can lose. A car that carries a credit is a car whose credit the game can
    /// display without anybody having to remember to.
    pub fn credit(&self) -> &'a [u8] {
        self.string_at(field::CREDIT)
    }

    /// A string named by a header field, or empty when the field says there is none.
    fn string_at(&self, field: usize) -> &'a [u8] {
        let at = le_u32(self.bytes, field);
        if at == NO_CREDIT || at > u16::MAX as u32 {
            return b"";
        }
        self.name(at as u16)
    }

    /// A name out of the string table, without its terminator. Names are for diagnostics; nothing
    /// about drawing depends on them.
    pub fn name(&self, at: u16) -> &'a [u8] {
        let (start, len) = self.strings;
        let from = start + at as usize;
        if at as usize >= len || from >= self.bytes.len() {
            return b"";
        }
        let tail = &self.bytes[from..start + len];
        let end = tail.iter().position(|&c| c == 0).unwrap_or(tail.len());
        &tail[..end]
    }
}

fn le_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn le_f32(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}
