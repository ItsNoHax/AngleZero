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

use std::collections::HashMap;
use std::path::Path;

use gltf::mesh::Mode;
use gltf::Gltf;

use crate::mat::{self, Bounds, Mat4};
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

pub fn run(path: &Path) -> Result<()> {
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
