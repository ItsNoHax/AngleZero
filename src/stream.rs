//! Reading a car a piece at a time, and the arithmetic of doing so.
//!
//! A car is the better part of a megabyte and a memory stick is not fast. Read whole, it stops the
//! title screen dead for a fraction of a second every time somebody presses L or R — which is the
//! one screen where the player is pressing L and R repeatedly. Read a chunk per frame instead and
//! the screen keeps running at its own rate while the car arrives over the next handful of frames.
//!
//! What is worth testing about that is not the file handle, which only exists on the console, but
//! the counting: how much to ask for next, when the last piece has landed, and what it means when
//! the device hands back less than was asked for. That is this, and it runs on the host.
//!
//! A short read is a failure, not a pause. `sceIoRead` returns less than requested at the end of a
//! file or when the read failed, and the size was measured before the first chunk — so anything
//! short of what was asked for, before the end, is a file that is no longer what it said it was.

/// How far through a car's file a load has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    size: usize,
    done: usize,
}

impl Progress {
    /// Starts a load of a file of `size` bytes.
    pub const fn new(size: usize) -> Progress {
        Progress { size, done: 0 }
    }

    /// How many bytes to ask for next, given a chunk budget, or zero when there is nothing left.
    ///
    /// Never overruns the file, so the last chunk is short by design and the caller can tell that
    /// apart from a short read: it asked for exactly what was left.
    pub const fn next_read(&self, chunk: usize) -> usize {
        let left = self.size - self.done;
        if left < chunk {
            left
        } else {
            chunk
        }
    }

    /// Everything that is left, for a load that has to finish now — the player pressed X.
    pub const fn rest(&self) -> usize {
        self.size - self.done
    }

    /// Records bytes that actually landed.
    pub fn advance(&mut self, bytes: usize) {
        self.done = if self.done + bytes > self.size {
            self.size
        } else {
            self.done + bytes
        };
    }

    pub const fn is_complete(&self) -> bool {
        self.done >= self.size
    }

    pub const fn done(&self) -> usize {
        self.done
    }

    pub const fn size(&self) -> usize {
        self.size
    }

    /// How far along, 0.0 to 1.0, for anything on screen that wants to show it.
    pub fn fraction(&self) -> f32 {
        if self.size == 0 {
            return 1.0;
        }
        self.done as f32 / self.size as f32
    }
}
