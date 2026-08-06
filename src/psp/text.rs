//! A 5×7 pixel font, baked into a texture at boot and drawn as 2D sprites.
//!
//! Drawing each lit pixel as its own quad would cost thousands of primitives for a HUD this
//! wordy. Instead the glyph bitmaps below are expanded once into a 128×64 texture, after which a
//! character costs two vertices.

use core::ffi::c_void;
use psp::sys::{
    self, GuPrimitive, GuState, MipmapLevel, TextureColorComponent, TextureEffect, TextureFilter,
    TexturePixelFormat, VertexType,
};

/// Glyph cell size in the font texture.
const CELL: usize = 8;
const TEX_W: usize = 128;
const TEX_H: usize = 64;
/// Printable range covered, starting at space. Reaches past `Z` so lowercase `x` — the drift
/// multiplier's `x2`, `x3` — has a cell of its own.
const FIRST_CHAR: u8 = 32;
const GLYPH_COUNT: usize = 96;

/// Vertex for 2D textured sprites: texture, then colour, then position.
#[repr(C)]
#[derive(Clone, Copy)]
struct SpriteVert {
    u: f32,
    v: f32,
    color: u32,
    x: f32,
    y: f32,
    z: f32,
}

/// Font texture, 32-bit so it can be blended straight over the scene.
static mut FONT_TEX: psp::Align16<[u32; TEX_W * TEX_H]> = psp::Align16([0; TEX_W * TEX_H]);

/// Longest single string this can draw.
const MAX_CHARS: usize = 128;

/// Bitmaps for ASCII 32..95, five bits wide and seven rows tall. Unlisted characters are blank.
const fn glyph(c: u8) -> [u8; 7] {
    match c {
        b'0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
        b'1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'2' => [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111],
        b'3' => [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110],
        b'4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
        b'5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
        b'6' => [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110],
        b'7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
        b'8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
        b'9' => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100],
        b'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        b'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        b'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        b'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        b'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        b'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        b'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111],
        b'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        b'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        b'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100],
        b'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        b'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        b'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        b'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        b'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        b'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        b'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        b'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        b'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        b'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        b'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        b'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        b'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        b'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        b'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        b'-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        b'.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
        b',' => [0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b00100, 0b01000],
        b'/' => [0b00001, 0b00010, 0b00010, 0b00100, 0b01000, 0b01000, 0b10000],
        b':' => [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000],
        b'!' => [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100],
        b'%' => [0b11001, 0b11010, 0b00010, 0b00100, 0b01000, 0b01011, 0b10011],
        b'>' => [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000],
        b'<' => [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010],
        b'+' => [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000],
        b'x' => [0b00000, 0b00000, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001],
        _ => [0; 7],
    }
}

/// Expands the glyph table into the font texture. Call once, before any drawing.
pub fn init() {
    unsafe {
        let tex = &raw mut FONT_TEX as *mut u32;
        for i in 0..GLYPH_COUNT {
            let ch = FIRST_CHAR + i as u8;
            let bits = glyph(ch);
            let cell_x = (i % 16) * CELL;
            let cell_y = (i / 16) * CELL;
            for (row, bitmap) in bits.iter().enumerate() {
                for col in 0..5 {
                    // Bit 4 is the leftmost pixel.
                    let on = (bitmap >> (4 - col)) & 1 == 1;
                    let px = cell_x + col;
                    let py = cell_y + row;
                    let value = if on { 0xffff_ffff } else { 0x0000_0000 };
                    *tex.add(py * TEX_W + px) = value;
                }
            }
        }
        // Nothing else writes here, but the GE reads it via uncached memory.
        sys::sceKernelDcacheWritebackAll();
    }
}

/// Binds the font texture. The 2D pass must already be set up.
pub fn bind() {
    unsafe {
        sys::sceGuEnable(GuState::Texture2D);
        sys::sceGuTexMode(TexturePixelFormat::Psm8888, 0, 0, 0);
        sys::sceGuTexImage(
            MipmapLevel::None,
            TEX_W as i32,
            TEX_H as i32,
            TEX_W as i32,
            &raw const FONT_TEX as *const c_void,
        );
        // Modulate so the per-vertex colour tints the glyph.
        sys::sceGuTexFunc(TextureEffect::Modulate, TextureColorComponent::Rgba);
        // Nearest filtering — this is a pixel font and must stay crisp.
        sys::sceGuTexFilter(TextureFilter::Nearest, TextureFilter::Nearest);
    }
}

/// Width in pixels of `text` at the given scale.
pub fn width(text: &[u8], scale: f32) -> f32 {
    text.len() as f32 * 6.0 * scale
}

/// Draws `text` with its top-left at `(x, y)`. Returns the x position just past the last glyph.
pub fn draw(text: &[u8], x: f32, y: f32, scale: f32, color: u32) -> f32 {
    unsafe {
        // Frame-lived storage: the GE reads this after we return (see `scratch`). Only what this
        // string actually needs is reserved — reserving the maximum every time filled the arena.
        let wanted = if text.len() > MAX_CHARS {
            MAX_CHARS
        } else {
            text.len()
        };
        if wanted == 0 {
            return x;
        }
        let verts = super::scratch::alloc::<SpriteVert>(wanted * 2);
        if verts.is_null() {
            return x;
        }
        let mut n = 0usize;
        let mut pen = x;

        for &raw_ch in text.iter() {
            if n + 2 > wanted * 2 {
                break;
            }
            // Lower case has no glyphs of its own apart from 'x'; fold the rest to upper.
            let ch = if raw_ch.is_ascii_lowercase() && raw_ch != b'x' {
                raw_ch.to_ascii_uppercase()
            } else {
                raw_ch
            };
            if ch == b' ' {
                pen += 6.0 * scale;
                continue;
            }
            if ch < FIRST_CHAR || ch >= FIRST_CHAR + GLYPH_COUNT as u8 {
                pen += 6.0 * scale;
                continue;
            }

            let i = (ch - FIRST_CHAR) as usize;
            let (cx, cy) = ((i % 16) * CELL, (i / 16) * CELL);

            *verts.add(n) = SpriteVert {
                u: cx as f32,
                v: cy as f32,
                color,
                x: pen,
                y,
                z: 0.0,
            };
            *verts.add(n + 1) = SpriteVert {
                u: (cx + CELL) as f32,
                v: (cy + CELL) as f32,
                color,
                x: pen + CELL as f32 * scale,
                y: y + CELL as f32 * scale,
                z: 0.0,
            };
            n += 2;
            pen += 6.0 * scale;
        }

        if n > 0 {
            sys::sceGumDrawArray(
                GuPrimitive::Sprites,
                VertexType::TEXTURE_32BITF
                    | VertexType::COLOR_8888
                    | VertexType::VERTEX_32BITF
                    | VertexType::TRANSFORM_2D,
                n as i32,
                core::ptr::null(),
                verts as *const c_void,
            );
        }
        pen
    }
}

/// Draws `text` with a one-pixel black drop shadow, as the HUD design asks for.
pub fn draw_shadowed(text: &[u8], x: f32, y: f32, scale: f32, color: u32) -> f32 {
    draw(text, x + scale, y + scale, scale, 0xff00_0000);
    draw(text, x, y, scale, color)
}

/// Centres `text` horizontally on `cx`.
pub fn draw_centered(text: &[u8], cx: f32, y: f32, scale: f32, color: u32) {
    draw_shadowed(text, cx - width(text, scale) * 0.5, y, scale, color);
}
