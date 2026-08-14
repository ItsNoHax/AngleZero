//! `inspect` — report what is inside a source model, and change nothing.
//!
//! This exists to be run before anything is converted. A car downloaded from a scanning site can
//! be a single 400,000-triangle shell or 600 named parts; it can be in metres or in centimetres,
//! Y-up or Z-up, facing +Z or -X. All of that changes what the converter has to do, and all of it
//! is visible from the glTF's own accessors and node hierarchy without touching a vertex.
//!
//! Only the JSON and the buffer views are read — `Gltf::open` rather than `gltf::import` — so the
//! embedded PNGs are never decoded. That is the difference between a report in a moment and one
//! after half a minute of image decoding.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use gltf::mesh::Mode;
use gltf::Gltf;

use crate::mat::{self, Bounds, Mat4};
use crate::model;
use crate::Result;

/// One mesh, as it is drawn: a mesh referenced by two nodes is counted twice, because that is what
/// the triangle budget will have to pay for.
struct MeshUse {
    node: String,
    triangles: usize,
    vertices: usize,
    primitives: usize,
    bounds: Bounds,
}

/// Triangles and part count per material name.
type Materials = HashMap<String, (usize, usize)>;

pub fn run(path: &Path, deep: bool, material: Option<&str>) -> Result<()> {
    let gltf = Gltf::open(path).map_err(|e| format!("could not open {}: {e}", path.display()))?;

    let mut uses = Vec::new();
    let mut bounds = Bounds::EMPTY;
    let mut warnings = Vec::new();
    let mut tree = String::new();
    let mut materials = Materials::new();

    for scene in gltf.scenes() {
        for node in scene.nodes() {
            let mut w = Walk {
                uses: &mut uses,
                bounds: &mut bounds,
                materials: &mut materials,
                warnings: &mut warnings,
                tree: &mut tree,
            };
            w.node(&node, &mat::IDENTITY, 0);
        }
    }

    if uses.is_empty() {
        return Err("GLB contains no mesh".into());
    }

    let triangles: usize = uses.iter().map(|u| u.triangles).sum();
    let vertices: usize = uses.iter().map(|u| u.vertices).sum();

    println!("Model: {}", path.display());
    let asset = &gltf.document.as_json().asset;
    if let Some(gen) = asset.generator.as_deref() {
        println!("Generator: {gen}");
    }
    if let Some(c) = asset.copyright.as_deref() {
        println!("Copyright: {c}");
    }
    // Sketchfab and friends hang the title, author and licence off `asset.extras`. The licences
    // these models come under generally require a credit, so this is not a curiosity: it is where
    // the text the game has to display comes from.
    if let Some(extras) = asset.extras.as_deref() {
        println!("Extras: {}", extras.get());
    }
    println!();
    println!("Scenes:       {}", gltf.document.scenes().len());
    println!("Nodes:        {}", gltf.document.nodes().len());
    println!("Meshes:       {}", gltf.document.meshes().len());
    println!("Vertices:     {}", commas(vertices));
    println!("Triangles:    {}", commas(triangles));
    println!("Materials:    {}", gltf.document.materials().len());
    println!("Textures:     {}", gltf.document.textures().len());
    println!("Images:       {}", gltf.document.images().len());
    println!("Animations:   {}", gltf.document.animations().len());
    println!();

    let size = bounds.size();
    println!("Bounding box (model units, node transforms applied):");
    for (axis, i) in [("X", 0), ("Y", 1), ("Z", 2)] {
        println!(
            "  {axis}: {:8.3} .. {:8.3}   ({:.3} across)",
            bounds.min[i], bounds.max[i], size[i]
        );
    }
    println!();
    println!("{}", read_the_box(&size));
    println!();

    println!("Materials, by how much of the car wears them:");
    let mut by_material: Vec<_> = materials.into_iter().collect();
    by_material.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for (name, (tris, parts)) in &by_material {
        println!(
            "  {:<40} {:>9} triangles  across {parts} parts",
            truncate(name, 40),
            commas(*tris)
        );
    }
    println!();

    uses.sort_by(|a, b| b.triangles.cmp(&a.triangles));
    println!("Drawn meshes ({}), largest first:", uses.len());
    for u in &uses {
        let c = centre(&u.bounds);
        println!(
            "  {:<46} {:>9} tris {:>8} v  at ({:6.2},{:6.2},{:6.2}){}",
            truncate(&u.node, 46),
            commas(u.triangles),
            commas(u.vertices),
            c[0],
            c[1],
            c[2],
            if u.primitives > 1 {
                format!(" {} prims", u.primitives)
            } else {
                String::new()
            }
        );
    }
    println!();

    println!("Node tree:");
    print!("{tree}");

    if !warnings.is_empty() {
        println!();
        for w in &warnings {
            println!("WARNING: {w}");
        }
    }

    if deep {
        println!();
        deep_report(path, material)?;
    }
    Ok(())
}

/// The half of the report that costs vertex reads: everything above comes out of the glTF's own
/// metadata, and metadata can be wrong. This runs the real extraction and measures the result.
///
/// The checks are chosen for the ways a source model quietly breaks a converter: an accessor whose
/// declared min/max disagrees with its contents, triangles with no area, parts with no UVs that a
/// texture stage would later map to a single texel, and vertices split so finely that welding is
/// the difference between a simplifier that works and one that cannot collapse anything.
fn deep_report(path: &Path, material: Option<&str>) -> Result<()> {
    let start = std::time::Instant::now();
    let model = crate::extract::load(path)?;
    let elapsed = start.elapsed();

    println!(
        "Extracted {} into {} parts, {} vertices, {} triangles in {:.2} s",
        model.source,
        commas(model.parts.len()),
        commas(model.vertices()),
        commas(model.triangles()),
        elapsed.as_secs_f32()
    );
    // How many real objects those parts add up to. Exporters emit one node per material, so a
    // bumper carrying paint, black trim and chrome arrives as three parts of one object — and it
    // is the object, not the part, that is visible or hidden as a whole.
    let objects: HashSet<&str> = model.parts.iter().map(|p| p.parent.as_str()).collect();
    println!(
        "  {} objects, once parts sharing a parent node are counted as one",
        commas(objects.len())
    );
    println!();

    let b = model.bounds();
    println!("Bounding box from the vertices themselves:");
    for (axis, i) in [("X", 0), ("Y", 1), ("Z", 2)] {
        println!(
            "  {axis}: {:8.3} .. {:8.3}   ({:.3} across)",
            b.min[i],
            b.max[i],
            b.size()[i]
        );
    }
    println!();

    let mut degenerate = 0usize;
    let mut without_uvs = (0usize, 0usize);
    let mut unique = HashSet::new();
    for part in &model.parts {
        if part.uvs.len() != part.positions.len() {
            without_uvs.0 += 1;
            without_uvs.1 += part.triangles();
        }
        for t in part.indices.chunks_exact(3) {
            let (a, b, c) = (
                part.positions[t[0] as usize],
                part.positions[t[1] as usize],
                part.positions[t[2] as usize],
            );
            let n = model::cross(model::sub(b, a), model::sub(c, a));
            // Twice the area. A triangle under a square micrometre covers no pixel at any distance
            // the car is ever seen from.
            if model::dot(n, n).sqrt() < 2e-12 {
                degenerate += 1;
            }
        }
        for p in &part.positions {
            unique.insert([p[0].to_bits(), p[1].to_bits(), p[2].to_bits()]);
        }
    }

    println!("Geometry:");
    println!(
        "  Degenerate triangles:     {:>9}",
        commas(degenerate)
    );
    println!(
        "  Parts without UVs:        {:>9}  ({} triangles)",
        commas(without_uvs.0),
        commas(without_uvs.1)
    );
    println!(
        "  Distinct positions:       {:>9}  of {} vertices ({:.1}% are seams or splits)",
        commas(unique.len()),
        commas(model.vertices()),
        100.0 * (1.0 - unique.len() as f32 / model.vertices().max(1) as f32)
    );
    println!();

    // What the material stage will have to work from. The base colour is the whole of the paint on
    // a car with no texture on that material, and on this pipeline it is what gets baked into the
    // vertex colours, so it is worth seeing before anything is baked.
    println!("Materials as extracted:");
    for (name, tris) in model.triangles_by_material() {
        let m = model
            .materials
            .iter()
            .find(|m| m.name == name)
            .expect("material named by a part");
        let c = m.base_color;
        println!(
            "  {:<34} {:>9} tris  rgba({:.2} {:.2} {:.2} {:.2})  metal {:.2} rough {:.2}{}{}",
            truncate(name, 34),
            commas(tris),
            c[0],
            c[1],
            c[2],
            c[3],
            m.metallic,
            m.roughness,
            match m.image {
                Some(i) => format!("  tex #{i}"),
                None => String::new(),
            },
            // Both of these are how a light is told from paint without reading its name: a lamp
            // lens is emissive, and glass and lenses are the parts modelled thin enough to be
            // marked double-sided.
            match (m.transparent, m.emissive != [0.0; 3], m.double_sided) {
                (false, false, false) => String::new(),
                (t, e, d) => format!(
                    " {}{}{}",
                    if t { "transparent " } else { "" },
                    if e { "emissive " } else { "" },
                    if d { "two-sided" } else { "" }
                ),
            },
        );
    }
    println!();

    if !model.images.is_empty() {
        println!("Embedded images ({}):", model.images.len());
        for (i, img) in model.images.iter().enumerate() {
            println!(
                "  #{i:<3} {:<28} {:>8} KB  {}",
                truncate(&img.name, 28),
                commas(img.data.len() / 1024),
                img.mime
            );
        }
        println!();
    }

    // Which parts wear one material, when a config has to tell them apart.
    //
    // A material override paints every part that wears it, and sometimes that is one thing too
    // many: the Golf's `material` is its grille bars, the strakes in the lower bumper *and* the
    // ring round the badge, which want black and black and silver. Splitting them needs the node
    // names, and this is the only place they are all listed.
    if let Some(fragment) = material {
        let want = fragment.to_ascii_lowercase();
        println!("Parts wearing a material matching `{fragment}`:");
        let mut parts: Vec<_> = model
            .parts
            .iter()
            .filter(|p| {
                model.materials[p.material]
                    .name
                    .to_ascii_lowercase()
                    .contains(&want)
            })
            .collect();
        parts.sort_by_key(|p| std::cmp::Reverse(p.triangles()));
        if parts.is_empty() {
            println!("  none");
        }
        for part in parts {
            let b = part.bounds();
            let c = centre(&b);
            let s = b.size();
            println!(
                "  {:<40} {:>8} tris  at ({:6.2},{:6.2},{:6.2})  {:.2}x{:.2}x{:.2}  [{}]",
                truncate(&part.node, 40),
                commas(part.triangles()),
                c[0],
                c[1],
                c[2],
                s[0],
                s[1],
                s[2],
                truncate(&model.materials[part.material].name, 20),
            );
        }
        println!();
    }

    // Names are given as the node's, because that is the string a car config's wheel and
    // importance rules are matched against.
    println!("Largest parts as extracted:");
    for part in largest_parts(&model, 12) {
        let b = part.bounds();
        let c = centre(&b);
        let s = b.size();
        println!(
            "  {:<38} {:>8} tris  at ({:6.2},{:6.2},{:6.2})  {:.2}x{:.2}x{:.2}  [{}]",
            truncate(&part.node, 38),
            commas(part.triangles()),
            c[0],
            c[1],
            c[2],
            s[0],
            s[1],
            s[2],
            truncate(&model.materials[part.material].name, 20),
        );
    }
    println!();

    let credit = &model.credit;
    if credit.title.is_some() || credit.author.is_some() {
        println!("Credit:");
        for (label, value) in [
            ("Title", &credit.title),
            ("Author", &credit.author),
            ("Licence", &credit.license),
        ] {
            if let Some(v) = value {
                println!("  {label}: {v}");
            }
        }
    }
    Ok(())
}

/// The report being built up as the scene graph is walked.
struct Walk<'a> {
    uses: &'a mut Vec<MeshUse>,
    bounds: &'a mut Bounds,
    materials: &'a mut Materials,
    warnings: &'a mut Vec<String>,
    tree: &'a mut String,
}

impl Walk<'_> {
    /// Walks one node and its children, accumulating the world transform on the way down.
    fn node(&mut self, node: &gltf::Node, parent: &Mat4, depth: usize) {
        let world = mat::mul(parent, &node.transform().matrix());
        let name = node.name().unwrap_or("<unnamed>").to_string();

        let mut line = format!("{:indent$}{name}", "", indent = depth * 2);
        if let Some(mesh) = node.mesh() {
            let mut u = MeshUse {
                node: name.clone(),
                triangles: 0,
                vertices: 0,
                primitives: mesh.primitives().len(),
                bounds: Bounds::EMPTY,
            };

            for prim in mesh.primitives() {
                let verts = prim
                    .get(&gltf::Semantic::Positions)
                    .map(|a| a.count())
                    .unwrap_or(0);
                let indexed = prim.indices().map(|a| a.count()).unwrap_or(verts);
                let tris = match prim.mode() {
                    Mode::Triangles => indexed / 3,
                    Mode::TriangleStrip | Mode::TriangleFan => indexed.saturating_sub(2),
                    other => {
                        self.warnings.push(format!(
                            "`{name}` uses unsupported primitive topology {other:?}; ignored"
                        ));
                        0
                    }
                };
                u.triangles += tris;
                u.vertices += verts;

                let material = prim
                    .material()
                    .name()
                    .unwrap_or("<default>")
                    .to_string();
                let entry = self.materials.entry(material).or_insert((0, 0));
                entry.0 += tris;
                entry.1 += 1;

                // The POSITION accessor carries its own min/max — the spec requires it — so the
                // box costs no vertex reads at all.
                let bb = prim.bounding_box();
                let mut local = Bounds::EMPTY;
                local.add(bb.min);
                local.add(bb.max);
                for c in mat::corners(&local) {
                    let p = mat::point(&world, c);
                    u.bounds.add(p);
                    self.bounds.add(p);
                }
            }

            line.push_str(&format!(
                "   [{} tris, {} verts]",
                commas(u.triangles),
                commas(u.vertices)
            ));
            self.uses.push(u);
        }
        self.tree.push_str(&line);
        self.tree.push('\n');

        for child in node.children() {
            self.node(&child, &world, depth + 1);
        }
    }
}

/// The `n` heaviest parts, which is where any triangle budget is won or lost.
fn largest_parts(model: &model::SourceModel, n: usize) -> Vec<&model::Part> {
    let mut parts: Vec<&model::Part> = model.parts.iter().collect();
    parts.sort_by_key(|p| std::cmp::Reverse(p.triangles()));
    parts.truncate(n);
    parts
}

/// The middle of a part's box. This is how a wheel is told from a mirror: four parts of similar
/// size at similar |X| and similar low Y, two forward and two back.
fn centre(b: &Bounds) -> [f32; 3] {
    if b.is_empty() {
        return [0.0; 3];
    }
    [
        (b.min[0] + b.max[0]) * 0.5,
        (b.min[1] + b.max[1]) * 0.5,
        (b.min[2] + b.max[2]) * 0.5,
    ]
}

/// Guesses the model's units and which way it faces, from the shape of its bounding box.
///
/// Worth printing rather than leaving to the eye: every one of these being wrong is silent. A car
/// authored in centimetres loads as a hundred-metre building, and one facing -Z drives backwards,
/// and neither shows up until it is on screen.
fn read_the_box(size: &[f32; 3]) -> String {
    let longest = size.iter().cloned().fold(0.0f32, f32::max);
    let axis = ["X", "Y", "Z"][size
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
        .map(|(i, _)| i)
        .unwrap_or(0)];

    // A real car is around 4.5 m long, 1.8 m wide, 1.4 m tall.
    let scale = match longest {
        l if (3.0..7.0).contains(&l) => "metres".to_string(),
        l if (300.0..700.0).contains(&l) => "centimetres — the converter will need scale = 0.01".into(),
        l if (0.03..0.07).contains(&l) => "hundreds of metres, or a model in odd units".into(),
        l => format!("unclear; longest axis is {l:.2} units against ~4.5 m for a real car"),
    };
    format!("Longest axis is {axis}, {longest:.2} units across. Units look like {scale}.")
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n - 1).chain("…".chars()).collect()
    }
}

/// 405557 as "405,557". The counts here span five orders of magnitude and get compared by eye.
fn commas(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::commas;

    #[test]
    fn thousands_separators_land_between_every_third_digit() {
        assert_eq!(commas(0), "0");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(1000), "1,000");
        assert_eq!(commas(405_557), "405,557");
        assert_eq!(commas(1_234_567), "1,234,567");
    }
}
