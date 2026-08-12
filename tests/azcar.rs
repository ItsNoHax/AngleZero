//! The compiled car format, from the reading side.
//!
//! The console opens these files off a memory stick that can be stale, half-copied, or left over
//! from an older build of the asset tool. Every one of those has to come back as a refusal rather
//! than as a car drawn from whatever the bytes happened to say, so most of this file is about what
//! `Car::parse` rejects.
//!
//! The bytes are assembled here by hand rather than by calling the asset tool, on purpose: it
//! makes the test an independent statement of the layout, so a writer that drifts is caught rather
//! than agreed with.

use angle_zero::azcar::{
    field, Car, Category, Error, MaterialDef, Mesh, WheelDef, HEADER_BYTES, MAGIC, MATERIAL_BLEND,
    MATERIAL_BYTES, MESH_BYTES, NO_TEXTURE, NO_WHEEL, VERSION, VERTEX_COLOR_8888_F32, WHEEL_BYTES,
    WHEEL_FRONT_LEFT,
};
use angle_zero::mesh::Vertex;

/// A car with one body mesh and one wheel, laid out the way the writer lays one out.
struct Builder {
    vertices: Vec<Vertex>,
    indices: Vec<u16>,
    meshes: Vec<Mesh>,
    materials: Vec<MaterialDef>,
    wheels: Vec<WheelDef>,
    strings: Vec<u8>,
    version: u16,
    vertex_format: u32,
}

impl Builder {
    fn typical() -> Builder {
        let mut strings = Vec::new();
        let body_name = push_name(&mut strings, "body");
        let wheel_name = push_name(&mut strings, "wheel_fl");
        let paint_name = push_name(&mut strings, "paint");
        let glass_name = push_name(&mut strings, "glass");

        Builder {
            // Six vertices: a triangle for the body, a triangle for the wheel.
            vertices: (0..6)
                .map(|i| Vertex::new(i as f32, 1.0, 2.0, 0xFF00_1122))
                .collect(),
            indices: vec![0, 1, 2, 3, 4, 5],
            meshes: vec![
                Mesh {
                    first_index: 0,
                    index_count: 3,
                    material: 0,
                    wheel: NO_WHEEL,
                    name: body_name,
                    flags: 0,
                    center: [0.0, 0.6, 0.0],
                    radius: 2.2,
                },
                Mesh {
                    first_index: 3,
                    index_count: 3,
                    material: 1,
                    wheel: 0,
                    name: wheel_name,
                    flags: 0,
                    center: [0.0, 0.0, 0.0],
                    radius: 0.3,
                },
            ],
            materials: vec![
                MaterialDef {
                    color: 0xFF20_3040,
                    texture: NO_TEXTURE,
                    name: paint_name,
                    category: Category::Body,
                    flags: 0,
                },
                MaterialDef {
                    color: 0x8010_1010,
                    texture: NO_TEXTURE,
                    name: glass_name,
                    category: Category::Window,
                    flags: MATERIAL_BLEND,
                },
            ],
            wheels: vec![WheelDef {
                corner: WHEEL_FRONT_LEFT,
                steers: true,
                name: wheel_name,
                hub: [-0.73, 0.29, 1.36],
                radius: 0.29,
                width: 0.2,
            }],
            strings,
            version: VERSION,
            vertex_format: VERTEX_COLOR_8888_F32,
        }
    }

    fn build(&self) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_BYTES];

        let meshes_at = align(&mut out);
        for m in &self.meshes {
            out.extend_from_slice(&m.encode());
        }
        let materials_at = align(&mut out);
        for m in &self.materials {
            out.extend_from_slice(&m.encode());
        }
        let wheels_at = align(&mut out);
        for w in &self.wheels {
            out.extend_from_slice(&w.encode());
        }
        let vertices_at = align(&mut out);
        for v in &self.vertices {
            out.extend_from_slice(&v.color.to_le_bytes());
            out.extend_from_slice(&v.x.to_le_bytes());
            out.extend_from_slice(&v.y.to_le_bytes());
            out.extend_from_slice(&v.z.to_le_bytes());
        }
        let indices_at = align(&mut out);
        for i in &self.indices {
            out.extend_from_slice(&i.to_le_bytes());
        }
        let strings_at = align(&mut out);
        out.extend_from_slice(&self.strings);

        out[field::MAGIC..4].copy_from_slice(&MAGIC);
        put_u16(&mut out, field::VERSION, self.version);
        put_u32(&mut out, field::VERTEX_FORMAT, self.vertex_format);
        put_u32(&mut out, field::VERTEX_COUNT, self.vertices.len() as u32);
        put_u32(&mut out, field::INDEX_COUNT, self.indices.len() as u32);
        put_u16(&mut out, field::MESH_COUNT, self.meshes.len() as u16);
        put_u16(&mut out, field::MATERIAL_COUNT, self.materials.len() as u16);
        put_u16(&mut out, field::WHEEL_COUNT, self.wheels.len() as u16);
        for (i, v) in [-0.9f32, 0.0, -2.1, 0.9, 1.3, 2.1].iter().enumerate() {
            put_f32(&mut out, field::BOUNDS + i * 4, *v);
        }
        put_u32(&mut out, field::MESHES_AT, meshes_at as u32);
        put_u32(&mut out, field::MATERIALS_AT, materials_at as u32);
        put_u32(&mut out, field::WHEELS_AT, wheels_at as u32);
        put_u32(&mut out, field::VERTICES_AT, vertices_at as u32);
        put_u32(&mut out, field::INDICES_AT, indices_at as u32);
        put_u32(&mut out, field::STRINGS_AT, strings_at as u32);
        put_u32(&mut out, field::STRINGS_BYTES, self.strings.len() as u32);
        let total = out.len() as u32;
        put_u32(&mut out, field::LENGTH, total);
        out
    }
}

fn align(out: &mut Vec<u8>) -> usize {
    while out.len() % 16 != 0 {
        out.push(0);
    }
    out.len()
}

fn push_name(strings: &mut Vec<u8>, name: &str) -> u16 {
    let at = strings.len() as u16;
    strings.extend_from_slice(name.as_bytes());
    strings.push(0);
    at
}

fn put_u16(out: &mut [u8], at: usize, v: u16) {
    out[at..at + 2].copy_from_slice(&v.to_le_bytes());
}

fn put_u32(out: &mut [u8], at: usize, v: u32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn put_f32(out: &mut [u8], at: usize, v: f32) {
    out[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

/// Parsing has to be done out of an aligned buffer, which is what the runtime arena gives it.
fn aligned(bytes: &[u8]) -> Vec<Vertex> {
    let mut buf = vec![Vertex::ZERO; bytes.len().div_ceil(16)];
    // Safety: `Vertex` is four 4-byte fields, so the vector is 16 bytes per element and at least
    // 4-byte aligned; in practice the allocator gives more, which is what the console's static
    // arena guarantees outright.
    unsafe {
        core::ptr::copy_nonoverlapping(
            bytes.as_ptr(),
            buf.as_mut_ptr() as *mut u8,
            bytes.len(),
        );
    }
    buf
}

fn parse_bytes(bytes: &[u8]) -> Result<(), Error> {
    let backing = aligned(bytes);
    let view =
        unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    Car::parse(view).map(|_| ())
}

#[test]
fn a_well_formed_car_reads_back_exactly_what_was_written() {
    let b = Builder::typical();
    let bytes = b.build();
    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    let car = Car::parse(view).expect("a car the writer would produce must parse");

    assert_eq!(car.vertex_count(), 6);
    assert_eq!(car.index_count(), 6);
    assert_eq!(car.triangle_count(), 2);
    assert_eq!(car.mesh_count(), 2);
    assert_eq!(car.material_count(), 2);
    assert_eq!(car.wheel_count(), 1);

    // The vertex array is read in place, so this is the check that the layout the GE will walk is
    // the layout that was written.
    let verts = car.vertices();
    assert_eq!(verts.len(), 6);
    assert_eq!(verts[3], Vertex::new(3.0, 1.0, 2.0, 0xFF00_1122));
    assert_eq!(car.indices(), &[0, 1, 2, 3, 4, 5]);

    let body = car.mesh(0);
    assert_eq!(body.wheel, NO_WHEEL);
    assert_eq!(body.index_count, 3);
    assert_eq!(car.name(body.name), b"body");

    let glass = car.material(1);
    assert_eq!(glass.category, Category::Window);
    assert!(glass.blended());
    assert!(!car.material(0).blended());
    assert_eq!(car.name(glass.name), b"glass");

    let w = car.wheel(0);
    assert_eq!(w.corner, WHEEL_FRONT_LEFT);
    assert!(w.steers);
    assert!((w.radius - 0.29).abs() < 1e-6);
    assert!((w.hub[2] - 1.36).abs() < 1e-6);
    assert_eq!(car.name(w.name), b"wheel_fl");

    assert_eq!(car.bounds()[4], 1.3);
}

#[test]
fn anything_that_is_not_a_car_is_refused() {
    assert_eq!(parse_bytes(&[]), Err(Error::TooShort));
    assert_eq!(parse_bytes(&[0u8; HEADER_BYTES - 1]), Err(Error::TooShort));
    assert_eq!(parse_bytes(&[0u8; HEADER_BYTES]), Err(Error::BadMagic));
}

/// The whole reason there is a version field: a car built by a different asset tool must not be
/// drawn as if it were this one.
#[test]
fn a_car_from_another_version_is_refused_rather_than_guessed_at() {
    let mut b = Builder::typical();
    b.version = VERSION + 1;
    assert_eq!(
        parse_bytes(&b.build()),
        Err(Error::UnsupportedVersion(VERSION + 1))
    );

    let mut b = Builder::typical();
    b.vertex_format = 99;
    assert_eq!(
        parse_bytes(&b.build()),
        Err(Error::UnsupportedVertexFormat(99))
    );
}

/// A file that stopped copying half way is the common memory-stick failure, and its length field
/// is the cheapest thing that catches it.
#[test]
fn a_truncated_file_is_caught_by_its_own_length() {
    let bytes = Builder::typical().build();
    let short = &bytes[..bytes.len() - 16];
    assert_eq!(parse_bytes(short), Err(Error::LengthMismatch));
}

#[test]
fn a_section_pointing_outside_the_file_is_refused() {
    let mut bytes = Builder::typical().build();
    put_u32(&mut bytes, field::VERTICES_AT, 0xFFFF_0000);
    assert_eq!(parse_bytes(&bytes), Err(Error::BadSection));

    let mut bytes = Builder::typical().build();
    // Inside the file, but overlapping the header, which no section may do.
    put_u32(&mut bytes, field::MESHES_AT, 16);
    assert_eq!(parse_bytes(&bytes), Err(Error::BadSection));
}

/// Vertices are handed to the hardware without a copy, so a section that is not 16-byte aligned
/// would be read by the GE at the wrong address. It has to be a refusal, not a fix-up.
#[test]
fn a_misaligned_vertex_section_is_refused() {
    let mut bytes = Builder::typical().build();
    let at = u32::from_le_bytes(bytes[field::VERTICES_AT..field::VERTICES_AT + 4].try_into().unwrap());
    put_u32(&mut bytes, field::VERTICES_AT, at + 4);
    assert_eq!(parse_bytes(&bytes), Err(Error::Misaligned));
}

#[test]
fn references_that_go_nowhere_are_refused() {
    // An index past the end of the vertex array.
    let mut b = Builder::typical();
    b.indices[2] = 99;
    assert_eq!(parse_bytes(&b.build()), Err(Error::DanglingReference));

    // A mesh naming a material that does not exist.
    let mut b = Builder::typical();
    b.meshes[0].material = 7;
    assert_eq!(parse_bytes(&b.build()), Err(Error::DanglingReference));

    // A mesh naming a wheel that does not exist.
    let mut b = Builder::typical();
    b.meshes[1].wheel = 3;
    assert_eq!(parse_bytes(&b.build()), Err(Error::DanglingReference));

    // A run of indices that leaves the buffer.
    let mut b = Builder::typical();
    b.meshes[1].index_count = 30;
    assert_eq!(parse_bytes(&b.build()), Err(Error::DanglingReference));

    // A run that is not whole triangles.
    let mut b = Builder::typical();
    b.meshes[1].index_count = 2;
    assert_eq!(parse_bytes(&b.build()), Err(Error::DanglingReference));
}

#[test]
fn an_unknown_material_category_is_refused() {
    let mut bytes = Builder::typical().build();
    let materials_at =
        u32::from_le_bytes(bytes[field::MATERIALS_AT..field::MATERIALS_AT + 4].try_into().unwrap())
            as usize;
    bytes[materials_at + MATERIAL_BYTES + 8] = 42;
    assert_eq!(parse_bytes(&bytes), Err(Error::DanglingReference));
}

#[test]
fn a_car_with_nothing_to_draw_is_refused() {
    let mut b = Builder::typical();
    b.meshes.clear();
    assert_eq!(parse_bytes(&b.build()), Err(Error::BadSection));
}

/// Names are diagnostics, so a bad one must not be able to take the car down with it.
#[test]
fn a_name_offset_past_the_string_table_reads_as_empty() {
    let bytes = Builder::typical().build();
    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    let car = Car::parse(view).unwrap();
    assert_eq!(car.name(9999), b"");
}

/// The header has to be a whole number of 16-byte lines, or the first section after it — and so
/// every section after that — lands somewhere the GE cannot be pointed at.
#[test]
fn the_header_is_a_whole_number_of_alignment_units() {
    assert_eq!(HEADER_BYTES % 16, 0);
    assert_eq!(MESH_BYTES % 16, 0);
    assert_eq!(MATERIAL_BYTES % 16, 0);
    assert_eq!(WHEEL_BYTES % 16, 0);
    assert_eq!(core::mem::size_of::<Vertex>(), 16);
}
