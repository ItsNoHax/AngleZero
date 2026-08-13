//! PSP entry point: GU bring-up, controller polling and the frame loop.
//!
//! All game logic lives in the `angle_zero` library so it stays host-testable. This module owns
//! only the things that need the hardware: the display list, the framebuffers, the pad, and the
//! clock that drives the fixed-timestep update.

pub mod audio;
pub mod car;
#[cfg(feature = "devtools")]
pub mod atractest;
#[cfg(feature = "devtools")]
pub mod capture;
#[cfg(feature = "harness")]
pub mod harness;
#[cfg(feature = "devtools")]
pub mod trace;
pub mod hud;
pub mod render;
pub mod scratch;
pub mod storage;
pub mod text;

use core::ffi::c_void;

use angle_zero::game::{Buttons, Game, Phase};
use angle_zero::track::Track;
use psp::sys::{
    self, ClearBuffer, CtrlButtons, CtrlMode, DepthFunc, DisplayPixelFormat, FrontFaceDirection,
    GuContextType, GuState, GuSyncBehavior, GuSyncMode, SceCtrlData, ShadingModel,
    TexturePixelFormat,
};
use psp::vram_alloc::get_vram_allocator;
use psp::{BUF_WIDTH, SCREEN_HEIGHT, SCREEN_WIDTH};

psp::module!("AngleZero", 1, 0);

/// The display list. One megabyte is generous, but the terrain ribbon alone can queue a lot of
/// vertices in a frame and overrunning this corrupts the GE state silently.
static mut LIST: psp::Align16<[u32; 262144]> = psp::Align16([0; 262144]);

/// Roughly 100 KB of centreline. Far too large for the stack, so it lives here.
static mut TRACK: Track = Track::EMPTY;
static mut GAME: Game = Game::new();

/// Why there is no car, if there is no car. Read by the title screen.
static mut CAR_LOAD: Option<car::LoadError> = None;

/// The car asset's loading fault, or `None` if it loaded.
pub fn car_fault() -> Option<car::LoadError> {
    unsafe { CAR_LOAD }
}

/// Takes the vehicle's proportions and its handling off whichever car it is now driving.
///
/// The wheels have to roll at the speed the car is going and the tyre marks have to land under the
/// tyres, and both of those are measurements of the model. What a car drives like is not — nothing
/// in a mesh says what an engine produces, so it is authored in the car's config and compiled in
/// beside the geometry — but it changes at the same moment and for the same reason, so it is taken
/// here too. A car that carries neither gets the numbers the game was tuned with.
fn refresh_car(game: &mut Game) {
    if let Some(car) = car::get(game.vehicle.model) {
        game.vehicle.shape = car.shape();
        game.vehicle.handling = car.handling();
    }
}

/// Ask the emulator to capture the display framebuffer. PPSSPP writes it to whatever path was
/// passed to `--screenshot-save`; on real hardware this devctl simply fails and does nothing —
/// which is exactly why a shipping build should not be issuing it twice a second.
/// A harness build writes its own frames, one per frame of the burst, so it never asks for this.
#[cfg(feature = "devtools")]
#[cfg_attr(feature = "harness", allow(dead_code))]
fn emit_screenshot() {
    const EMULATOR_DEVCTL_EMIT_SCREENSHOT: u32 = 0x20;

    unsafe {
        sys::sceIoDevctl(
            b"emulator:\0".as_ptr(),
            EMULATOR_DEVCTL_EMIT_SCREENSHOT,
            core::ptr::null_mut(),
            0,
            core::ptr::null_mut(),
            0,
        );
    }
}

/// Reads the pad into the core's controller struct.
///
/// A harness build never calls this — its input comes from a script keyed to the frame counter —
/// but it stays compiled so the two builds share one definition of what each button means.
#[cfg_attr(feature = "harness", allow(dead_code))]
fn read_buttons(pad: &SceCtrlData) -> Buttons {
    let b = pad.buttons;
    // The nub reads 0..255 with 128 at centre; positive steer is left, so the sign flips.
    let raw = -((pad.lx as f32) - 128.0) / 128.0;
    let analog_x = if raw > 0.2 || raw < -0.2 { raw } else { 0.0 };

    Buttons {
        cross: b.contains(CtrlButtons::CROSS),
        circle: b.contains(CtrlButtons::CIRCLE),
        square: b.contains(CtrlButtons::SQUARE),
        triangle: b.contains(CtrlButtons::TRIANGLE),
        up: b.contains(CtrlButtons::UP),
        down: b.contains(CtrlButtons::DOWN),
        left: b.contains(CtrlButtons::LEFT),
        right: b.contains(CtrlButtons::RIGHT),
        analog_x,
    }
}

pub fn psp_main() {
    psp::enable_home_button();

    unsafe {
        sys::sceCtrlSetSamplingCycle(0);
        sys::sceCtrlSetSamplingMode(CtrlMode::Analog);

        let allocator = get_vram_allocator().unwrap();
        let fbp0 = allocator
            .alloc_texture_pixels(BUF_WIDTH, SCREEN_HEIGHT, TexturePixelFormat::Psm8888)
            .as_mut_ptr_from_zero();
        let fbp1 = allocator
            .alloc_texture_pixels(BUF_WIDTH, SCREEN_HEIGHT, TexturePixelFormat::Psm8888)
            .as_mut_ptr_from_zero();
        let zbp = allocator
            .alloc_texture_pixels(BUF_WIDTH, SCREEN_HEIGHT, TexturePixelFormat::Psm4444)
            .as_mut_ptr_from_zero();

        sys::sceGuInit();
        sys::sceGuStart(GuContextType::Direct, &raw mut LIST as *mut _ as *mut c_void);
        sys::sceGuDrawBuffer(DisplayPixelFormat::Psm8888, fbp0 as _, BUF_WIDTH as i32);
        sys::sceGuDispBuffer(
            SCREEN_WIDTH as i32,
            SCREEN_HEIGHT as i32,
            fbp1 as _,
            BUF_WIDTH as i32,
        );
        sys::sceGuDepthBuffer(zbp as _, BUF_WIDTH as i32);
        sys::sceGuOffset(2048 - (SCREEN_WIDTH / 2), 2048 - (SCREEN_HEIGHT / 2));
        sys::sceGuViewport(2048, 2048, SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32);
        // The PSP's depth buffer runs backwards, hence the reversed range and GEQUAL below.
        sys::sceGuDepthRange(65535, 0);
        sys::sceGuScissor(0, 0, SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32);
        sys::sceGuEnable(GuState::ScissorTest);
        sys::sceGuDepthFunc(DepthFunc::GreaterOrEqual);
        sys::sceGuEnable(GuState::DepthTest);
        sys::sceGuFrontFace(FrontFaceDirection::CounterClockwise);
        sys::sceGuShadeModel(ShadingModel::Smooth);
        sys::sceGuEnable(GuState::CullFace);
        sys::sceGuEnable(GuState::ClipPlanes);
        sys::sceGuEnable(GuState::Fog);
        sys::sceGuFog(render::FOG_NEAR, render::FOG_FAR, render::FOG_COLOR);

        // rust-psp lazily creates its VFPU matrix context, but only inside `sceGumLoadIdentity`
        // and `sceGumLoadMatrix`. Every other `sceGum*` entry point assumes it already exists and
        // hits an `unreachable` if it does not, which surfaces as a break instruction rather than
        // a panic message. Touching it once here means later code can start with `sceGumMatrixMode`.
        sys::sceGumLoadIdentity();

        sys::sceGuFinish();
        sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
        sys::sceDisplayWaitVblankStart();
        sys::sceGuDisplay(true);

        // --- one-time world build ---
        let track = &mut *(&raw mut TRACK);
        Track::generate(track);
        scratch::init();
        // Every car on the stick, whatever they are called. A car that fails to load is not fatal
        // — the game runs, and the title screen says why there is no car.
        CAR_LOAD = car::load_all();
        render::init(track);
        text::init();
        hud::init_minimap(track);
        // init_minimap writes a static the GE reads every frame, and is the last thing to touch
        // one, so the cache has to go out after it rather than before.
        sys::sceKernelDcacheWritebackAll();

        audio::start();

        // Ask the console's own decoder what it makes of the music, once at boot.
        #[cfg(feature = "devtools")]
        atractest::run();

        let game = &mut *(&raw mut GAME);
        game.record = storage::load();
        game.car_count = car::count().max(1);
        refresh_car(game);
        game.enter_title(track);

        // The scripted run, loaded before the first frame so frame 0 already has its buttons.
        #[cfg(feature = "harness")]
        {
            harness::init();
            render::set_debug_mode(harness::mode());
        }

        let pad = &mut SceCtrlData::default();
        let mut frame: u32 = 0;
        #[cfg(not(feature = "harness"))]
        let mut last_tick = sys::sceKernelGetSystemTimeLow();

        // Hardware-only diagnostics: START toggles the readout, SELECT saves a frame and its
        // counters to ms0:/ANGLEZERO/. Both are edge-triggered off the raw pad, deliberately
        // outside the game's own input mapping so they cannot affect driving.
        // Without the overlay reading it at the top of the frame, a harness build only ever reads
        // this after the assignment further down, so its initial value really is dead there.
        #[cfg(feature = "devtools")]
        #[cfg_attr(feature = "harness", allow(unused_assignments))]
        let mut diag = capture::Diagnostics::default();
        #[cfg(all(feature = "devtools", not(feature = "harness")))]
        let mut show_debug = false;
        #[cfg(feature = "devtools")]
        let mut shots: u32 = 0;
        // Each capture is half a megabyte; enough for a burst, not enough to fill a stick.
        #[cfg(all(feature = "devtools", not(feature = "harness")))]
        const MAX_SHOTS: u32 = 40;
        // A harness run writes to a host filesystem rather than a memory stick, and wants a long
        // enough window that a two-frame artifact cannot fall outside it.
        #[cfg(feature = "harness")]
        const MAX_SHOTS: u32 = 64;
        #[cfg(all(feature = "devtools", not(feature = "harness")))]
        let mut burst: u32 = 0;
        #[cfg(all(feature = "devtools", not(feature = "harness")))]
        let mut prev_debug_buttons = CtrlButtons::empty();
        #[cfg(all(feature = "devtools", not(feature = "harness")))]
        let mut debug_mode: u32 = 0;

        loop {
            sys::sceCtrlReadBufferPositive(pad, 1);
            #[cfg(not(feature = "harness"))]
            let buttons = read_buttons(pad);
            // A harness run ignores the pad and replays its script off the frame counter, so that
            // input lands on the same frame every time.
            #[cfg(feature = "harness")]
            let buttons = harness::buttons_for(frame);

            // Microsecond clock, so the fixed-timestep accumulator sees real elapsed time
            // rather than an assumed 60 Hz.
            #[cfg(not(feature = "harness"))]
            let dt = {
                let now = sys::sceKernelGetSystemTimeLow();
                let dt = (now.wrapping_sub(last_tick)) as f32 / 1_000_000.0;
                last_tick = now;
                dt
            };
            // Fixed instead, so the run does not depend on how long the host took to write the
            // last half-megabyte capture. See `harness::DT`.
            #[cfg(feature = "harness")]
            let dt = harness::DT;

            // A scripted run captures a fixed window of *consecutive* frames instead. The
            // interactive burst below samples one frame in four, which is fine for eyeballing but
            // steps straight over a one-frame blink — and a blink is exactly what the host-side
            // detector looks for.
            #[cfg(feature = "harness")]
            let want_capture = harness::capturing(frame) && shots < MAX_SHOTS;

            // A tap saves one frame; holding SELECT saves a burst, which is the only practical
            // way to catch something that flickers for a few frames while you are also driving.
            #[cfg(all(feature = "devtools", not(feature = "harness")))]
            let want_capture = {
                let start_edge = pad.buttons.contains(CtrlButtons::START)
                    && !prev_debug_buttons.contains(CtrlButtons::START);
                let select_held = pad.buttons.contains(CtrlButtons::SELECT);
                let select_edge = select_held && !prev_debug_buttons.contains(CtrlButtons::SELECT);
                // L cycles a render-state override, for pinning down a fault that only appears
                // on hardware. See `render::DEBUG_MODES`. Every edge here has to be read before
                // `prev_debug_buttons` is updated, or it can never be true.
                let l_edge = pad.buttons.contains(CtrlButtons::LTRIGGER)
                    && !prev_debug_buttons.contains(CtrlButtons::LTRIGGER);
                prev_debug_buttons = pad.buttons;
                if start_edge {
                    show_debug = !show_debug;
                }
                if l_edge {
                    debug_mode = (debug_mode + 1) % render::DEBUG_MODES;
                    render::set_debug_mode(debug_mode);
                }
                if select_edge {
                    burst = 0;
                }
                // Every 4th frame while held. Denser than before, because a burst that samples
                // one frame in ten slid straight past the artifact this is meant to catch.
                let due = select_held && burst > 0 && burst % 4 == 0;
                if select_held {
                    burst += 1;
                }
                (select_edge || due) && shots < MAX_SHOTS
            };

            // Before the update, so the frame that gets rendered is one step of settled physics
            // after the placement rather than the raw pose.
            #[cfg(feature = "harness")]
            if let Some((node, speed)) = harness::place_at(frame) {
                game.vehicle
                    .place_at_node(track, node.min(angle_zero::track::NODE_COUNT - 1));
                game.vehicle.state.vx = speed;
            }

            #[cfg(feature = "devtools")]
            let work_start = sys::sceKernelGetSystemTimeLow();
            let driving = game.vehicle.model;
            game.update(track, buttons, dt);
            // The title screen can swap the car. Re-measure only when it actually changed, rather
            // than every frame for a value that almost never moves.
            if game.vehicle.model != driving {
                refresh_car(game);
            }
            if game.take_record_dirty() {
                storage::store(&game.record);
            }

            audio::set_impact_count(game.impact_count());
            audio::set_params(angle_zero::audio::params_for(
                game.vehicle.state.vx,
                angle_zero::hud::rpm(game.vehicle.state.vx, game.throttle_hint()),
                game.throttle_hint(),
                game.phase == Phase::Run,
                game.vehicle.drifting,
                game.vehicle.slip_angle,
            ));

            // Every dynamically-built vertex buffer for this frame comes from here, and must
            // stay valid until the GE has run the list below.
            scratch::reset();

            sys::sceGuStart(GuContextType::Direct, &raw mut LIST as *mut _ as *mut c_void);
            sys::sceGuClearColor(render::SKY_CLEAR);
            sys::sceGuClearDepth(0);
            sys::sceGuClear(
                ClearBuffer::COLOR_BUFFER_BIT
                    | ClearBuffer::DEPTH_BUFFER_BIT
                    | ClearBuffer::FAST_CLEAR_BIT,
            );

            sys::sceGuEnable(GuState::DepthTest);
            sys::sceGuEnable(GuState::Fog);
            sys::sceGuEnable(GuState::CullFace);
            sys::sceGuDisable(GuState::Texture2D);

            render::set_camera(&game.camera);
            render::draw_sky(&game.camera);
            render::draw_world(&game.camera);
            render::draw_car(&game.vehicle, track);
            if game.phase != Phase::Title {
                // Beams first: they lie on the road, under the smoke and the glows that stand
                // above it. Every one of these is additive with no depth write, so the order is
                // about what looks right rather than about what is occluded.
                render::draw_light_beams(&game.vehicle, track, game.braking_hint());
                render::draw_effects(&game.effects, &game.camera);
                render::draw_lamp_glows(&game.vehicle, &game.camera, game.braking_hint());
            }

            hud::begin();
            // Both of these draw over the frame, so a harness build leaves them out: the detector
            // compares pixels, and a diagnostic overlay is just more pixels to explain.
            #[cfg(all(feature = "devtools", not(feature = "harness")))]
            hud::debug_mode_label(render::debug_mode());
            // Mode 8 drops the whole 2D pass. The gaps are axis-aligned and 16-pixel aligned,
            // which is what a screen-space draw looks like and not what a triangle looks like,
            // so it is worth knowing whether the HUD is involved at all.
            #[cfg(feature = "devtools")]
            let skip_hud = render::debug_mode() == 8;
            #[cfg(not(feature = "devtools"))]
            let skip_hud = false;
            if !skip_hud {
                hud::draw(game, track);
            }
            hud::scanlines();
            #[cfg(all(feature = "devtools", not(feature = "harness")))]
            if show_debug {
                hud::debug_overlay(&diag, shots);
            }
            hud::end();

            // `sceGuFinish` reports how much of the display list this frame used; overrunning
            // the buffer corrupts GE state silently, so it is worth watching.
            #[cfg_attr(not(feature = "devtools"), allow(unused_variables))]
            let list_bytes = sys::sceGuFinish() as u32;
            sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);

            #[cfg(feature = "devtools")]
            {
                diag = capture::Diagnostics {
                    frame_us: sys::sceKernelGetSystemTimeLow().wrapping_sub(work_start),
                    list_bytes,
                    scratch_peak: scratch::high_water() as u32,
                    scratch_failures: scratch::failures(),
                    car_calls: render::car_calls(),
                    lamps: render::light_counts().0,
                    beams: render::light_counts().1,
                    live_skids: game.effects.live_skids() as u32,
                    live_puffs: game.effects.live_puffs() as u32,
                    drifting: game.vehicle.drifting,
                    speed_kph: (game.vehicle.speed_kph() + 0.5) as u32,
                    descent_percent: game.descent_percent(track),
                };
            }

            sys::sceDisplayWaitVblankStart();
            sys::sceGuSwapBuffers();

            // After the swap, so the capture is of the frame just shown rather than the one
            // still being drawn into.
            // Cleared every frame in every build, so the two tally identically.
            #[cfg_attr(not(feature = "devtools"), allow(unused_variables))]
            let stats = render::take_stats();

            #[cfg(feature = "devtools")]
            {
                trace::record(frame, &stats, game, diag.frame_us);
                // After the burst, so the ring covers the frames that were captured.
                #[cfg(feature = "harness")]
                let dump_trace = harness::burst_ends(frame);
                #[cfg(not(feature = "harness"))]
                let dump_trace = want_capture && (shots == 0 || burst == 1);
                if dump_trace {
                    trace::dump(shots);
                }
                if want_capture && capture::capture(game, &diag).is_some() {
                    shots += 1;
                }
            }

            frame += 1;
            // The script has run out, and a harness run has nobody watching it — so end the
            // process rather than leaving headless to sit out the rest of its `--timeout`.
            #[cfg(feature = "harness")]
            if harness::finished(frame) {
                sys::sceKernelExitGame();
            }
            #[cfg(all(feature = "devtools", not(feature = "harness")))]
            if frame % 30 == 0 {
                emit_screenshot();
            }
            let _ = frame;
        }
    }
}
