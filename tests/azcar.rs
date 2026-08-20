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
    field, Car, Category, Error, LightDef, LightKind, MaterialDef, Mesh, WheelDef, HEADER_BYTES,
    LIGHT_BYTES, LIGHT_STEERS, MAGIC, MATERIAL_BLEND, MATERIAL_BYTES, MESH_BYTES, NO_CREDIT,
    NO_TEXTURE, NO_WHEEL, VERSION, VERTEX_TEX_F32_COLOR_8888_F32, WHEEL_BYTES, WHEEL_FRONT_LEFT,
};
use angle_zero::azcar::{CarVertex, Silhouette};
use angle_zero::vehicle::CarShape;

/// A car with one body mesh and one wheel, laid out the way the writer lays one out.
struct Builder {
    vertices: Vec<CarVertex>,
    indices: Vec<u16>,
    meshes: Vec<Mesh>,
    materials: Vec<MaterialDef>,
    wheels: Vec<WheelDef>,
    /// Empty for a car from before there were lights, which must still load.
    lights: Vec<LightDef>,
    strings: Vec<u8>,
    version: u16,
    vertex_format: u32,
    credit: u32,
    name: u32,
    /// Positions and indices for the stand-in drawn while the file is still arriving. `None` for a
    /// car compiled before there were any, which must still load.
    silhouette: Option<(Vec<[f32; 3]>, Vec<u16>)>,
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
                .map(|i| CarVertex {
                    u: i as f32 * 0.1,
                    v: 0.25,
                    color: 0xFF00_1122,
                    x: i as f32,
                    y: 1.0,
                    z: 2.0,
                })
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
                camber: 0.0,
            }],
            lights: Vec::new(),
            strings,
            version: VERSION,
            vertex_format: VERTEX_TEX_F32_COLOR_8888_F32,
            credit: NO_CREDIT,
            name: NO_CREDIT,
            silhouette: None,
        }
    }

    fn build(&self) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_BYTES];

        // First of everything, because the console draws it from the first chunk of a load that
        // has not finished. Written by hand here, like the rest of this file, so the test states
        // the layout rather than agreeing with the writer's idea of it.
        let silhouette_at = match &self.silhouette {
            None => 0,
            Some((positions, indices)) => {
                let at = align(&mut out);
                out.extend_from_slice(&(positions.len() as u32).to_le_bytes());
                out.extend_from_slice(&(indices.len() as u32).to_le_bytes());
                let arrays_at = out.len();
                out.extend_from_slice(&[0u8; 8]);
                let positions_at = align(&mut out);
                for p in positions {
                    for v in p {
                        out.extend_from_slice(&v.to_le_bytes());
                    }
                }
                let indices_at = align(&mut out);
                for i in indices {
                    out.extend_from_slice(&i.to_le_bytes());
                }
                put_u32(&mut out, arrays_at, (positions_at - at) as u32);
                put_u32(&mut out, arrays_at + 4, (indices_at - at) as u32);
                at
            }
        };

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
        // Written only when the car has any, exactly as the writer does it: the offset staying zero
        // is what tells a reader there are none.
        let lights_at = if self.lights.is_empty() {
            0
        } else {
            let at = align(&mut out);
            for l in &self.lights {
                out.extend_from_slice(&l.encode());
            }
            at
        };
        let vertices_at = align(&mut out);
        for v in &self.vertices {
            // Texture, colour, position — the order the GE reads a vertex in, written out by hand
            // so that this test says what the layout is rather than agreeing with whatever the
            // struct happens to be.
            out.extend_from_slice(&v.u.to_le_bytes());
            out.extend_from_slice(&v.v.to_le_bytes());
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
        put_u32(&mut out, field::LIGHTS_AT, lights_at as u32);
        put_u16(&mut out, field::LIGHT_COUNT, self.lights.len() as u16);
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
        put_u32(&mut out, field::CREDIT, self.credit);
        put_u32(&mut out, field::NAME, self.name);
        // In 16-byte units: the header had two bytes left and an offset needs four.
        put_u16(
            &mut out,
            field::SILHOUETTE_AT_16,
            (silhouette_at / 16) as u16,
        );
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
/// A 16-byte-aligned box to copy a test car into.
///
/// The console reads a car straight out of a `repr(align(16))` static, and `Car::parse` refuses a
/// misaligned vertex section, so a test that fed it a plain `Vec<u8>` would be testing a case the
/// runtime never has.
#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct Align16([u8; 16]);

fn aligned(bytes: &[u8]) -> Vec<Align16> {
    let mut buf = vec![Align16([0u8; 16]); bytes.len().div_ceil(16)];
    // Safety: the vector is 16 bytes per element and 16-byte aligned by the type, which is what
    // the console's static arena guarantees outright.
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
    assert_eq!(
        verts[3],
        CarVertex {
            u: 0.3,
            v: 0.25,
            color: 0xFF00_1122,
            x: 3.0,
            y: 1.0,
            z: 2.0,
        }
    );
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

/// What the simulation takes off the asset. Getting these wrong is not a crash: it is wheels that
/// scrub the road at a speed that does not match the car's, and tyre marks laid beside the tyres.
#[test]
fn the_simulation_measures_the_car_it_was_given() {
    let mut b = Builder::typical();
    b.wheels = vec![
        WheelDef {
            corner: WHEEL_FRONT_LEFT,
            steers: true,
            name: 0,
            hub: [0.73, 0.29, 1.30],
            radius: 0.29,
            width: 0.2,
            camber: 0.0,
        },
        WheelDef {
            corner: 2,
            steers: false,
            name: 0,
            hub: [0.71, 0.29, -1.28],
            radius: 0.29,
            width: 0.2,
            camber: 0.0,
        },
        WheelDef {
            corner: 3,
            steers: false,
            name: 0,
            hub: [-0.75, 0.29, -1.30],
            radius: 0.29,
            width: 0.2,
            camber: 0.0,
        },
    ];
    let bytes = b.build();
    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    let shape = Car::parse(view).unwrap().shape();

    assert!((shape.wheel_radius - 0.29).abs() < 1e-6);
    // Both rear hubs, averaged, and only the rear ones: the front hub is at +1.30 and would drag
    // the answer to nearly zero if it were counted.
    assert!((shape.rear_hub_z + 1.29).abs() < 1e-5, "got {}", shape.rear_hub_z);
    // Averaged as distances from the centreline, so a model that is not quite symmetric does not
    // put one mark inside the car.
    assert!((shape.rear_hub_x - 0.73).abs() < 1e-5, "got {}", shape.rear_hub_x);
}

/// A car with no wheels still has to be drivable. Zeroes here would divide the wheel spin by
/// nothing and pile every tyre mark on the car's own origin.
#[test]
fn a_car_without_wheels_falls_back_rather_than_to_zero() {
    let mut b = Builder::typical();
    b.wheels.clear();
    b.meshes[1].wheel = NO_WHEEL;
    let bytes = b.build();
    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    let shape = Car::parse(view).unwrap().shape();
    assert_eq!(shape, CarShape::DEFAULT);
    assert!(shape.wheel_radius > 0.0);
}

/// The licences these models come under require attribution, so the credit travels inside the car
/// rather than in a readme a rebuild can lose. A car without one must read as empty rather than as
/// whatever string happens to sit at offset zero.
#[test]
fn the_attribution_line_comes_out_of_the_car() {
    let mut b = Builder::typical();
    b.credit = push_name(&mut b.strings, "MODEL BY BLACK SNOW, CC-BY-4.0") as u32;
    let bytes = b.build();
    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    assert_eq!(
        Car::parse(view).unwrap().credit(),
        b"MODEL BY BLACK SNOW, CC-BY-4.0"
    );

    let plain = Builder::typical().build();
    let backing = aligned(&plain);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, plain.len()) };
    assert_eq!(Car::parse(view).unwrap().credit(), b"");
}

/// The header has to be a whole number of 16-byte lines, or the first section after it — and so
/// every section after that — lands somewhere the GE cannot be pointed at.
#[test]
fn the_header_is_a_whole_number_of_alignment_units() {
    assert_eq!(HEADER_BYTES % 16, 0);
    assert_eq!(MESH_BYTES % 16, 0);
    assert_eq!(MATERIAL_BYTES % 16, 0);
    assert_eq!(WHEEL_BYTES % 16, 0);
    assert_eq!(LIGHT_BYTES % 16, 0);
    // The vertex array is read in place by the GE, so its record has to be a whole number of
    // 4-byte fields with no padding, and the section it starts has to stay 16-byte aligned.
    assert_eq!(core::mem::size_of::<CarVertex>(), 24);
}

/// The four lamps of a typical car, in the order the writer emits them.
fn typical_lights() -> Vec<LightDef> {
    vec![
        LightDef {
            kind: LightKind::Head,
            flags: LIGHT_STEERS,
            name: 0,
            at: [0.62, 0.70, 2.05],
            color: 0x59D2_F3FF,
            radius: 0.34,
            range: 24.0,
            spread: 2.4,
        },
        LightDef {
            kind: LightKind::Tail,
            flags: 0,
            name: 0,
            at: [-0.64, 0.96, -2.02],
            color: 0xFF44_55FF,
            radius: 0.30,
            range: 0.0,
            spread: 0.0,
        },
    ]
}

#[test]
fn the_lamps_read_back_exactly_what_was_written() {
    let mut b = Builder::typical();
    b.lights = typical_lights();
    let bytes = b.build();
    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    let car = Car::parse(view).expect("a car with lamps is still a car");

    assert_eq!(car.light_count(), 2);
    assert_eq!(car.light(0), Some(b.lights[0]));
    assert_eq!(car.light(1), Some(b.lights[1]));
    assert_eq!(car.light(2), None, "past the end is nothing, not the next record");
    assert!(car.light(0).unwrap().steers());
    assert!(!car.light(1).unwrap().steers());
    assert_eq!(car.lights().count(), 2);
}

/// The whole reason lights went into a reserved header field instead of a version bump: every car
/// already compiled has to keep loading, and keep driving, with no lamps and no complaint.
#[test]
fn a_car_from_before_there_were_lights_still_loads() {
    let bytes = Builder::typical().build();
    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    let car = Car::parse(view).expect("a car with no lights section is a car");

    assert_eq!(car.light_count(), 0);
    assert_eq!(car.light(0), None);
    assert_eq!(car.lights().count(), 0);
    // And nothing else about it has moved.
    assert_eq!(car.mesh_count(), 2);
    assert_eq!(car.wheel_count(), 1);
}

/// A kind this build has never heard of is one lamp it does not light, not a car it refuses. The
/// alternative would mean the first car compiled with indicators on it could not be loaded by any
/// build that predates them — which is the failure the format's reserved space exists to avoid.
#[test]
fn a_lamp_of_an_unknown_kind_is_skipped_rather_than_refusing_the_car() {
    let mut b = Builder::typical();
    b.lights = typical_lights();
    let mut bytes = b.build();

    // Reach into the written record and make the first lamp a kind from the future.
    let at = u32::from_le_bytes(bytes[field::LIGHTS_AT..field::LIGHTS_AT + 4].try_into().unwrap());
    bytes[at as usize] = 99;

    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    let car = Car::parse(view).expect("an unreadable lamp must not cost the whole car");

    assert_eq!(car.light_count(), 2, "the count is what the file says");
    assert_eq!(car.light(0), None, "but the lamp itself cannot be read");
    assert_eq!(car.lights().count(), 1, "so only the one that can be is drawn");
    assert_eq!(car.lights().next().unwrap().kind, LightKind::Tail);
}

/// A lights section that runs off the end of the file is a refusal like any other. The renderer
/// reads these without bounds checks, so this is the check.
#[test]
fn a_lights_section_that_does_not_fit_is_refused() {
    let mut b = Builder::typical();
    b.lights = typical_lights();
    let mut bytes = b.build();
    put_u16(&mut bytes, field::LIGHT_COUNT, 4000);
    assert_eq!(parse_bytes(&bytes), Err(Error::BadSection));

    let mut bytes = b.build();
    let at = u32::from_le_bytes(bytes[field::LIGHTS_AT..field::LIGHTS_AT + 4].try_into().unwrap());
    put_u32(&mut bytes, field::LIGHTS_AT, at + 1);
    assert_eq!(parse_bytes(&bytes), Err(Error::Misaligned));
}

/// Both halves have to agree. A count with no section behind it would have the reader decoding
/// whatever sits at offset zero, which is the header.
#[test]
fn a_light_count_without_a_section_is_no_lights_at_all() {
    let mut bytes = Builder::typical().build();
    put_u16(&mut bytes, field::LIGHT_COUNT, 6);
    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    assert_eq!(Car::parse(view).unwrap().light_count(), 0);
}

/// A silhouette to stand in for a car: a triangle at each corner of nothing in particular. The
/// shape does not matter here; where it is written and what it survives does.
fn typical_silhouette() -> (Vec<[f32; 3]>, Vec<u16>) {
    (
        vec![
            [-0.9, 0.0, -2.1],
            [0.9, 0.0, -2.1],
            [0.9, 1.3, 2.1],
            [-0.9, 1.3, 2.1],
        ],
        vec![0, 1, 2, 0, 2, 3],
    )
}

#[test]
fn a_silhouette_reads_back_exactly_what_was_written() {
    let mut b = Builder::typical();
    let (positions, indices) = typical_silhouette();
    b.silhouette = Some((positions.clone(), indices.clone()));
    let bytes = b.build();
    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };

    let car = Car::parse(view).unwrap();
    let sil = car.silhouette().expect("the car carries one");
    assert_eq!(sil.vertex_count(), positions.len());
    assert_eq!(sil.index_count(), indices.len());
    assert_eq!(sil.triangle_count(), 2);
    for (i, p) in positions.iter().enumerate() {
        let v = sil.vertex(i);
        assert_eq!([v.x, v.y, v.z], *p, "vertex {i}");
    }
}

/// The whole reason the section is where it is: the console draws it out of a file it has only
/// partly read, so it has to be found in a prefix — and refused, quietly, in a prefix that stops
/// short of it.
#[test]
fn a_silhouette_is_readable_before_the_rest_of_the_car_has_arrived() {
    let mut b = Builder::typical();
    b.silhouette = Some(typical_silhouette());
    let bytes = b.build();
    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };

    // Where the silhouette ends: everything from here on is a prefix that holds it.
    let whole = Silhouette::parse(view).expect("the whole file certainly holds one");
    let mut first_complete = None;
    for n in 0..=view.len() {
        let landed = Silhouette::parse(&view[..n]).is_some();
        if landed && first_complete.is_none() {
            first_complete = Some(n);
        }
        // Once it is readable it stays readable: a load only ever adds bytes.
        if let Some(at) = first_complete {
            assert_eq!(landed, n >= at, "at {n} bytes");
        }
    }
    let first_complete = first_complete.expect("a prefix reads it");
    assert!(
        first_complete < view.len(),
        "the silhouette must arrive before the file does, or it is worth nothing"
    );
    // And what a prefix reads is what the whole file reads.
    let early = Silhouette::parse(&view[..first_complete]).unwrap();
    assert_eq!(early.vertex_count(), whole.vertex_count());
    assert_eq!(early.index_count(), whole.index_count());
}

/// It is written in front of every other section, which is what makes the prefix above short.
#[test]
fn the_silhouette_comes_before_everything_else_in_the_file() {
    let mut b = Builder::typical();
    b.silhouette = Some(typical_silhouette());
    let bytes = b.build();
    let at = u16::from_le_bytes(
        bytes[field::SILHOUETTE_AT_16..field::SILHOUETTE_AT_16 + 2]
            .try_into()
            .unwrap(),
    ) as usize
        * 16;
    assert_eq!(at, HEADER_BYTES, "there is nothing to put in front of it");
    for field_at in [
        field::MESHES_AT,
        field::MATERIALS_AT,
        field::WHEELS_AT,
        field::VERTICES_AT,
        field::INDICES_AT,
        field::STRINGS_AT,
    ] {
        let section =
            u32::from_le_bytes(bytes[field_at..field_at + 4].try_into().unwrap()) as usize;
        assert!(section > at, "section at {field_at} should follow it");
    }
}

/// A car compiled before silhouettes existed is a car with no silhouette, not a car that fails.
/// The two spare header bytes were zero in every file that predates the field, which is exactly
/// what makes reading them safe.
#[test]
fn a_car_from_before_there_were_silhouettes_still_loads() {
    let bytes = Builder::typical().build();
    assert_eq!(bytes[field::SILHOUETTE_AT_16], 0);
    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    let car = Car::parse(view).unwrap();
    assert!(car.silhouette().is_none());
    assert!(Silhouette::parse(view).is_none());
}

/// A silhouette that does not check out costs the player a stand-in, not the car. It is an
/// additive section: refusing the whole file over it would be refusing a car that draws perfectly
/// well for the sake of something nobody sees for half a second.
#[test]
fn a_broken_silhouette_is_dropped_and_the_car_still_loads() {
    let mut b = Builder::typical();
    b.silhouette = Some(typical_silhouette());

    // An index that points past the positions — the GE would fetch a vertex from outside the car.
    let mut bytes = b.build();
    let at = HEADER_BYTES;
    let indices_at = at + u32::from_le_bytes(bytes[at + 12..at + 16].try_into().unwrap()) as usize;
    put_u16(&mut bytes, indices_at, 9999);
    assert_silhouette_dropped(&bytes);

    // An offset that runs off the end of the file.
    let mut bytes = b.build();
    put_u16(&mut bytes, field::SILHOUETTE_AT_16, 60_000);
    assert_silhouette_dropped(&bytes);

    // A count of triangles that is not a whole number of them.
    let mut bytes = b.build();
    put_u32(&mut bytes, at + 4, 7);
    assert_silhouette_dropped(&bytes);

    // Arrays the GE could not fetch by DMA, being off a 16-byte line.
    let mut bytes = b.build();
    let positions_at = u32::from_le_bytes(bytes[at + 8..at + 12].try_into().unwrap());
    put_u32(&mut bytes, at + 8, positions_at + 4);
    assert_silhouette_dropped(&bytes);
}

fn assert_silhouette_dropped(bytes: &[u8]) {
    let backing = aligned(bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    let car = Car::parse(view).expect("the car itself is untouched and still loads");
    assert!(car.silhouette().is_none(), "the silhouette should be dropped");
}

/// Camber is stored so the console can put back a lean the compiler took out of the vertices, and
/// a field that silently read as zero would be a wheel that spins upright on a car that does not
/// sit upright. It lives in the eight bytes the wheel record had spare, so a car from before it
/// existed reads as no lean at all — which is what such a car wants, since its geometry still has
/// the lean baked in.
#[test]
fn a_wheel_carries_its_camber() {
    let mut b = Builder::typical();
    b.wheels[0].camber = -0.0942;
    let bytes = b.build();
    let backing = aligned(&bytes);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, bytes.len()) };
    let car = Car::parse(view).unwrap();
    assert!((car.wheel(0).camber - -0.0942).abs() < 1.0e-6);

    // Zeroing those bytes is what an older file looks like, and it must read as upright.
    let mut older = b.build();
    let at = u32::from_le_bytes(
        older[field::WHEELS_AT..field::WHEELS_AT + 4].try_into().unwrap(),
    ) as usize;
    older[at + 24..at + 28].fill(0);
    let backing = aligned(&older);
    let view = unsafe { core::slice::from_raw_parts(backing.as_ptr() as *const u8, older.len()) };
    assert_eq!(Car::parse(view).unwrap().wheel(0).camber, 0.0);
}
