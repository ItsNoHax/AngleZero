//! Per-frame vertex arena, handed out through uncached memory.
//!
//! The GE reads vertex data by pointer, and in `GuContextType::Direct` it does not wait until the
//! end of the frame to do it: every `sceGumDrawArray` ends by advancing the list's stall address,
//! which kicks the hardware into executing that draw immediately. So there is no safe point at
//! which to write the data cache back — by the time the frame ends, the GE has already read every
//! buffer the frame referenced.
//!
//! Writing through an uncached address sidesteps the problem entirely: the data is in memory
//! before the draw call that points at it is ever issued. This is the same trick `sceGuStart`
//! uses for the display list itself.
//!
//! Getting this wrong does not fail cleanly. It reads as text losing its last few characters,
//! sprites appearing at wild coordinates, and geometry flickering — intermittently, and only on
//! hardware, because PPSSPP does not model the cache.

use psp::sys;

/// The PSP's data cache line.
const CACHE_LINE: usize = 64;
/// Bit that turns a main-memory address into an uncached view of the same bytes.
const UNCACHED: u32 = 0x4000_0000;

const SCRATCH_BYTES: usize = 96 * 1024;

#[repr(C, align(64))]
struct CacheAligned<T>(T);

static mut BUF: CacheAligned<[u8; SCRATCH_BYTES]> = CacheAligned([0; SCRATCH_BYTES]);
static mut USED: usize = 0;
static mut HIGH_WATER: usize = 0;
/// Counts exhaustion. Any non-zero value means a draw was silently skipped, which looks like
/// geometry flickering in and out rather than like an error.
static mut FAILURES: u32 = 0;

/// Drops any cached lines covering the arena, once, before anything is written through the
/// uncached view. The static initialiser populated it through the cache, and a line evicted
/// later would land on top of vertices the GE is about to read.
pub fn init() {
    unsafe {
        sys::sceKernelDcacheWritebackInvalidateRange(
            &raw const BUF as *const _,
            SCRATCH_BYTES as u32,
        );
    }
}

/// Frees the whole arena. Call once at the top of each frame, before anything draws.
pub fn reset() {
    unsafe {
        USED = 0;
    }
}

/// Reserves room for `n` values of `T`. The returned pointer is uncached, so anything written
/// through it is visible to the GE immediately — which is required, not merely convenient.
///
/// Returns null when exhausted, which callers treat as "draw nothing" rather than scribbling
/// past the end.
pub unsafe fn alloc<T>(n: usize) -> *mut T {
    // Cache-line granularity keeps separate allocations off each other's lines, which matters if
    // this ever goes back to a cached buffer with explicit flushes.
    let start = (USED + CACHE_LINE - 1) & !(CACHE_LINE - 1);
    let bytes = n * core::mem::size_of::<T>();
    if start + bytes > SCRATCH_BYTES {
        FAILURES = FAILURES.saturating_add(1);
        return core::ptr::null_mut();
    }
    USED = start + bytes;
    if USED > HIGH_WATER {
        HIGH_WATER = USED;
    }
    let base = &raw mut BUF as *mut u8 as u32;
    ((base | UNCACHED) + start as u32) as *mut T
}

/// Largest amount used by any frame so far, for tuning `SCRATCH_BYTES`.
pub fn high_water() -> usize {
    unsafe { HIGH_WATER }
}

/// How many allocations have been refused. Should always be zero.
pub fn failures() -> u32 {
    unsafe { FAILURES }
}
