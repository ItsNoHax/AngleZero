//! A software renderer for a compiled `.azcar`, at whatever size and angle you ask for.
//!
//! `docs/cars.md` has said for a while that the way to tell a bad mesh from a small one is to draw
//! the part on its own and large, because a wheel is twenty pixels on a 480-wide screen. This is
//! that, for the whole car: it reads the compiled asset and rasterises it with the rules
//! `draw_one_car` uses — opaque meshes then blended ones, back-face culling except where the
//! material says two-sided, and the same 16-bit depth buffer over the same 0.4 m to 2400 m frustum
//! the console runs.
//!
//! That last part is the reason this is not a generic model viewer. A car's shells sit millimetres
//! apart, and 16 bits across a 6000:1 frustum is about 2.4 mm at eight metres, so which surface
//! wins is a property of the *depth format* and not of the geometry. Rendering at float depth shows
//! a clean car and hides the fault being looked for.
//!
//! ```text
//! cargo run --release -p anglezero-asset --bin azview -- \
//!     assets/compiled/bmw_e39.azcar out.png --yaw 200 --pitch 12 --dist 6
//! ```
//!
//! `--no-cull` is the one flag worth knowing about: run it twice, with and without, and anything
//! that appears is something the console's culling is throwing away.

use std::path::PathBuf;

use angle_zero::azcar::{self, Car};

const NEAR: f32 = 0.4;
const FAR: f32 = 2400.0;

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args.iter().map(|s| s.as_str()).collect::<Vec<_>>()) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("ERROR: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

struct Options {
    input: PathBuf,
    output: PathBuf,
    yaw: f32,
    pitch: f32,
    dist: f32,
    fov: f32,
    width: usize,
    height: usize,
    lod: usize,
    /// Draw only meshes whose material or mesh name contains this.
    only: Option<String>,
    /// Draw everything *but* these meshes, for seeing what a shell is hiding.
    hide: Option<String>,
    /// Paint each mesh a flat colour of its own instead of its baked one.
    by_mesh: bool,
    /// Draw every triangle whatever the material says, for telling a missing surface from a
    /// culled one.
    no_cull: bool,
    /// Drop the atlas and draw the baked vertex colour alone, for telling a part that is dark
    /// because its texture is from one that is dark before the texture is even sampled.
    no_tex: bool,
    /// Draw only these mesh indices, for picking one out of several that share a name.
    mesh: Vec<usize>,
    /// What the camera orbits, in car space. Defaults to the middle of the car's bounds.
    look: Option<[f32; 3]>,
    background: [u8; 3],
}

fn run(args: &[&str]) -> Result<(), String> {
    let mut paths = Vec::new();
    let mut o = Options {
        input: PathBuf::new(),
        output: PathBuf::new(),
        yaw: 210.0,
        pitch: 14.0,
        dist: 7.0,
        fov: 45.0,
        width: 960,
        height: 640,
        lod: 0,
        only: None,
        hide: None,
        by_mesh: false,
        no_cull: false,
        no_tex: false,
        mesh: Vec::new(),
        look: None,
        background: [24, 24, 32],
    };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut number = |what: &str| -> Result<f32, String> {
            it.next()
                .ok_or_else(|| format!("{what} needs a number"))?
                .parse::<f32>()
                .map_err(|_| format!("{what} needs a number"))
        };
        match *a {
            "--yaw" => o.yaw = number("--yaw")?,
            "--pitch" => o.pitch = number("--pitch")?,
            "--dist" => o.dist = number("--dist")?,
            "--fov" => o.fov = number("--fov")?,
            "--lod" => o.lod = number("--lod")? as usize,
            "--size" => {
                let v = it.next().ok_or("--size needs WxH")?;
                let (w, h) = v.split_once('x').ok_or("--size needs WxH")?;
                o.width = w.parse().map_err(|_| "--size needs WxH")?;
                o.height = h.parse().map_err(|_| "--size needs WxH")?;
            }
            "--only" => o.only = Some(it.next().ok_or("--only needs a name")?.to_string()),
            "--hide" => o.hide = Some(it.next().ok_or("--hide needs a name")?.to_string()),
            "--by-mesh" => o.by_mesh = true,
            "--no-cull" => o.no_cull = true,
            "--no-tex" => o.no_tex = true,
            "--mesh" => o.mesh.push(number("--mesh")? as usize),
            "--look" => {
                let v = it.next().ok_or("--look needs x,y,z")?;
                let n: Vec<f32> = v.split(',').filter_map(|p| p.parse().ok()).collect();
                let [x, y, z] = n[..] else {
                    return Err("--look needs x,y,z".into());
                };
                o.look = Some([x, y, z]);
            }
            "--white" => o.background = [200, 200, 210],
            other if other.starts_with("--") => return Err(format!("unknown option `{other}`")),
            other => paths.push(other),
        }
    }
    let [input, output] = paths[..] else {
        return Err("view needs a .azcar and an output .png".into());
    };
    o.input = PathBuf::from(input);
    o.output = PathBuf::from(output);

    let bytes = std::fs::read(&o.input).map_err(|e| format!("{}: {e}", o.input.display()))?;
    let car = Car::parse(&bytes).map_err(|e| format!("{}: {}", o.input.display(), e.message()))?;
    for i in 0..car.lod(0).mesh_count as usize {
        let m = car.mesh(i);
        let mat = car.material(m.material as usize);
        // Where in the atlas this mesh actually samples, and how bright what it finds is. A part
        // that is dark despite a white vertex colour is a part whose texels are dark, and this is
        // the only way to see which texels those are without unpacking the file by hand.
        let verts = car.vertices();
        let idx = car.indices();
        let mut lo = [f32::INFINITY; 2];
        let mut hi = [f32::NEG_INFINITY; 2];
        for k in 0..m.index_count as usize {
            let v = &verts[idx[m.first_index as usize + k] as usize];
            lo[0] = lo[0].min(v.u);
            hi[0] = hi[0].max(v.u);
            lo[1] = lo[1].min(v.v);
            hi[1] = hi[1].max(v.v);
        }
        eprintln!(
            "      uv u[{:.3},{:.3}] v[{:.3},{:.3}]",
            lo[0], hi[0], lo[1], hi[1]
        );
        eprintln!(
            "  {i:2}  {:<10} {:<26} {:>6} tris  at ({:6.2},{:5.2},{:6.2}) r{:.2}{}{}",
            mat.category.name(),
            name(&car, m.name),
            m.index_count / 3,
            m.center[0],
            m.center[1],
            m.center[2],
            m.radius,
            if mat.blended() { "  blend" } else { "" },
            if mat.two_sided() || m.two_sided() { "  two-sided" } else { "" },
        );
    }
    let image = draw(&car, &o);
    image::save_buffer(
        &o.output,
        &image,
        o.width as u32,
        o.height as u32,
        image::ColorType::Rgb8,
    )
    .map_err(|e| format!("{}: {e}", o.output.display()))?;
    println!("{} -> {}", o.input.display(), o.output.display());
    Ok(())
}

/// The camera orbits the car's own centre, so a car is framed the same way whatever its bounds are.
fn draw(car: &Car, o: &Options) -> Vec<u8> {
    let b = car.bounds();
    let centre = o.look.unwrap_or([
        (b[0] + b[3]) * 0.5,
        (b[1] + b[4]) * 0.5,
        (b[2] + b[5]) * 0.5,
    ]);
    let (ys, yc) = (o.yaw.to_radians().sin(), o.yaw.to_radians().cos());
    let (ps, pc) = (o.pitch.to_radians().sin(), o.pitch.to_radians().cos());
    let eye = [
        centre[0] + o.dist * pc * ys,
        centre[1] + o.dist * ps,
        centre[2] + o.dist * pc * yc,
    ];

    // Right-handed look-at, then the same perspective the console sets up.
    let f = norm(sub(centre, eye));
    let s = norm(cross(f, [0.0, 1.0, 0.0]));
    let u = cross(s, f);

    let mut colour = vec![0u8; o.width * o.height * 3];
    for p in colour.chunks_exact_mut(3) {
        p.copy_from_slice(&o.background);
    }
    // Cleared to 0 and tested with GEQUAL, exactly as `psp/mod.rs` sets it up: near is 65535.
    let mut depth = vec![0u16; o.width * o.height];

    let lod = if car.lod_count() > o.lod {
        car.lod(o.lod)
    } else {
        car.lod(0)
    };
    let meshes = lod.first_mesh as usize..lod.first_mesh as usize + lod.mesh_count as usize;

    let tan = (o.fov.to_radians() * 0.5).tan();
    let aspect = o.width as f32 / o.height as f32;

    let project = |v: [f32; 3]| -> ([f32; 3], f32) {
        let d = sub(v, eye);
        let view = [dot(d, s), dot(d, u), -dot(d, f)];
        // View z is negative in front of the camera; depth here is the positive distance.
        let z = -view[2];
        let x = view[0] / (tan * aspect);
        let y = view[1] / tan;
        let win = [
            (x / z * 0.5 + 0.5) * o.width as f32,
            (0.5 - y / z * 0.5) * o.height as f32,
            65535.0 * NEAR * (FAR - z) / (z * (FAR - NEAR)),
        ];
        (win, z)
    };

    let texture = if o.no_tex { None } else { car.texture() };

    for blended in [false, true] {
        for i in meshes.clone() {
            let mesh = car.mesh(i);
            let material = car.material(mesh.material as usize);
            if material.blended() != blended {
                continue;
            }
            if !o.mesh.is_empty() && !o.mesh.contains(&i) {
                continue;
            }
            let mesh_name = name(car, mesh.name);
            let material_name = name(car, material.name);
            if let Some(only) = &o.only {
                if !mesh_name.contains(only) && !material_name.contains(only) {
                    continue;
                }
            }
            if let Some(hide) = &o.hide {
                if mesh_name.contains(hide) || material_name.contains(hide) {
                    continue;
                }
            }
            // Wheels are stored about their own hub; put them back on the car.
            let offset = if mesh.wheel == azcar::NO_WHEEL {
                [0.0; 3]
            } else {
                car.wheel(mesh.wheel as usize).hub
            };
            let flat = o.by_mesh.then(|| tint(i));

            let verts = car.vertices();
            let indices = car.indices();
            let run = mesh.first_index as usize..mesh.first_index as usize + mesh.index_count as usize;
            for tri in indices[run].chunks_exact(3) {
                let mut win = [[0.0f32; 3]; 3];
                let mut eyez = [0.0f32; 3];
                let mut ok = true;
                for (k, idx) in tri.iter().enumerate() {
                    let v = &verts[*idx as usize];
                    let (w, z) = project([
                        v.x + offset[0],
                        v.y + offset[1],
                        v.z + offset[2],
                    ]);
                    if z <= NEAR {
                        ok = false;
                    }
                    win[k] = w;
                    eyez[k] = z;
                }
                if !ok {
                    continue;
                }
                // Screen-space winding. Y is down in window space, so a counter-clockwise front
                // face — which is what `sceGuFrontFace` selects — has a negative area here.
                let area = (win[1][0] - win[0][0]) * (win[2][1] - win[0][1])
                    - (win[2][0] - win[0][0]) * (win[1][1] - win[0][1]);
                if area == 0.0 {
                    continue;
                }
                // Both flags, exactly as `draw_one_car` does it: the category sets one on the
                // material and the compiler sets the other per mesh, for a bucket whose parts the
                // sweep found were being culled into holes. Checking only the material here made
                // this viewer draw holes the console does not have, and — worse — made a fix to
                // `TWO_SIDED_SHARE` look like it had changed nothing at all.
                let culled = !material.two_sided() && !mesh.two_sided();
                if culled && !o.no_cull && area > 0.0 {
                    continue;
                }
                raster(
                    &mut colour,
                    &mut depth,
                    o,
                    &win,
                    &eyez,
                    [
                        &verts[tri[0] as usize],
                        &verts[tri[1] as usize],
                        &verts[tri[2] as usize],
                    ],
                    texture.as_ref(),
                    material.blended(),
                    flat,
                );
            }
        }
    }
    colour
}

#[allow(clippy::too_many_arguments)]
fn raster(
    colour: &mut [u8],
    depth: &mut [u16],
    o: &Options,
    win: &[[f32; 3]; 3],
    eyez: &[f32; 3],
    v: [&azcar::CarVertex; 3],
    texture: Option<&azcar::Texture>,
    blend: bool,
    flat: Option<[u8; 3]>,
) {
    let minx = win.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxx = (win.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil() as isize)
        .clamp(0, o.width as isize) as usize;
    let miny = win.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize;
    let maxy = (win.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil() as isize)
        .clamp(0, o.height as isize) as usize;

    let area = (win[1][0] - win[0][0]) * (win[2][1] - win[0][1])
        - (win[2][0] - win[0][0]) * (win[1][1] - win[0][1]);
    let inv_area = 1.0 / area;

    for y in miny..maxy {
        for x in minx..maxx {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let w0 = ((win[2][0] - win[1][0]) * (py - win[1][1])
                - (win[2][1] - win[1][1]) * (px - win[1][0]))
                * inv_area;
            let w1 = ((win[0][0] - win[2][0]) * (py - win[2][1])
                - (win[0][1] - win[2][1]) * (px - win[2][0]))
                * inv_area;
            let w2 = 1.0 - w0 - w1;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                continue;
            }
            let z = win[0][2] * w0 + win[1][2] * w1 + win[2][2] * w2;
            let z = z.clamp(0.0, 65535.0) as u16;
            let at = y * o.width + x;
            if z < depth[at] {
                continue;
            }

            // Perspective-correct attributes, which is what the GE does for texture coordinates.
            let iw = [1.0 / eyez[0], 1.0 / eyez[1], 1.0 / eyez[2]];
            let denom = w0 * iw[0] + w1 * iw[1] + w2 * iw[2];
            let p = [w0 * iw[0] / denom, w1 * iw[1] / denom, w2 * iw[2] / denom];

            let mut rgb = [0.0f32; 3];
            let mut alpha = 0.0f32;
            for k in 0..3 {
                let c = v[k].color;
                rgb[0] += p[k] * (c & 0xFF) as f32;
                rgb[1] += p[k] * ((c >> 8) & 0xFF) as f32;
                rgb[2] += p[k] * ((c >> 16) & 0xFF) as f32;
                alpha += p[k] * ((c >> 24) & 0xFF) as f32;
            }
            if let Some(t) = texture {
                let u = v[0].u * p[0] + v[1].u * p[1] + v[2].u * p[2];
                let vv = v[0].v * p[0] + v[1].v * p[1] + v[2].v * p[2];
                let tx = ((u.clamp(0.0, 1.0) * t.width as f32) as usize).min(t.width - 1);
                let ty = ((vv.clamp(0.0, 1.0) * t.height as f32) as usize).min(t.height - 1);
                let at = (ty * t.width + tx) * 2;
                let texel = t.pixels[at] as u16 | ((t.pixels[at + 1] as u16) << 8);
                let tr = ((texel & 0x1F) << 3) as f32;
                let tg = (((texel >> 5) & 0x3F) << 2) as f32;
                let tb = (((texel >> 11) & 0x1F) << 3) as f32;
                rgb[0] = rgb[0] * tr / 255.0;
                rgb[1] = rgb[1] * tg / 255.0;
                rgb[2] = rgb[2] * tb / 255.0;
            }
            if let Some(f) = flat {
                rgb = [f[0] as f32, f[1] as f32, f[2] as f32];
            }

            let dst = &mut colour[at * 3..at * 3 + 3];
            if blend {
                let a = (alpha / 255.0).clamp(0.0, 1.0);
                for k in 0..3 {
                    dst[k] = (rgb[k] * a + dst[k] as f32 * (1.0 - a)).clamp(0.0, 255.0) as u8;
                }
                // Blended surfaces still write depth on the console — nothing turns the mask off
                // around the car — so this does too.
            } else {
                for k in 0..3 {
                    dst[k] = rgb[k].clamp(0.0, 255.0) as u8;
                }
            }
            depth[at] = z;
        }
    }
}

fn name<'a>(car: &Car<'a>, at: u16) -> String {
    String::from_utf8_lossy(car.name(at)).into_owned()
}

/// A distinguishable colour per mesh index, for `--by-mesh`.
fn tint(i: usize) -> [u8; 3] {
    let h = (i as f32 * 0.618_034) % 1.0 * 6.0;
    let x = (255.0 * (1.0 - (h % 2.0 - 1.0).abs())) as u8;
    match h as usize {
        0 => [255, x, 0],
        1 => [x, 255, 0],
        2 => [0, 255, x],
        3 => [0, x, 255],
        4 => [x, 0, 255],
        _ => [255, 0, x],
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt();
    [a[0] / l, a[1] / l, a[2] / l]
}
