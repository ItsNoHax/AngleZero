//! One texture per car, built by packing every image the materials sample into a grid.
//!
//! The runtime has six materials, not fifty-seven: parts are merged by category, so a draw call
//! covers the paint and the badges on it at once. That rules out one texture per material, and it
//! is why this packs instead — every source image gets a tile in a single image, shared by every
//! material that samples it, the vertices' UVs are rewritten into that tile at compile time, and
//! the console binds one texture for the whole car and never switches.
//!
//! Materials with no image get a tile too, filled with white. That is what makes the rest simple:
//! every vertex has a valid UV and the renderer needs no branch for the untextured case, while a
//! white tile multiplies to exactly the colour that vertex already had. Which is also why nothing
//! in the colour pipeline changed — glTF's base colour is `texture × factor`, the factor is
//! already baked into the vertex with the light term, so the tile only has to supply the texture,
//! and a material without one supplies 1.0.
//!
//! Every tile that holds an image is the same size — a packer that graded them by importance would
//! spend its complexity on a judgement it has no information to make. What the grid does not do is
//! give a tile to a material that has no image. Those need one texel, not a thousand, and counting
//! them into the grid is what kept every car at 32 px tiles: the Golf has twenty-one materials but
//! only fifteen images, and the E39 sixty-one and four. Sizing the grid by the images alone, and
//! parking every flat colour in one shared tile a texel each, takes the Golf to 64 px and the E39
//! to 85 — four and seven times the texels, on the same 256×256 atlas and for no extra memory.
//!
//! That mattered because a 32 px tile is what kept the car on `Nearest`. Filtering needs a gutter,
//! a gutter costs two texels of the tile in each axis, and two of thirty-two is 6% — see `GUTTER`
//! for what that was blamed for and what was actually going on.

use std::collections::HashMap;

use crate::model::SourceModel;

/// The atlas is always this square. 256×256 at two bytes a texel is 128 KB, which sits beside a
/// 155 KB mesh without changing what the arena has to hold.
pub const ATLAS: usize = 256;

/// A ring of each tile's own edge colour, one texel wide, so the hardware can filter.
///
/// Bilinear sampling near a tile's edge reaches a texel outside it, and in an atlas that texel
/// belongs to a different material — which is why the car was point-sampled for as long as the
/// tiles had no border. Point-sampling is what made the S15's tyre tread a handful of blocks: the
/// tread is a fine pattern crushed into a tile, and nearest picks one texel of it per pixel with
/// nothing in between. Filling the ring with a copy of the edge underneath it means the filter has
/// something correct to reach for, and the tile can be sampled the way the image wanted.
///
/// The ring costs two texels of the tile in each axis, which is 3% of a 64 px tile and was 6% of a
/// 32 px one, and that arithmetic was blamed for something it did not do. Twice the gutter turned
/// the Golf's front end olive-yellow and twice it was backed out as unaffordable. It is not the
/// percentage. The band that flips is not the grille and not `Light_Map`: it is the *interior*
/// mesh, whose `material` carries a matcap, and a matcap is a gradient with no stable answer to
/// sample — change the tile by one texel in any direction and those panels land on a different
/// strip of sky. A config can say `flat = true` and stop believing such an image, and once the
/// Golf's did, the gutter went in at the first attempt.
const GUTTER: usize = 1;

/// The smallest grid that holds this many tiles.
///
/// Not rounded to a power of two. Nothing needs it to be — the atlas itself is 256×256 and that is
/// the only size the hardware has an opinion about, while a tile is just a rectangle inside it. The
/// rounding was costing a factor of two in each axis at the worst of it: eighteen tiles is 5×5 at
/// 51 px, and was 8×8 at 32. The last row and column may be a pixel short of the edge when the grid
/// does not divide 256; those pixels are never sampled.
fn tiles_across(tiles: usize) -> usize {
    let mut across = 1usize;
    while across * across < tiles.max(1) {
        across += 1;
    }
    across
}

/// Where one material's pixels ended up, in texture coordinates.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tile {
    pub u0: f32,
    pub v0: f32,
    pub u1: f32,
    pub v1: f32,
}

impl Tile {
    /// Maps a source UV into this tile.
    ///
    /// Clamped, because a UV outside the unit square means the source repeated its texture, and
    /// in an atlas that would sample whatever material is packed next door. Clamping shows the
    /// edge of the pattern instead of somebody else's paint; `wrapped` reports how often it
    /// happened so the report can say so.
    pub fn map(&self, uv: [f32; 2]) -> [f32; 2] {
        [
            self.u0 + (self.u1 - self.u0) * uv[0].clamp(0.0, 1.0),
            self.v0 + (self.v1 - self.v0) * uv[1].clamp(0.0, 1.0),
        ]
    }
}

/// The whole-unit shift that brings a part's texture coordinates into the unit square.
///
/// Clamping in `map` assumes a coordinate outside the unit square means the source repeated its
/// texture. Usually it does. But an exporter is also free to write the same square addressed from
/// the other side — the 190E's every material arrives with V in [-1, 0], which is one flipped axis
/// and not a repeat at all — and clamping that lands the entire part on one row of texels. Its
/// alloys came out solid black off the top edge of a tile that has a correctly packed wheel in it,
/// which is what "190E rims showing weird" was.
///
/// So: if the island spans less than a unit on an axis it fits in the square and is moved there
/// whole, which cannot change how it samples. If it spans more, the source really is tiling,
/// nothing an atlas can do will show that, and it is left to the clamp. Moving the island whole is
/// also what keeps a triangle intact — wrapping each coordinate on its own would send any triangle
/// straddling the seam the long way across the tile.
///
/// The tolerance is not decoration. A UV island that runs the full width of its image lands at
/// [-0.0000006, 0.9999994] as often as at [0, 1], and `floor` puts that first coordinate in the
/// cell below — so a hair of float noise asked for a whole-unit shift, the island moved to
/// [1, 2], and `map` clamped every vertex of it onto the tile's last texel. That is what happened
/// to the AE86's tyres: `pneu` is one image of tread and lettering across the full square, and the
/// decal vanished into a single flat colour. An island grazing the boundary is already where it
/// belongs; only one a clear distance outside is worth moving.
pub fn unit_shift(uvs: &[[f32; 2]]) -> [f32; 2] {
    /// Nothing this close to the edge counts as outside it. A thousandth is the same resolution
    /// `weld` quantises texture coordinates to, and far finer than one texel of any tile.
    const EDGE: f32 = 1.0e-3;
    let mut shift = [0.0f32; 2];
    for axis in 0..2 {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for uv in uvs {
            lo = lo.min(uv[axis]);
            hi = hi.max(uv[axis]);
        }
        if lo.is_finite() && hi - lo < 1.0 {
            let by = -(lo + EDGE).floor();
            // Only if it actually lands the island inside. A shift that leaves either end out is a
            // different wrong answer, not a better one.
            if lo + by >= -EDGE && hi + by <= 1.0 + EDGE {
                shift[axis] = by;
            }
        }
    }
    shift
}

/// One arrangement of the materials' pictures in a 256×256 image.
pub struct Layout {
    /// RGBA8, `ATLAS` × `ATLAS`.
    pub pixels: Vec<u8>,
    /// One per source material, indexed the same way.
    pub tiles: Vec<Tile>,
    /// How much of the atlas one tile's content spans, as a fraction — 83 texels of 256 is 0.324.
    ///
    /// The simplifier needs it. UVs are rewritten into tile space before decimation, so a tile
    /// twice as wide makes the same source texture twice as far across in the numbers the
    /// simplifier sees, and its trade-off between moving a vertex and sliding a texture moves with
    /// it. Dividing by this puts the question back in the source's own UV space, where it belongs
    /// and where the packer cannot reach it. See `simplify::UV_WEIGHT`.
    pub span: f32,
    /// The tile every flat colour has a texel in, as its top-left pixel and its side, if there are
    /// any flat colours.
    shared: Option<(usize, usize, usize)>,
    /// Source images that were resized, as (name, from, to), once per slot.
    resized: Vec<(String, (u32, u32), (u32, u32))>,
    /// Set when the grid came out too fine to be worth packing, and the layout was left blank.
    skipped: bool,
    /// Tiles used and tiles across, for the report.
    pub grid: (usize, usize),
}

/// What a tile is given to: every material its own, or every image.
#[derive(Clone, Copy, PartialEq)]
enum SlotPer {
    Material,
    Image,
}

pub struct Atlas {
    /// What the car ships with and the console samples: one tile per source image, shared by every
    /// material that draws it. See `Atlas::build` for why there are two layouts.
    pub packed: Layout,
    /// What the geometry is built against: one tile per textured material, as the atlas was before
    /// images were shared. Every UV the weld, the simplifier and the coarse levels see is in this
    /// layout, and `remap` carries each one into `packed` as the file is written.
    pub working: Layout,
    /// How many materials brought a real image rather than a flat colour.
    pub textured: usize,
    /// Source images that were resized for the packed atlas, as (name, from, to), for the report.
    pub resized: Vec<(String, (u32, u32), (u32, u32))>,
    /// The full-resolution image of every material the config called a palette, kept rather than
    /// packed: the compiler samples these per vertex and bakes the result into the vertex colour.
    /// Indexed by source material.
    pub palettes: HashMap<usize, Decoded>,
    pub warnings: Vec<String>,
}

impl Atlas {
    /// Packs every material in the model into one image — twice.
    ///
    /// The atlas that ships gives a tile to each source *image*, not to each material, because a
    /// tile holds nothing but its image: scaled, and ringed with a copy of its own edge. The
    /// material's colour is not in it (glTF's factor is baked into the vertex with the light term),
    /// a flat or palette material never had a tile at all, and the gutter is the image's own edge,
    /// so two materials over one picture were given two identical tiles. The Xsara's five `Cabin_*`
    /// materials sample one label sheet and took five slots of eighteen, which made the grid 5×5
    /// and its 1024 px livery 49 texels a side. The E39's six textured materials are four images,
    /// the 190E's sixty are forty-three. Shared, an image costs one slot however many materials
    /// draw it, and every other image gets the room back.
    ///
    /// The geometry is not built against that layout, though, and that is deliberate. The weld
    /// keys vertices by their atlas UV, so two tiles are what kept two materials of the same colour
    /// from welding into each other along a seam; and the weld's rounding and the simplifier's
    /// quadrics are computed on the atlas coordinates themselves, so moving a tile moves which
    /// collapses win. Built against the shared layout, the nine cars that have materials in common
    /// kept a different set of triangles at every level, from 16 fewer on the E39's LOD0 to 123
    /// fewer on the 190E's LOD1 — a texture change reaching into bodywork that had been judged by
    /// eye and signed off. So the per-material layout is still built, as `working`, and everything
    /// up to the writer works in it exactly as before; `remap` moves each finished coordinate to
    /// the same point of the same picture in `packed`. The geometry is the same to the bit, and
    /// only the texture and the coordinates into it change.
    ///
    /// Images are decoded here and nowhere else — this is the only stage that needs the pixels,
    /// and on the E36 it is eighteen embedded PNGs against a report that otherwise costs 28 ms.
    pub fn build(model: &SourceModel, rules: &crate::config::MaterialRules) -> Atlas {
        let mut warnings = Vec::new();

        // Decoding comes before the layout, not during it, because the grid is sized by how many
        // materials actually arrive with a usable image and a texture that will not decode falls
        // back to a flat colour like any other. Decoded once each: several materials can share one
        // image, and the E36's headlight PNG is 715 KB.
        let mut decoded: HashMap<usize, Option<Decoded>> = HashMap::new();
        for material in &model.materials {
            match material.image {
                // A material the config calls flat has had its image disbelieved, so it is not
                // decoded either — and on the Golf that is the 715 KB the matcap would have cost.
                // A palette is decoded, but for the compiler rather than for a tile.
                Some(img) if !rules.is_flat(&material.name) => {
                    decoded
                        .entry(img)
                        .or_insert_with(|| decode(model, img, &mut warnings));
                }
                _ => {}
            }
        }
        // Asked again per material rather than read off `decoded`, because two materials can share
        // one image and only one of them be called flat — or a palette. That one samples white like
        // any flat colour, and the image stays in the grid for the other.
        let images: Vec<Option<(usize, &Decoded)>> = model
            .materials
            .iter()
            .map(|material| {
                if rules.is_flat(&material.name) || rules.is_palette(&material.name) {
                    return None;
                }
                let img = material.image?;
                decoded[&img].as_ref().map(|d| (img, d))
            })
            .collect();

        let working = lay_out(&images, SlotPer::Material);
        let packed = lay_out(&images, SlotPer::Image);
        if packed.skipped {
            warnings.push(format!(
                "{} source images do not fit an atlas of {ATLAS}px at a usable tile size; \
                 textures were skipped",
                images.iter().flatten().map(|(img, _)| *img).collect::<std::collections::HashSet<_>>().len()
            ));
        }

        let mut atlas = Atlas {
            textured: images.iter().filter(|m| m.is_some()).count(),
            resized: packed.resized.clone(),
            packed,
            working,
            palettes: HashMap::new(),
            warnings,
        };

        // The palettes go out whole, for the compiler to sample per vertex. Keyed by material and
        // cloned rather than shared, because several materials can name the same image — on the
        // Golf that is three interior materials and one 512×512 sheet.
        for (i, material) in model.materials.iter().enumerate() {
            if !rules.is_palette(&material.name) {
                continue;
            }
            match material.image.and_then(|img| decoded.get(&img)) {
                Some(Some(image)) => {
                    atlas.palettes.insert(i, image.clone());
                }
                // Named as a palette and has no image to be one. Worth saying: the material will
                // draw its base colour, which is not what the config asked for and not obviously
                // wrong on screen either.
                _ => atlas.warnings.push(format!(
                    "`{}` is named as a palette but has no image the converter could read",
                    material.name
                )),
            }
        }
        atlas
    }

    /// A coordinate in `working` moved to the same point of the same picture in `packed`.
    ///
    /// Every coordinate the pipeline produces lies in some material's working tile — `Tile::map`
    /// put it there, the weld keeps one of the coordinates it merges, collapse moves a vertex onto
    /// another — or else in the flat colours' white tile, where `white_texel` and the coarse levels'
    /// baked triangles point. The first is carried by its position within the tile, the second
    /// lands in the packed white tile, which is just as white. Anything else would be a bug
    /// upstream; it is clamped into the nearest tile rather than sent somewhere arbitrary.
    pub fn remap(&self, uv: [f32; 2]) -> [f32; 2] {
        const EPS: f32 = 1.0e-6;
        let (w, p) = (&self.working, &self.packed);
        let scale = |a0: f32, a1: f32, b0: f32, b1: f32, x: f32| {
            if a1 - a0 > EPS {
                (b0 + (b1 - b0) * ((x - a0) / (a1 - a0))).clamp(b0, b1)
            } else {
                b0
            }
        };
        let within = |t: &Tile| {
            uv[0] >= t.u0 - EPS && uv[0] <= t.u1 + EPS && uv[1] >= t.v0 - EPS && uv[1] <= t.v1 + EPS
        };
        // A material's own texel or tile first, so a flat colour keeps a texel of its own.
        if let Some(i) = w.tiles.iter().position(within) {
            let (a, b) = (w.tiles[i], p.tiles[i]);
            return [scale(a.u0, a.u1, b.u0, b.u1, uv[0]), scale(a.v0, a.v1, b.v0, b.v1, uv[1])];
        }
        if let (Some(a), Some(b)) = (w.shared, p.shared) {
            let px = [uv[0] * ATLAS as f32, uv[1] * ATLAS as f32];
            let (ax1, ay1) = ((a.0 + a.2) as f32, (a.1 + a.2) as f32);
            if px[0] >= a.0 as f32 && px[0] <= ax1 && px[1] >= a.1 as f32 && px[1] <= ay1 {
                let n = ATLAS as f32;
                let inner = |o: usize, s: usize| ((o + GUTTER) as f32 / n, (o + s - GUTTER) as f32 / n);
                let ((au0, au1), (av0, av1)) = (inner(a.0, a.2), inner(a.1, a.2));
                let ((bu0, bu1), (bv0, bv1)) = (inner(b.0, b.2), inner(b.1, b.2));
                return [
                    scale(au0, au1, bu0, bu1, uv[0].clamp(au0, au1)),
                    scale(av0, av1, bv0, bv1, uv[1].clamp(av0, av1)),
                ];
            }
        }
        let distance = |t: &Tile| {
            let du = (t.u0 - uv[0]).max(uv[0] - t.u1).max(0.0);
            let dv = (t.v0 - uv[1]).max(uv[1] - t.v1).max(0.0);
            du * du + dv * dv
        };
        match (0..w.tiles.len()).min_by(|&a, &b| distance(&w.tiles[a]).total_cmp(&distance(&w.tiles[b]))) {
            Some(i) => {
                let (a, b) = (w.tiles[i], p.tiles[i]);
                let (u, v) = (uv[0].clamp(a.u0, a.u1), uv[1].clamp(a.v0, a.v1));
                [scale(a.u0, a.u1, b.u0, b.u1, u), scale(a.v0, a.v1, b.v0, b.v1, v)]
            }
            None => uv,
        }
    }

    /// The packed atlas as 16-bit 5650, which is what the car is drawn with.
    ///
    /// No alpha: what blends on this car is decided per material by the renderer — glass and lamp
    /// glows have their alpha in the vertex colour — and 5650 spends every one of its sixteen bits
    /// on colour rather than five of them on a channel nothing reads.
    pub fn to_5650(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(ATLAS * ATLAS * 2);
        for px in self.packed.pixels.chunks_exact(4) {
            let (r, g, b) = (px[0] as u16, px[1] as u16, px[2] as u16);
            let v = (r >> 3) | ((g >> 2) << 5) | ((b >> 3) << 11);
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

/// Lays the materials' images out on a grid, a slot per material or a slot per image.
///
/// `images` is per material: the image it samples and its decoded pixels, or `None` for a
/// material that draws its vertex colour alone. Every tile that holds an image is the same size,
/// and all the flat colours share one more, a texel each.
fn lay_out(images: &[Option<(usize, &Decoded)>], per: SlotPer) -> Layout {
    let mut out = Layout {
        pixels: vec![0; ATLAS * ATLAS * 4],
        tiles: vec![Tile::default(); images.len()],
        // Replaced below once the grid is known; the value only matters to a car that has a grid
        // at all, and the early return below leaves every tile degenerate anyway.
        span: 1.0,
        shared: None,
        resized: Vec::new(),
        skipped: false,
        grid: (0, 0),
    };
    // What a slot is keyed by. In the per-material layout the material's own index, so no two
    // materials ever share; in the packed one the image's, in the order materials first ask for it.
    let key = |i: usize, img: usize| match per {
        SlotPer::Material => i,
        SlotPer::Image => img,
    };
    let mut slot_of: HashMap<usize, usize> = HashMap::new();
    for (i, m) in images.iter().enumerate() {
        if let Some((img, _)) = m {
            let next = slot_of.len();
            slot_of.entry(key(i, *img)).or_insert(next);
        }
    }
    let textured = slot_of.len();
    let flat = images.iter().filter(|m| m.is_none()).count();
    // One slot per image, and one more shared by every flat colour if there are any.
    let slots = textured + usize::from(flat > 0);
    let across = tiles_across(slots);
    let tile = (ATLAS / across).max(1);
    // What is left of a tile once the gutter has its ring, and the only part any UV addresses.
    let content = tile.saturating_sub(2 * GUTTER);
    out.grid = (slots, across);
    if tile < 4 {
        out.skipped = true;
        return out;
    }

    out.span = content as f32 / ATLAS as f32;

    // The flat colours' shared tile sits after the images, and is white throughout so that a
    // coordinate landing anywhere in it still multiplies to no change.
    let shared = slot_origin(textured, across, tile);
    if flat > 0 {
        fill_white(&mut out.pixels, shared.0, shared.1, tile);
        out.shared = Some((shared.0, shared.1, tile));
    }

    // Each slot is blitted once, by whichever material reaches it first, and every later material
    // keyed to it is handed the same rectangle.
    let mut placed: HashMap<usize, Tile> = HashMap::new();
    let mut next_flat = 0usize;
    for (i, m) in images.iter().enumerate() {
        out.tiles[i] = match m {
            Some((img, image)) => {
                let k = key(i, *img);
                if let Some(t) = placed.get(&k) {
                    *t
                } else {
                    let (x0, y0) = slot_origin(slot_of[&k], across, tile);
                    if image.width as usize != content || image.height as usize != content {
                        out.resized.push((
                            image.name.clone(),
                            (image.width, image.height),
                            (content as u32, content as u32),
                        ));
                    }
                    blit_scaled(&mut out.pixels, image, x0 + GUTTER, y0 + GUTTER, content);
                    replicate_edges(&mut out.pixels, x0, y0, tile);

                    // The content's outer edges, not the tile's: UV 0 and 1 are the outside of the
                    // first and last content texel, so the image is addressable end to end, and
                    // what a filter reaches for beyond them is the gutter, which is a copy of the
                    // texel it is already standing on. Nothing sees the neighbouring image at any
                    // filter setting.
                    let t = Tile {
                        u0: (x0 + GUTTER) as f32 / ATLAS as f32,
                        v0: (y0 + GUTTER) as f32 / ATLAS as f32,
                        u1: (x0 + GUTTER + content) as f32 / ATLAS as f32,
                        v1: (y0 + GUTTER + content) as f32 / ATLAS as f32,
                    };
                    placed.insert(k, t);
                    t
                }
            }
            None => {
                // A texel, and a degenerate tile that maps every coordinate onto its centre. That
                // is not a loss of anything: the tile is white, so what the material draws is the
                // colour already in the vertex, and every point of a full tile of white gave the
                // same answer as its centre does. Keeping them distinct rather than sharing one
                // texel is what lets the compiler's check that each material samples its own tile
                // still mean something.
                //
                // Wrapping if a car somehow brings more flat materials than the shared tile has
                // texels — 900 at the smallest tile this can produce — costs nothing either, for
                // the same reason: they are all the same white.
                //
                // Inside the gutter like everything else. The whole tile is white so the ring
                // would be harmless, but a texel on the ring is a texel a filter blends with the
                // tile next door, and there is no reason to be the one exception.
                let n = next_flat % (content * content);
                next_flat += 1;
                let centre = |at: usize| (at as f32 + 0.5) / ATLAS as f32;
                let (u, v) = (
                    centre(shared.0 + GUTTER + n % content),
                    centre(shared.1 + GUTTER + n / content),
                );
                Tile { u0: u, v0: v, u1: u, v1: v }
            }
        };
    }
    out
}

#[derive(Clone)]
pub struct Decoded {
    name: String,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
}

impl Decoded {
    /// One texel at the source's own resolution, as 0–1 linear-ish RGB.
    ///
    /// Point-sampled, and that is not a compromise here: this exists for a palette, and the answer
    /// to "which swatch does this vertex point at" is one swatch. Interpolating between two of
    /// them would invent a colour the model never named — which is exactly what filtering the
    /// atlas was doing.
    pub fn sample(&self, uv: [f32; 2]) -> [f32; 3] {
        let x = ((uv[0].clamp(0.0, 1.0) * self.width as f32) as usize).min(self.width as usize - 1);
        let y =
            ((uv[1].clamp(0.0, 1.0) * self.height as f32) as usize).min(self.height as usize - 1);
        let at = (y * self.width as usize + x) * 4;
        [
            self.pixels[at] as f32 / 255.0,
            self.pixels[at + 1] as f32 / 255.0,
            self.pixels[at + 2] as f32 / 255.0,
        ]
    }
}

fn decode(model: &SourceModel, index: usize, warnings: &mut Vec<String>) -> Option<Decoded> {
    let image = model.images.get(index)?;
    match image::load_from_memory(&image.data) {
        Ok(img) => {
            let rgba = img.to_rgba8();
            Some(Decoded {
                name: image.name.clone(),
                width: rgba.width(),
                height: rgba.height(),
                pixels: rgba.into_raw(),
            })
        }
        Err(e) => {
            // Not fatal. A material whose image will not decode falls back to its base colour,
            // which is what every untextured material uses anyway.
            warnings.push(format!(
                "texture `{}` ({}) could not be decoded and fell back to a flat colour: {e}",
                image.name, image.mime
            ));
            None
        }
    }
}

/// The top-left pixel of a grid slot.
fn slot_origin(slot: usize, across: usize, tile: usize) -> (usize, usize) {
    ((slot % across) * tile, (slot / across) * tile)
}

/// Every source pixel a texel covers, averaged — the box filter a mip chain would use, done once
/// at compile time because the console has no mip chain.
///
/// This was nearest-neighbour, on the argument that a 1024 source going into a 32 px tile is a
/// factor of 32 and averaging over that returns the mean colour of the image, whereas point
/// sampling "at least keeps a stripe a stripe". It does keep a stripe. What it does not do is keep
/// it in the same place, and that is what cost this three attempts at the gutter.
///
/// A texel here holds one source row out of the eight or sixteen it stands on, chosen by which row
/// `y * height / tile` happens to land on. Change the number of texels by even one — which is
/// exactly what taking a gutter out of the tile does — and every texel is a different source row.
/// The Golf's `Light_Map` puts the dark headlight photo directly above the orange tail-light
/// strip, and the grille samples a band about a texel tall across that boundary, so at 64 texels
/// it read dark and at 62 it read orange and the whole grille came out olive-yellow. That was read
/// as the gutter being unaffordable and it was the sampling being unstable: 62 is not a worse
/// answer than 64, it is a different roll of the same dice.
///
/// Averaging cannot flip like that, because the average over a boundary moves continuously as the
/// box does. It is also simply the right answer for minification, and the original objection was
/// about a factor of 32 that no car has any more — the tiles are 30 to 83 px now, so the drop is a
/// factor of 8 to 16 and a box that size still resolves the tread on a tyre.
fn blit_scaled(atlas: &mut [u8], image: &Decoded, x0: usize, y0: usize, tile: usize) {
    let (w, h) = (image.width as usize, image.height as usize);
    for y in 0..tile {
        // Half-open, and never empty: a source smaller than the tile magnifies, and then the box
        // is the single pixel the texel's own centre falls in.
        let sy0 = y * h / tile;
        let sy1 = (((y + 1) * h + tile - 1) / tile).max(sy0 + 1).min(h);
        for x in 0..tile {
            let sx0 = x * w / tile;
            let sx1 = (((x + 1) * w + tile - 1) / tile).max(sx0 + 1).min(w);
            let mut sum = [0u32; 4];
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let src = (sy * w + sx) * 4;
                    for k in 0..4 {
                        sum[k] += image.pixels[src + k] as u32;
                    }
                }
            }
            let n = ((sy1 - sy0) * (sx1 - sx0)) as u32;
            let dst = ((y0 + y) * ATLAS + x0 + x) * 4;
            for k in 0..4 {
                atlas[dst + k] = (sum[k] / n) as u8;
            }
        }
    }
}

/// Extends the content out over the gutter, a copy of whatever it is standing next to.
///
/// Rows before columns, so the four corners are written twice and end up holding the corner texel
/// of the content — which is what a filter sampling a corner of the tile has to find.
fn replicate_edges(atlas: &mut [u8], x0: usize, y0: usize, tile: usize) {
    let texel = |atlas: &[u8], x: usize, y: usize| {
        let at = (y * ATLAS + x) * 4;
        [atlas[at], atlas[at + 1], atlas[at + 2], atlas[at + 3]]
    };
    let (last, inner) = (tile - 1, tile - 1 - GUTTER);
    for x in GUTTER..=inner {
        let (top, bottom) = (
            texel(atlas, x0 + x, y0 + GUTTER),
            texel(atlas, x0 + x, y0 + inner),
        );
        put(atlas, x0 + x, y0, top);
        put(atlas, x0 + x, y0 + last, bottom);
    }
    for y in 0..tile {
        let (left, right) = (
            texel(atlas, x0 + GUTTER, y0 + y),
            texel(atlas, x0 + inner, y0 + y),
        );
        put(atlas, x0, y0 + y, left);
        put(atlas, x0 + last, y0 + y, right);
    }
}

fn put(atlas: &mut [u8], x: usize, y: usize, rgba: [u8; 4]) {
    let at = (y * ATLAS + x) * 4;
    atlas[at..at + 4].copy_from_slice(&rgba);
}

/// White, so the tile multiplies to whatever colour the vertex already carried.
fn fill_white(atlas: &mut [u8], x0: usize, y0: usize, tile: usize) {
    for y in 0..tile {
        for x in 0..tile {
            let dst = ((y0 + y) * ATLAS + x0 + x) * 4;
            atlas[dst..dst + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Material;
    use image::ImageEncoder;

    fn model_with(materials: Vec<Material>) -> SourceModel {
        SourceModel {
            source: "test".into(),
            credit: Default::default(),
            parts: Vec::new(),
            materials,
            images: Vec::new(),
        }
    }

    fn no_rules() -> crate::config::MaterialRules {
        crate::config::MaterialRules::default()
    }

    fn flat(name: &str, colour: [f32; 4]) -> Material {
        Material {
            name: name.into(),
            base_color: colour,
            metallic: 0.0,
            roughness: 1.0,
            emissive: [0.0; 3],
            image: None,
            double_sided: false,
            transparent: false,
        }
    }

    /// A material and the 1×1 PNG it brings, so a test has an image the packer can actually decode.
    /// Flat materials share one tile between them, so nothing below can tell the layout apart
    /// without one.
    fn textured(name: &str, at: usize, rgb: [u8; 3]) -> (Material, crate::model::Image) {
        let mut data = Vec::new();
        image::codecs::png::PngEncoder::new(&mut data)
            .write_image(&[rgb[0], rgb[1], rgb[2], 255], 1, 1, image::ExtendedColorType::Rgba8)
            .expect("encode a 1x1 png");
        (
            Material {
                image: Some(at),
                ..flat(name, [1.0; 4])
            },
            crate::model::Image {
                name: name.into(),
                mime: "image/png".into(),
                data,
            },
        )
    }

    /// The atlas sampled the way `bind_car_texture` asks the GE to, and the way `azview` matches.
    fn bilinear(atlas: &Atlas, u: f32, v: f32) -> [f32; 3] {
        let at = |x: usize, y: usize| {
            let i = (y * ATLAS + x) * 4;
            [
                atlas.packed.pixels[i] as f32,
                atlas.packed.pixels[i + 1] as f32,
                atlas.packed.pixels[i + 2] as f32,
            ]
        };
        let (x, y) = (u * ATLAS as f32 - 0.5, v * ATLAS as f32 - 0.5);
        let (fx, fy) = (x.floor(), y.floor());
        let (sx, sy) = (x - fx, y - fy);
        let x0 = (fx.max(0.0) as usize).min(ATLAS - 1);
        let y0 = (fy.max(0.0) as usize).min(ATLAS - 1);
        let (x1, y1) = ((x0 + 1).min(ATLAS - 1), (y0 + 1).min(ATLAS - 1));
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        let mut out = [0.0f32; 3];
        for k in 0..3 {
            let top = a[k] + (b[k] - a[k]) * sx;
            let bottom = c[k] + (d[k] - c[k]) * sx;
            out[k] = top + (bottom - top) * sy;
        }
        out
    }

    #[test]
    fn every_material_gets_its_own_tile_and_they_do_not_overlap() {
        let atlas = Atlas::build(&model_with(vec![
            flat("a", [1.0, 0.0, 0.0, 1.0]),
            flat("b", [0.0, 1.0, 0.0, 1.0]),
            flat("c", [0.0, 0.0, 1.0, 1.0]),
        ]), &no_rules());
        assert_eq!(atlas.packed.tiles.len(), 3);
        for (i, a) in atlas.packed.tiles.iter().enumerate() {
            // Not `<`: a flat colour's tile is one texel, so its two corners are the same point.
            assert!(a.u0 <= a.u1 && a.v0 <= a.v1);
            for b in &atlas.packed.tiles[i + 1..] {
                let apart = a.u1 < b.u0 || b.u1 < a.u0 || a.v1 < b.v0 || b.v1 < a.v0;
                assert!(apart, "tiles overlap: {a:?} and {b:?}");
            }
        }
    }

    /// The whole point of the layout: a material with no image costs a texel, not a tile, so the
    /// grid is sized by the images and everything textured gets more of the atlas. Twelve flat
    /// colours beside three images used to force a 4×4 grid at 64 px; now three images and their
    /// shared neighbour make 2×2 at 128.
    #[test]
    fn flat_colours_do_not_shrink_the_tiles_the_images_get() {
        let (material, image) = textured("paint", 0, [255, 0, 0]);
        let mut materials = vec![material; 3];
        materials.extend((0..12).map(|i| flat(&format!("plastic{i}"), [0.2, 0.2, 0.2, 1.0])));
        let mut model = model_with(materials);
        model.images.push(image);

        let atlas = Atlas::build(&model, &no_rules());
        assert_eq!(atlas.textured, 3);
        let side = (atlas.packed.tiles[0].u1 - atlas.packed.tiles[0].u0) * ATLAS as f32;
        assert!(
            (side - 126.0).abs() < 0.01,
            "a 2x2 grid is a 128px tile, less the gutter at each edge; got {side}"
        );
        for t in &atlas.packed.tiles[3..] {
            assert_eq!(t.u0, t.u1, "a flat colour is one texel wide");
            assert_eq!(t.v0, t.v1, "a flat colour is one texel tall");
        }
    }

    /// Every flat colour lands somewhere white, wherever in the shared tile it was put. Sampling
    /// one of the images by mistake would tint a part that the model said had no texture at all.
    #[test]
    fn a_flat_colour_still_samples_white_when_it_shares_a_tile_with_others() {
        let (material, image) = textured("paint", 0, [255, 0, 0]);
        let mut materials = vec![material];
        materials.extend((0..40).map(|i| flat(&format!("trim{i}"), [0.5, 0.5, 0.5, 1.0])));
        let mut model = model_with(materials);
        model.images.push(image);

        let atlas = Atlas::build(&model, &no_rules());
        for (i, t) in atlas.packed.tiles.iter().enumerate().skip(1) {
            let m = t.map([0.4, 0.7]);
            assert_eq!(bilinear(&atlas, m[0], m[1]), [255.0; 3], "material {i}");
        }
    }

    /// The gutter, and what it is for. Two materials with nothing in common but a shared edge in
    /// the atlas: filtered, neither may pick up a trace of the other anywhere in its own tile.
    /// Without the ring this fails at every edge, which is why the car was point-sampled until
    /// there was one.
    #[test]
    fn no_corner_of_a_tile_can_be_filtered_into_its_neighbours() {
        let (red, red_png) = textured("red", 0, [255, 0, 0]);
        let (blue, blue_png) = textured("blue", 1, [0, 0, 255]);
        let mut model = model_with(vec![red, blue]);
        model.images.push(red_png);
        model.images.push(blue_png);

        let atlas = Atlas::build(&model, &no_rules());
        let (want, other) = ([255.0, 0.0, 0.0], [0.0, 0.0, 255.0]);
        let t = atlas.packed.tiles[0];
        for uv in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [0.5, 1.0]] {
            let m = t.map(uv);
            let got = bilinear(&atlas, m[0], m[1]);
            assert_eq!(got, want, "{uv:?} sampled {got:?}, not its own red");
            assert_ne!(got, other);
        }
    }

    /// The ring is a copy of the edge under it, not black and not the mean of anything: the point
    /// is that a filter reaching past the content finds the colour it is already standing on.
    #[test]
    fn the_gutter_repeats_the_edge_it_borders() {
        let (material, png) = textured("paint", 0, [255, 0, 0]);
        let mut model = model_with(vec![material]);
        model.images.push(png);
        let atlas = Atlas::build(&model, &no_rules());

        // One material is a 1×1 grid, so the tile is the whole atlas and its ring is the border.
        let texel = |x: usize, y: usize| {
            let i = (y * ATLAS + x) * 4;
            &atlas.packed.pixels[i..i + 3]
        };
        for i in 0..ATLAS {
            assert_eq!(texel(i, 0), texel(i.clamp(1, ATLAS - 2), 1), "top at {i}");
            assert_eq!(
                texel(i, ATLAS - 1),
                texel(i.clamp(1, ATLAS - 2), ATLAS - 2),
                "bottom at {i}"
            );
            assert_eq!(texel(0, i), texel(1, i), "left at {i}");
            assert_eq!(texel(ATLAS - 1, i), texel(ATLAS - 2, i), "right at {i}");
        }
    }

    /// A material the config calls flat is one whose image the model was wrong about — the Golf's
    /// matcap. It takes a texel in the shared tile like any other flat colour, and, which is the
    /// point, it stops taking a slot away from the images that are real pictures.
    #[test]
    fn a_material_the_config_calls_flat_gives_up_its_tile() {
        let (paint, paint_png) = textured("paint", 0, [255, 0, 0]);
        let (matcap, matcap_png) = textured("matcap", 1, [0, 0, 255]);
        let mut model = model_with(vec![paint, matcap]);
        model.images.push(paint_png);
        model.images.push(matcap_png);

        let rules: crate::config::MaterialRules =
            toml::from_str("colour = [{ match = [\"matcap\"], rgb = [10, 10, 12], flat = true }]")
                .expect("the rule parses");
        let atlas = Atlas::build(&model, &rules);

        assert_eq!(atlas.textured, 1, "only the real picture is a texture now");
        let t = atlas.packed.tiles[1];
        assert_eq!((t.u0, t.v0), (t.u1, t.v1), "a flat material gets one texel");
        assert_eq!(bilinear(&atlas, t.u0, t.v0), [255.0; 3], "and it is white");
    }

    /// The point of packing by image: materials over one picture share its tile, and the grid is
    /// sized by the pictures. Five materials over one sheet beside two other images were eight
    /// slots and a 3×3 grid; they are four now, a 2×2 at 128.
    #[test]
    fn materials_over_one_image_share_its_tile() {
        let (sheet, sheet_png) = textured("sheet", 0, [255, 0, 0]);
        let (a, a_png) = textured("a", 1, [0, 255, 0]);
        let (b, b_png) = textured("b", 2, [0, 0, 255]);
        let mut materials: Vec<Material> = (0..5)
            .map(|i| Material { name: format!("cabin{i}"), ..sheet.clone() })
            .collect();
        materials.extend([a, b, flat("trim", [0.2; 4])]);
        let mut model = model_with(materials);
        model.images.extend([sheet_png, a_png, b_png]);

        let atlas = Atlas::build(&model, &no_rules());
        assert_eq!(atlas.textured, 7, "still seven materials drawing a picture");
        assert_eq!(atlas.packed.grid, (4, 2));
        for t in &atlas.packed.tiles[1..5] {
            assert_eq!((t.u0, t.v0, t.u1, t.v1), {
                let s = atlas.packed.tiles[0];
                (s.u0, s.v0, s.u1, s.v1)
            });
        }
        let side = (atlas.packed.tiles[0].u1 - atlas.packed.tiles[0].u0) * ATLAS as f32;
        assert!((side - 126.0).abs() < 0.01, "a 2x2 grid less its gutter; got {side}");
        // And each of them samples the sheet, not a neighbour.
        for (i, want) in [(0, [255.0, 0.0, 0.0]), (4, [255.0, 0.0, 0.0]), (5, [0.0, 255.0, 0.0])] {
            let m = atlas.packed.tiles[i].map([0.5, 0.5]);
            assert_eq!(bilinear(&atlas, m[0], m[1]), want, "material {i}");
        }
        // The layout the geometry is built in still gives each its own.
        assert_eq!(atlas.working.grid, (8, 3));
    }

    /// Two materials over one image where the config calls one of them flat: the image stays in
    /// the grid for the other, and the flat one draws white.
    #[test]
    fn a_shared_image_stays_for_the_material_that_still_wants_it() {
        let (paint, png) = textured("paint", 0, [255, 0, 0]);
        let dull = Material { name: "dull".into(), ..paint.clone() };
        let mut model = model_with(vec![paint, dull]);
        model.images.push(png);
        let rules: crate::config::MaterialRules =
            toml::from_str("colour = [{ match = [\"dull\"], rgb = [10, 10, 12], flat = true }]")
                .expect("the rule parses");
        let atlas = Atlas::build(&model, &rules);
        assert_eq!(atlas.textured, 1);
        let m = atlas.packed.tiles[0].map([0.5, 0.5]);
        assert_eq!(bilinear(&atlas, m[0], m[1]), [255.0, 0.0, 0.0]);
        let t = atlas.packed.tiles[1];
        assert_eq!((t.u0, t.v0), (t.u1, t.v1));
        assert_eq!(bilinear(&atlas, t.u0, t.v0), [255.0; 3]);
    }

    /// `remap` carries a coordinate from the per-material layout to the same point of the same
    /// picture in the packed one, and a flat colour's texel to its own packed texel.
    #[test]
    fn remap_lands_on_the_same_point_of_the_same_picture() {
        let (sheet, sheet_png) = textured("sheet", 0, [255, 0, 0]);
        let (other, other_png) = textured("other", 1, [0, 0, 255]);
        let twin = Material { name: "twin".into(), ..sheet.clone() };
        let mut model = model_with(vec![sheet, twin, other, flat("trim", [0.5; 4])]);
        model.images.extend([sheet_png, other_png]);
        let atlas = Atlas::build(&model, &no_rules());

        for i in 0..4 {
            for uv in [[0.0, 0.0], [0.25, 0.75], [1.0, 1.0]] {
                let from = atlas.working.tiles[i].map(uv);
                let got = atlas.remap(from);
                let want = atlas.packed.tiles[i].map(uv);
                assert!(
                    (got[0] - want[0]).abs() < 1e-5 && (got[1] - want[1]).abs() < 1e-5,
                    "material {i} at {uv:?}: {got:?}, wanted {want:?}"
                );
            }
        }
    }

    /// A palette is kept whole and out of the grid, because the whole point is that it is sampled
    /// at the source's resolution instead of being crushed into texels. Two swatches side by side
    /// in a two-pixel image is the smallest thing that can tell the difference: packed into a tile
    /// they would still be two texels, but the Golf's real sheet is forty-eight swatches across
    /// two thirds of an image, which is one texel each at any tile this packer makes.
    #[test]
    fn a_palette_is_kept_whole_and_takes_no_tile() {
        let mut data = Vec::new();
        image::codecs::png::PngEncoder::new(&mut data)
            .write_image(
                &[255, 0, 0, 255, 0, 0, 255, 255],
                2,
                1,
                image::ExtendedColorType::Rgba8,
            )
            .expect("encode a 2x1 png");
        let swatches = Material {
            image: Some(0),
            ..flat("swatches", [1.0; 4])
        };
        let mut model = model_with(vec![swatches]);
        model.images.push(crate::model::Image {
            name: "swatches".into(),
            mime: "image/png".into(),
            data,
        });

        let rules: crate::config::MaterialRules =
            toml::from_str("palette = [\"swatches\"]").expect("the rule parses");
        let atlas = Atlas::build(&model, &rules);

        assert_eq!(atlas.textured, 0, "a palette is not a texture in the atlas");
        let t = atlas.packed.tiles[0];
        assert_eq!((t.u0, t.v0), (t.u1, t.v1), "and it takes a texel, not a tile");
        assert_eq!(bilinear(&atlas, t.u0, t.v0), [255.0; 3], "which is white");

        let kept = atlas.palettes.get(&0).expect("the palette was kept");
        assert_eq!(kept.sample([0.1, 0.5]), [1.0, 0.0, 0.0]);
        assert_eq!(kept.sample([0.9, 0.5]), [0.0, 0.0, 1.0]);
        assert!(atlas.warnings.is_empty(), "{:?}", atlas.warnings);
    }

    /// Naming a material that brings no image is a config that will do nothing and look almost
    /// right, which is the kind of mistake worth a line of output.
    #[test]
    fn a_palette_with_no_image_says_so() {
        let rules: crate::config::MaterialRules =
            toml::from_str("palette = [\"paint\"]").expect("the rule parses");
        let atlas = Atlas::build(&model_with(vec![flat("paint", [1.0; 4])]), &rules);
        assert_eq!(atlas.warnings.len(), 1, "{:?}", atlas.warnings);
        assert!(atlas.warnings[0].contains("paint"));
    }

    #[test]
    fn the_grid_is_the_smallest_square_that_holds_its_tiles() {
        assert_eq!(tiles_across(0), 1);
        assert_eq!(tiles_across(1), 1);
        assert_eq!(tiles_across(2), 2);
        assert_eq!(tiles_across(4), 2);
        // The rounding this replaced took eighteen tiles to 8×8 and a 32px tile.
        assert_eq!(tiles_across(18), 5);
        assert_eq!(tiles_across(64), 8);
    }

    /// A material with no image has to multiply to no change at all, or every untextured part of
    /// the car — which on the E36 is the paint, the glass and the tyres — would shift colour the
    /// day textures were turned on.
    #[test]
    fn a_material_with_no_image_gets_a_tile_that_changes_nothing() {
        let atlas = Atlas::build(&model_with(vec![flat("red", [1.0, 0.0, 0.0, 1.0])]), &no_rules());
        let t = atlas.packed.tiles[0];
        let x = ((t.u0 + t.u1) * 0.5 * ATLAS as f32) as usize;
        let y = ((t.v0 + t.v1) * 0.5 * ATLAS as f32) as usize;
        let at = (y * ATLAS + x) * 4;
        assert_eq!(&atlas.packed.pixels[at..at + 3], &[255, 255, 255]);
        assert_eq!(atlas.textured, 0);
    }

    /// A UV that ran off the unit square would land in the next material's tile.
    #[test]
    fn uvs_are_clamped_into_their_own_tile() {
        let atlas = Atlas::build(&model_with(vec![flat("a", [1.0; 4]), flat("b", [0.0; 4])]), &no_rules());
        let t = atlas.packed.tiles[0];
        for uv in [[0.0, 0.0], [1.0, 1.0], [4.0, -2.0], [-0.5, 9.0]] {
            let m = t.map(uv);
            assert!(m[0] >= t.u0 && m[0] <= t.u1, "{uv:?} mapped to {m:?}");
            assert!(m[1] >= t.v0 && m[1] <= t.v1, "{uv:?} mapped to {m:?}");
        }
    }

    /// The 190E's case: a whole model exported with V in [-1, 0], which is the unit square
    /// addressed from the other side and not a repeat. Clamping put every alloy on one row of
    /// texels and drew them black.
    #[test]
    fn an_island_outside_the_unit_square_is_moved_into_it() {
        let uvs = [[0.011, -0.997], [0.982, -0.024], [0.5, -0.5]];
        let shift = unit_shift(&uvs);
        assert_eq!(shift, [0.0, 1.0]);
        for uv in uvs {
            let v = uv[1] + shift[1];
            assert!((0.0..=1.0).contains(&v), "{uv:?} shifted to {v}");
        }
    }

    /// An island that fills its image exactly, with float noise either side. `floor` alone read the
    /// low end as belonging to the cell below and shifted the whole thing off the tile — the AE86's
    /// tyre decal.
    #[test]
    fn an_island_grazing_the_boundary_is_left_alone() {
        assert_eq!(unit_shift(&[[-6.0e-7, 0.5], [0.9999994, 0.5]]), [0.0, 0.0]);
        assert_eq!(unit_shift(&[[0.0, 0.01], [1.0, 0.99]]), [0.0, 0.0]);
    }

    /// A source that genuinely tiles cannot be shown in an atlas at all, so it is left where it is
    /// for the clamp to deal with — moving it would only pick a different wrong answer.
    #[test]
    fn a_tiling_island_is_left_alone() {
        assert_eq!(unit_shift(&[[0.0, 0.0], [4.0, 3.0]]), [0.0, 0.0]);
        assert_eq!(unit_shift(&[]), [0.0, 0.0]);
        // Already inside, and stays: `floor` of a coordinate in [0, 1) is zero.
        assert_eq!(unit_shift(&[[0.2, 0.3], [0.9, 0.8]]), [0.0, 0.0]);
    }

    #[test]
    fn the_atlas_converts_to_the_hardware_format_at_two_bytes_a_texel() {
        let atlas = Atlas::build(&model_with(vec![flat("white", [1.0; 4])]), &no_rules());
        let packed = atlas.to_5650();
        assert_eq!(packed.len(), ATLAS * ATLAS * 2);
        let t = atlas.packed.tiles[0];
        let x = ((t.u0 + t.u1) * 0.5 * ATLAS as f32) as usize;
        let y = ((t.v0 + t.v1) * 0.5 * ATLAS as f32) as usize;
        let at = (y * ATLAS + x) * 2;
        let v = u16::from_le_bytes([packed[at], packed[at + 1]]);
        assert_eq!(v & 0x1F, 0x1F, "red full in the low five bits");
        assert_eq!((v >> 5) & 0x3F, 0x3F, "green full in the middle six");
        assert_eq!(v >> 11, 0x1F, "blue full in the top five");
    }
}
