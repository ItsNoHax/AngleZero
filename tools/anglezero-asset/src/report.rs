//! What the converter says it did.
//!
//! An asset pipeline that prints only "done" is one whose output can only be judged by looking at
//! the game, which is a slow way to find out that a headlight was decimated to four triangles. The
//! numbers here are the ones that turned out to explain a bad result: where the budget went, what
//! it cost each category in geometric error, and which decisions were made by guessing.
//!
//! Warnings are for anything the converter did that a person might not have wanted. It never
//! refuses to write a car over a warning — an oversized car is still a car worth looking at — but
//! it says so, every time, at the end where it will be read.

use angle_zero::azcar::{CarVertex, Category, MaterialDef, Mesh, WheelDef};
use angle_zero::mesh::Vertex;

use crate::categorise::Assignment;
use crate::mat::Bounds;
use crate::model::SourceModel;

/// One category's journey from source to compiled.
pub struct Line {
    pub category: Category,
    pub wheel: Option<u8>,
    pub source: usize,
    pub welded: usize,
    pub compiled: usize,
    /// meshoptimizer's error, as a fraction of the mesh's own size.
    pub error: f32,
}

pub struct Report {
    pub car: String,
    pub source_triangles: usize,
    pub source_vertices: usize,
    pub source_materials: usize,
    pub source_textures: usize,
    pub welded_away: usize,
    /// Viewpoints the visibility sweep took, and what it found nobody can see.
    pub views: usize,
    pub hidden_parts: usize,
    pub hidden_triangles: usize,
    pub stuck: Vec<(Category, usize)>,
    pub lines: Vec<Line>,
    pub categories: Vec<(Category, usize, usize)>,
    /// How each category was arrived at, and for how many parts. Sorting materials is the stage
    /// most likely to be quietly wrong on a new car, and this is what makes it reviewable without
    /// running the game.
    pub reasons: Vec<(Category, &'static str, usize)>,
    pub out_triangles: usize,
    pub out_vertices: usize,
    pub out_meshes: usize,
    pub out_materials: usize,
    pub out_wheels: usize,
    pub wheel_radius: f32,
    /// Triangles at each level of detail, LOD0 first.
    pub levels: Vec<usize>,
    /// Triangles left out because the config named them, and how many patterns were given.
    pub dropped_by_name: (usize, usize),
    /// How many source materials brought a real image into the atlas.
    pub textured_materials: usize,
    /// Source images that had to be resized into their tile, as (name, from, to).
    pub resized: Vec<(String, (u32, u32), (u32, u32))>,
    /// Every lamp the car carries, and how each one was arrived at.
    pub lights: Vec<(angle_zero::azcar::LightDef, &'static str, &'static str)>,
    /// What the car will drive like, after the config's defaults have been filled in.
    pub handling: angle_zero::vehicle::CarHandling,
    pub bounds: Bounds,
    pub bytes: usize,
    pub mesh_bytes: usize,
    pub warnings: Vec<String>,
}

impl Report {
    pub fn new(car: &str, model: &SourceModel) -> Report {
        Report {
            car: car.to_string(),
            source_triangles: model.triangles(),
            source_vertices: model.vertices(),
            source_materials: model.materials.len(),
            source_textures: model.images.len(),
            welded_away: 0,
            views: 0,
            hidden_parts: 0,
            hidden_triangles: 0,
            stuck: Vec::new(),
            lines: Vec::new(),
            categories: Vec::new(),
            reasons: Vec::new(),
            out_triangles: 0,
            out_vertices: 0,
            out_meshes: 0,
            out_materials: 0,
            out_wheels: 0,
            wheel_radius: 0.0,
            levels: Vec::new(),
            dropped_by_name: (0, 0),
            textured_materials: 0,
            resized: Vec::new(),
            lights: Vec::new(),
            handling: angle_zero::vehicle::CarHandling::DEFAULT,
            bounds: Bounds::EMPTY,
            bytes: 0,
            mesh_bytes: 0,
            warnings: Vec::new(),
        }
    }

    pub fn warn(&mut self, message: String) {
        self.warnings.push(message);
    }

    pub fn note_welding(&mut self, dropped: usize) {
        self.welded_away = dropped;
    }

    pub fn note_visibility(&mut self, seen: &crate::visibility::Visibility, triangles: usize) {
        self.views = seen.views;
        self.hidden_parts = seen.hidden_parts();
        self.hidden_triangles = triangles;
    }

    /// Triangles dropped because nothing could simplify them and nothing much could see them.
    /// What the lamp detector found, in the order the lamps are written.
    pub fn note_lights(&mut self, found: &crate::lamps::Found) {
        self.lights = found
            .lights
            .iter()
            .zip(&found.filled)
            .map(|(l, (_, side, how))| (*l, side.name(), *how))
            .collect();
    }

    pub fn note_stuck(&mut self, category: Category, triangles: usize) {
        self.stuck.push((category, triangles));
    }

    pub fn note_categories(&mut self, model: &SourceModel, assignment: &Assignment) {
        for (i, part) in model.parts.iter().enumerate() {
            let c = assignment.categories[i];
            match self.categories.iter_mut().find(|(k, _, _)| *k == c) {
                Some(entry) => {
                    entry.1 += part.triangles();
                    entry.2 += 1;
                }
                None => self.categories.push((c, part.triangles(), 1)),
            }
            match self
                .reasons
                .iter_mut()
                .find(|(k, r, _)| *k == c && *r == assignment.reasons[i])
            {
                Some(entry) => entry.2 += 1,
                None => self.reasons.push((c, assignment.reasons[i], 1)),
            }
        }
        self.categories.sort_by(|a, b| b.1.cmp(&a.1));
        self.reasons.sort_by(|a, b| b.2.cmp(&a.2));
    }

    pub fn note_bucket(
        &mut self,
        category: Category,
        wheel: Option<u8>,
        source: usize,
        welded: usize,
        compiled: usize,
        error: f32,
    ) {
        self.lines.push(Line {
            category,
            wheel,
            source,
            welded,
            compiled,
            error,
        });
    }

    pub fn note_output(
        &mut self,
        vertices: &[Vertex],
        indices: &[u16],
        meshes: &[Mesh],
        materials: &[MaterialDef],
        wheels: &[WheelDef],
        bounds: Bounds,
    ) {
        self.out_vertices = vertices.len();
        self.out_triangles = indices.len() / 3;
        self.out_meshes = meshes.len();
        self.out_materials = materials.len();
        self.out_wheels = wheels.len();
        self.wheel_radius = wheels.first().map(|w| w.radius).unwrap_or(0.0);
        self.bounds = bounds;
        self.mesh_bytes = vertices.len() * core::mem::size_of::<CarVertex>() + indices.len() * 2;
    }

    /// Triangles at each level, LOD0 first, and what every level costs in memory together.
    ///
    /// The memory figure has to cover all of them even though the triangle count reported above
    /// is LOD0's: the whole file is loaded into the arena whether or not the far levels are being
    /// drawn this frame.
    pub fn note_levels(&mut self, triangles: Vec<usize>, vertices: usize, indices: usize) {
        self.levels = triangles;
        self.mesh_bytes = vertices * core::mem::size_of::<CarVertex>() + indices * 2;
    }

    /// What went into the car's one texture: how many materials brought an image, how many images
    /// the source had, and what each was resized to.
    pub fn note_texture(
        &mut self,
        textured: usize,
        images: usize,
        resized: &[(String, (u32, u32), (u32, u32))],
    ) {
        self.textured_materials = textured;
        self.source_textures = images;
        self.resized = resized.to_vec();
    }

    /// Triangles left out because the config named them, and how many patterns did it.
    pub fn note_dropped_by_name(&mut self, triangles: usize, patterns: usize) {
        self.dropped_by_name = (triangles, patterns);
    }

    pub fn note_size(&mut self, bytes: &[u8]) {
        self.bytes = bytes.len();
    }

    /// The checks that are worth a line at the end of the run.
    ///
    /// All of them are things that produce a car rather than an error, which is exactly why they
    /// need saying: nothing else about the run will look wrong.
    pub fn check(&mut self, budget: usize) {
        // Nothing here repeats what wheel identification already said: every path through it that
        // finds no wheels explains which one it took.
        if self.out_triangles > budget * 6 / 5 {
            self.warn(format!(
                "final triangle count is {}, against a budget of {budget}",
                self.out_triangles
            ));
        }
        // meshoptimizer's error is relative to each mesh's own extent, so a wheel and a body panel
        // are judged on their own scale. Past a few per cent the silhouette is visibly not the
        // shape it was.
        for line in &self.lines {
            if line.error > 0.05 && line.compiled > 0 {
                self.warnings.push(format!(
                    "{} cost {:.1}% of its own size to reach {} triangles — it is losing its shape",
                    describe(line),
                    line.error * 100.0,
                    line.compiled
                ));
            }
        }
        for line in &self.lines {
            if line.compiled == 0 && line.source > 0 {
                self.warnings.push(format!(
                    "{} was reduced to nothing from {} triangles",
                    describe(line),
                    line.source
                ));
            }
        }
        let stuck: usize = self.stuck.iter().map(|(_, t)| *t).sum();
        if stuck > 0 {
            self.warnings.push(format!(
                "{} triangles would not simplify at any budget and were dropped — geometry with no \
                 collapse left to make, worth almost nothing on screen",
                commas(stuck)
            ));
        }
        let size = self.bounds.size();
        if size[2] < 2.0 || size[2] > 7.0 {
            self.warn(format!(
                "the compiled car is {:.2} m long, which is not a car — check `scale`",
                size[2]
            ));
        }
        if self.out_materials > 8 {
            self.warn(format!(
                "the car has {} materials, and so at least that many draw calls",
                self.out_materials
            ));
        }
    }

    pub fn print(&self) {
        println!("AngleZero Asset Report");
        println!("======================");
        println!();
        println!("Car: {}", self.car);
        println!();
        println!("Source:");
        println!("  Triangles:    {:>12}", commas(self.source_triangles));
        println!("  Vertices:     {:>12}", commas(self.source_vertices));
        println!("  Materials:    {:>12}", commas(self.source_materials));
        println!("  Textures:     {:>12}", commas(self.source_textures));
        println!();
        println!("Compiled:");
        println!("  Triangles:    {:>12}", commas(self.out_triangles));
        println!("  Vertices:     {:>12}", commas(self.out_vertices));
        println!("  Materials:    {:>12}", commas(self.out_materials));
        println!("  Meshes:       {:>12}", commas(self.out_meshes));
        println!("  Wheels:       {:>12}", commas(self.out_wheels));
        let reduction = if self.source_triangles > 0 {
            100.0 * (1.0 - self.out_triangles as f32 / self.source_triangles as f32)
        } else {
            0.0
        };
        println!("  Reduction:    {reduction:>11.2}%");
        if self.levels.len() > 1 {
            let names = ["LOD0 (player)", "LOD1 (near)", "LOD2 (far)"];
            println!();
            println!("Levels of detail:");
            for (i, tris) in self.levels.iter().enumerate() {
                println!(
                    "  {:<16} {:>9} triangles",
                    names.get(i).copied().unwrap_or("LOD"),
                    commas(*tris)
                );
            }
        }
        println!();

        let s = self.bounds.size();
        println!(
            "Size: {:.2} m long, {:.2} m wide, {:.2} m tall, wheels {:.3} m radius",
            s[2], s[0], s[1], self.wheel_radius
        );
        println!(
            "Ground at y = {:.3}, which is where it should be for wheels to touch the road",
            self.bounds.min[1]
        );
        println!();

        // Printed whether or not the config said anything, because "this car drives like the
        // default one" is the fact most worth seeing, and it is invisible in the config file.
        let h = self.handling;
        let default = h == angle_zero::vehicle::CarHandling::DEFAULT;
        println!(
            "Handling: {:.0} kg, {:.0} N, top {:.0} km/h, {:.0}% grip{}",
            h.mass,
            h.engine,
            h.top_speed * 3.6,
            h.grip * 100.0,
            if default { "  (the game's default car)" } else { "" }
        );
        println!();

        // Always printed, including the zero: a car whose brake lights did not come out is the
        // case this exists for, and a section that disappears when it is empty is a section nobody
        // notices is missing.
        println!("Lights:");
        for kind in [
            angle_zero::azcar::LightKind::Head,
            angle_zero::azcar::LightKind::Tail,
            angle_zero::azcar::LightKind::Brake,
            angle_zero::azcar::LightKind::Reverse,
        ] {
            let n = self.lights.iter().filter(|(l, _, _)| l.kind == kind).count();
            println!("  {:<14} {n:>3}", plural(kind));
        }
        for (light, side, how) in &self.lights {
            println!(
                "  {:<14} {side:>5} at ({:>5.2}, {:>4.2}, {:>5.2}), {:.2} m across{}   {how}",
                light.kind.name(),
                light.at[0],
                light.at[1],
                light.at[2],
                light.radius * 2.0,
                if light.range > 0.0 {
                    format!(", {:.0} m beam", light.range)
                } else {
                    String::new()
                },
            );
        }
        println!();

        if self.dropped_by_name.0 > 0 {
            println!(
                "Left out by name: {} triangles, matching {} pattern(s) in [reduce] drop",
                commas(self.dropped_by_name.0),
                self.dropped_by_name.1
            );
            println!();
        }

        if self.views > 0 {
            println!(
                "Visibility: {} parts and {} triangles are not visible from any of {} viewpoints",
                commas(self.hidden_parts),
                commas(self.hidden_triangles),
                self.views
            );
            println!(
                "            {:.1}% of the source model, dropped before the budget was shared out",
                100.0 * self.hidden_triangles as f32 / self.source_triangles.max(1) as f32
            );
            println!();
        }

        println!("Where the budget went:");
        println!(
            "  {:<22} {:>9} {:>9} {:>9}   {}",
            "", "source", "welded", "compiled", "error"
        );
        let mut lines: Vec<&Line> = self.lines.iter().collect();
        lines.sort_by_key(|l| std::cmp::Reverse(l.compiled));
        for line in lines {
            println!(
                "  {:<22} {:>9} {:>9} {:>9}   {:>5.2}%",
                describe(line),
                commas(line.source),
                commas(line.welded),
                commas(line.compiled),
                line.error * 100.0
            );
        }
        println!();

        println!("How the parts were sorted:");
        for (category, reason, count) in &self.reasons {
            println!("  {:<10} {count:>4} parts   {reason}", category.name());
        }
        println!();

        println!(
            "Texture: one {}x{} atlas, {} of {} source images used, {} materials textured",
            crate::texture::ATLAS,
            crate::texture::ATLAS,
            self.resized.len(),
            self.source_textures,
            self.textured_materials,
        );
        if let Some((name, from, _)) = self.resized.iter().max_by_key(|(_, f, _)| f.0 * f.1) {
            println!(
                "         largest was `{}` at {}x{}, into a {}px tile",
                name,
                from.0,
                from.1,
                crate::texture::ATLAS / crate::texture::tiles_across(self.source_materials),
            );
        }
        println!();

        println!("Memory:");
        println!(
            "  Mesh:         {:>9} KB   ({} vertices, {} indices)",
            self.mesh_bytes / 1024,
            commas(self.out_vertices),
            commas(self.out_triangles * 3)
        );
        println!(
            "  Metadata:     {:>9} KB",
            (self.bytes.saturating_sub(self.mesh_bytes)) / 1024
        );
        println!("  Total:        {:>9} KB", self.bytes / 1024);
        println!();
        println!("Rendering:");
        println!("  Draw calls:   {:>9}", self.out_meshes);
        println!("  Materials:    {:>9}", self.out_materials);
        println!();

        if self.warnings.is_empty() {
            println!("No warnings.");
        } else {
            println!("Warnings:");
            for w in &self.warnings {
                println!("  {w}");
            }
        }
    }
}

fn describe(line: &Line) -> String {
    match line.wheel {
        Some(c) => format!(
            "{} {}",
            crate::wheels::CORNER_NAMES[c as usize],
            line.category.name()
        ),
        None => line.category.name().to_string(),
    }
}

/// How the tally line names a kind of lamp, which is the plural because it is counting them.
fn plural(kind: angle_zero::azcar::LightKind) -> &'static str {
    use angle_zero::azcar::LightKind::*;
    match kind {
        Head => "Headlights:",
        Tail => "Tail lights:",
        Brake => "Brake lights:",
        Reverse => "Reverse:",
    }
}

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
