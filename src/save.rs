//! Persisted best time, score and combo.
//!
//! The encoding is deliberately paranoid. This file lives on a memory stick that can be removed
//! mid-write, and a half-written record that decodes as a plausible one would leave the player
//! with a best time they can never beat. Anything that does not check out reads as "no record".

/// Magic, version, three `u32` fields, checksum.
pub const RECORD_BYTES: usize = 20;

const MAGIC: [u8; 3] = *b"AZ0";
const VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// Best completed run, in hundredths of a second. Zero means no run has finished yet.
    pub best_time_cs: u32,
    pub best_score: u32,
    pub best_combo: u32,
}

impl Record {
    /// Whether a run has ever been completed.
    #[inline]
    pub fn has_time(&self) -> bool {
        self.best_time_cs > 0
    }

    pub fn encode(&self) -> [u8; RECORD_BYTES] {
        let mut out = [0u8; RECORD_BYTES];
        out[0..3].copy_from_slice(&MAGIC);
        out[3] = VERSION;
        out[4..8].copy_from_slice(&self.best_time_cs.to_le_bytes());
        out[8..12].copy_from_slice(&self.best_score.to_le_bytes());
        out[12..16].copy_from_slice(&self.best_combo.to_le_bytes());
        let sum = checksum(&out[0..16]);
        out[16..20].copy_from_slice(&sum.to_le_bytes());
        out
    }

    /// Returns `None` for anything short, foreign, newer, or corrupt.
    pub fn decode(bytes: &[u8]) -> Option<Record> {
        if bytes.len() < RECORD_BYTES {
            return None;
        }
        if bytes[0..3] != MAGIC || bytes[3] != VERSION {
            return None;
        }
        let stored = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
        if stored != checksum(&bytes[0..16]) {
            return None;
        }
        Some(Record {
            best_time_cs: u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            best_score: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
            best_combo: u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
        })
    }

    /// Folds a finished run in. Each record improves on its own, so a quick scrappy run cannot
    /// wipe out a hard-won score. Returns whether anything actually improved.
    pub fn merge_run(&mut self, time: f32, score: f32, best_combo: u32) -> bool {
        // Only a genuine finish counts; a zero time would be an unbeatable record.
        if !(time > 0.0) {
            return false;
        }
        let time_cs = (time * 100.0) as u32;
        let score = if score > 0.0 { score as u32 } else { 0 };
        let mut improved = false;

        if self.best_time_cs == 0 || time_cs < self.best_time_cs {
            self.best_time_cs = time_cs;
            improved = true;
        }
        if score > self.best_score {
            self.best_score = score;
            improved = true;
        }
        if best_combo > self.best_combo {
            self.best_combo = best_combo;
            improved = true;
        }
        improved
    }
}

/// FNV-1a, which is plenty for catching a truncated or scribbled-on file.
fn checksum(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811C_9DC5;
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}
