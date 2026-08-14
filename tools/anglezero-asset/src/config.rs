//! The per-car configuration file.
//!
//! This file exists so that adding a car is a `.glb` and a `.toml`, and never a change to Rust. No
//! two source models agree on anything: the E36 calls its wheels `Object_4.001` and
//! `wheel_bmw_aplg_16x7.002`, another car will call them `Wheel_FL`, a third will call them
//! `polySurface88`. Whatever the converter can work out for itself it works out; whatever it
//! cannot is named here.
//!
//! Everything has a default that is right for a well-behaved model, so a minimal config is a name.

use std::path::Path;

use serde::Deserialize;

use crate::Result;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CarConfig {
    /// What the car is called, for the report and the game's own listings.
    pub name: String,

    /// Multiplies every position. For models authored in centimetres, or simply too big.
    #[serde(default = "one")]
    pub scale: f32,

    /// Triangle budget, overridden by `--triangles` on the command line.
    #[serde(default = "default_triangles")]
    pub triangles: usize,

    #[serde(default)]
    pub wheels: Wheels,

    #[serde(default)]
    pub materials: MaterialRules,

    #[serde(default)]
    pub spawn: Spawn,

    #[serde(default)]
    pub reduce: Reduction,

    #[serde(default)]
    pub handling: Handling,

    #[serde(default)]
    pub lights: Lights,

    /// Triangle budgets for the coarser levels, nearest first. Empty means a car with one level,
    /// which is what every car was until there was something on screen to spend a second one on.
    ///
    /// Not a ratio of the main budget: what a car can lose without falling apart depends on the
    /// car, and the whole point of the level is that it is looked at from further away.
    #[serde(default)]
    pub lods: Vec<usize>,
}

/// How the car drives, as distinct from what it looks like.
///
/// Nothing in a mesh says what an engine produces, so unlike everything else in this file these
/// cannot be measured off the model — they are the one part of a car that is authored rather than
/// converted. Every field defaults to the numbers the game was tuned around, so a car that says
/// nothing here drives exactly as the game always has, and a car that says something is opting in.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handling {
    /// Kerb mass, kg.
    #[serde(default = "default_mass")]
    pub mass: f32,
    /// Yaw inertia, kg·m². Left out, it is derived from the mass at the tuned car's ratio, which
    /// is closer to right than reusing a heavier car's number outright.
    #[serde(default)]
    pub inertia: Option<f32>,
    #[serde(default = "default_front_axle")]
    pub front_axle: f32,
    #[serde(default = "default_rear_axle")]
    pub rear_axle: f32,
    /// Drive force at full throttle from rest, N.
    #[serde(default = "default_engine")]
    pub engine: f32,
    /// Where drive force has tailed off, m/s.
    #[serde(default = "default_top_speed")]
    pub top_speed: f32,
    #[serde(default = "default_brake")]
    pub brake: f32,
    /// Steering lock at a standstill, radians.
    #[serde(default = "default_steer_lock")]
    pub steer_lock: f32,
    /// Multiplies grip. Below 1.0 the car slides earlier.
    #[serde(default = "one")]
    pub grip: f32,
}

fn default_mass() -> f32 {
    DEFAULT.mass
}
fn default_front_axle() -> f32 {
    DEFAULT.front_axle
}
fn default_rear_axle() -> f32 {
    DEFAULT.rear_axle
}
fn default_engine() -> f32 {
    DEFAULT.engine
}
fn default_top_speed() -> f32 {
    DEFAULT.top_speed
}
fn default_brake() -> f32 {
    DEFAULT.brake
}
fn default_steer_lock() -> f32 {
    DEFAULT.steer_lock
}

const DEFAULT: angle_zero::vehicle::CarHandling = angle_zero::vehicle::CarHandling::DEFAULT;

impl Default for Handling {
    fn default() -> Self {
        Handling {
            mass: DEFAULT.mass,
            inertia: None,
            front_axle: DEFAULT.front_axle,
            rear_axle: DEFAULT.rear_axle,
            engine: DEFAULT.engine,
            top_speed: DEFAULT.top_speed,
            brake: DEFAULT.brake,
            steer_lock: DEFAULT.steer_lock,
            grip: 1.0,
        }
    }
}

impl Handling {
    /// The record the asset carries, with inertia filled in if it was left out.
    ///
    /// A car's yaw inertia is roughly proportional to its mass for cars of similar shape, and the
    /// tuned car gives the constant. Guessing it from the mass is much better than leaving a
    /// 950 kg car with a 1420 kg car's reluctance to rotate, which reads as heavy steering that
    /// nothing in the config explains.
    pub fn resolve(&self) -> angle_zero::vehicle::CarHandling {
        angle_zero::vehicle::CarHandling {
            mass: self.mass,
            inertia: self
                .inertia
                .unwrap_or(self.mass * (DEFAULT.inertia / DEFAULT.mass)),
            front_axle: self.front_axle,
            rear_axle: self.rear_axle,
            engine: self.engine,
            top_speed: self.top_speed,
            brake: self.brake,
            steer_lock: self.steer_lock,
            grip: self.grip,
        }
    }
}

/// The car's lamps.
///
/// Most of this is measurable and therefore absent from most configs: which parts are lenses is
/// what the material categoriser already worked out, and where a lamp sits and how big it is are
/// read off that part's own geometry. What cannot be measured is what a lamp is *for* — a lens says
/// nothing about whether it comes on under braking — and how far a headlight is supposed to throw,
/// which is a decision about the game rather than a fact about the model.
///
/// The four named slots exist for the same reason the wheel corners do: detection is conservative
/// and refuses rather than guesses, so there has to be a way to say what it would not.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lights {
    /// Turn the whole thing off for a car that should have none.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Whether the headlights swing with the road wheels. Off by default: they are fixed units
    /// behind a fixed grille, and a car that is sliding should light where its nose points.
    #[serde(default)]
    pub steer: bool,
    /// How far a headlight throws, metres.
    #[serde(default = "default_range")]
    pub range: f32,
    /// Half-width of the beam where it lands, metres.
    #[serde(default = "default_spread")]
    pub spread: f32,

    #[serde(default)]
    pub headlight_left: Option<Anchor>,
    #[serde(default)]
    pub headlight_right: Option<Anchor>,
    #[serde(default)]
    pub tail_left: Option<Anchor>,
    #[serde(default)]
    pub tail_right: Option<Anchor>,
    #[serde(default)]
    pub brake_left: Option<Anchor>,
    #[serde(default)]
    pub brake_right: Option<Anchor>,
    #[serde(default)]
    pub reverse_left: Option<Anchor>,
    #[serde(default)]
    pub reverse_right: Option<Anchor>,
}

impl Default for Lights {
    fn default() -> Self {
        Lights {
            enabled: true,
            steer: false,
            range: default_range(),
            spread: default_spread(),
            headlight_left: None,
            headlight_right: None,
            tail_left: None,
            tail_right: None,
            brake_left: None,
            brake_right: None,
            reverse_left: None,
            reverse_right: None,
        }
    }
}

/// One lamp, said outright.
///
/// Every field is optional and overrides only itself, so a config can name the part a lamp belongs
/// to and still let its size be measured, or move a lamp two centimetres without describing it
/// again. An anchor with nothing in it means "there is a lamp in this slot" and nothing more, which
/// is enough to overrule a detection that refused.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    /// Node-name fragment for the lens, when the categoriser cannot tell which part it is.
    #[serde(default)]
    pub node: Option<String>,
    /// Where the lens is, in car space, with the wheels on the ground at y = 0. For a model with
    /// no lens geometry at all.
    #[serde(default)]
    pub at: Option<[f32; 3]>,
    /// Half-size of the lamp's glow, metres. Measured off the lens if left out.
    #[serde(default)]
    pub radius: Option<f32>,
    /// Linear RGB, 0 to 1.
    #[serde(default)]
    pub color: Option<[f32; 3]>,
    /// How bright the lamp is when it is fully on, 0 to 1.
    #[serde(default)]
    pub intensity: Option<f32>,
}

impl Lights {
    /// The eight slots, named as the config names them, in the order lamps are written.
    ///
    /// One list, so that the checker, the detector and the report cannot disagree about which slots
    /// exist — adding an indicator later is a line here and nothing else.
    pub fn slots(&self) -> [(&'static str, &Option<Anchor>); 8] {
        [
            ("headlight_left", &self.headlight_left),
            ("headlight_right", &self.headlight_right),
            ("tail_left", &self.tail_left),
            ("tail_right", &self.tail_right),
            ("brake_left", &self.brake_left),
            ("brake_right", &self.brake_right),
            ("reverse_left", &self.reverse_left),
            ("reverse_right", &self.reverse_right),
        ]
    }
}

/// How far a headlight throws, and how wide the light is where it lands.
///
/// Tuned against the chase camera rather than against a real headlight, because that is the only
/// place anybody looks at it from. At 24 m the lit patch stopped inside the length of road the
/// camera can see over the car's roof, which read as a bright smudge at the bumper rather than as
/// headlights; the light has to reach past what the eye is looking at. 40 m is about where the fog
/// starts taking it anyway.
fn default_range() -> f32 {
    40.0
}

fn default_spread() -> f32 {
    3.8
}

/// How the triangle budget is shared out.
///
/// The budget is spent in proportion to how much of each category the player can actually see,
/// which the converter measures. These multiply that measurement, for the cases where what is
/// worth spending on differs from what is large on screen: a headlight is a small number of pixels
/// and most of what makes a car recognisable, and a door card is a lot of pixels nobody looks at.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reduction {
    #[serde(default = "one")]
    pub body: f32,
    #[serde(default = "one")]
    pub window: f32,
    #[serde(default = "one")]
    pub tyre: f32,
    #[serde(default = "default_interior_weight")]
    pub interior: f32,
    #[serde(default = "default_light_weight")]
    pub light: f32,
    #[serde(default = "one")]
    pub chrome: f32,
    /// Extra weight for anything that belongs to a wheel, on top of its category's.
    ///
    /// Four wheels share one allocation while a body panel has one, so an unweighted split gives
    /// each wheel a quarter of what its importance deserves — and the plan puts wheels and tyres
    /// in the top group. A wheel that decimates to a wedge is one of the two or three things that
    /// most obviously says "cheap 3D".
    #[serde(default = "default_wheel_weight")]
    pub wheel: f32,
    /// Parts the visibility pass never saw are dropped outright. Turn this off to compile a car
    /// whole, which is the way to check what the pass is throwing away.
    #[serde(default = "yes")]
    pub drop_hidden: bool,

    /// Per-part weights, by node-name fragment: `"amdb11_brakedisc" = 0.3`.
    ///
    /// The category weights above decide how the budget is split *between* categories; this one
    /// splits it *within* one, and the two are not interchangeable. A wheel is the case that needs
    /// it: the alloy and the brake hardware behind it are both bright metal, so they land in the
    /// same category and then compete on how many pixels the visibility sweep saw — which the
    /// hardware wins, through the gaps between the spokes, at the expense of the wheel in front of
    /// it. Naming the two parts separately is the only way to say which of them the corner is
    /// actually for.
    ///
    /// Fragments are matched against the node and its parent, case-insensitively, and every match
    /// multiplies. A part nothing matches keeps its measured weight of 1.
    #[serde(default)]
    pub parts: std::collections::HashMap<String, f32>,

    /// Node-name fragments for parts to leave out of the car entirely.
    ///
    /// For geometry the visibility sweep can see but that is not worth a triangle — the sweep
    /// answers "is any of this on screen", not "is any of it worth drawing". The case that needs
    /// it is detail behind an opening: brake hardware behind the spokes of an alloy is visible
    /// through the gaps, so it is allocated a share of its bucket, and every triangle it takes
    /// comes out of the wheel in front of it. Dropped, the gaps read as gaps, which at the size a
    /// wheel is on screen is what they should look like anyway.
    #[serde(default)]
    pub drop: Vec<String>,
}

fn default_wheel_weight() -> f32 {
    4.0
}

impl Default for Reduction {
    fn default() -> Self {
        Reduction {
            body: 1.0,
            window: 1.0,
            tyre: 1.0,
            interior: default_interior_weight(),
            light: default_light_weight(),
            chrome: 1.0,
            wheel: default_wheel_weight(),
            drop_hidden: true,
            parts: std::collections::HashMap::new(),
            drop: Vec::new(),
        }
    }
}

impl Reduction {
    /// What a single part is worth, over and above its category.
    pub fn part_weight(&self, node: &str, parent: &str) -> f32 {
        let (node, parent) = (node.to_ascii_lowercase(), parent.to_ascii_lowercase());
        let mut w = 1.0;
        for (fragment, weight) in &self.parts {
            let f = fragment.to_ascii_lowercase();
            if !f.is_empty() && (node.contains(&f) || parent.contains(&f)) {
                w *= weight.max(0.0);
            }
        }
        w
    }

    pub fn weight(&self, category: angle_zero::azcar::Category) -> f32 {
        use angle_zero::azcar::Category::*;
        match category {
            Body => self.body,
            Window => self.window,
            Tyre => self.tyre,
            Interior => self.interior,
            Light => self.light,
            Chrome => self.chrome,
        }
        .max(0.0)
    }
}

/// Seen through glass, from outside, in the dark. It reads as shapes rather than as furniture.
fn default_interior_weight() -> f32 {
    0.4
}

/// Lamps, grille and badges: small on screen and most of what says which car this is.
fn default_light_weight() -> f32 {
    1.6
}

fn yes() -> bool {
    true
}

/// How to find the wheels.
///
/// Two ways, because models come both ways. `match` is for the common case where the four wheels
/// are named alike and told apart only by where they are; the four corner entries are for models
/// that name their corners, which the plan asks for and which is the only thing that works when a
/// car's wheels are not symmetric.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Wheels {
    /// Node-name fragments that mark a part as belonging to a wheel assembly. A wheel is usually
    /// three or four objects — tyre, rim, brake disc, caliper — and all of them have to turn
    /// together, so this matches parts, not whole wheels.
    #[serde(default, rename = "match")]
    pub patterns: Vec<String>,

    /// Overrides the measured rolling radius. Left out, it is measured off the tyres.
    #[serde(default)]
    pub radius: Option<f32>,

    #[serde(default)]
    pub front_left: Option<Corner>,
    #[serde(default)]
    pub front_right: Option<Corner>,
    #[serde(default)]
    pub rear_left: Option<Corner>,
    #[serde(default)]
    pub rear_right: Option<Corner>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Corner {
    /// Node-name fragment for this corner specifically.
    pub node: String,
}

/// Which of the six runtime categories a source material belongs to.
///
/// Matched against the material's name, case-insensitively, as a substring. The converter has
/// heuristics of its own — transparency, emissiveness, whether the part sits inside a wheel — and
/// these rules are consulted first, because a name a person wrote is better evidence than a
/// number a shader author left behind.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialRules {
    #[serde(default)]
    pub body: Vec<String>,
    #[serde(default)]
    pub window: Vec<String>,
    #[serde(default)]
    pub tyre: Vec<String>,
    #[serde(default)]
    pub interior: Vec<String>,
    #[serde(default)]
    pub light: Vec<String>,
    #[serde(default)]
    pub chrome: Vec<String>,
    /// Colours to use in place of what a material declares. See `ColourRule`.
    #[serde(default)]
    pub colour: Vec<ColourRule>,
}

/// A colour to use in place of the one a material declares.
///
/// The one thing a category cannot fix. A model is free to describe black plastic with a white base
/// colour and no texture, and several here do: the E36's front lip and rear valance are 360
/// triangles of `etki_modparts.001` at rgba(1,1,1,1) untextured, so they draw as a white strip
/// across the bottom of both bumpers. Nothing about that is a budget, a category or a UV — the
/// model simply says white, and whatever renderer it was authored for must have said otherwise.
///
/// Keyed by the same name fragments the category rules use, and applied after the texture is
/// sampled into the atlas, so a material that *does* carry a texture is left alone by default and
/// only overridden if it is named here.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColourRule {
    /// Name fragments this applies to, matched case-insensitively like the category rules.
    #[serde(rename = "match")]
    pub match_: Vec<String>,
    /// sRGB, 0–255.
    pub rgb: [u8; 3],
    /// Ignore whatever image the material brought, and let `rgb` be the whole answer.
    ///
    /// For a material whose texture is not a picture of the surface. The Golf's `material` — its
    /// grille bars, the strakes in the lower bumper and the ring round the badge — carries a
    /// matcap: three vertical strips of sky, horizon and sunset that a renderer is supposed to
    /// index by the surface normal to fake a reflection. Sampled as a flat texture it is a
    /// gradient, those panels sit a texel from where the horizon turns orange, and any change at
    /// all to how the image is resampled moves them across it. That is what turned the Golf's
    /// front end olive-yellow whenever the tile size moved, and it is why the gutter looked
    /// unaffordable for two sessions: the fault was never the gutter's 6%, it was that a matcap
    /// has no stable answer to sample.
    ///
    /// So the config says the model is wrong about the *texture* as well, in the same breath as
    /// the colour, because it is the same judgement: a person looking at the car can see that the
    /// image is not a picture of the thing it is on.
    #[serde(default)]
    pub flat: bool,
    /// Paint only the part of the material inside this box, and leave the rest alone.
    ///
    /// For the surface an exporter merged into something it is not. A material rule is the wrong
    /// grain when one material is two things — and a *part* rule would be no better, because the
    /// Golf's grille bars, bumper strakes, mirrors, window surrounds and the ring round the badge
    /// are all one 50,880-triangle part. There is nothing in the file to key on. There is only
    /// where they sit.
    ///
    /// In compiled car space: metres, Y up, Z forward, the wheels on the ground and the wheelbase
    /// centred — the same coordinates `azview --look` takes, so a box can be read off a render and
    /// checked by painting it something garish.
    #[serde(default)]
    pub inside: Option<Region>,
}

/// A box in compiled car space.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Region {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Region {
    pub fn contains(&self, p: [f32; 3]) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }
}

/// sRGB to linear, the inverse of `compile::srgb`, so a colour a config names survives the round
/// trip back out to the vertex it is baked into.
fn linear(v: f32) -> f32 {
    if v <= 0.040_45 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

impl MaterialRules {
    /// The replacement colour for a material, if the config named one, in the linear space a
    /// glTF base colour is in.
    ///
    /// Decoded from sRGB, because that is what the field says it is and what a person reading a
    /// colour off a picker will type. It was going in raw, which meant the number in a config was
    /// linear whatever the comment claimed, and the E36's black plastic trim at `[28, 28, 30]` was
    /// compiling to a mid grey around 96 — dark enough to fix the pale band it was written for,
    /// which is why nobody noticed, and about three times the colour that was asked for.
    pub fn colour_for(&self, name: &str) -> Option<[f32; 3]> {
        self.rule_for(name, false).map(ColourRule::linear_rgb)
    }

    /// The colour for the part of a material inside a box, and the box, if the config named one.
    ///
    /// Looked up separately from `colour_for` rather than found by the same search, so a material
    /// can carry both: one rule saying what it is, another saying what a corner of it is instead.
    pub fn region_for(&self, name: &str) -> Option<(&Region, [f32; 3])> {
        let rule = self.rule_for(name, true)?;
        rule.inside.as_ref().map(|r| (r, rule.linear_rgb()))
    }

    /// Whether the config says to throw this material's image away. See `ColourRule::flat`.
    ///
    /// Asked of the whole-material rule only. Whether an image is believed is a judgement about
    /// the image, and a box carves up a surface rather than the texture behind it.
    pub fn is_flat(&self, name: &str) -> bool {
        self.rule_for(name, false).is_some_and(|r| r.flat)
    }

    fn rule_for(&self, name: &str, regional: bool) -> Option<&ColourRule> {
        let name = name.to_ascii_lowercase();
        self.colour.iter().find(|rule| {
            rule.inside.is_some() == regional
                && rule
                    .match_
                    .iter()
                    .any(|f| !f.is_empty() && name.contains(&f.to_ascii_lowercase()))
        })
    }
}

impl ColourRule {
    fn linear_rgb(&self) -> [f32; 3] {
        self.rgb.map(|c| linear(c as f32 / 255.0))
    }
}

/// Where the car sits relative to the origin the game drives it by, after the converter has put
/// the wheels on the ground and centred the wheelbase. Rarely needed; there for the model whose
/// idea of straight ahead is not the game's.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spawn {
    #[serde(default)]
    pub offset_x: f32,
    #[serde(default)]
    pub offset_y: f32,
    #[serde(default)]
    pub offset_z: f32,
    /// Degrees about Y, for a model that faces -Z or sideways.
    #[serde(default)]
    pub yaw: f32,
}

fn one() -> f32 {
    1.0
}

fn default_triangles() -> usize {
    10_000
}

impl CarConfig {
    pub fn load(path: &Path) -> Result<CarConfig> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("could not read config {}: {e}", path.display()))?;
        let config: CarConfig = toml::from_str(&text)
            .map_err(|e| format!("invalid config {}: {e}", path.display()))?;
        config.check()?;
        Ok(config)
    }

    /// The config for a model with nothing said about it. Every car can be converted without one;
    /// what a config buys is the parts the converter would otherwise have to guess.
    pub fn unconfigured(name: &str) -> CarConfig {
        CarConfig {
            name: name.to_string(),
            scale: 1.0,
            triangles: default_triangles(),
            wheels: Wheels::default(),
            materials: MaterialRules::default(),
            spawn: Spawn::default(),
            reduce: Reduction::default(),
            handling: Handling::default(),
            lights: Lights::default(),
            lods: Vec::new(),
        }
    }

    fn check(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err("invalid car configuration: name is empty".into());
        }
        if !(self.scale > 0.0) || !self.scale.is_finite() {
            return Err(format!(
                "invalid car configuration: scale is {}, which is not a positive number",
                self.scale
            ));
        }
        if self.triangles < 100 {
            return Err(format!(
                "invalid car configuration: a budget of {} triangles is not a car",
                self.triangles
            ));
        }
        // A beam with no length is a beam that is drawn and cannot be seen, which looks exactly
        // like one that was never written.
        if self.lights.enabled && !(self.lights.range > 0.0 && self.lights.spread > 0.0) {
            return Err(format!(
                "invalid light configuration: a beam {} m long and {} m wide is not a beam",
                self.lights.range, self.lights.spread
            ));
        }
        for (slot, anchor) in self.lights.slots() {
            let Some(a) = anchor else { continue };
            if let Some(i) = a.intensity {
                if !(0.0..=1.0).contains(&i) {
                    return Err(format!(
                        "invalid light configuration: {slot} intensity is {i}, which is not \
                         between 0 and 1"
                    ));
                }
            }
            if let Some(r) = a.radius {
                if !(r > 0.0) {
                    return Err(format!(
                        "invalid light configuration: {slot} radius is {r}, which is not a \
                         positive number"
                    ));
                }
            }
        }
        // Corners are all-or-nothing: three named corners and a fourth left to be guessed would
        // put a wheel somewhere no one asked for.
        let named = [
            &self.wheels.front_left,
            &self.wheels.front_right,
            &self.wheels.rear_left,
            &self.wheels.rear_right,
        ]
        .iter()
        .filter(|c| c.is_some())
        .count();
        if named != 0 && named != 4 {
            return Err(format!(
                "invalid wheel configuration: {named} of 4 corners are named; name all four or none"
            ));
        }
        Ok(())
    }

    /// The corner node fragments, when the config names all four.
    pub fn named_corners(&self) -> Option<[&str; 4]> {
        Some([
            self.wheels.front_left.as_ref()?.node.as_str(),
            self.wheels.front_right.as_ref()?.node.as_str(),
            self.wheels.rear_left.as_ref()?.node.as_str(),
            self.wheels.rear_right.as_ref()?.node.as_str(),
        ])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<CarConfig> {
        let config: CarConfig = toml::from_str(text).map_err(|e| e.to_string())?;
        config.check()?;
        Ok(config)
    }

    #[test]
    fn a_config_can_be_just_a_name() {
        let c = parse(r#"name = "BMW E36""#).unwrap();
        assert_eq!(c.name, "BMW E36");
        assert_eq!(c.scale, 1.0);
        assert_eq!(c.triangles, 10_000);
        assert!(c.wheels.patterns.is_empty());
        assert!(c.named_corners().is_none());
    }

    #[test]
    fn corner_names_are_read_the_way_the_plan_writes_them() {
        let c = parse(
            r#"
            name = "Test"
            [wheels.front_left]
            node = "wheel_fl"
            [wheels.front_right]
            node = "wheel_fr"
            [wheels.rear_left]
            node = "wheel_rl"
            [wheels.rear_right]
            node = "wheel_rr"
            "#,
        )
        .unwrap();
        assert_eq!(
            c.named_corners().unwrap(),
            ["wheel_fl", "wheel_fr", "wheel_rl", "wheel_rr"]
        );
    }

    /// Half a wheel configuration is worse than none: the corners not named would be placed by a
    /// rule the other three were exempted from.
    #[test]
    fn naming_some_corners_but_not_all_is_an_error() {
        let err = parse(
            r#"
            name = "Test"
            [wheels.front_left]
            node = "wheel_fl"
            "#,
        )
        .unwrap_err();
        assert!(err.contains("name all four or none"), "{err}");
    }

    /// A minimal config still turns the lights on, because a car with lamps is the normal case and
    /// the numbers that cannot be measured have defaults that suit any car.
    #[test]
    fn lights_are_on_by_default_and_need_no_configuration() {
        let c = parse(r#"name = "BMW E36""#).unwrap();
        assert!(c.lights.enabled);
        assert!(!c.lights.steer, "headlights are bolted to the body unless asked");
        assert!(c.lights.range > 0.0 && c.lights.spread > 0.0);
        assert!(c.lights.slots().iter().all(|(_, a)| a.is_none()));
    }

    #[test]
    fn a_lamp_can_be_placed_and_coloured_outright() {
        let c = parse(
            r#"
            name = "Test"
            [lights]
            steer = true
            range = 30.0
            [lights.brake_left]
            at = [0.7, 1.0, -2.1]
            color = [1.0, 0.2, 0.15]
            intensity = 0.9
            radius = 0.3
            [lights.brake_right]
            node = "brake_r"
            "#,
        )
        .unwrap();
        assert!(c.lights.steer);
        assert_eq!(c.lights.range, 30.0);
        let left = c.lights.brake_left.as_ref().unwrap();
        assert_eq!(left.at, Some([0.7, 1.0, -2.1]));
        assert_eq!(left.intensity, Some(0.9));
        assert_eq!(c.lights.brake_right.as_ref().unwrap().node.as_deref(), Some("brake_r"));
    }

    /// Numbers that would produce a lamp nobody can see are refused here, where the message can say
    /// which slot, rather than on a handheld with no debugger.
    #[test]
    fn nonsense_lamp_numbers_are_refused() {
        let err = parse("name = \"T\"\n[lights]\nrange = 0.0").unwrap_err();
        assert!(err.contains("is not a beam"), "{err}");

        let err = parse(
            "name = \"T\"\n[lights.tail_left]\nintensity = 4.0",
        )
        .unwrap_err();
        assert!(err.contains("tail_left") && err.contains("between 0 and 1"), "{err}");

        let err = parse("name = \"T\"\n[lights.tail_left]\nradius = -1.0").unwrap_err();
        assert!(err.contains("radius"), "{err}");

        // And a beam that is not asked for is not checked: a car with its lights off is allowed to
        // leave nonsense in the table it is not using.
        assert!(parse("name = \"T\"\n[lights]\nenabled = false\nrange = 0.0").is_ok());
    }

    #[test]
    fn nonsense_numbers_are_refused() {
        assert!(parse("name = \"T\"\nscale = 0.0").is_err());
        assert!(parse("name = \"T\"\nscale = -1.0").is_err());
        assert!(parse("name = \"T\"\ntriangles = 12").is_err());
        assert!(parse("name = \"\"").is_err());
    }

    /// A misspelled key that is quietly ignored is a setting that silently does nothing, which on
    /// a converter means a car that is wrong for no visible reason.
    #[test]
    fn an_unknown_key_is_an_error_rather_than_ignored() {
        let err = parse("name = \"T\"\ntriangels = 9000").unwrap_err();
        assert!(err.contains("triangels"), "{err}");
    }
}
