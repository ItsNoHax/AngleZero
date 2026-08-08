//! A self-driving, reproducible run, for the automated glitch hunt.
//!
//! Finding a flicker means comparing a frame against its neighbours, which only means anything if
//! the run that produced them can be repeated exactly. Two things otherwise make that impossible:
//!
//! - **The frame delta comes from the clock.** `sceKernelGetSystemTimeLow` under headless advances
//!   with emulated cycles, and the capture below writes half a megabyte per frame, so the delta
//!   varies with host IO. The car then reaches a different place by frame 300 on every run, and
//!   before/after screenshots compare two different views. Here `DT` is fixed, so frame index *is*
//!   the clock. Nothing else in the game reads a clock or a random number, so a harness run is a
//!   pure function of the script below.
//!
//! - **Input arrives on wall-clock time.** `scripts/psp_input.py` drives the WebSocket debugger and
//!   sleeps in host seconds, which bears no fixed relation to emulated frames under `fastForward`.
//!   So the same script starts the run at a different frame each time. This module replays a
//!   frame-indexed script instead, and needs no debugger at all.
//!
//! The script is read from `ms0:/ANGLEZERO/SCRIPT.TXT` rather than compiled in, so probing a
//! different corner does not mean waiting for another build:
//!
//! ```text
//! # frames are 1/60 s apart
//! burst 300 40      # capture 40 consecutive frames from frame 300
//! place 3 1200 90   # drop the car on node 1200 at 90 km/h
//! 0 -               # no buttons: the title camera
//! 90 x              # cross: starts the run, and holds the throttle
//! 400 xl            # still accelerating, now steering left
//! ```
//!
//! Letters are `x`, `o`, `s`, `t` for the face buttons and `u`, `d`, `l`, `r` for the d-pad; `-`
//! means nothing held. Each line holds until the next one's frame.
//!
//! `place` exists because most of the track is otherwise unreachable. Driving there needs a
//! steering script that survives every corner in between, and one mistake ends the run against a
//! guard rail — so a hairpin two thirds of the way down could not be looked at at all. Dropping the
//! car onto a node reaches any of them directly. It is the same call the triangle-key rescue makes.

use angle_zero::game::Buttons;
use psp::sys::{self, IoOpenFlags};

/// The fixed frame delta. 1/60 s is what the game is built around, and two exact 1/120 s physics
/// substeps fall out of it, leaving nothing in the accumulator to carry between frames.
pub const DT: f32 = 1.0 / 60.0;

const PATH: &[u8] = b"ms0:/ANGLEZERO/SCRIPT.TXT\0";

const CROSS: u16 = 1 << 0;
const CIRCLE: u16 = 1 << 1;
const SQUARE: u16 = 1 << 2;
const TRIANGLE: u16 = 1 << 3;
const UP: u16 = 1 << 4;
const DOWN: u16 = 1 << 5;
const LEFT: u16 = 1 << 6;
const RIGHT: u16 = 1 << 7;

const MAX_STEPS: usize = 32;

#[derive(Clone, Copy)]
struct Step {
    frame: u32,
    mask: u16,
}

static mut STEPS: [Step; MAX_STEPS] = [Step { frame: 0, mask: 0 }; MAX_STEPS];
static mut STEP_COUNT: usize = 0;
static mut BURST_START: u32 = 300;
static mut BURST_FRAMES: u32 = 40;
/// Frame to drop the car onto `PLACE_NODE`, or `NEVER` for a run that just drives from the start.
static mut PLACE_FRAME: u32 = NEVER;
static mut PLACE_NODE: u32 = 0;
static mut PLACE_KPH: u32 = 90;
/// A `render::DEBUG_MODES` override to run under, for bisecting a cause: 0 is normal, 2 drops the
/// depth test, 6 the terrain, and so on.
static mut MODE: u32 = 0;

const NEVER: u32 = u32::MAX;

/// The script used when there is no file, or the file is unreadable: idle on the title long enough
/// for its camera sweep to settle, then drive straight with the throttle down.
fn install_default() {
    unsafe {
        STEPS[0] = Step { frame: 0, mask: 0 };
        STEPS[1] = Step {
            frame: 90,
            mask: CROSS,
        };
        STEP_COUNT = 2;
        BURST_START = 300;
        BURST_FRAMES = 40;
        PLACE_FRAME = NEVER;
    }
}

fn mask_from_letters(s: &[u8]) -> u16 {
    let mut mask = 0;
    for &c in s {
        mask |= match c {
            b'x' => CROSS,
            b'o' => CIRCLE,
            b's' => SQUARE,
            b't' => TRIANGLE,
            b'u' => UP,
            b'd' => DOWN,
            b'l' => LEFT,
            b'r' => RIGHT,
            _ => 0,
        };
    }
    mask
}

/// Parses a leading decimal number. Returns the value and how many bytes it consumed.
fn parse_num(s: &[u8]) -> (u32, usize) {
    let mut value = 0u32;
    let mut n = 0;
    while n < s.len() && s[n].is_ascii_digit() {
        value = value.saturating_mul(10).saturating_add((s[n] - b'0') as u32);
        n += 1;
    }
    (value, n)
}

/// Splits a line on whitespace, yielding at most four fields — as many as `place` needs.
fn fields(line: &[u8]) -> ([&[u8]; 4], usize) {
    let mut out: [&[u8]; 4] = [&[], &[], &[], &[]];
    let mut count = 0;
    let mut i = 0;
    while i < line.len() && count < 4 {
        while i < line.len() && (line[i] == b' ' || line[i] == b'\t' || line[i] == b'\r') {
            i += 1;
        }
        let start = i;
        while i < line.len() && line[i] != b' ' && line[i] != b'\t' && line[i] != b'\r' {
            i += 1;
        }
        if i > start {
            out[count] = &line[start..i];
            count += 1;
        }
    }
    (out, count)
}

fn parse(text: &[u8]) {
    unsafe {
        STEP_COUNT = 0;
        let mut start = 0;
        while start <= text.len() {
            // One line at a time, so a truncated read cannot run off the end.
            let mut end = start;
            while end < text.len() && text[end] != b'\n' {
                end += 1;
            }
            let line = &text[start..end];
            start = end + 1;

            let (f, count) = fields(line);
            if count == 0 || f[0].starts_with(b"#") {
                if start > text.len() {
                    break;
                }
                continue;
            }

            if f[0] == b"burst" {
                if count >= 2 {
                    BURST_START = parse_num(f[1]).0;
                }
                if count >= 3 {
                    BURST_FRAMES = parse_num(f[2]).0;
                }
            } else if f[0] == b"mode" {
                if count >= 2 {
                    MODE = parse_num(f[1]).0;
                }
            } else if f[0] == b"place" {
                if count >= 3 {
                    PLACE_FRAME = parse_num(f[1]).0;
                    PLACE_NODE = parse_num(f[2]).0;
                }
                // Speed is optional: `place_at_node` leaves the car stationary, and a still frame
                // shows none of the artifacts that only appear while the world is moving.
                if count >= 4 {
                    PLACE_KPH = parse_num(f[3]).0;
                }
            } else if f[0][0].is_ascii_digit() && STEP_COUNT < MAX_STEPS {
                let (frame, used) = parse_num(f[0]);
                if used == f[0].len() {
                    let mask = if count >= 2 {
                        mask_from_letters(f[1])
                    } else {
                        0
                    };
                    STEPS[STEP_COUNT] = Step { frame, mask };
                    STEP_COUNT += 1;
                }
            }

            if start > text.len() {
                break;
            }
        }

        if STEP_COUNT == 0 {
            install_default();
        }
    }
}

/// Loads the script. Call once at boot, before the frame loop.
pub fn init() {
    unsafe {
        let fd = sys::sceIoOpen(PATH.as_ptr(), IoOpenFlags::RD_ONLY, 0o777);
        if fd.0 < 0 {
            install_default();
            return;
        }
        let mut buf = [0u8; 1024];
        let read = sys::sceIoRead(fd, buf.as_mut_ptr() as *mut _, buf.len() as u32);
        sys::sceIoClose(fd);
        if read <= 0 {
            install_default();
            return;
        }
        parse(&buf[..read as usize]);
    }
}

/// The buttons held on `frame`: the last step whose frame has been reached.
pub fn buttons_for(frame: u32) -> Buttons {
    let mut mask = 0u16;
    unsafe {
        for i in 0..STEP_COUNT {
            if STEPS[i].frame <= frame {
                mask = STEPS[i].mask;
            }
        }
    }
    Buttons {
        cross: mask & CROSS != 0,
        circle: mask & CIRCLE != 0,
        square: mask & SQUARE != 0,
        triangle: mask & TRIANGLE != 0,
        up: mask & UP != 0,
        down: mask & DOWN != 0,
        left: mask & LEFT != 0,
        right: mask & RIGHT != 0,
        // The d-pad is enough to steer with, and leaving the nub centred keeps the script's text
        // form simple: one letter per button, no analog axis to encode.
        analog_x: 0.0,
    }
}

/// The render-state override this run asked for.
pub fn mode() -> u32 {
    unsafe { MODE }
}

/// The node to drop the car onto this frame, with the speed to give it, or `None` on every other
/// frame. Only fires once: a run that kept replacing the car would never show it moving.
pub fn place_at(frame: u32) -> Option<(usize, f32)> {
    unsafe {
        if PLACE_FRAME != NEVER && frame == PLACE_FRAME {
            Some((PLACE_NODE as usize, PLACE_KPH as f32 / 3.6))
        } else {
            None
        }
    }
}

/// Whether `frame` falls inside the capture burst.
pub fn capturing(frame: u32) -> bool {
    unsafe { frame >= BURST_START && frame < BURST_START.saturating_add(BURST_FRAMES) }
}

/// Whether `frame` is the frame just past the burst, which is where the trace is dumped.
///
/// The interactive capture dumps the ring when the burst *starts*, because there the point is to
/// react to something already seen and the interesting frames are the ones before the press. Here it
/// is the other way round: the frames worth explaining are the ones just captured, so the dump has
/// to come after them or the trace and the screenshots cover disjoint stretches of the run.
pub fn burst_ends(frame: u32) -> bool {
    unsafe { frame == BURST_START.saturating_add(BURST_FRAMES) }
}

/// Whether the run is over. Headless has no other way to know: the frame loop never returns, so
/// without this the run would sit at its `--timeout` long after the last frame worth capturing.
pub fn finished(frame: u32) -> bool {
    unsafe { frame > BURST_START.saturating_add(BURST_FRAMES) }
}
