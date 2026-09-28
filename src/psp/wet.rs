//! The road after rain: each lamp's reflection smeared along the tarmac toward the eye, and the
//! car's tail lamps doubled in the road behind it.
//!
//! Only drawn when the player has picked a wet road on the title screen. Geometry is
//! `angle_zero::scenery::wet_streak`; this is the draw.

use core::ffi::c_void;

use angle_zero::camera::Camera;
use angle_zero::math::{cos, sin, Vec3};
use angle_zero::mesh::Vertex;
use angle_zero::scenery::{self, LAMP_STRIDE};
use angle_zero::track::{Track, NODE_COUNT};
use angle_zero::vehicle::Vehicle;
use psp::sys::{self, GuPrimitive, GuState, MatrixMode, VertexType};

use super::render::rgba;

const VERTEX_FORMAT: VertexType = VertexType::from_bits_truncate(
    VertexType::COLOR_8888.bits() | VertexType::VERTEX_32BITF.bits() | VertexType::TRANSFORM_3D.bits(),
);

const SODIUM: u32 = rgba(0xFF, 0xC4, 0x78, 0x9C);
const TAIL: u32 = rgba(0xFF, 0x3A, 0x3A, 0x48);
/// The same bias the ground pools use: these lie on the road too.
const STREAK_DEPTH_BIAS: i32 = 64;

fn fade(color: u32, k: f32) -> u32 {
    let a = ((color >> 24) as f32 * k) as u32;
    (color & 0x00ff_ffff) | (a.min(255) << 24)
}

pub unsafe fn draw(wet: bool, vehicle: &Vehicle, camera: &Camera, track: &Track) {
    if !wet {
        return;
    }
    const MAX_STREAKS: usize = 10;
    let cap = (MAX_STREAKS + 2) * 24;
    let ptr = super::scratch::alloc::<Vertex>(cap);
    if ptr.is_null() {
        return;
    }
    let out = core::slice::from_raw_parts_mut(ptr, cap);
    let mut w = 0usize;
    let v = |p: Vec3, c: u32| Vertex::new(p.x, p.y, p.z, c);

    let mut streaks = 0;
    let mut lamp = 0usize;
    while lamp < NODE_COUNT && streaks < MAX_STREAKS {
        if let Some(s) = scenery::wet_streak(track, lamp, camera.pos) {
            for k in 0..4 {
                let (a, b, ka) = s[k];
                let (c, d, kb) = s[k + 1];
                let (ca, cb) = (fade(SODIUM, ka), fade(SODIUM, kb));
                out[w..w + 6].copy_from_slice(&[v(a, ca), v(b, ca), v(d, cb), v(a, ca), v(d, cb), v(c, cb)]);
                w += 6;
            }
            streaks += 1;
        }
        lamp += LAMP_STRIDE;
    }

    // The tail lamps in the road behind the car.
    let st = vehicle.state;
    let (f, r) = ((sin(st.yaw), cos(st.yaw)), (cos(st.yaw), -sin(st.yaw)));
    for side in [-0.62f32, 0.62] {
        let at = |back: f32, half: f32| {
            Vec3::new(
                st.x - f.0 * back + r.0 * (side + half),
                st.y + 0.05,
                st.z - f.1 * back + r.1 * (side + half),
            )
        };
        let (near, far) = (fade(TAIL, 1.0), fade(TAIL, 0.0));
        let (a, b, c, d) = (at(2.05, -0.09), at(2.05, 0.09), at(3.4, 0.05), at(3.4, -0.05));
        out[w..w + 6].copy_from_slice(&[v(a, near), v(b, near), v(c, far), v(a, near), v(c, far), v(d, far)]);
        w += 6;
    }

    sys::sceGuEnable(GuState::Blend);
    sys::sceGuBlendFunc(sys::BlendOp::Add, sys::BlendFactor::SrcAlpha, sys::BlendFactor::Fix, 0, 0xffff_ffff);
    sys::sceGuDepthMask(1);
    sys::sceGuDepthOffset(STREAK_DEPTH_BIAS);
    sys::sceGuDisable(GuState::CullFace);
    sys::sceGumMatrixMode(MatrixMode::Model);
    sys::sceGumLoadIdentity();
    sys::sceGumDrawArray(GuPrimitive::Triangles, VERTEX_FORMAT, w as i32, core::ptr::null(), out.as_ptr() as *const c_void);
    sys::sceGuDepthOffset(0);
    sys::sceGuEnable(GuState::CullFace);
    sys::sceGuDepthMask(0);
    sys::sceGuDisable(GuState::Blend);
}
