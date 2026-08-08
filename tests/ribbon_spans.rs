//! Every ribbon has to be culled on the same decision, or the road disappears out from under
//! the grass.
//!
//! Each ribbon used to compute its own visible span from its own chunk bounding spheres, and
//! `chunk_visible` scales its threshold by the chunk's radius:
//!
//! ```text
//! to_chunk.length() - chunk.radius > fog_far      // too far
//! to_chunk.dot(forward) > -(chunk.radius + 12.0)  // behind the camera
//! ```
//!
//! A terrain chunk reaches 190 m to either side of the centreline, so its bounding sphere has a
//! radius of a couple of hundred metres. A road chunk is twelve metres wide, so its radius is
//! forty-odd. Both tests are therefore far more generous to the terrain, and there are camera
//! positions where the terrain is drawn and the road is not — the hillside covering ground the
//! tarmac should have, with a hard straight edge along the chunk boundary. On the title screen,
//! where the camera swings low and wide around the parked car, it is unmissable.
//!
//! The renderer now takes one span from the terrain and draws every other ribbon over it. That is
//! only correct if the terrain's span is never *narrower* than another ribbon's, which is what
//! this pins: a chunk any ribbon would have drawn on its own must be inside the terrain's span.

use angle_zero::math::Vec3;
use angle_zero::mesh::{chunk_visible, Ribbon, Station, CHUNK_COUNT};
use angle_zero::track::{Track, BAY_NODE, NODE_COUNT};

const FOG_FAR: f32 = 330.0;

const fn st(lateral: f32, y: f32) -> Station {
    Station::new(lateral, y, 0)
}

/// The real tables from `src/psp/render.rs`, which is not host-compilable.
const TERRAIN: [Station; 12] = [
    st(-190.0, -78.0), st(-96.0, -40.0), st(-48.0, -17.0), st(-22.0, -4.2),
    st(-11.0, -0.9), st(-7.2, -0.25), st(7.2, -0.25), st(11.0, -0.9),
    st(22.0, -4.2), st(48.0, -17.0), st(96.0, -40.0), st(190.0, -78.0),
];
const ROAD: [Station; 5] = [
    st(-6.4, 0.0), st(-5.2, 0.02), st(0.0, 0.03), st(5.2, 0.02), st(6.4, 0.0),
];
const RAIL: [Station; 2] = [st(7.5, 0.55), st(7.5, 0.95)];

fn track() -> Box<Track> {
    let mut t = Box::new(Track::EMPTY);
    Track::generate(&mut t);
    t
}

fn span<const V: usize>(r: &Ribbon<V>, eye: Vec3, forward: Vec3) -> Option<(usize, usize)> {
    let mut lo = None;
    let mut hi = 0usize;
    for (i, c) in r.chunks.iter().enumerate() {
        if chunk_visible(c, eye, forward, FOG_FAR) {
            if lo.is_none() {
                lo = Some(i);
            }
            hi = i;
        }
    }
    lo.map(|l| (l, hi))
}

/// Every camera the game actually puts you at: the title orbit, and the chase camera down the run.
fn cameras(t: &Track) -> Vec<(Vec3, Vec3)> {
    let mut out = Vec::new();

    let car = t.nodes[BAY_NODE].p;
    for step in 0..72 {
        let a = step as f32 / 72.0 * core::f32::consts::TAU;
        let eye = Vec3::new(car.x + a.sin() * 10.5, car.y + 3.1, car.z + a.cos() * 10.5);
        let look = Vec3::new(car.x, car.y + 0.95, car.z);
        out.push((eye, look.sub(eye).normalized()));
    }

    let mut i = 4usize;
    while i < NODE_COUNT - 4 {
        let n = &t.nodes[i];
        let eye = Vec3::new(n.p.x - n.dir.x * 7.4, n.p.y + 3.3, n.p.z - n.dir.z * 7.4);
        let look = Vec3::new(n.p.x, n.p.y + 0.9, n.p.z);
        out.push((eye, look.sub(eye).normalized()));
        i += 23;
    }
    out
}

#[test]
fn the_terrain_span_covers_every_other_ribbon() {
    // The ribbons are hundreds of kilobytes each; they do not fit a default test stack.
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(check)
        .unwrap()
        .join()
        .expect("see the panic above");
}

fn check() {
    let t = track();
    let mut terrain: Box<Ribbon<40_000>> = Box::new(Ribbon::EMPTY);
    let mut road: Box<Ribbon<14_000>> = Box::new(Ribbon::EMPTY);
    let mut rail: Box<Ribbon<8_000>> = Box::new(Ribbon::EMPTY);
    terrain.build_shelved(&t, &TERRAIN);
    road.build(&t, &ROAD);
    rail.build(&t, &RAIL);

    let mut checked = 0usize;
    for (eye, forward) in cameras(&t) {
        let Some(ground) = span(&terrain, eye, forward) else {
            continue;
        };
        for (name, other) in [("road", span(&road, eye, forward)), ("rail", span(&rail, eye, forward))] {
            let Some((lo, hi)) = other else { continue };
            assert!(
                lo >= ground.0 && hi <= ground.1,
                "the {name} would draw chunks {lo}..={hi} but the terrain's span is \
                 {}..={} — drawing every ribbon over the terrain's span would cull {name} \
                 the game is supposed to show (of {CHUNK_COUNT} chunks)",
                ground.0,
                ground.1,
            );
            checked += 1;
        }
    }
    assert!(checked > 100, "only {checked} comparisons ran; the sweep is not exercising anything");
}
