//! One texture per car, built by packing every material into a grid.
//!
//! The runtime has six materials, not fifty-seven: parts are merged by category, so a draw call
//! covers the paint and the badges on it at once. That rules out one texture per material, and it
//! is why this packs instead — every source material gets a tile in a single image, the vertices'
//! UVs are rewritten into that tile at compile time, and the console binds one texture for the
//! whole car and never switches.
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
//! That mattered because both open texture faults were the same starvation. The S15's tyre tread
//! and the Golf's grille plastics are each about a texel tall in a 32 px tile, which is why the
//! tread came out blocky and why a one-texel gutter round each tile — which fixes the tread outright
//! — shifted the grille's UV band from the dark part of its image into the orange part and turned it
//! olive-yellow. The gutter is not wrong; at 32 px there was no room for it.

use std::collections::HashMap;

use crate::model::SourceModel;

/// The atlas is always this square. 256×256 at two bytes a texel is 128 KB, which sits beside a
/// 155 KB mesh without changing what the arena has to hold.
pub const ATLAS: usize = 256;



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

pub struct Atlas {
    /// RGBA8, `ATLAS` × `ATLAS`. Converted to a PSP pixel format by the writer.
    pub pixels: Vec<u8>,
    /// One per source material, indexed the same way.
    pub tiles: Vec<Tile>,
    /// How many materials brought a real image rather than a flat colour.
    pub textured: usize,
    /// Source images that were resized, as (name, from, to), for the report.
    pub resized: Vec<(String, (u32, u32), (u32, u32))>,
    pub warnings: Vec<String>,
}

impl Atlas {
    /// Packs every material in the model into one image.
    ///
    /// Images are decoded here and nowhere else — this is the only stage that needs the pixels,
    /// and on the E36 it is eighteen embedded PNGs against a report that otherwise costs 28 ms.
    pub fn build(model: &SourceModel) -> Atlas {
        let mut atlas = Atlas {
            pixels: vec![0; ATLAS * ATLAS * 4],
            tiles: vec![Tile::default(); model.materials.len()],
            textured: 0,
            resized: Vec::new(),
            warnings: Vec::new(),
        };

        // Decoding comes before the layout, not during it, because the grid is sized by how many
        // materials actually arrive with a usable image and a texture that will not decode falls
        // back to a flat colour like any other. Decoded once each: several materials can share one
        // image, and the E36's headlight PNG is 715 KB.
        let mut decoded: HashMap<usize, Option<Decoded>> = HashMap::new();
        for material in &model.materials {
            if let Some(img) = material.image {
                decoded
                    .entry(img)
                    .or_insert_with(|| decode(model, img, &mut atlas.warnings));
            }
        }
        let image_for = |material: &crate::model::Material| -> Option<&Decoded> {
            material.image.and_then(|img| decoded[&img].as_ref())
        };

        let textured = model.materials.iter().filter(|m| image_for(m).is_some()).count();
        let flat = model.materials.len() - textured;
        // One slot per image, and one more shared by every flat colour if there are any.
        let slots = textured + usize::from(flat > 0);
        let across = tiles_across(slots);
        let tile = (ATLAS / across).max(1);
        if tile < 4 {
            atlas.warnings.push(format!(
                "{textured} textured materials do not fit an atlas of {ATLAS}px at a usable tile \
                 size; textures were skipped"
            ));
            return atlas;
        }

        // The flat colours' shared tile sits after the images, and is white throughout so that a
        // coordinate landing anywhere in it still multiplies to no change.
        let shared = slot_origin(textured, across, tile);
        if flat > 0 {
            fill_white(&mut atlas.pixels, shared.0, shared.1, tile);
        }

        let mut next_image = 0usize;
        let mut next_flat = 0usize;
        for (i, material) in model.materials.iter().enumerate() {
            atlas.tiles[i] = match image_for(material) {
                Some(image) => {
                    let (x0, y0) = slot_origin(next_image, across, tile);
                    next_image += 1;
                    atlas.textured += 1;
                    if image.width as usize != tile || image.height as usize != tile {
                        atlas.resized.push((
                            image.name.clone(),
                            (image.width, image.height),
                            (tile as u32, tile as u32),
                        ));
                    }
                    blit_scaled(&mut atlas.pixels, image, x0, y0, tile);

                    // Half a texel in from each edge. The GE samples tile-nearest for the car, but
                    // a UV landing exactly on the boundary still rounds outward on hardware, and
                    // the neighbour is a different material rather than more of the same.
                    let half = 0.5 / ATLAS as f32;
                    Tile {
                        u0: x0 as f32 / ATLAS as f32 + half,
                        v0: y0 as f32 / ATLAS as f32 + half,
                        u1: (x0 + tile) as f32 / ATLAS as f32 - half,
                        v1: (y0 + tile) as f32 / ATLAS as f32 - half,
                    }
                }
                None => {
                    // A texel, and a degenerate tile that maps every coordinate onto its centre.
                    // That is not a loss of anything: the tile is white, so what the material draws
                    // is the colour already in the vertex, and every point of a full tile of white
                    // gave the same answer as its centre does. Keeping them distinct rather than
                    // sharing one texel is what lets the compiler's check that each material
                    // samples its own tile still mean something.
                    //
                    // Wrapping if a car somehow brings more flat materials than the shared tile has
                    // texels — 1,024 at the smallest tile this can produce — costs nothing either,
                    // for the same reason: they are all the same white.
                    let n = next_flat % (tile * tile);
                    next_flat += 1;
                    let centre = |at: usize| (at as f32 + 0.5) / ATLAS as f32;
                    let (u, v) = (
                        centre(shared.0 + n % tile),
                        centre(shared.1 + n / tile),
                    );
                    Tile { u0: u, v0: v, u1: u, v1: v }
                }
            };
        }
        atlas
    }

    /// The atlas as 16-bit 5650, which is what the car is drawn with.
    ///
    /// No alpha: what blends on this car is decided per material by the renderer — glass and lamp
    /// glows have their alpha in the vertex colour — and 5650 spends every one of its sixteen bits
    /// on colour rather than five of them on a channel nothing reads.
    pub fn to_5650(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(ATLAS * ATLAS * 2);
        for px in self.pixels.chunks_exact(4) {
            let (r, g, b) = (px[0] as u16, px[1] as u16, px[2] as u16);
            let v = (r >> 3) | ((g >> 2) << 5) | ((b >> 3) << 11);
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }
}

struct Decoded {
    name: String,
    width: u32,
    height: u32,
    pixels: Vec<u8>,
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

/// Nearest-neighbour, which is the right filter for the size of drop involved: a 1024×1024 source
/// going into a 32×32 tile is a factor of 32, and averaging over that reduces most car textures to
/// their mean colour. Point-sampling at least keeps a stripe a stripe.
fn blit_scaled(atlas: &mut [u8], image: &Decoded, x0: usize, y0: usize, tile: usize) {
    for y in 0..tile {
        let sy = (y * image.height as usize / tile).min(image.height as usize - 1);
        for x in 0..tile {
            let sx = (x * image.width as usize / tile).min(image.width as usize - 1);
            let src = (sy * image.width as usize + sx) * 4;
            let dst = ((y0 + y) * ATLAS + x0 + x) * 4;
            atlas[dst..dst + 4].copy_from_slice(&image.pixels[src..src + 4]);
        }
    }
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

    /// A 1×1 red PNG, so a material can bring an image a test can actually decode. Flat materials
    /// share one tile between them, so nothing below can tell the layout apart without one.
    fn textured(name: &str) -> (Material, crate::model::Image) {
        let mut data = Vec::new();
        image::codecs::png::PngEncoder::new(&mut data)
            .write_image(&[255, 0, 0, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .expect("encode a 1x1 png");
        (
            Material {
                image: Some(0),
                ..flat(name, [1.0; 4])
            },
            crate::model::Image {
                name: name.into(),
                mime: "image/png".into(),
                data,
            },
        )
    }

    #[test]
    fn every_material_gets_its_own_tile_and_they_do_not_overlap() {
        let atlas = Atlas::build(&model_with(vec![
            flat("a", [1.0, 0.0, 0.0, 1.0]),
            flat("b", [0.0, 1.0, 0.0, 1.0]),
            flat("c", [0.0, 0.0, 1.0, 1.0]),
        ]));
        assert_eq!(atlas.tiles.len(), 3);
        for (i, a) in atlas.tiles.iter().enumerate() {
            // Not `<`: a flat colour's tile is one texel, so its two corners are the same point.
            assert!(a.u0 <= a.u1 && a.v0 <= a.v1);
            for b in &atlas.tiles[i + 1..] {
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
        let (material, image) = textured("paint");
        let mut materials = vec![material; 3];
        materials.extend((0..12).map(|i| flat(&format!("plastic{i}"), [0.2, 0.2, 0.2, 1.0])));
        let mut model = model_with(materials);
        model.images.push(image);

        let atlas = Atlas::build(&model);
        assert_eq!(atlas.textured, 3);
        let side = (atlas.tiles[0].u1 - atlas.tiles[0].u0) * ATLAS as f32;
        assert!(
            (side - 127.0).abs() < 0.01,
            "a 2x2 grid is a 128px tile, less the half texel at each edge; got {side}"
        );
        for t in &atlas.tiles[3..] {
            assert_eq!(t.u0, t.u1, "a flat colour is one texel wide");
            assert_eq!(t.v0, t.v1, "a flat colour is one texel tall");
        }
    }

    /// Every flat colour lands somewhere white, wherever in the shared tile it was put. Sampling
    /// one of the images by mistake would tint a part that the model said had no texture at all.
    #[test]
    fn a_flat_colour_still_samples_white_when_it_shares_a_tile_with_others() {
        let (material, image) = textured("paint");
        let mut materials = vec![material];
        materials.extend((0..40).map(|i| flat(&format!("trim{i}"), [0.5, 0.5, 0.5, 1.0])));
        let mut model = model_with(materials);
        model.images.push(image);

        let atlas = Atlas::build(&model);
        for (i, t) in atlas.tiles.iter().enumerate().skip(1) {
            let m = t.map([0.4, 0.7]);
            let x = (m[0] * ATLAS as f32) as usize;
            let y = (m[1] * ATLAS as f32) as usize;
            let at = (y * ATLAS + x) * 4;
            assert_eq!(&atlas.pixels[at..at + 3], &[255, 255, 255], "material {i}");
        }
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
        let atlas = Atlas::build(&model_with(vec![flat("red", [1.0, 0.0, 0.0, 1.0])]));
        let t = atlas.tiles[0];
        let x = ((t.u0 + t.u1) * 0.5 * ATLAS as f32) as usize;
        let y = ((t.v0 + t.v1) * 0.5 * ATLAS as f32) as usize;
        let at = (y * ATLAS + x) * 4;
        assert_eq!(&atlas.pixels[at..at + 3], &[255, 255, 255]);
        assert_eq!(atlas.textured, 0);
    }

    /// A UV that ran off the unit square would land in the next material's tile.
    #[test]
    fn uvs_are_clamped_into_their_own_tile() {
        let atlas = Atlas::build(&model_with(vec![flat("a", [1.0; 4]), flat("b", [0.0; 4])]));
        let t = atlas.tiles[0];
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
        let atlas = Atlas::build(&model_with(vec![flat("white", [1.0; 4])]));
        let packed = atlas.to_5650();
        assert_eq!(packed.len(), ATLAS * ATLAS * 2);
        let t = atlas.tiles[0];
        let x = ((t.u0 + t.u1) * 0.5 * ATLAS as f32) as usize;
        let y = ((t.v0 + t.v1) * 0.5 * ATLAS as f32) as usize;
        let at = (y * ATLAS + x) * 2;
        let v = u16::from_le_bytes([packed[at], packed[at + 1]]);
        assert_eq!(v & 0x1F, 0x1F, "red full in the low five bits");
        assert_eq!((v >> 5) & 0x3F, 0x3F, "green full in the middle six");
        assert_eq!(v >> 11, 0x1F, "blue full in the top five");
    }
}
