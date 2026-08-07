//! Feeds `assets/SND0.AT3` to the PSP's own ATRAC3 decoder and writes down what it says.
//!
//! The XMB refuses to play the file while every check that can be made on a PC says it is
//! correct: the RIFF header is byte-identical to files that do play, the frames carry the same
//! unit id, band count and joint-stereo id, and ffmpeg decodes it cleanly. So ask the decoder
//! that actually rejects it — `sceAtrac` is the same one the XMB uses, and it returns an error
//! code rather than just going quiet.
//!
//! Results land in `ms0:/ANGLEZERO/ATRAC.TXT`. Emulators implement this call too, so the answer
//! can be read without the handheld.

use psp::sys::{self, IoOpenFlags};

/// The music, embedded so the test needs nothing on the memory stick. `sceAtrac` reads it in
/// place, and the hardware wants it aligned.
static AT3: psp::Align16<[u8; include_bytes!("../../assets/SND0.AT3").len()]> =
    psp::Align16(*include_bytes!("../../assets/SND0.AT3"));

fn push_str(out: &mut [u8], w: &mut usize, s: &[u8]) {
    for &c in s {
        if *w < out.len() {
            out[*w] = c;
            *w += 1;
        }
    }
}

/// Signed decimal, and hex too — `sceAtrac` error codes are only recognisable in hex.
fn push_i32(out: &mut [u8], w: &mut usize, mut v: i32) {
    if v < 0 {
        push_str(out, w, b"-");
        v = -v;
    }
    let mut digits = [0u8; 12];
    let mut n = 0;
    if v == 0 {
        digits[0] = b'0';
        n = 1;
    }
    let mut u = v as u32;
    while u > 0 && n < 12 {
        digits[n] = b'0' + (u % 10) as u8;
        u /= 10;
        n += 1;
    }
    for i in 0..n {
        if *w < out.len() {
            out[*w] = digits[n - 1 - i];
            *w += 1;
        }
    }
}

fn push_hex(out: &mut [u8], w: &mut usize, v: u32) {
    push_str(out, w, b"0x");
    for shift in (0..8).rev() {
        let nib = ((v >> (shift * 4)) & 0xF) as u8;
        let c = if nib < 10 { b'0' + nib } else { b'A' + nib - 10 };
        if *w < out.len() {
            out[*w] = c;
            *w += 1;
        }
    }
}

/// Runs the test and writes the report. Returns the atrac id, or the negative error code.
pub fn run() -> i32 {
    let mut text = [0u8; 1024];
    let mut w = 0usize;

    unsafe {
        let ptr = &raw const AT3 as *mut core::ffi::c_void;
        let len = AT3.0.len();

        push_str(&mut text, &mut w, b"SND0.AT3 through sceAtrac\nsize ");
        push_i32(&mut text, &mut w, len as i32);

        // A negative id here is the whole answer: it is the decoder refusing the file, with a
        // reason attached.
        let id = sys::sceAtracSetDataAndGetID(ptr, len);
        push_str(&mut text, &mut w, b"\nsceAtracSetDataAndGetID ");
        push_i32(&mut text, &mut w, id);
        push_str(&mut text, &mut w, b" (");
        push_hex(&mut text, &mut w, id as u32);
        push_str(&mut text, &mut w, b")");

        if id >= 0 {
            let mut bitrate: u32 = 0;
            let r = sys::sceAtracGetBitrate(id, &mut bitrate as *mut u32 as *mut i32);
            push_str(&mut text, &mut w, b"\nbitrate rc ");
            push_i32(&mut text, &mut w, r);
            push_str(&mut text, &mut w, b" value ");
            push_i32(&mut text, &mut w, bitrate as i32);

            let mut max: i32 = 0;
            let r = sys::sceAtracGetMaxSample(id, &mut max);
            push_str(&mut text, &mut w, b"\nmaxsample rc ");
            push_i32(&mut text, &mut w, r);
            push_str(&mut text, &mut w, b" value ");
            push_i32(&mut text, &mut w, max);

            // Decoding one frame proves the stream is not merely accepted but usable.
            static mut PCM: psp::Align16<[u16; 4096]> = psp::Align16([0; 4096]);
            let (mut n, mut end, mut remain) = (0i32, 0i32, 0i32);
            let r = sys::sceAtracDecodeData(
                id,
                &raw mut PCM as *mut u16,
                &mut n,
                &mut end,
                &mut remain,
            );
            push_str(&mut text, &mut w, b"\ndecode rc ");
            push_i32(&mut text, &mut w, r);
            push_str(&mut text, &mut w, b" (");
            push_hex(&mut text, &mut w, r as u32);
            push_str(&mut text, &mut w, b") samples ");
            push_i32(&mut text, &mut w, n);

            // Silence out of a "successful" decode would point at the bitstream rather than the
            // header, so measure rather than assume.
            let pcm = &*(&raw const PCM);
            let mut peak = 0i32;
            for i in 0..(n.max(0) as usize * 2).min(4096) {
                let s = pcm.0[i] as i16 as i32;
                let a = if s < 0 { -s } else { s };
                if a > peak {
                    peak = a;
                }
            }
            push_str(&mut text, &mut w, b"\nfirst frame peak ");
            push_i32(&mut text, &mut w, peak);

            sys::sceAtracReleaseAtracID(id);
        }
        push_str(&mut text, &mut w, b"\n");

        sys::sceIoMkdir(b"ms0:/ANGLEZERO\0".as_ptr(), 0o777);
        let fd = sys::sceIoOpen(
            b"ms0:/ANGLEZERO/ATRAC.TXT\0".as_ptr(),
            IoOpenFlags::WR_ONLY | IoOpenFlags::CREAT | IoOpenFlags::TRUNC,
            0o777,
        );
        if fd.0 >= 0 {
            sys::sceIoWrite(fd, text.as_ptr() as *const _, w);
            sys::sceIoClose(fd);
        }
        id
    }
}
