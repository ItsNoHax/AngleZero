//! Cut banks: where the road bends hard, the hillside on the inside is cut back and faced with a
//! sprayed-concrete lattice, with grass over the crest behind it.
//!
//! The spans come from `angle_zero::scenery::cut_banks`, which keeps every bank clear of the road's
//! other legs. The face is textured with UVs (a steep face streaks under a top-down projection);
//! the grass over the crest uses the hillside's own projected tile, so the two meet without a join.

use core::ffi::c_void;

use angle_zero::math::Vec3;
use angle_zero::mesh::{self, Chunk, Vertex};
use angle_zero::scenery::{self, BankSpan, BANK_BACK, BANK_FOOT};
use angle_zero::texgen::{self, Surface};
use angle_zero::track::{Track, CARPARK_SIDE};
use psp::sys::{self, GuPrimitive, VertexType};

use super::render::rgb;
use super::trees::{TexVertex, TEX_VERTEX_FORMAT};

const VERTEX_FORMAT: VertexType = VertexType::from_bits_truncate(
    VertexType::COLOR_8888.bits() | VertexType::VERTEX_32BITF.bits() | VertexType::TRANSFORM_3D.bits(),
);

const MAX_SPANS: usize = 64;
static mut SPANS: [BankSpan; MAX_SPANS] = [BankSpan::ZERO; MAX_SPANS];
static mut SPAN_COUNT: usize = 0;

const FACE_VERTS: usize = 40_000;
const TOP_VERTS: usize = 40_000;
static mut FACE: psp::Align16<[TexVertex; FACE_VERTS]> = psp::Align16([TexVertex::ZERO; FACE_VERTS]);
static mut TOP: psp::Align16<[Vertex; TOP_VERTS]> = psp::Align16([Vertex::ZERO; TOP_VERTS]);
static mut FACE_CHUNKS: [Chunk; mesh::CHUNK_COUNT] =
    [Chunk { start: 0, count: 0, center: Vec3::ZERO, radius: 0.0 }; mesh::CHUNK_COUNT];
static mut TOP_CHUNKS: [Chunk; mesh::CHUNK_COUNT] =
    [Chunk { start: 0, count: 0, center: Vec3::ZERO, radius: 0.0 }; mesh::CHUNK_COUNT];

/// Concrete under the lattice tile, lifted as the road is for its own.
const fn concrete(k: u32) -> u32 {
    let l = texgen::lift_percent(Surface::Lattice);
    rgb(0x34 * k / 100 * l / 100, 0x39 * k / 100 * l / 100, 0x3E * k / 100 * l / 100)
}
const FACE_FOOT: u32 = concrete(42);
const FACE_CREST: u32 = concrete(70);
const GUTTER: u32 = rgb(0x0C, 0x0D, 0x0F);

const fn grass(shade: u32) -> u32 {
    let k = shade * texgen::lift_percent(Surface::Grass);
    rgb(k * 34 / 10000, k * 48 / 10000, k * 28 / 10000)
}

pub fn spans() -> &'static [BankSpan] {
    unsafe {
        let all = &*(&raw const SPANS);
        &all[..SPAN_COUNT]
    }
}

/// Height of whichever bank stands at `node`, `lateral` metres out, if any.
pub fn surface(node: usize, lateral: f32) -> Option<f32> {
    spans()
        .iter()
        .filter_map(|s| scenery::bank_surface(s, node as u32, lateral))
        .fold(None, |a: Option<f32>, h| Some(a.map_or(h, |a| a.max(h))))
}

/// The last node beside the car park, set at `init`. No cut bank stands on its side before it.
static mut CARPARK_TO: usize = 0;

fn bay(node: usize, side: f32) -> bool {
    side * CARPARK_SIDE > 0.0 && node <= unsafe { CARPARK_TO } + 12
}

fn grow(lo: &mut Vec3, hi: &mut Vec3, x: f32, y: f32, z: f32) {
    *lo = Vec3::new(lo.x.min(x), lo.y.min(y), lo.z.min(z));
    *hi = Vec3::new(hi.x.max(x), hi.y.max(y), hi.z.max(z));
}

pub unsafe fn init(track: &Track) {
    CARPARK_TO = angle_zero::track::carpark_open_nodes(track).1;
    SPAN_COUNT = scenery::cut_banks(track, &mut *(&raw mut SPANS), bay);
    let face = &mut (*(&raw mut FACE)).0;
    let top = &mut (*(&raw mut TOP)).0;
    let (fc, tc) = (&mut *(&raw mut FACE_CHUNKS), &mut *(&raw mut TOP_CHUNKS));
    let nodes_per_chunk = mesh::CHUNK_NODES * mesh::RENDER_STRIDE;
    let tile = texgen::tile_metres(Surface::Lattice);
    let (mut fw, mut tw) = (0usize, 0usize);

    for c in 0..mesh::CHUNK_COUNT {
        let (f0, t0) = (fw, tw);
        let (mut lo, mut hi) = (Vec3::new(f32::MAX, f32::MAX, f32::MAX), Vec3::new(f32::MIN, f32::MIN, f32::MIN));
        let (first, last) = (c * nodes_per_chunk, (c + 1) * nodes_per_chunk);
        for span in spans() {
            let (from, to) = ((span.from as usize).max(first), (span.to as usize).min(last));
            if from >= to {
                continue;
            }
            // A section every two nodes; `a` and `b` are the two ends of one step.
            let mut i = from;
            while i < to && fw + 24 <= FACE_VERTS && tw + 30 <= TOP_VERTS {
                let j = (i + 2).min(to);
                let section = |k: usize| {
                    let n = &track.nodes[k];
                    let h = scenery::bank_height_at(span, k as u32);
                    let crest_l = scenery::bank_crest(h);
                    let mid_l = (crest_l + BANK_BACK) * 0.5;
                    let at = |l: f32, y: f32| {
                        Vec3::new(n.p.x + n.nrm.x * l * span.side, n.p.y + y, n.p.z + n.nrm.z * l * span.side)
                    };
                    let mid_y = scenery::bank_surface(span, k as u32, mid_l * span.side).unwrap_or(scenery::terrain_drop(mid_l));
                    (
                        at(BANK_FOOT - 0.35, -0.27),
                        at(BANK_FOOT, -0.3),
                        at(crest_l, h),
                        at(mid_l, mid_y),
                        at(BANK_BACK, scenery::terrain_drop(BANK_BACK)),
                        n.s,
                        // Slope length of the face, for the tile's V.
                        angle_zero::math::hypot(crest_l - BANK_FOOT, h + 0.3),
                    )
                };
                let (ga, fa, ca, ma, ba, sa, la) = section(i);
                let (gb, fb, cb, mb, bb, sb, lb) = section(j);
                // Face, lattice-textured: U along the road, V up the slope.
                let tv = |p: Vec3, u: f32, v: f32, color: u32| TexVertex { u: u / tile, v: v / tile, color, x: p.x, y: p.y, z: p.z };
                let q = [
                    tv(fa, sa, 0.0, FACE_FOOT),
                    tv(fb, sb, 0.0, FACE_FOOT),
                    tv(cb, sb, lb, FACE_CREST),
                    tv(ca, sa, la, FACE_CREST),
                ];
                face[fw..fw + 6].copy_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
                fw += 6;
                // Gutter strip at the foot, the crest-to-back grass in two bands.
                let v = |p: Vec3, color: u32| Vertex::new(p.x, p.y, p.z, color);
                for (a, b, cc, d, col_in, col_out) in [
                    (ga, gb, fb, fa, GUTTER, GUTTER),
                    (ca, cb, mb, ma, grass(80), grass(66)),
                    (ma, mb, bb, ba, grass(66), grass(64)),
                ] {
                    top[tw..tw + 6].copy_from_slice(&[v(a, col_in), v(b, col_in), v(cc, col_out), v(a, col_in), v(cc, col_out), v(d, col_out)]);
                    tw += 6;
                }
                for p in [ga, gb, ca, cb, ba, bb] {
                    grow(&mut lo, &mut hi, p.x, p.y, p.z);
                }
                i = j;
            }
        }
        let center = if fw > f0 { lo.add(hi).scale(0.5) } else { Vec3::ZERO };
        let radius = if fw > f0 { hi.sub(center).length() } else { 0.0 };
        fc[c] = Chunk { start: f0 as u32, count: (fw - f0) as u32, center, radius };
        tc[c] = Chunk { start: t0 as u32, count: (tw - t0) as u32, center, radius };
    }
}

/// Draws the visible banks. Culling off: which way a face winds depends on the bank's side.
pub unsafe fn draw(visible: impl Fn(&Chunk) -> bool) -> u32 {
    let mut drawn = 0u32;
    super::surfaces::bind_uv(Surface::Lattice);
    let face = &raw const FACE as *const TexVertex;
    for chunk in (*(&raw const FACE_CHUNKS)).iter() {
        if chunk.count == 0 || !visible(chunk) {
            continue;
        }
        drawn += chunk.count;
        sys::sceGumDrawArray(GuPrimitive::Triangles, TEX_VERTEX_FORMAT, chunk.count as i32, core::ptr::null(), face.add(chunk.start as usize) as *const c_void);
    }
    super::surfaces::bind(Surface::Grass);
    let top = &raw const TOP as *const Vertex;
    for chunk in (*(&raw const TOP_CHUNKS)).iter() {
        if chunk.count == 0 || !visible(chunk) {
            continue;
        }
        drawn += chunk.count;
        sys::sceGumDrawArray(GuPrimitive::Triangles, VERTEX_FORMAT, chunk.count as i32, core::ptr::null(), top.add(chunk.start as usize) as *const c_void);
    }
    super::surfaces::unbind();
    drawn
}
