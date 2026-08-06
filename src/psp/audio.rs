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

/// Impact counter published by the game thread. The audio thread keeps its own copy of the last
/// value it acted on, so a hit fires exactly once no matter how the two threads interleave.
static mut IMPACT_SEQ: u32 = 0;

/// Hands the synthesiser the latest car state. Cheap enough to call every frame.
pub fn set_params(params: Params) {
    unsafe {
        PARAMS = params;
    }
}

/// Publishes the game's running impact count; the audio thread thumps on each change.
pub fn set_impact_count(count: u32) {
    unsafe {
        IMPACT_SEQ = count;
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

        let mut last_impact = IMPACT_SEQ;

        while RUNNING {
            let params = PARAMS;
            let impacts = IMPACT_SEQ;
            if impacts != last_impact {
                last_impact = impacts;
                synth.trigger_impact(0.9);
            }
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
            // Above the main thread's 32 — lower numbers win on the PSP — so a buffer is always
            // refilled before the hardware runs dry.
            //
            // Dropping this to 0x40 was tried, on the theory that the audio thread preempting
            // the main one could delay the buffer swap past vblank and tear the display. It did
            // not fix the display fault, and it made playback choppy: at a lower priority the
            // synthesiser gets scheduled behind a frame's work, and a late buffer is an audible
            // gap. The reasoning was sound but the premise was wrong, so it stays high.
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
