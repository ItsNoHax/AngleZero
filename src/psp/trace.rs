//! A rolling record of what each frame submitted, dumped on demand.
//!
//! Screenshots are the wrong instrument for a fault you cannot time: SELECT samples one frame, and
//! an artifact lasting a handful of frames slips between samples. This keeps the last few hundred
//! frames in a ring and writes them out *after* the fact, so the evidence is captured by reacting
//! to the artifact rather than anticipating it.
//!
//! It answers the question a picture cannot: when part of the world vanishes, was the draw call
//! issued at all? If the road's chunk count holds steady across the bad frames, nothing was
//! dropped and the fault is further down the pipeline than culling or submission.

use angle_zero::game::Game;
use psp::sys::{self, IoOpenFlags};

use super::render::DrawStats;

/// Ten seconds at 60 fps. 16 bytes a frame, so the whole ring is under 10 KB.
const FRAMES: usize = 600;

#[derive(Clone, Copy, Default)]
pub struct Frame {
    pub index: u32,
    pub road: u16,
    pub terrain: u16,
    pub lines: u16,
    pub rails: u16,
    pub dashes: u16,
    pub props: u16,
    pub verts: u32,
    pub node: u16,
    pub speed_kph: u16,
    pub frame_us: u32,
    pub road_mask: u32,
    pub terrain_mask: u32,
}

static mut RING: [Frame; FRAMES] = [Frame {
    index: 0,
    road: 0,
    terrain: 0,
    lines: 0,
    rails: 0,
    dashes: 0,
    props: 0,
    verts: 0,
    node: 0,
    speed_kph: 0,
    frame_us: 0,
    road_mask: 0,
    terrain_mask: 0,
}; FRAMES];
static mut WRITE: usize = 0;
static mut FILLED: usize = 0;

/// Records one frame. Overwrites the oldest once the ring is full.
pub fn record(index: u32, stats: &DrawStats, game: &Game, frame_us: u32) {
    unsafe {
        let f = &mut RING[WRITE % FRAMES];
        *f = Frame {
            index,
            road: stats.road,
            terrain: stats.terrain,
            lines: stats.lines,
            rails: stats.rails,
            dashes: stats.dashes,
            props: stats.props,
            verts: stats.verts,
            node: game.vehicle.locator.last_idx as u16,
            speed_kph: (game.vehicle.speed_kph() + 0.5) as u16,
            frame_us,
            road_mask: stats.road_mask,
            terrain_mask: stats.terrain_mask,
        };
        WRITE += 1;
        if FILLED < FRAMES {
            FILLED += 1;
        }
    }
}

fn push_num(out: &mut [u8], w: &mut usize, mut value: u32) {
    let mut digits = [0u8; 10];
    let mut n = 0;
    if value == 0 {
        digits[0] = b'0';
        n = 1;
    }
    while value > 0 && n < 10 {
        digits[n] = b'0' + (value % 10) as u8;
        value /= 10;
        n += 1;
    }
    for i in 0..n {
        if *w < out.len() {
            out[*w] = digits[n - 1 - i];
            *w += 1;
        }
    }
}

fn push_str(out: &mut [u8], w: &mut usize, s: &[u8]) {
    for &c in s {
        if *w < out.len() {
            out[*w] = c;
            *w += 1;
        }
    }
}

/// Writes the ring to `ms0:/ANGLEZERO/TRACEnnn.TXT`, oldest frame first.
pub fn dump(index: u32) -> bool {
    unsafe {
        // Creates the directory itself rather than relying on a screenshot having been saved
        // first — the trace is the more important of the two and must not depend on the other.
        sys::sceIoMkdir(b"ms0:/ANGLEZERO\0".as_ptr(), 0o777);

        let mut path = [0u8; 40];
        let stem = b"ms0:/ANGLEZERO/TRACE";
        let mut w = 0;
        for &c in stem {
            path[w] = c;
            w += 1;
        }
        path[w] = b'0' + ((index / 100) % 10) as u8;
        path[w + 1] = b'0' + ((index / 10) % 10) as u8;
        path[w + 2] = b'0' + (index % 10) as u8;
        w += 3;
        for &c in b".TXT" {
            path[w] = c;
            w += 1;
        }
        path[w] = 0;

        let fd = sys::sceIoOpen(
            path.as_ptr(),
            IoOpenFlags::WR_ONLY | IoOpenFlags::CREAT | IoOpenFlags::TRUNC,
            0o777,
        );
        if fd.0 < 0 {
            return false;
        }

        let mut buf = [0u8; 1024];
        let mut w = 0usize;
        push_str(
            &mut buf,
            &mut w,
            b"# frame road terrain lines rails dashes props verts node kph us roadmask terrainmask\n",
        );
        sys::sceIoWrite(fd, buf.as_ptr() as *const _, w);

        // Oldest first, so the last line is the frame SELECT was pressed on.
        let start = if FILLED < FRAMES { 0 } else { WRITE % FRAMES };
        for i in 0..FILLED {
            let f = RING[(start + i) % FRAMES];
            let mut line = [0u8; 96];
            let mut w = 0usize;
            for v in [
                f.index,
                f.road as u32,
                f.terrain as u32,
                f.lines as u32,
                f.rails as u32,
                f.dashes as u32,
                f.props as u32,
                f.verts,
                f.node as u32,
                f.speed_kph as u32,
                f.frame_us,
                f.road_mask,
                f.terrain_mask,
            ] {
                push_num(&mut line, &mut w, v);
                push_str(&mut line, &mut w, b" ");
            }
            push_str(&mut line, &mut w, b"\n");
            sys::sceIoWrite(fd, line.as_ptr() as *const _, w);
        }

        sys::sceIoClose(fd);
        true
    }
}
