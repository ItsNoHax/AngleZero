//! Reading and writing the record file.
//!
//! Plain file IO rather than `sceUtilitySavedata`. The utility route pops a system dialog and
//! wants a whole save-game structure; this is three numbers, and homebrew can write its own
//! directory on the memory stick directly.
//!
//! Every failure path here is silent by design. A missing memory stick, a full one, or a
//! scribbled file should cost the player their records, not their game.

use angle_zero::save::{Record, RECORD_BYTES};
use psp::sys::{self, IoOpenFlags};

const DIR: &[u8] = b"ms0:/PSP/SAVEDATA/ANGLEZERO\0";
const PATH: &[u8] = b"ms0:/PSP/SAVEDATA/ANGLEZERO/RECORD.BIN\0";

/// Loads the stored record, or a blank one if there is nothing readable there.
pub fn load() -> Record {
    unsafe {
        let fd = sys::sceIoOpen(PATH.as_ptr(), IoOpenFlags::RD_ONLY, 0o777);
        if fd.0 < 0 {
            return Record::default();
        }
        let mut buf = [0u8; RECORD_BYTES];
        let read = sys::sceIoRead(fd, buf.as_mut_ptr() as *mut _, RECORD_BYTES as u32);
        sys::sceIoClose(fd);

        if read < RECORD_BYTES as i32 {
            return Record::default();
        }
        Record::decode(&buf).unwrap_or_default()
    }
}

/// Writes the record back. Failures are swallowed — there is nothing useful to tell the player
/// mid-run, and losing a best time is better than interrupting the game.
pub fn store(record: &Record) {
    unsafe {
        // Both of these fail harmlessly if the directory already exists.
        sys::sceIoMkdir(b"ms0:/PSP/SAVEDATA\0".as_ptr(), 0o777);
        sys::sceIoMkdir(DIR.as_ptr(), 0o777);

        let fd = sys::sceIoOpen(
            PATH.as_ptr(),
            IoOpenFlags::WR_ONLY | IoOpenFlags::CREAT | IoOpenFlags::TRUNC,
            0o777,
        );
        if fd.0 < 0 {
            return;
        }
        let bytes = record.encode();
        sys::sceIoWrite(fd, bytes.as_ptr() as *const _, RECORD_BYTES);
        sys::sceIoClose(fd);
    }
}
