//! The 2D overlay — HUD, minimap, title and results screens.
//!
//! Everything here uses `TRANSFORM_2D` vertices, which bypass the matrix pipeline and are given
//! directly in screen pixels, so the HUD lands on exact pixel boundaries at 480×272.

use core::ffi::c_void;

use angle_zero::game::{Game, Phase, Toast};
use angle_zero::hud::{self as core_hud, Gear, RevZone};
use angle_zero::math::{abs, clamp};
use angle_zero::track::{Track, NODE_COUNT};
use psp::sys::{self, GuPrimitive, GuState, VertexType};

use super::render::{rgb, rgba};
use super::text;

pub const SCREEN_W: f32 = 480.0;
pub const SCREEN_H: f32 = 272.0;

// Palette.
const TEXT: u32 = rgb(0xEE, 0xF3, 0xF7);
const DIM: u32 = rgb(0x9F, 0xB0, 0xBD);
const AMBER: u32 = rgb(0xFF, 0xD1, 0x66);
const GREEN: u32 = rgb(0x7E, 0xE0, 0x81);
const WARN: u32 = rgb(0xFF, 0x5A, 0x4D);
const ACCENT: u32 = rgb(0xFF, 0x7A, 0x59);
const OUTLINE: u32 = rgb(0x4D, 0x58, 0x65);
const PANEL: u32 = rgba(0x0A, 0x0E, 0x12, 0x8C);
const MAP_PATH: u32 = rgb(0x7F, 0x8D, 0x99);

/// Untextured 2D vertex.
#[repr(C)]
#[derive(Clone, Copy)]
struct Rect2D {
    color: u32,
    x: f32,
    y: f32,
    z: f32,
}

const RECT_FORMAT: VertexType = VertexType::from_bits_truncate(
    VertexType::COLOR_8888.bits()
        | VertexType::VERTEX_32BITF.bits()
        | VertexType::TRANSFORM_2D.bits(),
);

/// Minimap geometry, projected once at boot.
const MAP_X: f32 = 8.0;
// Leaves room below the box for the DESCENT readout without touching the bottom edge.
const MAP_Y: f32 = 156.0;
const MAP_W: f32 = 70.0;
const MAP_H: f32 = 88.0;
const MAP_STRIDE: usize = 3;
const MAP_POINTS: usize = NODE_COUNT / MAP_STRIDE + 1;

static mut MAP_LINE: psp::Align16<[Rect2D; MAP_POINTS]> = psp::Align16(
    [Rect2D {
        color: MAP_PATH,
        x: 0.0,
        y: 0.0,
        z: 0.0,
    }; MAP_POINTS],
);
static mut MAP_LEN: usize = 0;
/// World-to-map transform, kept so the car dot can use the same projection.
static mut MAP_SCALE: f32 = 1.0;
static mut MAP_OX: f32 = 0.0;
static mut MAP_OZ: f32 = 0.0;

/// Projects the centreline into the minimap box once, so each frame only plots the car.
pub fn init_minimap(track: &Track) {
    unsafe {
        let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
        let (mut min_z, mut max_z) = (f32::MAX, f32::MIN);
        for n in track.nodes.iter() {
            min_x = if n.p.x < min_x { n.p.x } else { min_x };
            max_x = if n.p.x > max_x { n.p.x } else { max_x };
            min_z = if n.p.z < min_z { n.p.z } else { min_z };
            max_z = if n.p.z > max_z { n.p.z } else { max_z };
        }
        let span_x = max_x - min_x;
        let span_z = max_z - min_z;
        // One scale for both axes so the track keeps its shape.
        let scale = {
            let sx = (MAP_W - 6.0) / span_x;
            let sz = (MAP_H - 6.0) / span_z;
            if sx < sz {
                sx
            } else {
                sz
            }
        };
        MAP_SCALE = scale;
        MAP_OX = (min_x + max_x) * 0.5;
        MAP_OZ = (min_z + max_z) * 0.5;

        let line = &raw mut MAP_LINE as *mut Rect2D;
        let mut w = 0usize;
        let mut i = 0usize;
        while i < NODE_COUNT && w < MAP_POINTS {
            let n = &track.nodes[i];
            let (x, y) = map_project(n.p.x, n.p.z);
            *line.add(w) = Rect2D {
                color: MAP_PATH,
                x,
                y,
                z: 0.0,
            };
            w += 1;
            i += MAP_STRIDE;
        }
        MAP_LEN = w;
    }
}

/// North-up projection into the minimap box.
unsafe fn map_project(x: f32, z: f32) -> (f32, f32) {
    (
        MAP_X + MAP_W * 0.5 + (x - MAP_OX) * MAP_SCALE,
        MAP_Y + MAP_H * 0.5 + (z - MAP_OZ) * MAP_SCALE,
    )
}

/// Prepares the GU for the 2D pass. Call after the 3D scene is drawn.
pub fn begin() {
    unsafe {
        sys::sceGuDisable(GuState::DepthTest);
        sys::sceGuDisable(GuState::Fog);
        sys::sceGuDisable(GuState::CullFace);
        sys::sceGuEnable(GuState::Blend);
        sys::sceGuBlendFunc(
            sys::BlendOp::Add,
            sys::BlendFactor::SrcAlpha,
            sys::BlendFactor::OneMinusSrcAlpha,
            0,
            0,
        );
    }
}

/// One-in-three scanlines at 16% black. Drawn last, over everything.
pub fn scanlines() {
    let color = rgba(0x00, 0x00, 0x00, 0x29);
    let mut y = 0.0;
    while y < SCREEN_H {
        fill_rect(0.0, y, SCREEN_W, 1.0, color);
        y += 3.0;
    }
}

pub fn end() {
    unsafe {
        sys::sceGuDisable(GuState::Blend);
        sys::sceGuDisable(GuState::Texture2D);
        sys::sceGuEnable(GuState::DepthTest);
    }
}

fn fill_rect(x: f32, y: f32, w: f32, h: f32, color: u32) {
    unsafe {
        sys::sceGuDisable(GuState::Texture2D);
        // Frame-lived, because the GE reads this long after this call returns.
        let v = super::scratch::alloc::<Rect2D>(2);
        if v.is_null() {
            return;
        }
        *v = Rect2D {
            color,
            x,
            y,
            z: 0.0,
        };
        *v.add(1) = Rect2D {
            color,
            x: x + w,
            y: y + h,
            z: 0.0,
        };
        sys::sceGumDrawArray(
            GuPrimitive::Sprites,
            RECT_FORMAT,
            2,
            core::ptr::null(),
            v as *const c_void,
        );
    }
}

fn panel(x: f32, y: f32, w: f32, h: f32) {
    fill_rect(x, y, w, h, PANEL);
    fill_rect(x, y, w, 1.0, OUTLINE);
    fill_rect(x, y + h - 1.0, w, 1.0, OUTLINE);
    fill_rect(x, y, 1.0, h, OUTLINE);
    fill_rect(x + w - 1.0, y, 1.0, h, OUTLINE);
}

/// Writes an unsigned integer into `buf`, right-aligned to its natural width. Returns the slice.
fn digits(buf: &mut [u8; 12], value: u32) -> &[u8] {
    let mut d = [0u8; 10];
    let n = core_hud::score_digits(value, &mut d);
    for i in 0..n {
        buf[i] = b'0' + d[i];
    }
    &buf[..n]
}

/// Writes an integer with thousands separators, as the score is shown.
fn grouped_digits(buf: &mut [u8; 16], value: u32) -> &[u8] {
    let mut d = [0u8; 10];
    let n = core_hud::score_digits(value, &mut d);
    let mut w = 0usize;
    for i in 0..n {
        if i > 0 && (n - i) % 3 == 0 {
            buf[w] = b',';
            w += 1;
        }
        buf[w] = b'0' + d[i];
        w += 1;
    }
    &buf[..w]
}

/// Two-digit zero-padded field, for the clock.
fn pad2(buf: &mut [u8; 2], value: u32) -> &[u8] {
    buf[0] = b'0' + (value / 10 % 10) as u8;
    buf[1] = b'0' + (value % 10) as u8;
    &buf[..]
}

/// The debug readout, toggled with START. Deliberately terse: it has to fit on one line at the
/// bottom of a 480 px screen and still be legible in a photograph of the handheld.
///
/// `SCF` is the one to watch. It counts refused vertex-arena allocations, and any non-zero value
/// means draws were silently dropped — which looks like flickering geometry, not like an error.
#[cfg(feature = "devtools")]
static mut SUM: u64 = 0;
#[cfg(feature = "devtools")]
static mut N: u64 = 0;
#[cfg(feature = "devtools")]
static mut PK: u32 = 0;

#[cfg(feature = "devtools")]
pub fn debug_overlay(diag: &super::capture::Diagnostics, shots: u32) {
    let mut l = [0u8; 96];
    let mut w = 0usize;
    let mut b = [0u8; 12];

    let mut field = |l: &mut [u8; 96], w: &mut usize, name: &[u8], v: u32| {
        for &c in name {
            l[*w] = c;
            *w += 1;
        }
        for &c in digits(&mut b, v) {
            l[*w] = c;
            *w += 1;
        }
        l[*w] = b' ';
        *w += 1;
    };

    // Rolling average and peak, so a single unlucky frame does not read as a regression.
    unsafe {
        SUM += diag.frame_us as u64;
        N += 1;
        if N > 90 && diag.frame_us > PK { PK = diag.frame_us; }
    }
    field(&mut l, &mut w, b"US", diag.frame_us);
    field(&mut l, &mut w, b" AVG", unsafe { (SUM / N.max(1)) as u32 });
    field(&mut l, &mut w, b" PK", unsafe { PK });
    field(&mut l, &mut w, b" LST", diag.list_bytes);
    field(&mut l, &mut w, b" SCR", diag.scratch_peak);
    field(&mut l, &mut w, b" SCF", diag.scratch_failures);
    field(&mut l, &mut w, b" SK", diag.live_skids);
    field(&mut l, &mut w, b" SM", diag.live_puffs);
    field(&mut l, &mut w, b" SHOT", shots);

    // Red when something has actually gone wrong, so it is obvious in a photo.
    let color = if diag.scratch_failures > 0 { WARN } else { GREEN };
    text::bind();
    text::draw_shadowed(&l[..w], 4.0, 262.0, 1.0, color);
}

/// Names the active render-state override, top-right, so a photograph of the screen records
/// which one was on. Always shown, because the fault it is chasing is easiest to see with the
/// rest of the debug overlay off.
#[cfg(feature = "devtools")]
pub fn debug_mode_label(mode: u32) {
    const NAMES: [&[u8]; 6] = [
        b"MODE 0 NORMAL",
        b"MODE 1 NO CULL",
        b"MODE 2 NO DEPTH",
        b"MODE 3 NO FOG",
        b"MODE 4 NO CULL+DEPTH+FOG",
        b"MODE 5 NO SKY",
    ];
    let name = NAMES[(mode as usize) % NAMES.len()];
    let colour = if mode == 0 { DIM } else { GREEN };
    text::bind();
    text::draw_shadowed(name, SCREEN_W - text::width(name, 1.0) - 4.0, 40.0, 1.0, colour);
}

pub fn draw(game: &Game, track: &Track) {
    match game.phase {
        Phase::Title => draw_title(game),
        Phase::Run => draw_run(game, track),
        Phase::Results => draw_results(game),
    }
    draw_toast(game);
}

fn draw_title(game: &Game) {
    text::bind();
    // Kept clear of the middle of the frame so the orbiting car stays visible behind it.
    text::draw_centered(b"ANGLEZERO", SCREEN_W * 0.5, 26.0, 3.0, TEXT);
    text::draw_centered(b"SEKIRA DESCENT", SCREEN_W * 0.5, 58.0, 1.0, DIM);
    text::draw_centered(b"PRESS X TO START", SCREEN_W * 0.5, 232.0, 1.0, GREEN);
    draw_best(game, 250.0);
}

/// The stored record, shown once there is one to show.
fn draw_best(game: &Game, y: f32) {
    let r = &game.record;
    if !r.has_time() {
        return;
    }
    let mut line = [0u8; 48];
    let mut w = 0usize;
    let mut buf = [0u8; 12];
    let mut p = [0u8; 2];

    for &c in b"BEST " {
        line[w] = c;
        w += 1;
    }
    let (m, s, cs) = core_hud::split_time(r.best_time_cs as f32 / 100.0);
    for &c in digits(&mut buf, m) {
        line[w] = c;
        w += 1;
    }
    line[w] = b':';
    w += 1;
    for &c in pad2(&mut p, s) {
        line[w] = c;
        w += 1;
    }
    line[w] = b'.';
    w += 1;
    for &c in pad2(&mut p, cs) {
        line[w] = c;
        w += 1;
    }
    for &c in b"  " {
        line[w] = c;
        w += 1;
    }
    let mut g = [0u8; 16];
    for &c in grouped_digits(&mut g, r.best_score) {
        line[w] = c;
        w += 1;
    }
    for &c in b"  x" {
        line[w] = c;
        w += 1;
    }
    for &c in digits(&mut buf, r.best_combo) {
        line[w] = c;
        w += 1;
    }
    text::draw_centered(&line[..w], SCREEN_W * 0.5, y, 1.0, AMBER);
}

fn draw_results(game: &Game) {
    let r = &game.result;
    fill_rect(0.0, 0.0, SCREEN_W, SCREEN_H, rgba(0x05, 0x07, 0x0C, 0xC0));
    text::bind();
    text::draw_centered(b"RUN COMPLETE", SCREEN_W * 0.5, 64.0, 2.0, AMBER);

    let (m, s, cs) = core_hud::split_time(r.time);
    let mut line = [0u8; 24];
    let mut w = 0usize;
    let mut buf = [0u8; 12];
    for &c in b"TIME " {
        line[w] = c;
        w += 1;
    }
    for &c in digits(&mut buf, m) {
        line[w] = c;
        w += 1;
    }
    line[w] = b':';
    w += 1;
    let mut p = [0u8; 2];
    for &c in pad2(&mut p, s) {
        line[w] = c;
        w += 1;
    }
    line[w] = b'.';
    w += 1;
    for &c in pad2(&mut p, cs) {
        line[w] = c;
        w += 1;
    }
    text::draw_centered(&line[..w], SCREEN_W * 0.5, 110.0, 1.0, TEXT);

    let mut score_line = [0u8; 32];
    let mut w = 0usize;
    for &c in b"DRIFT SCORE " {
        score_line[w] = c;
        w += 1;
    }
    let mut gbuf = [0u8; 16];
    for &c in grouped_digits(&mut gbuf, r.score as u32) {
        score_line[w] = c;
        w += 1;
    }
    text::draw_centered(&score_line[..w], SCREEN_W * 0.5, 130.0, 1.0, TEXT);

    let mut combo_line = [0u8; 24];
    let mut w = 0usize;
    for &c in b"BEST COMBO x" {
        combo_line[w] = c;
        w += 1;
    }
    let mut cbuf = [0u8; 12];
    for &c in digits(&mut cbuf, r.best_combo) {
        combo_line[w] = c;
        w += 1;
    }
    text::draw_centered(&combo_line[..w], SCREEN_W * 0.5, 150.0, 1.0, ACCENT);

    draw_best(game, 172.0);
    text::draw_centered(b"PRESS TRIANGLE TO RUN AGAIN", SCREEN_W * 0.5, 208.0, 1.0, GREEN);
}

fn draw_run(game: &Game, track: &Track) {
    let st = &game.vehicle.state;

    // --- bottom right: speed, gear, revs ---
    let kph = (abs(st.vx) * 3.6 + 0.5) as u32;
    panel(370.0, 210.0, 102.0, 54.0);
    text::bind();
    let mut buf = [0u8; 12];
    let d = digits(&mut buf, kph);
    text::draw_shadowed(d, 380.0, 224.0, 2.5, TEXT);
    text::draw_shadowed(b"KM/H", 380.0, 246.0, 1.0, DIM);

    let gear_label: [u8; 1] = match core_hud::gear(st.vx) {
        Gear::Reverse => [b'R'],
        Gear::Forward(g) => [b'0' + g],
    };
    fill_rect(446.0, 222.0, 18.0, 20.0, rgba(0x0A, 0x0E, 0x12, 0xC0));
    fill_rect(446.0, 222.0, 18.0, 1.0, OUTLINE);
    fill_rect(446.0, 241.0, 18.0, 1.0, OUTLINE);
    text::bind();
    text::draw_shadowed(&gear_label, 451.0, 226.0, 1.6, AMBER);

    // Rev bar, 96 x 4 px.
    let rpm = core_hud::rpm(st.vx, game.throttle_hint());
    let zone = match core_hud::rev_zone(rpm) {
        RevZone::Green => GREEN,
        RevZone::Amber => AMBER,
        RevZone::Red => WARN,
    };
    fill_rect(374.0, 214.0, 96.0, 4.0, rgba(0x00, 0x00, 0x00, 0xA0));
    fill_rect(374.0, 214.0, 96.0 * clamp(rpm, 0.0, 1.0), 4.0, zone);

    // --- top left: drift score and combo ---
    text::bind();
    text::draw_shadowed(b"DRIFT", 10.0, 10.0, 1.0, DIM);
    let mut gbuf = [0u8; 16];
    let sd = grouped_digits(&mut gbuf, game.scoring.score as u32);
    text::draw_shadowed(sd, 10.0, 22.0, 2.0, TEXT);

    if game.scoring.combo > 1 {
        let mut cbuf = [0u8; 12];
        let mut line = [0u8; 12];
        line[0] = b'x';
        let d = digits(&mut cbuf, game.scoring.combo);
        let mut w = 1;
        for &c in d {
            line[w] = c;
            w += 1;
        }
        text::draw_shadowed(&line[..w], 10.0, 44.0, 1.6, ACCENT);
    }
    // Combo timer bar, 74 x 3 px.
    fill_rect(10.0, 62.0, 74.0, 3.0, rgba(0x00, 0x00, 0x00, 0xA0));
    fill_rect(10.0, 62.0, 74.0 * game.scoring.combo_fraction(), 3.0, ACCENT);

    // --- top centre: clock and subtitle ---
    let (m, s, cs) = core_hud::split_time(game.run_time);
    let mut clock = [0u8; 12];
    let mut w = 0usize;
    let mut b12 = [0u8; 12];
    for &c in digits(&mut b12, m) {
        clock[w] = c;
        w += 1;
    }
    clock[w] = b':';
    w += 1;
    let mut p = [0u8; 2];
    for &c in pad2(&mut p, s) {
        clock[w] = c;
        w += 1;
    }
    clock[w] = b'.';
    w += 1;
    for &c in pad2(&mut p, cs) {
        clock[w] = c;
        w += 1;
    }
    text::bind();
    text::draw_centered(&clock[..w], SCREEN_W * 0.5, 8.0, 1.8, TEXT);
    text::draw_centered(b"ANGLEZERO - SEKIRA DESCENT", SCREEN_W * 0.5, 28.0, 1.0, DIM);

    // --- bottom left: minimap and descent ---
    draw_minimap(game, track);

}

fn draw_minimap(game: &Game, track: &Track) {
    unsafe {
        panel(MAP_X - 2.0, MAP_Y - 2.0, MAP_W + 4.0, MAP_H + 4.0);

        sys::sceGuDisable(GuState::Texture2D);
        sys::sceGumDrawArray(
            GuPrimitive::LineStrip,
            RECT_FORMAT,
            MAP_LEN as i32,
            core::ptr::null(),
            &raw const MAP_LINE as *const c_void,
        );

        // Finish marker, then the car on top of it.
        let fin = &track.nodes[NODE_COUNT - 1];
        let (fx, fy) = map_project(fin.p.x, fin.p.z);
        fill_rect(fx - 1.5, fy - 1.5, 3.0, 3.0, AMBER);

        let (cx, cy) = map_project(game.vehicle.state.x, game.vehicle.state.z);
        fill_rect(cx - 2.0, cy - 2.0, 4.0, 4.0, rgb(0xFF, 0x4D, 0x3D));
    }

    let mut line = [0u8; 16];
    let mut w = 0usize;
    for &c in b"DESCENT " {
        line[w] = c;
        w += 1;
    }
    let mut buf = [0u8; 12];
    for &c in digits(&mut buf, game.descent_percent(track)) {
        line[w] = c;
        w += 1;
    }
    line[w] = b'%';
    w += 1;
    text::bind();
    text::draw_shadowed(&line[..w], MAP_X, MAP_Y + MAP_H + 4.0, 1.0, DIM);
}

fn draw_toast(game: &Game) {
    let Some(toast) = game.toast else {
        return;
    };
    let alpha = (game.toast_opacity() * 255.0) as u32;
    if alpha == 0 {
        return;
    }

    let (msg, color): (&[u8], u32) = match toast {
        Toast::Go => (b"GO!", GREEN),
        Toast::WallTap => (b"WALL TAP", WARN),
        Toast::WrongWay => (b"WRONG WAY", WARN),
        Toast::BackOnTrack => (b"BACK ON TRACK", AMBER),
        Toast::ComboUp(_) => (b"COMBO UP", AMBER),
    };
    // Re-tint with the fade alpha.
    let faded = (color & 0x00ff_ffff) | (alpha << 24);
    text::bind();

    if let Toast::ComboUp(n) = toast {
        let mut line = [0u8; 16];
        let mut w = 0usize;
        for &c in b"COMBO x" {
            line[w] = c;
            w += 1;
        }
        let mut buf = [0u8; 12];
        for &c in digits(&mut buf, n) {
            line[w] = c;
            w += 1;
        }
        text::draw_centered(&line[..w], SCREEN_W * 0.5, 84.0, 1.4, faded);
    } else {
        text::draw_centered(msg, SCREEN_W * 0.5, 84.0, 1.4, faded);
    }
}
