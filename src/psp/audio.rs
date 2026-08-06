//! Feeds the synthesised engine and tyre noise to the hardware.
//!
//! Audio runs on its own thread because `sceAudioOutputBlocking` does exactly what it says: it
//! parks until the hardware has taken the buffer. Called from the frame loop it would pin the
//! game to the audio clock, and any hitch in rendering would break the sound up.

use core::ffi::c_void;

use angle_zero::audio::{Params, Synth, FRAMES_PER_BUFFER};
use psp::sys::{self, AudioFormat, ThreadAttributes};

/// Written by the game thread each frame, read by the audio thread each buffer.
///
/// Deliberately unsynchronised. These are four independent `f32`s describing a continuous
/// signal; the worst a torn read can do is use one buffer's worth of slightly stale pitch, which
/// is inaudible. A lock here would risk stalling the audio thread, which is audible.
static mut PARAMS: Params = Params {
    engine_freq: 52.0,
    engine_gain: 0.012,
    cutoff: 700.0,
    squeal_gain: 0.0,
};

static mut SYNTH: Synth = Synth::new();
static mut BUFFER: psp::Align16<[i16; FRAMES_PER_BUFFER * 2]> =
    psp::Align16([0; FRAMES_PER_BUFFER * 2]);
static mut RUNNING: bool = false;

/// Hands the synthesiser the latest car state. Cheap enough to call every frame.
pub fn set_params(params: Params) {
    unsafe {
        PARAMS = params;
    }
}

extern "C" fn audio_thread(_argc: usize, _argv: *mut c_void) -> i32 {
    unsafe {
        let channel = sys::sceAudioChReserve(-1, FRAMES_PER_BUFFER as i32, AudioFormat::Stereo);
        if channel < 0 {
            // No channel available: run silently rather than taking the game down with us.
            return 0;
        }

        let buffer = &raw mut BUFFER as *mut i16;
        let synth = &mut *(&raw mut SYNTH);

        while RUNNING {
            let params = PARAMS;
            let slice = core::slice::from_raw_parts_mut(buffer, FRAMES_PER_BUFFER * 2);
            synth.render(&params, slice);
            // Full volume; the mix is already scaled well below the rail in the synthesiser.
            sys::sceAudioOutputBlocking(channel, 0x8000, buffer as *mut c_void);
        }

        sys::sceAudioChRelease(channel);
        0
    }
}

/// Starts the audio thread. Failure is silent — a game without sound still plays.
pub fn start() {
    unsafe {
        RUNNING = true;
        let id = sys::sceKernelCreateThread(
            b"angle_zero_audio\0".as_ptr(),
            audio_thread,
            // Above the main thread's 32, so buffers are refilled promptly even under load.
            0x12,
            32 * 1024,
            ThreadAttributes::USER,
            core::ptr::null_mut(),
        );
        if id.0 >= 0 {
            sys::sceKernelStartThread(id, 0, core::ptr::null_mut());
        }
    }
}
