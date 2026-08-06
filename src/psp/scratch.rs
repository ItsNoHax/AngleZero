//! Per-frame vertex arena.
//!
//! The GE reads vertex data asynchronously: a `sceGumDrawArray` only queues a command holding a
//! *pointer*, and the hardware does not touch the data until the list is kicked at
//! `sceGuFinish`/`sceGuSync`. Anything built on the stack has therefore already been destroyed by
//! the time it is read, and a static buffer reused by a later call in the same frame is just as
//! wrong. Both mistakes happen to survive PPSSPP often enough to look fine, and then fail on
//! hardware.
//!
//! So every dynamically-built vertex buffer is bump-allocated here, stays valid for the whole
//! frame, and is flushed out of the data cache before the GE runs.

use psp::sys;

const SCRATCH_BYTES: usize = 96 * 1024;

static mut BUF: psp::Align16<[u8; SCRATCH_BYTES]> = psp::Align16([0; SCRATCH_BYTES]);
static mut USED: usize = 0;
static mut HIGH_WATER: usize = 0;

/// Frees the whole arena. Call once at the top of each frame, before anything draws.
pub fn reset() {
    unsafe {
        USED = 0;
    }
}

/// Reserves room for `n` values of `T`, aligned for the GE. Returns null when exhausted, which
/// callers treat as "draw nothing" rather than scribbling past the end.
pub unsafe fn alloc<T>(n: usize) -> *mut T {
    const ALIGN: usize = 16;
    let start = (USED + ALIGN - 1) & !(ALIGN - 1);
    let bytes = n * core::mem::size_of::<T>();
    if start + bytes > SCRATCH_BYTES {
        return core::ptr::null_mut();
    }
    USED = start + bytes;
    if USED > HIGH_WATER {
        HIGH_WATER = USED;
    }
    (&raw mut BUF as *mut u8).add(start) as *mut T
}

/// Pushes this frame's writes out of the data cache so the GE sees them.
pub fn flush() {
    unsafe {
        if USED > 0 {
            sys::sceKernelDcacheWritebackRange(&raw const BUF as *const _, USED as u32);
        }
    }
}

/// Largest amount used by any frame so far, for tuning `SCRATCH_BYTES`.
pub fn high_water() -> usize {
    unsafe { HIGH_WATER }
}
