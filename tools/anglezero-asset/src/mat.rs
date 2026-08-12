//! Just enough 4×4 arithmetic to flatten a glTF node hierarchy.
//!
//! Column-major, matching what glTF stores and what `gltf`'s `Transform::matrix` hands back:
//! `m[c][r]`, so `m[3]` is the translation. Deliberately not shared with the game's `math::Mat4` —
//! that one is `no_std` and row-major for the GU, and quietly reinterpreting one as the other is
//! exactly the sort of mistake that produces a car lying on its side.

pub type Mat4 = [[f32; 4]; 4];

pub const IDENTITY: Mat4 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// `a` applied after `b`: the product that composes a parent transform with a child's.
pub fn mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for (c, col) in out.iter_mut().enumerate() {
        for (r, cell) in col.iter_mut().enumerate() {
            *cell = (0..4).map(|k| a[k][r] * b[c][k]).sum();
        }
    }
    out
}

/// Transforms a position: rotated, scaled and translated.
pub fn point(m: &Mat4, p: [f32; 3]) -> [f32; 3] {
    let mut out = [m[3][0], m[3][1], m[3][2]];
    for (i, &v) in p.iter().enumerate() {
        for (o, cell) in out.iter_mut().enumerate() {
            *cell += m[i][o] * v;
        }
    }
    out
}

/// An axis-aligned box that grows to contain whatever it is shown.
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Bounds {
    pub const EMPTY: Bounds = Bounds {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };

    pub fn add(&mut self, p: [f32; 3]) {
        for i in 0..3 {
            self.min[i] = self.min[i].min(p[i]);
            self.max[i] = self.max[i].max(p[i]);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0]
    }

    pub fn size(&self) -> [f32; 3] {
        if self.is_empty() {
            return [0.0; 3];
        }
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }
}

/// The eight corners of a box, so a transformed box can be re-fitted axis-aligned.
pub fn corners(b: &Bounds) -> [[f32; 3]; 8] {
    let mut out = [[0.0f32; 3]; 8];
    for (i, c) in out.iter_mut().enumerate() {
        for axis in 0..3 {
            c[axis] = if i & (1 << axis) == 0 {
                b.min[axis]
            } else {
                b.max[axis]
            };
        }
    }
    out
}
