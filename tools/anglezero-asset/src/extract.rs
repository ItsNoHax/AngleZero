//! glTF in, `SourceModel` out. The only module in the tool that knows what glTF is.
//!
//! Everything past this point sees flat triangle lists in world space, so the rest of the pipeline
//! does not have to care that the E36 arrives as 472 nodes six levels deep, or that another car
//! might arrive as one node with sixty primitives.
//!
//! Buffers are imported but images are not: `import_buffers` is a few milliseconds, while decoding
//! the eighteen embedded PNGs is most of a minute, and geometry work needs none of it. The texture
//! stage decodes what it actually uses, when it uses it.

use std::path::Path;

use gltf::mesh::Mode;

use crate::mat::{self, Mat4};
use crate::model::{Credit, Image, Material, Part, SourceModel};
use crate::Result;

pub fn load(path: &Path) -> Result<SourceModel> {
    let gltf = gltf::Gltf::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;
    let gltf::Gltf { document, blob } = gltf;
    let buffers = gltf::import_buffers(&document, path.parent(), blob)
        .map_err(|e| format!("could not read the buffers in {}: {e}", path.display()))?;

    let materials: Vec<Material> = document
        .materials()
        .map(|m| convert_material(&m))
        .chain(std::iter::once(default_material()))
        .collect();
    // glTF's "no material" is the last slot, appended above, so a primitive without one still has
    // somewhere to point.
    let default_material = materials.len() - 1;

    let images = document
        .images()
        .enumerate()
        .map(|(i, img)| encoded_image(i, &img, &buffers, path.parent()))
        .collect::<Result<Vec<_>>>()?;

    let mut model = SourceModel {
        source: path.display().to_string(),
        credit: read_credit(&document),
        parts: Vec::new(),
        materials,
        images,
    };

    for scene in document.scenes() {
        for node in scene.nodes() {
            walk(
                &node,
                "",
                &mat::IDENTITY,
                &buffers,
                default_material,
                &mut model.parts,
            )?;
        }
    }

    if model.parts.is_empty() {
        return Err("GLB contains no mesh".into());
    }
    if model.triangles() == 0 {
        return Err("GLB contains meshes but no triangles".into());
    }
    Ok(model)
}

fn walk(
    node: &gltf::Node,
    parent_name: &str,
    parent: &Mat4,
    buffers: &[gltf::buffer::Data],
    default_material: usize,
    out: &mut Vec<Part>,
) -> Result<()> {
    let world = mat::mul(parent, &node.transform().matrix());
    let name = node.name().unwrap_or("<unnamed>").to_string();

    if let Some(mesh) = node.mesh() {
        for prim in mesh.primitives() {
            if let Some(part) = convert_primitive(
                &prim,
                &name,
                parent_name,
                &world,
                buffers,
                default_material,
            )? {
                out.push(part);
            }
        }
    }

    for child in node.children() {
        walk(&child, &name, &world, buffers, default_material, out)?;
    }
    Ok(())
}

fn convert_primitive(
    prim: &gltf::Primitive,
    node: &str,
    parent: &str,
    world: &Mat4,
    buffers: &[gltf::buffer::Data],
    default_material: usize,
) -> Result<Option<Part>> {
    let reader = prim.reader(|b| buffers.get(b.index()).map(|d| &d.0[..]));

    let Some(positions) = reader.read_positions() else {
        return Err(format!("`{node}` has no valid vertex positions"));
    };
    let positions: Vec<[f32; 3]> = positions.map(|p| mat::point(world, p)).collect();
    if positions.is_empty() {
        return Ok(None);
    }

    let normals: Vec<[f32; 3]> = reader
        .read_normals()
        .map(|it| {
            it.map(|n| crate::model::normalize(mat::direction(world, n)))
                .collect()
        })
        .unwrap_or_default();

    let uvs: Vec<[f32; 2]> = reader
        .read_tex_coords(0)
        .map(|t| t.into_f32().collect())
        .unwrap_or_default();

    // One index list, whatever the source used to express it.
    let raw: Vec<u32> = match reader.read_indices() {
        Some(it) => it.into_u32().collect(),
        None => (0..positions.len() as u32).collect(),
    };
    let indices = match prim.mode() {
        Mode::Triangles => raw,
        Mode::TriangleStrip => strip_to_list(&raw),
        Mode::TriangleFan => fan_to_list(&raw),
        other => {
            return Err(format!(
                "`{node}` uses unsupported primitive topology {other:?}"
            ))
        }
    };
    if indices.is_empty() {
        return Ok(None);
    }
    if let Some(&worst) = indices.iter().max() {
        if worst as usize >= positions.len() {
            return Err(format!(
                "`{node}` indexes vertex {worst} of {}",
                positions.len()
            ));
        }
    }

    let mut part = Part {
        node: node.to_string(),
        parent: parent.to_string(),
        material: prim.material().index().unwrap_or(default_material),
        positions,
        normals,
        uvs,
        indices,
    };
    part.ensure_normals();
    Ok(Some(part))
}

/// `abc bcd cde` — every other triangle is wound backwards, and the flip has to be undone or half
/// the surface faces inwards once back-face culling is on.
fn strip_to_list(strip: &[u32]) -> Vec<u32> {
    let mut out = Vec::new();
    for i in 0..strip.len().saturating_sub(2) {
        let (a, b, c) = (strip[i], strip[i + 1], strip[i + 2]);
        if a == b || b == c || a == c {
            continue;
        }
        if i % 2 == 0 {
            out.extend_from_slice(&[a, b, c]);
        } else {
            out.extend_from_slice(&[a, c, b]);
        }
    }
    out
}

/// Every triangle shares the first vertex.
fn fan_to_list(fan: &[u32]) -> Vec<u32> {
    let mut out = Vec::new();
    for i in 1..fan.len().saturating_sub(1) {
        out.extend_from_slice(&[fan[0], fan[i], fan[i + 1]]);
    }
    out
}

fn convert_material(m: &gltf::Material) -> Material {
    let pbr = m.pbr_metallic_roughness();
    Material {
        name: m.name().unwrap_or("<unnamed>").to_string(),
        base_color: pbr.base_color_factor(),
        metallic: pbr.metallic_factor(),
        roughness: pbr.roughness_factor(),
        emissive: m.emissive_factor(),
        image: pbr
            .base_color_texture()
            .map(|t| t.texture().source().index()),
        double_sided: m.double_sided(),
        transparent: !matches!(m.alpha_mode(), gltf::material::AlphaMode::Opaque)
            || pbr.base_color_factor()[3] < 0.999,
    }
}

fn default_material() -> Material {
    Material {
        name: "<default>".to_string(),
        base_color: [1.0, 1.0, 1.0, 1.0],
        metallic: 0.0,
        roughness: 1.0,
        emissive: [0.0; 3],
        image: None,
        double_sided: false,
        transparent: false,
    }
}

/// Keeps the encoded bytes, not pixels. Whether an image is ever decoded depends on whether the
/// material that references it survives to the texture stage.
fn encoded_image(
    index: usize,
    img: &gltf::Image,
    buffers: &[gltf::buffer::Data],
    base: Option<&Path>,
) -> Result<Image> {
    let name = img
        .name()
        .map(str::to_string)
        .unwrap_or_else(|| format!("image{index}"));
    match img.source() {
        gltf::image::Source::View { view, mime_type } => {
            let data = &buffers[view.buffer().index()].0;
            let start = view.offset();
            let end = start + view.length();
            Ok(Image {
                name,
                mime: mime_type.to_string(),
                data: data[start..end].to_vec(),
            })
        }
        // An external file beside the .glb rather than a chunk inside it. Read whole: a separate
        // file is not worth deferring, and a .glb with external textures is unusual enough that
        // the simple path is the right one.
        gltf::image::Source::Uri { uri, mime_type } => {
            let full = base.unwrap_or(Path::new(".")).join(uri);
            let data = std::fs::read(&full)
                .map_err(|e| format!("could not read texture {}: {e}", full.display()))?;
            Ok(Image {
                name: format!("{name} <{uri}>"),
                mime: mime_type.unwrap_or("image/png").to_string(),
                data,
            })
        }
    }
}

/// Sketchfab and similar exporters hang the title, author and licence off `asset.extras`.
fn read_credit(document: &gltf::Document) -> Credit {
    let Some(extras) = document.as_json().asset.extras.as_deref() else {
        return Credit::default();
    };
    let json = extras.get();
    Credit {
        title: json_string(json, "title"),
        author: json_string(json, "author"),
        license: json_string(json, "license"),
    }
}

/// Pulls one string field out of a flat JSON object.
///
/// A dependency on a JSON parser for four fields of a metadata blob is not worth it: `extras` is
/// free-form, so a missing or oddly-shaped field has to be tolerated either way, and tolerating it
/// is all this does.
fn json_string(json: &str, key: &str) -> Option<String> {
    let at = json.find(&format!("\"{key}\""))?;
    let rest = &json[at + key.len() + 2..];
    let open = rest.find('"')?;
    let mut out = String::new();
    let mut escaped = false;
    for c in rest[open + 1..].chars() {
        match c {
            _ if escaped => {
                out.push(c);
                escaped = false;
            }
            '\\' => escaped = true,
            '"' => return Some(out),
            _ => out.push(c),
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a one-triangle GLB in memory, so the whole import path can be tested without the
    /// source cars — which are tens of megabytes and deliberately not in the repository.
    ///
    /// The triangle sits on a node translated by (1, 2, 3) and rotated a quarter turn about Y, so
    /// a loader that forgets to apply node transforms, or applies them in the wrong order, cannot
    /// pass.
    fn one_triangle_glb() -> Vec<u8> {
        // Positions then indices, tightly packed.
        let mut bin = Vec::new();
        for v in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]] {
            for c in v {
                bin.extend_from_slice(&c.to_le_bytes());
            }
        }
        for i in [0u32, 1, 2] {
            bin.extend_from_slice(&i.to_le_bytes());
        }

        // A quarter turn about Y as a quaternion: +X ends up pointing at -Z.
        let s = (0.5f32).sqrt();
        let json = format!(
            r#"{{
              "asset":{{"version":"2.0","extras":{{"title":"Test Car","author":"Nobody"}}}},
              "scene":0,
              "scenes":[{{"nodes":[0]}}],
              "nodes":[{{"name":"hull","translation":[1,2,3],"rotation":[0,{s},0,{s}],"mesh":0}}],
              "meshes":[{{"name":"hull_mesh","primitives":[
                {{"attributes":{{"POSITION":0}},"indices":1,"material":0}}]}}],
              "materials":[{{"name":"paint","pbrMetallicRoughness":{{"baseColorFactor":[0.2,0.4,0.6,1.0]}}}}],
              "accessors":[
                {{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0,0,0],"max":[1,0,1]}},
                {{"bufferView":1,"componentType":5125,"count":3,"type":"SCALAR"}}],
              "bufferViews":[
                {{"buffer":0,"byteOffset":0,"byteLength":36}},
                {{"buffer":0,"byteOffset":36,"byteLength":12}}],
              "buffers":[{{"byteLength":48}}]
            }}"#
        );

        let mut json = json.into_bytes();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        while bin.len() % 4 != 0 {
            bin.push(0);
        }

        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&((28 + json.len() + bin.len()) as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json);
        glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"BIN\0");
        glb.extend_from_slice(&bin);
        glb
    }

    fn load_test_glb(name: &str, bytes: &[u8]) -> Result<crate::model::SourceModel> {
        let path = std::env::temp_dir().join(name);
        std::fs::write(&path, bytes).unwrap();
        let out = load(&path);
        let _ = std::fs::remove_file(&path);
        out
    }

    #[test]
    fn a_node_transform_is_baked_into_the_positions() {
        let model = load_test_glb("anglezero_one_triangle.glb", &one_triangle_glb()).unwrap();
        assert_eq!(model.parts.len(), 1);
        let part = &model.parts[0];
        assert_eq!(part.node, "hull");
        assert_eq!(part.triangles(), 1);

        // Rotated a quarter turn about Y — (1,0,0) becomes (0,0,-1) — then translated by (1,2,3).
        let expect = [[1.0, 2.0, 3.0], [1.0, 2.0, 2.0], [2.0, 2.0, 3.0]];
        for (got, want) in part.positions.iter().zip(expect) {
            for k in 0..3 {
                assert!(
                    (got[k] - want[k]).abs() < 1e-5,
                    "vertex {got:?} should be {want:?}"
                );
            }
        }
    }

    #[test]
    fn a_primitive_without_normals_gets_them_from_its_faces() {
        let model = load_test_glb("anglezero_normals.glb", &one_triangle_glb()).unwrap();
        let part = &model.parts[0];
        assert_eq!(part.normals.len(), part.positions.len());
        // The triangle is flat on the XZ plane and wound so that its face points down.
        for n in &part.normals {
            assert!((n[1] + 1.0).abs() < 1e-5, "normal {n:?} is not -Y");
        }
    }

    #[test]
    fn materials_and_credit_survive_the_trip() {
        let model = load_test_glb("anglezero_material.glb", &one_triangle_glb()).unwrap();
        let m = &model.materials[model.parts[0].material];
        assert_eq!(m.name, "paint");
        assert!((m.base_color[2] - 0.6).abs() < 1e-6);
        assert!(!m.transparent);
        assert_eq!(model.credit.title.as_deref(), Some("Test Car"));
        assert_eq!(model.credit.author.as_deref(), Some("Nobody"));
    }

    /// A file with nothing to draw must be refused, not compiled into an empty car.
    #[test]
    fn an_empty_model_is_an_error() {
        let empty = br#"{"asset":{"version":"2.0"},"scene":0,"scenes":[{"nodes":[]}]}"#;
        let mut json = empty.to_vec();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2u32.to_le_bytes());
        glb.extend_from_slice(&((20 + json.len()) as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(&json);

        let Err(err) = load_test_glb("anglezero_empty.glb", &glb) else {
            panic!("a model with nothing to draw was accepted");
        };
        assert!(err.contains("no mesh"), "unhelpful error: {err}");
    }

    #[test]
    fn strips_alternate_winding_is_undone() {
        // 0-1-2-3 as a strip is two triangles; the second is stored backwards.
        assert_eq!(strip_to_list(&[0, 1, 2, 3]), vec![0, 1, 2, 1, 3, 2]);
    }

    #[test]
    fn degenerate_strip_links_are_dropped() {
        // A repeated index is how strips are stitched together, and the two zero-area triangles it
        // makes are not triangles. Only 1-2-3 survives.
        assert_eq!(strip_to_list(&[0, 1, 1, 2, 3]), vec![1, 2, 3]);
    }

    #[test]
    fn fans_share_their_first_vertex() {
        assert_eq!(fan_to_list(&[0, 1, 2, 3]), vec![0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn credit_fields_come_out_of_the_extras_blob() {
        let json = r#"{"author":"Black Snow (https://sketchfab.com/BlackSnow02)","license":"CC-BY-4.0","title":"BMW 3-Series e36 [Street]"}"#;
        assert_eq!(json_string(json, "author").unwrap(), "Black Snow (https://sketchfab.com/BlackSnow02)");
        assert_eq!(json_string(json, "license").unwrap(), "CC-BY-4.0");
        assert_eq!(json_string(json, "title").unwrap(), "BMW 3-Series e36 [Street]");
        assert!(json_string(json, "missing").is_none());
    }
}
