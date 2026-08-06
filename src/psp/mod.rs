//! PSP entry point: GU bring-up, controller polling and the frame loop.
//!
//! All game logic lives in the `angle_zero` library so it stays host-testable. This module owns
//! only the things that need the hardware: the display list, the framebuffers, the pad, and the
//! clock that drives the fixed-timestep update.

pub mod audio;
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

/// Ask the emulator to capture the display framebuffer. PPSSPP writes it to whatever path was
/// passed to `--screenshot-save`; on real hardware this devctl simply fails and does nothing.
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
        render::init(track);
        text::init();
        hud::init_minimap(track);

        audio::start();

        let game = &mut *(&raw mut GAME);
        game.record = storage::load();
        game.enter_title(track);

        let pad = &mut SceCtrlData::default();
        let mut frame: u32 = 0;
        let mut last_tick = sys::sceKernelGetSystemTimeLow();

        loop {
            sys::sceCtrlReadBufferPositive(pad, 1);
            let buttons = read_buttons(pad);

            // Microsecond clock, so the fixed-timestep accumulator sees real elapsed time
            // rather than an assumed 60 Hz.
            let now = sys::sceKernelGetSystemTimeLow();
            let dt = (now.wrapping_sub(last_tick)) as f32 / 1_000_000.0;
            last_tick = now;

            game.update(track, buttons, dt);
            if game.take_record_dirty() {
                storage::store(&game.record);
            }

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
                render::draw_headlight_beams(&game.vehicle.state);
                render::draw_effects(&game.effects, &game.camera);
                render::draw_lamp_glows(&game.vehicle.state, &game.camera, game.braking_hint());
            }

            hud::begin();
            hud::draw(game, track);
            hud::scanlines();
            hud::end();

            // The GE reads through uncached memory, so this frame's vertices have to leave the
            // data cache before the list is kicked.
            scratch::flush();

            sys::sceGuFinish();
            sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
            sys::sceDisplayWaitVblankStart();
            sys::sceGuSwapBuffers();

            frame += 1;
            if frame % 30 == 0 {
                emit_screenshot();
            }
        }
    }
}
