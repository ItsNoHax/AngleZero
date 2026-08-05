#![no_std]
#![no_main]

psp::module!("AngleZero", 1, 0);

use core::ffi::c_void;
use psp::sys::{
    self, ClearBuffer, CtrlButtons, CtrlMode, DisplayPixelFormat, GuContextType, GuState,
    GuSyncBehavior, GuSyncMode, SceCtrlData, TexturePixelFormat, sceCtrlSetSamplingCycle,
    sceCtrlSetSamplingMode,
};
use psp::vram_alloc::get_vram_allocator;
use psp::{BUF_WIDTH, SCREEN_HEIGHT, SCREEN_WIDTH};

static mut LIST: psp::Align16<[u32; 262144]> = psp::Align16([0; 262144]);

// Colours are ABGR, not ARGB: 0xff4040e0 has R=0xe0 and renders red.
const IDLE: u32 = 0xff30_2010; // navy
const CROSS: u32 = 0xff40_40e0; // red
const CIRCLE: u32 = 0xff40_e040; // green

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

pub fn psp_main() {
    psp::enable_home_button();

    unsafe {
        sceCtrlSetSamplingCycle(0);
        sceCtrlSetSamplingMode(CtrlMode::Analog);

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
        sys::sceGuStart(
            GuContextType::Direct,
            &raw mut LIST as *mut _ as *mut c_void,
        );
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
        sys::sceGuDepthRange(65535, 0);
        sys::sceGuScissor(0, 0, SCREEN_WIDTH as i32, SCREEN_HEIGHT as i32);
        sys::sceGuEnable(GuState::ScissorTest);
        sys::sceGuFinish();
        sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
        sys::sceDisplayWaitVblankStart();
        sys::sceGuDisplay(true);

        let ctrl_data = &mut SceCtrlData::default();
        let mut frame: u32 = 0;

        loop {
            sys::sceCtrlReadBufferPositive(ctrl_data, 1);

            let color = if ctrl_data.buttons.contains(CtrlButtons::CROSS) {
                CROSS
            } else if ctrl_data.buttons.contains(CtrlButtons::CIRCLE) {
                CIRCLE
            } else {
                IDLE
            };

            sys::sceGuStart(
                GuContextType::Direct,
                &raw mut LIST as *mut _ as *mut c_void,
            );
            sys::sceGuClearColor(color);
            sys::sceGuClear(ClearBuffer::COLOR_BUFFER_BIT | ClearBuffer::FAST_CLEAR_BIT);
            sys::sceGuFinish();
            sys::sceGuSync(GuSyncMode::Finish, GuSyncBehavior::Wait);
            sys::sceDisplayWaitVblankStart();
            sys::sceGuSwapBuffers();

            frame += 1;
            if frame % 60 == 0 {
                emit_screenshot();
            }
        }
    }
}
