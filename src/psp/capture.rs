//! On-device capture, for diagnosing things that only go wrong on real hardware.
//!
//! PPSSPP's software rasteriser is far more forgiving than the GE: it has no 16-bit depth buffer
//! to fight over, no cache to leave stale, and no display list to overrun. So a glitch that only
//! shows up on a PSP needs pixels *from* the PSP.
//!
//! Frames are written to `ms0:/ANGLEZERO/` alongside a text dump of the counters that would
//! explain a dropped draw. Everything here is a debug aid — it blocks on file IO and will visibly
//! hitch the frame it runs on.

use angle_zero::game::Game;
use psp::sys::{self, IoOpenFlags};

const DIR: &[u8] = b"ms0:/ANGLEZERO\0";

/// Counters worth having next to a screenshot.
#[derive(Clone, Copy, Debug, Default)]
pub struct Diagnostics {
    /// CPU microseconds spent on the last frame, excluding the vblank wait.
    pub frame_us: u32,
    /// Largest display list this session, in bytes, against the 1 MB buffer.
    pub list_bytes: u32,
    /// High-water mark of the per-frame vertex arena, in bytes.
    pub scratch_peak: u32,
    /// Times the arena ran out. Any non-zero value means draws were silently dropped.
    pub scratch_failures: u32,
    /// Car draw calls submitted last frame. One car is a dozen; a benchmark mode's field is that
    /// many times over, and this is what says the extra cars were actually drawn.
    pub car_calls: u32,
    /// Lamp glows and beam patches drawn. Vehicle lighting's two halves cost very different
    /// amounts, and a field of cars is only explicable if they are counted apart.
    pub lamps: u32,
    pub beams: u32,
    pub live_skids: u32,
    pub live_puffs: u32,
    pub drifting: bool,
    pub speed_kph: u32,
    pub descent_percent: u32,
}

/// Where the next capture will be written. Probed once so a second session does not overwrite
/// the first session's evidence.
static mut NEXT_INDEX: u32 = 0;
static mut PROBED: bool = false;

/// Builds `ms0:/ANGLEZERO/SHOTnnn.<ext>`, NUL-terminated.
fn path_for(index: u32, ext: &[u8; 3], out: &mut [u8; 40]) {
    let stem = b"ms0:/ANGLEZERO/SHOT";
    let mut w = 0;
    for &c in stem {
        out[w] = c;
        w += 1;
    }
    out[w] = b'0' + ((index / 100) % 10) as u8;
    out[w + 1] = b'0' + ((index / 10) % 10) as u8;
    out[w + 2] = b'0' + (index % 10) as u8;
    out[w + 3] = b'.';
    w += 4;
    for &c in ext {
        out[w] = c;
        w += 1;
    }
    out[w] = 0;
}

fn exists(path: &[u8]) -> bool {
    unsafe {
        let fd = sys::sceIoOpen(path.as_ptr(), IoOpenFlags::RD_ONLY, 0o777);
        if fd.0 >= 0 {
            sys::sceIoClose(fd);
            true
        } else {
            false
        }
    }
}

/// Finds the first unused slot, so captures accumulate across boots rather than overwriting.
unsafe fn probe_index() {
    if PROBED {
        return;
    }
    PROBED = true;
    let mut path = [0u8; 40];
    for i in 0..200u32 {
        path_for(i, b"BMP", &mut path);
        if !exists(&path) {
            NEXT_INDEX = i;
            return;
        }
    }
    NEXT_INDEX = 199;
}

/// Appends a decimal integer to `out`.
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

/// Writes the current frame and its counters. Returns the index used, or `None` on failure.
pub fn capture(game: &Game, diag: &Diagnostics) -> Option<u32> {
    unsafe {
        sys::sceIoMkdir(DIR.as_ptr(), 0o777);
        probe_index();
        let index = NEXT_INDEX;
        NEXT_INDEX = NEXT_INDEX.saturating_add(1);

        // --- the frame itself ---
        let mut path = [0u8; 40];
        path_for(index, b"BMP", &mut path);
        let bmp = psp::screenshot_bmp();
        let fd = sys::sceIoOpen(
            path.as_ptr(),
            IoOpenFlags::WR_ONLY | IoOpenFlags::CREAT | IoOpenFlags::TRUNC,
            0o777,
        );
        if fd.0 < 0 {
            return None;
        }
        sys::sceIoWrite(fd, bmp.as_ptr() as *const _, bmp.len());
        sys::sceIoClose(fd);

        // --- the counters beside it ---
        let mut text = [0u8; 512];
        let mut w = 0usize;
        push_str(&mut text, &mut w, b"shot ");
        push_num(&mut text, &mut w, index);
        push_str(&mut text, &mut w, b"\nframe_us ");
        push_num(&mut text, &mut w, diag.frame_us);
        push_str(&mut text, &mut w, b"\nlist_bytes ");
        push_num(&mut text, &mut w, diag.list_bytes);
        push_str(&mut text, &mut w, b"\nscratch_peak ");
        push_num(&mut text, &mut w, diag.scratch_peak);
        push_str(&mut text, &mut w, b"\nscratch_failures ");
        push_num(&mut text, &mut w, diag.scratch_failures);
        push_str(&mut text, &mut w, b"\nlive_skids ");
        push_num(&mut text, &mut w, diag.live_skids);
        push_str(&mut text, &mut w, b"\nlive_puffs ");
        push_num(&mut text, &mut w, diag.live_puffs);
        push_str(&mut text, &mut w, b"\ndrifting ");
        push_num(&mut text, &mut w, diag.drifting as u32);
        push_str(&mut text, &mut w, b"\nspeed_kph ");
        push_num(&mut text, &mut w, diag.speed_kph);
        push_str(&mut text, &mut w, b"\ndescent_percent ");
        push_num(&mut text, &mut w, diag.descent_percent);
        push_str(&mut text, &mut w, b"\nimpacts ");
        push_num(&mut text, &mut w, game.impact_count());
        push_str(&mut text, &mut w, b"\n");

        path_for(index, b"TXT", &mut path);
        let fd = sys::sceIoOpen(
            path.as_ptr(),
            IoOpenFlags::WR_ONLY | IoOpenFlags::CREAT | IoOpenFlags::TRUNC,
            0o777,
        );
        if fd.0 >= 0 {
            sys::sceIoWrite(fd, text.as_ptr() as *const _, w);
            sys::sceIoClose(fd);
        }

        Some(index)
    }
}
