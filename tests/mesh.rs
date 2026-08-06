//! Geometry construction: ribbons and chunking.

use angle_zero::mesh::{
    self, Ribbon, Station, CHUNK_COUNT, CHUNK_NODES, RENDER_NODES, RENDER_STRIDE,
};
use angle_zero::track::Track;

fn track() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

const ROAD: [Station; 5] = [
    Station::new(-6.4, 0.0, 0xff20_1c1a),
    Station::new(-5.2, 0.02, 0xff20_1c1a),
    Station::new(0.0, 0.03, 0xff20_1c1a),
    Station::new(5.2, 0.02, 0xff20_1c1a),
    Station::new(6.4, 0.0, 0xff20_1c1a),
];

#[test]
fn the_render_mesh_decimates_the_gameplay_centreline() {
    // Gameplay uses all 2620 nodes; the mesh does not need that density at 480x272.
    assert!(RENDER_NODES < angle_zero::track::NODE_COUNT);
    assert_eq!(
        RENDER_NODES,
        (angle_zero::track::NODE_COUNT + RENDER_STRIDE - 1) / RENDER_STRIDE
    );
    assert_eq!(CHUNK_COUNT, (RENDER_NODES + CHUNK_NODES - 1) / CHUNK_NODES);
}

#[test]
fn every_chunk_bounds_the_vertices_it_covers() {
    let t = track();
    let mut r = Box::new(Ribbon::<{ mesh::ribbon_capacity(5) }>::EMPTY);
    r.build(&t, &ROAD);

    let mut covered = 0;
    for c in r.chunks.iter() {
        assert!(c.count > 0, "empty chunk");
        covered += c.count;
        for v in &r.verts[c.start as usize..(c.start + c.count) as usize] {
            let d = ((v.x - c.center.x).powi(2)
                + (v.y - c.center.y).powi(2)
                + (v.z - c.center.z).powi(2))
            .sqrt();
            assert!(
                d <= c.radius + 1e-3,
                "vertex {d} from chunk centre, radius only {}",
                c.radius
            );
        }
    }
    assert_eq!(covered as usize, r.len);
}

#[test]
fn chunks_overlap_by_a_node_so_the_road_has_no_gaps() {
    let t = track();
    let mut r = Box::new(Ribbon::<{ mesh::ribbon_capacity(5) }>::EMPTY);
    r.build(&t, &ROAD);

    // The last node of one chunk is the first of the next, so consecutive chunk bounding
    // spheres must overlap rather than leave a hole between them.
    for w in r.chunks.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let d = ((a.center.x - b.center.x).powi(2)
            + (a.center.y - b.center.y).powi(2)
            + (a.center.z - b.center.z).powi(2))
        .sqrt();
        assert!(
            d <= a.radius + b.radius,
            "chunks are {d} apart but only span {} + {}",
            a.radius,
            b.radius
        );
    }
}

#[test]
fn the_ribbon_fits_the_capacity_reserved_for_it() {
    let t = track();
    let mut r = Box::new(Ribbon::<{ mesh::ribbon_capacity(12) }>::EMPTY);
    let terrain: [Station; 12] = [
        Station::new(-190.0, -78.0, 0xff1c3022),
        Station::new(-96.0, -40.0, 0xff1c3022),
        Station::new(-48.0, -17.0, 0xff1c3022),
        Station::new(-22.0, -4.2, 0xff1c3022),
        Station::new(-11.0, -0.9, 0xff1c3022),
        Station::new(-7.2, -0.25, 0xff1c3022),
        Station::new(7.2, -0.25, 0xff1c3022),
        Station::new(11.0, -0.9, 0xff1c3022),
        Station::new(22.0, -4.2, 0xff1c3022),
        Station::new(48.0, -17.0, 0xff1c3022),
        Station::new(96.0, -40.0, 0xff1c3022),
        Station::new(190.0, -78.0, 0xff1c3022),
    ];
    r.build(&t, &terrain);
    assert!(r.len > 0);
    assert!(
        r.len <= mesh::ribbon_capacity(12),
        "{} verts overflowed capacity {}",
        r.len,
        mesh::ribbon_capacity(12)
    );
}

#[test]
fn road_vertices_sit_where_the_stations_say_relative_to_the_centreline() {
    let t = track();
    let mut r = Box::new(Ribbon::<{ mesh::ribbon_capacity(5) }>::EMPTY);
    r.build(&t, &ROAD);

    // Every vertex must lie within the widest station's offset of some centreline node. Sample
    // the vertices, but search every node — a coarse node search would itself add error.
    let widest = ROAD
        .iter()
        .fold(0.0f32, |m, s| m.max(s.lateral.abs()));

    let mut v_i = 0;
    while v_i < r.len {
        let v = &r.verts[v_i];
        let mut best = f32::INFINITY;
        for n in t.nodes.iter() {
            let d = ((v.x - n.p.x).powi(2) + (v.z - n.p.z).powi(2)).sqrt();
            if d < best {
                best = d;
            }
        }
        assert!(
            best <= widest + 0.05,
            "road vertex {best} m from the centreline, widest station is {widest}"
        );
        v_i += 37;
    }
}

#[test]
fn triangle_strips_are_wound_so_the_road_faces_up() {
    let t = track();
    let mut r = Box::new(Ribbon::<{ mesh::ribbon_capacity(5) }>::EMPTY);
    r.build(&t, &ROAD);

    // Accumulate the signed Y of every triangle normal across a chunk. Degenerate joining
    // triangles contribute nothing, so they cannot skew this.
    let c = &r.chunks[4];
    let s = c.start as usize;
    let n = c.count as usize;
    let mut y_sum = 0.0f32;
    for i in 0..n.saturating_sub(2) {
        let (a, b, cc) = (&r.verts[s + i], &r.verts[s + i + 1], &r.verts[s + i + 2]);
        let (ux, uy, uz) = (b.x - a.x, b.y - a.y, b.z - a.z);
        let (vx, vy, vz) = (cc.x - a.x, cc.y - a.y, cc.z - a.z);
        let ny = uz * vx - ux * vz;
        // Strips alternate winding every triangle.
        y_sum += if i % 2 == 0 { ny } else { -ny };
        let _ = (uy, vy);
    }
    assert!(
        y_sum > 0.0,
        "road strip is wound face-down (summed normal Y {y_sum})"
    );
}

#[test]
fn a_box_has_twelve_triangles() {
    let mut out = [mesh::Vertex::ZERO; 36];
    let n = mesh::build_box(&mut out, 1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 0xffff_ffff);
    assert_eq!(n, 36);

    // Extents must match the requested size.
    let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
    for v in out.iter() {
        for (k, c) in [v.x, v.y, v.z].iter().enumerate() {
            lo[k] = lo[k].min(*c);
            hi[k] = hi[k].max(*c);
        }
    }
    assert!((hi[0] - lo[0] - 1.0).abs() < 1e-5);
    assert!((hi[1] - lo[1] - 2.0).abs() < 1e-5);
    assert!((hi[2] - lo[2] - 3.0).abs() < 1e-5);
}

#[test]
fn an_upright_cylinder_stands_on_the_point_it_is_given() {
    let mut out = [mesh::Vertex::ZERO; 256];
    let n = mesh::build_upright_cylinder(&mut out, 8, 0.5, 0.28, 10.0, -3.0, 4.0, 0xffff_ffff);
    assert!(n > 0 && n <= out.len());
    for v in &out[..n] {
        // Sits between its base and its top, never below.
        assert!(v.y >= -3.0 - 1e-4 && v.y <= -3.0 + 0.28 + 1e-4, "y {}", v.y);
        let r = ((v.x - 10.0).powi(2) + (v.z - 4.0).powi(2)).sqrt();
        assert!(r <= 0.5 + 1e-4, "radius {r}");
    }
    // The base ring is present, so the tyre does not float.
    assert!(out[..n].iter().any(|v| (v.y + 3.0).abs() < 1e-4));
}

#[test]
fn a_cone_tapers_to_a_point() {
    let mut out = [mesh::Vertex::ZERO; 256];
    let n = mesh::build_cone(&mut out, 6, 0.24, 0.62, 0.0, 0.0, 0.0, 0xffff_ffff);
    assert!(n > 0);
    let apexes = out[..n].iter().filter(|v| (v.y - 0.62).abs() < 1e-4).count();
    assert_eq!(apexes, 6, "one apex vertex per side");
    for v in &out[..n] {
        let r = (v.x * v.x + v.z * v.z).sqrt();
        // Radius shrinks to nothing at the tip.
        let expected = 0.24 * (1.0 - v.y / 0.62);
        assert!(r <= expected + 1e-3, "radius {r} at height {}", v.y);
    }
}

#[test]
fn a_ground_quad_is_flat_and_the_size_asked_for() {
    let mut out = [mesh::Vertex::ZERO; 6];
    let forward = angle_zero::math::Vec2::new(0.0, 1.0);
    let n = mesh::build_ground_quad(&mut out, 5.0, -2.0, 7.0, forward, 17.0, 6.5, 0xffff_ffff);
    assert_eq!(n, 6);
    for v in &out[..n] {
        assert!((v.y + 2.0).abs() < 1e-5, "quad is not flat");
    }
    let (min_z, max_z) = out[..n]
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v.z), b.max(v.z)));
    let (min_x, max_x) = out[..n]
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), v| (a.min(v.x), b.max(v.x)));
    assert!((max_z - min_z - 34.0).abs() < 1e-4, "length {}", max_z - min_z);
    assert!((max_x - min_x - 13.0).abs() < 1e-4, "width {}", max_x - min_x);
}

#[test]
fn a_cylinder_closes_on_itself() {
    let mut out = [mesh::Vertex::ZERO; 256];
    let sides = 9;
    let n = mesh::build_cylinder(&mut out, sides, 0.36, 0.26, 0xffff_ffff);
    assert!(n > 0 && n <= out.len());

    // Radius is honoured on the round axis (the cylinder's axis is X).
    for v in &out[..n] {
        let r = (v.y * v.y + v.z * v.z).sqrt();
        assert!(r <= 0.36 + 1e-4, "radius {r} exceeded 0.36");
        assert!(v.x.abs() <= 0.13 + 1e-4, "width {} exceeded half of 0.26", v.x);
    }
}
