//! The internal representation of a source car, between glTF and `.azcar`.
//!
//! Deliberately dull: flat triangle lists, world-space positions, `f32` throughout. Every clever
//! thing glTF can do — node hierarchies, strips and fans, sparse accessors, whichever of the eight
//! index widths a given exporter felt like — is resolved on the way in, so that simplification,
//! material merging and the writer all face one shape of data instead of eight.
//!
//! Positions are baked into world space at extraction. Nothing downstream wants a node hierarchy:
//! the car is drawn as one rigid body plus four wheels, and a wheel is re-centred on its own hub
//! from its bounding box, which is more reliable than trusting a source model's pivot to be
//! anywhere near the axle. Scanned models routinely put every pivot at the origin.

use std::collections::HashMap;

use crate::mat::Bounds;

/// A whole car as it came out of the source file.
pub struct SourceModel {
    /// Where it came from, for the report.
    pub source: String,
    /// Title, author and licence, if the exporter recorded them. Models from scanning sites carry
    /// licences that require a credit, and the game has to be able to display one.
    pub credit: Credit,
    pub parts: Vec<Part>,
    pub materials: Vec<Material>,
    /// Embedded images, still compressed. Decoding is deferred until something needs the pixels.
    pub images: Vec<Image>,
}

#[derive(Default)]
pub struct Credit {
    pub title: Option<String>,
    pub author: Option<String>,
    pub license: Option<String>,
}

/// One drawable piece: a single glTF primitive, with its node's transform already applied.
pub struct Part {
    /// The node that drew it, e.g. `E36_coupe_grille_new_BMWE36_paint_0`.
    pub node: String,
    /// The node above it, which in every exporter seen so far is the part proper — the node is one
    /// child per material, the parent is the object a person would name.
    pub parent: String,
    pub material: usize,
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Triangle list. Strips and fans are expanded on the way in.
    pub indices: Vec<u32>,
}

pub struct Material {
    pub name: String,
    pub base_color: [f32; 4],
    pub metallic: f32,
    pub roughness: f32,
    pub emissive: [f32; 3],
    /// Index into `SourceModel::images`, via the base colour texture.
    pub image: Option<usize>,
    pub double_sided: bool,
    /// True when the material asks to be blended or cut out — the flag the window and light
    /// categories are recognised by as much as the name is.
    pub transparent: bool,
}

pub struct Image {
    pub name: String,
    /// The encoded bytes exactly as they sat in the GLB, plus what they are.
    pub mime: String,
    pub data: Vec<u8>,
}

impl Part {
    pub fn triangles(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::EMPTY;
        for p in &self.positions {
            b.add(*p);
        }
        b
    }

    /// Fills in flat normals for a part that arrived without any.
    ///
    /// Area-weighted, because the cross product of two triangle edges is already twice the
    /// triangle's area: summing the raw cross products weights each face by how much of the
    /// surface it actually is, which is what stops a fan of slivers from dominating the average.
    pub fn ensure_normals(&mut self) {
        if self.normals.len() == self.positions.len() {
            return;
        }
        let mut acc = vec![[0.0f32; 3]; self.positions.len()];
        for t in self.indices.chunks_exact(3) {
            let (a, b, c) = (
                self.positions[t[0] as usize],
                self.positions[t[1] as usize],
                self.positions[t[2] as usize],
            );
            let n = cross(sub(b, a), sub(c, a));
            for &i in t {
                let v = &mut acc[i as usize];
                for k in 0..3 {
                    v[k] += n[k];
                }
            }
        }
        for n in &mut acc {
            *n = normalize(*n);
        }
        self.normals = acc;
    }
}

impl SourceModel {
    pub fn triangles(&self) -> usize {
        self.parts.iter().map(|p| p.triangles()).sum()
    }

    pub fn vertices(&self) -> usize {
        self.parts.iter().map(|p| p.positions.len()).sum()
    }

    pub fn bounds(&self) -> Bounds {
        let mut b = Bounds::EMPTY;
        for p in &self.parts {
            for v in &p.positions {
                b.add(*v);
            }
        }
        b
    }

    /// Triangle count per material name, largest first. Drives both the report and the decision
    /// about which materials are worth keeping apart.
    pub fn triangles_by_material(&self) -> Vec<(&str, usize)> {
        let mut by: HashMap<&str, usize> = HashMap::new();
        for p in &self.parts {
            *by.entry(self.materials[p.material].name.as_str())
                .or_default() += p.triangles();
        }
        let mut out: Vec<_> = by.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        out
    }
}

pub fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    if len > 1e-12 {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 1.0, 0.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two triangles sharing an edge, both facing +Y.
    #[test]
    fn generated_normals_face_the_way_the_winding_says() {
        let mut p = Part {
            node: "quad".into(),
            parent: "quad".into(),
            material: 0,
            positions: vec![
                [0.0, 0.0, 0.0],
                [1.0, 0.0, 0.0],
                [1.0, 0.0, 1.0],
                [0.0, 0.0, 1.0],
            ],
            normals: Vec::new(),
            uvs: Vec::new(),
            indices: vec![0, 2, 1, 0, 3, 2],
        };
        p.ensure_normals();
        assert_eq!(p.normals.len(), 4);
        for n in &p.normals {
            assert!((n[1] - 1.0).abs() < 1e-5, "normal {n:?} is not +Y");
        }
    }

    /// A part that already has normals keeps them: they are the artist's, and smoothing groups
    /// carry information that face normals cannot reconstruct.
    #[test]
    fn supplied_normals_are_left_alone() {
        let mut p = Part {
            node: "t".into(),
            parent: "t".into(),
            material: 0,
            positions: vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            normals: vec![[0.0, 0.0, -1.0]; 3],
            uvs: Vec::new(),
            indices: vec![0, 1, 2],
        };
        p.ensure_normals();
        assert_eq!(p.normals[0], [0.0, 0.0, -1.0]);
    }
}
