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
        }
    }
}

impl Reduction {
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
