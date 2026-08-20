//! The `.azcar` car asset: what the console loads instead of a 3D model file.
//!
//! A compiled car is a single file that the runtime opens, checks, and draws straight out of. It
//! is not a document to be parsed — everything expensive was decided on a development machine by
//! `tools/anglezero-asset`, and what is left here is a header, a few dozen fixed-size records, and
//! two arrays that go to the hardware untouched.
//!
//! Three things shape the layout:
//!
//! * **Vertices, indices and texture pixels are read in place.** Every section starts on a
//!   16-byte boundary, so when the file is loaded into a 16-byte-aligned buffer the vertex array
//!   is already where the GE wants it, in `GU_TEXTURE_32BITF | GU_COLOR_8888 | GU_VERTEX_32BITF`
//!   order, and the texture is already 5650 for `sceGuTexImage`. Nothing is copied or converted at
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


/// Stable magic. Present in every version.
pub const MAGIC: [u8; 4] = *b"AZCR";
/// The only version this build can read.
pub const VERSION: u16 = 1;
/// Bytes before the first section. A multiple of 16, so section offsets stay aligned.
pub const HEADER_BYTES: usize = 112;

/// The most one compiled car may be.
///
/// The console reads a car into a fixed slot — there is no allocator — so this is a real ceiling
/// rather than advice, and it is here rather than in the console's loader so that the compiler can
/// refuse a car nobody could load. That is the whole trade behind it: a limit that is checked on a
/// development machine, by name, before the file is ever copied, instead of one discovered on a
/// title screen when a car declines to appear.
///
/// 1.25 MB against the largest car this repo compiles at 920 KB.
pub const MAX_CAR_BYTES: usize = 1280 * 1024;

pub const MESH_BYTES: usize = 32;
pub const MATERIAL_BYTES: usize = 16;
pub const WHEEL_BYTES: usize = 32;
pub const LIGHT_BYTES: usize = 32;
/// Nine `f32`, padded to the section alignment.
pub const HANDLING_BYTES: usize = 48;
/// The level table's own header, before the per-level records.
pub const LOD_HEADER_BYTES: usize = 16;
pub const LOD_BYTES: usize = 16;

/// Vertex layout tags written into the header. The field exists so that a change of layout is
/// refused cleanly by a build that predates it rather than drawn as noise.
///
/// `GU_COLOR_8888 | GU_VERTEX_32BITF`. What cars were before they had textures.
pub const VERTEX_COLOR_8888_F32: u32 = 1;
/// `GU_TEXTURE_32BITF | GU_COLOR_8888 | GU_VERTEX_32BITF`, in that order, which is the order the
/// GE reads them in. Floats rather than the 16-bit texture coordinates the hardware also takes:
/// eight bytes a vertex more, and no scale convention to get wrong on a machine with no debugger.
/// The compact form is the obvious next thing to try if a car ever has to be smaller.
pub const VERTEX_TEX_F32_COLOR_8888_F32: u32 = 2;

/// A car's vertex. Field order is the hardware's, not a preference.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CarVertex {
    pub u: f32,
    pub v: f32,
    pub color: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Pixel formats a texture section can be in. Only one so far.
///
/// 5650 rather than anything with alpha: what blends on a car is decided per material by the
/// renderer, and the alpha it uses is in the vertex colour, so a texture channel for it would
/// spend a fifth of every texel on something nothing reads.
pub const TEXTURE_5650: u16 = 0;
/// Width, height, format, flags, then the pixels.
pub const TEXTURE_HEADER_BYTES: usize = 16;

/// The silhouette section's own header: two counts and where its two arrays start, relative to the
/// section. Self-describing, so the arrays can be padded into alignment without the reader having
/// to know how much padding the writer chose.
pub const SILHOUETTE_HEADER_BYTES: usize = 16;

/// A silhouette vertex. A position, and deliberately nothing else — no colour, no texture
/// coordinate, because the thing is drawn in one flat colour and would only be throwing them away.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SilhouetteVertex {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

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

/// Mesh flags.
/// Drawn with back-face culling off, whatever the material says.
///
/// Two-sidedness is a property of a *surface* and the six categories are not fine enough to carry
/// it: a car's bodywork is a closed shell that must be culled and also, in the same category, the
/// black plastic behind the grille, which is one sheet of triangles wound whichever way the model's
/// author left them. Culled, that sheet is a hole you can see the scenery through.
///
/// So the compiler measures which parts read as their own back face across the visibility sweep and
/// puts them in a mesh of their own, sharing the category's material and vertices and costing one
/// more draw call. This is the field the format reserved for a decision like this.
pub const MESH_TWO_SIDED: u16 = 1 << 0;

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
    /// Whether this run has to be drawn with culling off, over and above what its material says.
    pub fn two_sided(&self) -> bool {
        self.flags & MESH_TWO_SIDED != 0
    }

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

/// What a lamp on a car is for.
///
/// Four kinds rather than a light system: what a lamp does is decided by which of the driver's
/// actions switches it on, and these are the four answers. Anything a future car wants — an
/// indicator, a fog lamp — is another kind here and no change to how one is drawn.
///
/// Deliberately *not* validated the way [`Category`] is. A material whose category this build does
/// not know is a surface it cannot draw at all, so the car is refused; a lamp it does not know is
/// one lamp it does not light, and refusing the whole car over it would mean that the first car
/// compiled with indicators could not be loaded by any build that predates them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum LightKind {
    /// Lights the road ahead. On whenever the car is being driven.
    Head = 0,
    /// The rear lamps as they burn normally, and brighter under braking. Most cars use one lens
    /// for both, which is why this is one kind and not two.
    Tail = 1,
    /// A lens that is dark until the brake is applied — a separate high-level lamp, where the car
    /// has one.
    Brake = 2,
    /// White, and only while the car is actually going backwards.
    Reverse = 3,
}

impl LightKind {
    pub fn from_u8(v: u8) -> Option<LightKind> {
        Some(match v {
            0 => LightKind::Head,
            1 => LightKind::Tail,
            2 => LightKind::Brake,
            3 => LightKind::Reverse,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            LightKind::Head => "headlight",
            LightKind::Tail => "tail light",
            LightKind::Brake => "brake light",
            LightKind::Reverse => "reverse light",
        }
    }
}

/// Light flags.
/// The lamp swings with the road wheels rather than staying bolted to the body.
pub const LIGHT_STEERS: u8 = 1 << 0;

/// A lamp on a car: where it is, what colour it burns, and how far it throws.
///
/// This is the whole of the lighting data an asset carries, and it is deliberately not a light
/// source. The console has no per-pixel lighting and is not getting one; what a lamp costs here is
/// a handful of additive triangles, so what the record has to say is where to put them, how big,
/// and how bright — not a photometric description of a bulb.
///
/// `range` and `spread` are the beam's, and are zero for a lamp that only glows. They stand in for
/// the cone angles a real spot light would have: a beam is drawn as a widening patch of light lying
/// on the road, so what it needs is how far up the road it reaches and how wide it is when it gets
/// there, which is that cone intersected with the tarmac and nothing else.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightDef {
    pub kind: LightKind,
    pub flags: u8,
    pub name: u16,
    /// Where the lens sits in car space, with the wheels on the ground at y = 0.
    pub at: [f32; 3],
    /// Colour and full brightness together, `GU_COLOR_8888`. The alpha is the lamp's intensity
    /// when it is fully on, which is what the runtime scales rather than a separate field: every
    /// use of it multiplies into a vertex colour anyway.
    pub color: u32,
    /// Half-size of the lamp's glow, metres.
    pub radius: f32,
    /// How far up the road the beam reaches, metres. Zero for a lamp with no beam.
    pub range: f32,
    /// Half-width of the beam where it lands, metres.
    pub spread: f32,
}

impl LightDef {
    /// Decodes a light, or `None` for a kind this build does not know.
    fn decode(b: &[u8]) -> Option<LightDef> {
        Some(LightDef {
            kind: LightKind::from_u8(b[0])?,
            flags: b[1],
            name: le_u16(b, 2),
            at: [le_f32(b, 4), le_f32(b, 8), le_f32(b, 12)],
            color: le_u32(b, 16),
            radius: le_f32(b, 20),
            range: le_f32(b, 24),
            spread: le_f32(b, 28),
        })
    }

    pub fn encode(&self) -> [u8; LIGHT_BYTES] {
        let mut o = [0u8; LIGHT_BYTES];
        o[0] = self.kind as u8;
        o[1] = self.flags;
        o[2..4].copy_from_slice(&self.name.to_le_bytes());
        for (i, c) in self.at.iter().enumerate() {
            o[4 + i * 4..8 + i * 4].copy_from_slice(&c.to_le_bytes());
        }
        o[16..20].copy_from_slice(&self.color.to_le_bytes());
        o[20..24].copy_from_slice(&self.radius.to_le_bytes());
        o[24..28].copy_from_slice(&self.range.to_le_bytes());
        o[28..32].copy_from_slice(&self.spread.to_le_bytes());
        o
    }

    /// Whether this lamp turns with the front wheels.
    pub fn steers(&self) -> bool {
        self.flags & LIGHT_STEERS != 0
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
    /// Where the car's lamps are, or zero for a car that does not say where its lights are.
    ///
    /// Added the same way `HANDLING_AT` was, and for the same reason it was not a version bump:
    /// `parse` refuses any version but its own, so incrementing it would make every car already on
    /// a memory stick unreadable — to add a section that a build which has never heard of it skips
    /// perfectly well. A car with no lights section is a car with no lamps, which is what every car
    /// was until this.
    pub const LIGHTS_AT: usize = 104;
    /// How many lamps that section holds. In the header, where every other array's count is.
    pub const LIGHT_COUNT: usize = 108;
    /// Where the silhouette is, **in 16-byte units**, or zero for a car that carries none.
    ///
    /// The odd unit is the price of the header being full: `LIGHT_COUNT` ends at 110 and the
    /// header is 112, so there were two bytes left and an offset needs four. Growing the header
    /// was not an option — a reader that expected 128 bytes would refuse every car already on a
    /// memory stick, because their first section starts at 112 — and neither was a version bump,
    /// for the same reason spelled out at `LIGHTS_AT`. Sixteen-byte units reach a megabyte, every
    /// section is 16-byte aligned anyway, and the silhouette is written first, at 112, so seven is
    /// the number that actually goes in here.
    ///
    /// Its counts live in the section rather than beside this, which is what the level table
    /// already does and for the same reason: there was nowhere left to put them.
    pub const SILHOUETTE_AT_16: usize = 110;
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
    light_count: usize,
    meshes_at: usize,
    materials_at: usize,
    wheels_at: usize,
    lights_at: usize,
    vertices_at: usize,
    indices_at: usize,
    strings: (usize, usize),
    handling_at: usize,
    /// Offset of the texture section, or zero for a car drawn on vertex colour alone.
    texture_at: usize,
    /// Offset of the level table and how many levels it holds, or `(0, 0)` for a car with one.
    lods: (usize, usize),
    bounds: [f32; 6],
}

/// The shape of a car, with none of the detail: what stands in while the rest of it is read.
///
/// A car is the better part of a megabyte and arrives over a handful of frames. This is the part
/// that arrives first — a few hundred triangles, positions only, written at the very front of the
/// file so that the load's first chunk already holds it. Drawn flat and dark, it is the car's
/// outline, which is enough to answer a press of L or R on the frame it happened instead of when
/// the read finishes.
///
/// It is a copy of geometry the file already contains, and that duplication is the whole trade: a
/// second seek to the middle of a file, on a memory stick, costs more than the fifteen kilobytes
/// this spends.
#[derive(Clone, Copy, Debug)]
pub struct Silhouette<'a> {
    bytes: &'a [u8],
    vertices_at: usize,
    indices_at: usize,
    vertex_count: usize,
    index_count: usize,
}

impl<'a> Silhouette<'a> {
    /// Finds the silhouette in however much of a file has been read so far.
    ///
    /// `bytes` is a *prefix*: the front of the file, not the whole of it. That is the point — this
    /// is asked before the car exists, and everything it reads has to be checked against what has
    /// actually landed rather than against what the header says the file will be. `None` covers
    /// both "this car has no silhouette" and "not enough of it is here yet", because the caller
    /// does the same thing either way and asks again next frame.
    pub fn parse(bytes: &'a [u8]) -> Option<Silhouette<'a>> {
        if bytes.len() < HEADER_BYTES {
            return None;
        }
        let at = le_u16(bytes, field::SILHOUETTE_AT_16) as usize * 16;
        if at < HEADER_BYTES || at + SILHOUETTE_HEADER_BYTES > bytes.len() {
            return None;
        }

        let vertex_count = le_u32(bytes, at) as usize;
        let index_count = le_u32(bytes, at + 4) as usize;
        let vertices_at = at + le_u32(bytes, at + 8) as usize;
        let indices_at = at + le_u32(bytes, at + 12) as usize;
        if vertex_count == 0 || index_count == 0 || index_count % 3 != 0 {
            return None;
        }
        // Indices are 16-bit, as they are for the car proper.
        if vertex_count > u16::MAX as usize + 1 {
            return None;
        }

        for (start, len, aligned) in [
            (
                vertices_at,
                vertex_count * core::mem::size_of::<SilhouetteVertex>(),
                true,
            ),
            (indices_at, index_count * 2, true),
        ] {
            if start < at || start > bytes.len() || bytes.len() - start < len {
                return None;
            }
            // The GE fetches both of these by DMA out of the buffer the file was read into.
            if aligned && start % 16 != 0 {
                return None;
            }
        }

        // Every index has to be inside the vertex array, checked here rather than trusted, because
        // the alternative is the GE fetching a vertex from beyond the end of the car.
        for i in 0..index_count {
            if le_u16(bytes, indices_at + i * 2) as usize >= vertex_count {
                return None;
            }
        }

        Some(Silhouette {
            bytes,
            vertices_at,
            indices_at,
            vertex_count,
            index_count,
        })
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

    /// One index, decoded. For anything that wants to look at the shape rather than draw it.
    pub fn index(&self, i: usize) -> u16 {
        le_u16(self.bytes, self.indices_at + i * 2)
    }

    /// One vertex, decoded. For anything that wants to look at the shape rather than draw it.
    pub fn vertex(&self, i: usize) -> SilhouetteVertex {
        let at = self.vertices_at + i * core::mem::size_of::<SilhouetteVertex>();
        SilhouetteVertex {
            x: le_f32(self.bytes, at),
            y: le_f32(self.bytes, at + 4),
            z: le_f32(self.bytes, at + 8),
        }
    }

    /// Raw pointer to the positions, for handing to the GE.
    pub fn vertices_ptr(&self) -> *const u8 {
        // Safety: `parse` checked the section lies inside the buffer.
        unsafe { self.bytes.as_ptr().add(self.vertices_at) }
    }

    pub fn indices_ptr(&self) -> *const u8 {
        unsafe { self.bytes.as_ptr().add(self.indices_at) }
    }
}

/// A car's texture, borrowed from the loaded file.
#[derive(Clone, Copy, Debug)]
pub struct Texture<'a> {
    pub width: usize,
    pub height: usize,
    /// `TEXTURE_5650`.
    pub format: u16,
    pub pixels: &'a [u8],
}

/// One level of detail: a run of meshes, and how far away it is good enough.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lod {
    pub first_mesh: u32,
    pub mesh_count: u16,
    /// Use this level beyond this many metres. LOD0's is zero.
    pub min_distance: f32,
    pub triangles: u32,
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
        if format != VERTEX_TEX_F32_COLOR_8888_F32 {
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
        let lights_at = le_u32(bytes, field::LIGHTS_AT) as usize;
        // Both halves have to agree before a lamp is read: a car that says where its lights are but
        // not how many has none, and so does one that says how many but not where.
        let light_count = if lights_at == 0 {
            0
        } else {
            le_u16(bytes, field::LIGHT_COUNT) as usize
        };

        let meshes_at = le_u32(bytes, field::MESHES_AT) as usize;
        let materials_at = le_u32(bytes, field::MATERIALS_AT) as usize;
        let wheels_at = le_u32(bytes, field::WHEELS_AT) as usize;
        let vertices_at = le_u32(bytes, field::VERTICES_AT) as usize;
        let indices_at = le_u32(bytes, field::INDICES_AT) as usize;
        let strings_at = le_u32(bytes, field::STRINGS_AT) as usize;
        let strings_bytes = le_u32(bytes, field::STRINGS_BYTES) as usize;
        let handling_at = le_u32(bytes, field::HANDLING_AT) as usize;
        let texture_at = le_u32(bytes, field::TEXTURES_AT) as usize;
        let texture_count = le_u16(bytes, field::TEXTURE_COUNT) as usize;
        // One atlas or none. More than one would mean the renderer binding per mesh, which is the
        // cost the atlas exists to avoid, so the format says so rather than leaving it implied.
        if texture_count > 1 {
            return Err(Error::BadSection);
        }
        let texture_bytes = if texture_count == 1 && texture_at + TEXTURE_HEADER_BYTES <= bytes.len()
        {
            let w = le_u16(bytes, texture_at) as usize;
            let h = le_u16(bytes, texture_at + 2) as usize;
            TEXTURE_HEADER_BYTES + w * h * 2
        } else {
            0
        };
        let lods_at = le_u32(bytes, field::LODS_AT) as usize;
        // The count lives in the section rather than the header, because the header had one field
        // left for this and the section is the thing that grows.
        let lod_count = if lods_at != 0 && lods_at + LOD_HEADER_BYTES <= bytes.len() {
            le_u16(bytes, lods_at) as usize
        } else {
            0
        };

        let car = Car {
            bytes,
            vertex_count,
            index_count,
            mesh_count,
            material_count,
            wheel_count,
            light_count,
            meshes_at,
            materials_at,
            wheels_at,
            lights_at,
            vertices_at,
            indices_at,
            strings: (strings_at, strings_bytes),
            handling_at,
            texture_at: if texture_bytes > 0 { texture_at } else { 0 },
            lods: (lods_at, lod_count),
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
            (lights_at, light_count * LIGHT_BYTES, true),
            (
                vertices_at,
                vertex_count * core::mem::size_of::<CarVertex>(),
                true,
            ),
            (indices_at, index_count * 2, true),
            (strings_at, strings_bytes, false),
            // Zero means the car does not carry one. Both optional sections say so with the
            // offset, so the length is what has to be zeroed to skip the check.
            (
                handling_at,
                if handling_at == 0 { 0 } else { HANDLING_BYTES },
                true,
            ),
            (
                lods_at,
                if lod_count == 0 {
                    0
                } else {
                    LOD_HEADER_BYTES + lod_count * LOD_BYTES
                },
                true,
            ),
            (texture_at, texture_bytes, true),
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

        // Every mesh, not just LOD0's: the coarser levels live past `mesh_count` in the same array
        // and are just as capable of dangling. `total_meshes` is what the level table reaches.
        let mut total_meshes = mesh_count;
        for i in 0..lod_count {
            let at = lods_at + LOD_HEADER_BYTES + i * LOD_BYTES;
            let end = le_u32(bytes, at) as usize + le_u16(bytes, at + 4) as usize;
            total_meshes = total_meshes.max(end);
        }
        if meshes_at + total_meshes * MESH_BYTES > bytes.len() {
            return Err(Error::BadSection);
        }

        for i in 0..total_meshes {
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

    /// How many bytes the file is, which is how much of a slot it occupies on the console.
    pub fn byte_len(&self) -> usize {
        self.bytes.len()
    }

    pub fn vertex_count(&self) -> usize {
        self.vertex_count
    }

    pub fn index_count(&self) -> usize {
        self.index_count
    }

    /// Triangles in the car as it is drawn up close — LOD0 only.
    ///
    /// Not the whole index array: the coarser levels live in it too, and a car that reported 14,774
    /// triangles because it carries three copies of itself would be answering a question nobody
    /// asked. What the budget was spent on, and what the console draws for the player's car, is
    /// this one.
    pub fn triangle_count(&self) -> usize {
        let lod0 = self.lod(0);
        let mut indices = 0;
        for i in 0..lod0.mesh_count as usize {
            indices += self.mesh(lod0.first_mesh as usize + i).index_count as usize;
        }
        indices / 3
    }

    /// Triangles across every level, which is what the file actually costs to hold.
    pub fn total_triangle_count(&self) -> usize {
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

    /// How many lamps the car carries, including any this build cannot read.
    pub fn light_count(&self) -> usize {
        self.light_count
    }

    /// One lamp, or `None` for a kind this build does not know how to light.
    pub fn light(&self, i: usize) -> Option<LightDef> {
        if i >= self.light_count {
            return None;
        }
        LightDef::decode(&self.bytes[self.lights_at + i * LIGHT_BYTES..])
    }

    /// Every lamp this build can light, in the order the compiler wrote them.
    ///
    /// The order is not arbitrary and the renderer relies on it being stable: lamps are drawn in
    /// one additive pass, and two lenses at the same place blended in either order come out the
    /// same, which is the property that lets this be an iterator rather than a sort.
    pub fn lights(&self) -> impl Iterator<Item = LightDef> + '_ {
        (0..self.light_count).filter_map(|i| self.light(i))
    }

    /// min xyz, max xyz, in car space with the wheels on the ground at y = 0.
    pub fn bounds(&self) -> [f32; 6] {
        self.bounds
    }

    /// The vertex array, laid out exactly as the GU consumes it.
    pub fn vertices(&self) -> &'a [CarVertex] {
        // Safety: `parse` checked that the section is 16-byte aligned inside a buffer the caller
        // aligned, and that `vertex_count` vertices fit. `CarVertex` is `repr(C)` over six 4-byte
        // fields with no padding and no invalid bit patterns.
        unsafe {
            core::slice::from_raw_parts(
                self.bytes.as_ptr().add(self.vertices_at) as *const CarVertex,
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

    /// The car's texture: size, format, and the pixels, ready for `sceGuTexImage`.
    ///
    /// One for the whole car. Every source material has a tile in it and the compiler rewrote the
    /// UVs to match, so the renderer binds this once and never switches texture again — which is
    /// the entire reason the compiler packs an atlas rather than keeping textures apart.
    pub fn texture(&self) -> Option<Texture<'a>> {
        if self.texture_at == 0 {
            return None;
        }
        let at = self.texture_at;
        let width = le_u16(self.bytes, at) as usize;
        let height = le_u16(self.bytes, at + 2) as usize;
        Some(Texture {
            width,
            height,
            format: le_u16(self.bytes, at + 4),
            pixels: &self.bytes[at + TEXTURE_HEADER_BYTES..at + TEXTURE_HEADER_BYTES + width * height * 2],
        })
    }

    /// The car's silhouette, if it carries one.
    ///
    /// Checked here rather than in `parse`, and by the same code that checks it mid-load: the
    /// section is additive, so a car whose silhouette does not check out is a car with no
    /// silhouette, not a car that cannot be drawn. Refusing the whole file over a stand-in nobody
    /// sees for half a second would be the wrong trade entirely.
    pub fn silhouette(&self) -> Option<Silhouette<'a>> {
        Silhouette::parse(self.bytes)
    }

    /// How many levels of detail the car carries. One means only the meshes `mesh_count` covers.
    pub fn lod_count(&self) -> usize {
        self.lods.1.max(1)
    }

    /// One level. Level 0 is always the full-detail car, whether or not a table was written.
    pub fn lod(&self, i: usize) -> Lod {
        if i >= self.lods.1 {
            return Lod {
                first_mesh: 0,
                mesh_count: self.mesh_count as u16,
                min_distance: 0.0,
                triangles: (self.index_count / 3) as u32,
            };
        }
        let at = self.lods.0 + LOD_HEADER_BYTES + i * LOD_BYTES;
        Lod {
            first_mesh: le_u32(self.bytes, at),
            mesh_count: le_u16(self.bytes, at + 4),
            min_distance: le_f32(self.bytes, at + 8),
            triangles: le_u32(self.bytes, at + 12),
        }
    }

    /// The coarsest level that is still good enough at this distance.
    ///
    /// Walked from the far end so that the answer is the last level whose threshold the distance
    /// is past. Levels are written near-to-far and LOD0's threshold is zero, so this always finds
    /// one — a car with no table finds level 0 and draws exactly what it always drew.
    pub fn lod_for_distance(&self, metres: f32) -> Lod {
        let mut chosen = self.lod(0);
        for i in (1..self.lod_count()).rev() {
            let lod = self.lod(i);
            if metres >= lod.min_distance {
                chosen = lod;
                break;
            }
        }
        chosen
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
