//! Drawing the city in the basin (see `angle_zero::city`).
//!
//! Drawn in the sky pass after the ridges and valley lights, with depth writes off, so the
//! hillside and trees drawn later cover it where they stand in front. At a kilometre and more a
//! 16-bit depth buffer resolves nothing anyway: the order here is the only occlusion there is.
//!
//! The lights and windows are single-pixel points the GE transforms itself, built once at boot.
//! A point always covers exactly one pixel, so a light can move a pixel at a time as the camera
//! moves but never change shape and twinkle, which is what sub-pixel quads did to the stars.

use core::ffi::c_void;

use angle_zero::city::{self, Tower, CITY_LIGHTS, CITY_TOWERS};
use angle_zero::math::{hypot, Vec3};
use angle_zero::mesh::Vertex;
use angle_zero::scenery::{hash, LightKind, ValleyLight};
use angle_zero::track::Track;
use psp::sys::{self, GuPrimitive, MatrixMode, VertexType};

use super::render::rgb;

const VERTEX_FORMAT: VertexType = VertexType::from_bits_truncate(
    VertexType::COLOR_8888.bits() | VertexType::VERTEX_32BITF.bits() | VertexType::TRANSFORM_3D.bits(),
);

/// Street, expressway and rail lights.
static mut LIGHTS: psp::Align16<[Vertex; CITY_LIGHTS]> = psp::Align16([Vertex::ZERO; CITY_LIGHTS]);
static mut LIGHT_COUNT: usize = 0;

/// Each tower's box: four sides and a roof, two triangles each.
const BOX_VERTS: usize = 30;
static mut BODIES: psp::Align16<[Vertex; BOX_VERTS * CITY_TOWERS]> = psp::Align16([Vertex::ZERO; BOX_VERTS * CITY_TOWERS]);

/// Lit windows, tower by tower, and where each tower's run of them starts and ends.
const WINDOWS: usize = 3200;
static mut WINDOW_POINTS: psp::Align16<[Vertex; WINDOWS]> = psp::Align16([Vertex::ZERO; WINDOWS]);
static mut WINDOW_RUNS: [(u16, u16); CITY_TOWERS] = [(0, 0); CITY_TOWERS];
static mut TOWER_COUNT: usize = 0;

/// Red aircraft beacons on the tallest towers, and the broadcast tower's tip.
static mut BEACONS: psp::Align16<[Vertex; CITY_TOWERS + 1]> = psp::Align16([Vertex::ZERO; CITY_TOWERS + 1]);
static mut BEACON_COUNT: usize = 0;

/// The broadcast tower as lines: four legs, three bands, and the deck.
const MAST_LINES: usize = 4 + 3 * 4 + 4;
static mut MAST: psp::Align16<[Vertex; MAST_LINES * 2]> = psp::Align16([Vertex::ZERO; MAST_LINES * 2]);

static mut FRAME: u32 = 0;

fn light_color(kind: LightKind) -> (u32, u32, u32) {
    match kind {
        LightKind::Sodium => (0xFF, 0xB3, 0x5C),
        LightKind::WarmWhite => (0xFF, 0xE0, 0xA8),
        LightKind::ColdWhite => (0xDF, 0xE8, 0xFF),
        LightKind::Amber => (0xFF, 0x9A, 0x4A),
        LightKind::Red => (0xFF, 0x4A, 0x3A),
        LightKind::Pink => (0xFF, 0x5F, 0xB0),
        LightKind::Cyan => (0x5F, 0xE6, 0xFF),
        LightKind::Green => (0xB8, 0xFF, 0xD8),
    }
}

/// Dimmer with distance from the car park, where the city is seen from.
fn shade(c: (u32, u32, u32), dist: f32) -> u32 {
    let k = (1.35 - dist / 2600.0).clamp(0.6, 1.0);
    rgb((c.0 as f32 * k) as u32, (c.1 as f32 * k) as u32, (c.2 as f32 * k) as u32)
}

pub unsafe fn init(track: &Track) {
    let (ox, oz, _) = city::origin(track);
    let dist = |p: Vec3| hypot(p.x - ox, p.z - oz);

    // Lights.
    let mut lights = [ValleyLight::ZERO; CITY_LIGHTS];
    LIGHT_COUNT = city::city_lights(track, &mut lights);
    let out = &mut (*(&raw mut LIGHTS)).0;
    for (i, l) in lights[..LIGHT_COUNT].iter().enumerate() {
        out[i] = Vertex::new(l.p.x, l.p.y, l.p.z, shade(light_color(l.kind), dist(l.p)));
    }

    // Towers, farthest first, each with its windows on the two faces toward the car park.
    let mut towers = [Tower::ZERO; CITY_TOWERS];
    TOWER_COUNT = city::city_towers(track, &mut towers);
    let bodies = &mut (*(&raw mut BODIES)).0;
    let windows = &mut (*(&raw mut WINDOW_POINTS)).0;
    let runs = &mut *(&raw mut WINDOW_RUNS);
    let beacons = &mut (*(&raw mut BEACONS)).0;
    let (mut ww, mut bw) = (0usize, 0usize);
    const BODY: u32 = rgb(0x0A, 0x0E, 0x17);
    const ROOF: u32 = rgb(0x0E, 0x13, 0x1E);
    for (t, tower) in towers[..TOWER_COUNT].iter().enumerate() {
        let b = tower.base;
        let (x0, x1, z0, z1) = (b.x - tower.half_w, b.x + tower.half_w, b.z - tower.half_d, b.z + tower.half_d);
        let (y0, y1) = (b.y, b.y + tower.height);
        let corner = [(x0, z0), (x1, z0), (x1, z1), (x0, z1)];
        let o = &mut bodies[t * BOX_VERTS..(t + 1) * BOX_VERTS];
        let mut w = 0;
        for k in 0..4 {
            let (a, c) = (corner[k], corner[(k + 1) % 4]);
            let q = [(a.0, y0, a.1), (c.0, y0, c.1), (c.0, y1, c.1), (a.0, y0, a.1), (c.0, y1, c.1), (a.0, y1, a.1)];
            for (x, y, z) in q {
                o[w] = Vertex::new(x, y, z, BODY);
                w += 1;
            }
        }
        for (x, z) in [corner[0], corner[1], corner[2], corner[0], corner[2], corner[3]] {
            o[w] = Vertex::new(x, y1, z, ROOF);
            w += 1;
        }

        // Windows every 10 m across and up, which is about two pixels from the car park: close
        // enough to read as lit floors, far enough apart that neighbours never merge.
        let start = ww;
        let (tint, dim) = if tower.warm { ((0xFF, 0xE6, 0xB8), 0.75) } else { ((0xD6, 0xE4, 0xFF), 0.7) };
        for k in 0..4 {
            let (a, c) = (corner[k], corner[(k + 1) % 4]);
            // Only the faces turned toward the car park, pushed a little proud of the wall.
            let (mx, mz) = ((a.0 + c.0) * 0.5, (a.1 + c.1) * 0.5);
            let (nx, nz) = ((mx - b.x), (mz - b.z));
            if nx * (ox - mx) + nz * (oz - mz) <= 0.0 {
                continue;
            }
            let len = hypot(nx, nz);
            let (px, pz) = (nx / len * 0.5, nz / len * 0.5);
            let width = hypot(c.0 - a.0, c.1 - a.1);
            let cols = (width / 10.0) as usize;
            let rows = ((tower.height - 12.0) / 10.0) as usize;
            for i in 0..cols {
                for j in 0..rows {
                    let r = hash((t * 7919 + k * 131 + i * 17 + j * 3) as u32);
                    if r % 100 >= 30 || ww >= WINDOWS {
                        continue;
                    }
                    let f = (i as f32 + 0.5) / cols as f32;
                    let x = a.0 + (c.0 - a.0) * f + px;
                    let z = a.1 + (c.1 - a.1) * f + pz;
                    let y = y0 + 8.0 + j as f32 * 10.0;
                    let k = dim + (r >> 8) as f32 % 25.0 / 100.0;
                    let col = rgb((tint.0 as f32 * k) as u32, (tint.1 as f32 * k) as u32, (tint.2 as f32 * k) as u32);
                    windows[ww] = Vertex::new(x, y, z, col);
                    ww += 1;
                }
            }
        }
        runs[t] = (start as u16, (ww - start) as u16);

        if tower.beacon() {
            beacons[bw] = Vertex::new(b.x, y1 + 2.0, b.z, rgb(0xFF, 0x30, 0x20));
            bw += 1;
        }
    }

    // The broadcast tower: four orange legs to the tip, white bands across them, a lit deck.
    let (base, height) = city::broadcast_tower(track);
    let mast = &mut (*(&raw mut MAST)).0;
    let mut mw = 0;
    let foot = 34.0;
    let tip = Vec3::new(base.x, base.y + height, base.z);
    let leg = |k: usize, f: f32| {
        let (sx, sz) = [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)][k];
        let r = foot * (1.0 - f);
        Vec3::new(base.x + sx * r, base.y + height * f, base.z + sz * r)
    };
    const ORANGE: u32 = rgb(0xD8, 0x62, 0x22);
    const WHITE: u32 = rgb(0xC8, 0xC8, 0xC0);
    let mut line = |a: Vec3, b: Vec3, c: u32| {
        mast[mw] = Vertex::new(a.x, a.y, a.z, c);
        mast[mw + 1] = Vertex::new(b.x, b.y, b.z, c);
        mw += 2;
    };
    for k in 0..4 {
        line(leg(k, 0.0), tip, ORANGE);
    }
    for f in [0.28, 0.5, 0.72] {
        for k in 0..4 {
            line(leg(k, f), leg((k + 1) % 4, f), WHITE);
        }
    }
    for k in 0..4 {
        let (a, b) = (leg(k, 0.45), leg((k + 1) % 4, 0.45));
        line(Vec3::new(a.x, a.y - 3.0, a.z), Vec3::new(b.x, b.y - 3.0, b.z), rgb(0xFF, 0xD9, 0xA8));
    }
    beacons[bw] = Vertex::new(tip.x, tip.y + 2.0, tip.z, rgb(0xFF, 0x30, 0x20));
    bw += 1;
    BEACON_COUNT = bw;
}

unsafe fn submit(prim: GuPrimitive, verts: *const Vertex, count: usize) {
    if count > 0 {
        sys::sceGumDrawArray(prim, VERTEX_FORMAT, count as i32, core::ptr::null(), verts as *const c_void);
    }
}

/// The city: street lights, then the towers far to near with their windows, the broadcast tower
/// and the beacons. Called in the sky pass, depth writes off.
pub unsafe fn draw() {
    FRAME = FRAME.wrapping_add(1);
    sys::sceGumMatrixMode(MatrixMode::Model);
    sys::sceGumLoadIdentity();
    submit(GuPrimitive::Points, (*(&raw const LIGHTS)).0.as_ptr(), LIGHT_COUNT);
    let bodies = (*(&raw const BODIES)).0.as_ptr();
    let windows = (*(&raw const WINDOW_POINTS)).0.as_ptr();
    let runs = &*(&raw const WINDOW_RUNS);
    for t in 0..TOWER_COUNT {
        submit(GuPrimitive::Triangles, bodies.add(t * BOX_VERTS), BOX_VERTS);
        let (start, count) = runs[t];
        submit(GuPrimitive::Points, windows.add(start as usize), count as usize);
    }
    submit(GuPrimitive::Lines, (*(&raw const MAST)).0.as_ptr(), MAST_LINES * 2);
    // Beacons blink together, a second on and a second off.
    if (FRAME / 60) % 2 == 0 {
        submit(GuPrimitive::Points, (*(&raw const BEACONS)).0.as_ptr(), BEACON_COUNT);
    }
}
