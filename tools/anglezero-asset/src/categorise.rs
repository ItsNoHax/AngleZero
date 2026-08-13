//! Reducing dozens of source materials to the six the renderer knows.
//!
//! The E36 arrives with 57 materials. The console does not want 57 of anything: each one is a
//! state change and a draw call, and the renderer's whole material system is two questions —
//! blend or not, cull or not — because the vertex colours already carry both the paint and the
//! light. So materials are not translated, they are sorted into six bins, and everything in a bin
//! is drawn in one call.
//!
//! That merge is free of visual cost precisely because colour is per-vertex. Merging the black
//! trim into the same bin as the red paint does not make the trim red; it makes them one draw
//! call with two colours in it.
//!
//! Evidence is weighed in a fixed order: what the config says, then where the part is, then what
//! the material's own numbers say, then what it is called. The config wins because a person wrote
//! it about this model. Position beats naming because a part inside a wheel arch is a wheel part
//! whatever a shader author called it.

use angle_zero::azcar::Category;

use crate::config::{CarConfig, MaterialRules};
use crate::model::{Material, SourceModel};
use crate::wheels::Found;

/// Words that name a category across the models this has been pointed at. Sources come from
/// everywhere, so the German and Russian for the parts that matter are here too: the E36's rims
/// are `felgen` and its headlights are `fara`.
const WINDOW_WORDS: &[&str] = &["glass", "glas", "window", "windshield", "windscreen", "screen"];
const LIGHT_WORDS: &[&str] = &[
    "light", "lamp", "lens", "fara", "signal", "blinker", "leuchte", "indicator", "reflector",
];
const TYRE_WORDS: &[&str] = &["tyre", "tire", "rubber", "reifen", "tread"];
const CHROME_WORDS: &[&str] = &["chrom", "chrome", "felgen", "alloy", "rim", "mirror"];
const INTERIOR_WORDS: &[&str] = &[
    "interior", "int", "seat", "sitz", "dash", "carpet", "leather", "cabin", "gauge", "console",
    "steeringwheel", "headliner",
];

/// One decision per part, with the reason kept for the report.
pub struct Assignment {
    pub categories: Vec<Category>,
    /// Why each part landed where it did, for `--explain`.
    pub reasons: Vec<&'static str>,
}

pub fn assign(model: &SourceModel, config: &CarConfig, wheels: &Found) -> Assignment {
    let mut categories = Vec::with_capacity(model.parts.len());
    let mut reasons = Vec::with_capacity(model.parts.len());

    for (i, part) in model.parts.iter().enumerate() {
        let material = &model.materials[part.material];
        let (category, why) = decide(i, part.node.as_str(), material, config, wheels);
        categories.push(category);
        reasons.push(why);
    }

    Assignment {
        categories,
        reasons,
    }
}

fn decide(
    part: usize,
    node: &str,
    material: &Material,
    config: &CarConfig,
    wheels: &Found,
) -> (Category, &'static str) {
    if let Some(c) = from_config(&config.materials, &material.name, node) {
        return (c, "named in the config");
    }

    // Inside a wheel, the heaviest part is the tyre and everything else is hardware. Transparency
    // still wins — some models put a plastic wheel-arch liner in with the wheel.
    if let Some(w) = wheels.wheels.iter().find(|w| w.parts.contains(&part)) {
        if material.transparent {
            return (Category::Window, "transparent part of a wheel");
        }
        if w.tyre == Some(part) {
            return (Category::Tyre, "the largest part of a wheel");
        }
        return (Category::Chrome, "wheel hardware");
    }

    if material.transparent {
        return (Category::Window, "the material blends or is not opaque");
    }
    if material.emissive != [0.0; 3] {
        return (Category::Light, "the material is emissive");
    }

    // A lightmap is a baked texture, not a lamp, and the name is one of the commonest in any asset
    // that came out of a game engine. The VW's `Light_Map` is on its boot lid, its spoiler, its rear
    // bumper and both mirrors — 23,476 triangles of bodywork that were being given a lamp's share of
    // the triangle budget, and that later put the car's tail lights at bumper height, because a lamp
    // is placed where its lens is and this "lens" was the whole back of the car.
    if is_lightmap(&material.name) || is_lightmap(node) {
        return (Category::Body, "a lightmap texture, not a lamp");
    }

    for (words, category, why) in [
        (WINDOW_WORDS, Category::Window, "named like glass"),
        (LIGHT_WORDS, Category::Light, "named like a lamp"),
        (TYRE_WORDS, Category::Tyre, "named like a tyre"),
        (CHROME_WORDS, Category::Chrome, "named like brightwork"),
        (INTERIOR_WORDS, Category::Interior, "named like the cabin"),
    ] {
        if any_word(&material.name, words) || any_word(node, words) {
            return (category, why);
        }
    }

    // Bright, polished and metallic, with no name to go on. Anything darker than this is a painted
    // panel that happens to have been given a metallic workflow, which most car models do.
    if material.metallic > 0.7 && material.roughness < 0.25 && luma(&material.base_color) > 0.5 {
        return (Category::Chrome, "bright, smooth and metallic");
    }

    (Category::Body, "nothing said otherwise")
}

fn from_config(rules: &MaterialRules, material: &str, node: &str) -> Option<Category> {
    for (list, category) in [
        (&rules.window, Category::Window),
        (&rules.light, Category::Light),
        (&rules.tyre, Category::Tyre),
        (&rules.chrome, Category::Chrome),
        (&rules.interior, Category::Interior),
        (&rules.body, Category::Body),
    ] {
        for fragment in list {
            let f = fragment.to_ascii_lowercase();
            if !f.is_empty()
                && (material.to_ascii_lowercase().contains(&f)
                    || node.to_ascii_lowercase().contains(&f))
            {
                return Some(category);
            }
        }
    }
    None
}

/// Whether a name contains one of these words *as a word*.
///
/// Plain substring matching cannot be used here and the reason is instructive: `int` is a perfectly
/// good name for the interior material, and it is also inside `paint`. So a name is split on
/// anything that is not a letter or a digit, and a keyword has to be a whole token — except for
/// keywords of five letters or more, which are distinctive enough to match inside a token, which
/// is what catches `brakelight` and `headlight2`.
fn any_word(name: &str, words: &[&str]) -> bool {
    let lower = name.to_ascii_lowercase();
    let tokens: Vec<&str> = lower
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|t| !t.is_empty())
        .collect();
    words.iter().any(|w| {
        tokens
            .iter()
            .any(|t| t == w || (w.len() >= 5 && t.contains(w)))
    })
}

/// Whether a name is a lightmap rather than a light.
///
/// Matched on the joined tokens so that `Light_Map`, `lightMap` and `light map` all read the same,
/// which is the point: these are three spellings of one convention, and every one of them is a
/// texture that has nothing to do with lamps.
pub fn is_lightmap(name: &str) -> bool {
    let joined: String = name
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    ["lightmap", "lightmask", "lightbake", "bakedlight"]
        .iter()
        .any(|w| joined.contains(w))
}

fn luma(c: &[f32; 4]) -> f32 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material(name: &str) -> Material {
        Material {
            name: name.to_string(),
            base_color: [0.5, 0.5, 0.5, 1.0],
            metallic: 0.0,
            roughness: 1.0,
            emissive: [0.0; 3],
            image: None,
            double_sided: false,
            transparent: false,
        }
    }

    fn category(m: &Material, node: &str) -> Category {
        decide(
            usize::MAX,
            node,
            m,
            &CarConfig::unconfigured("T"),
            &Found::default(),
        )
        .0
    }

    /// The names this was written against, from the E36's own material list.
    #[test]
    fn the_e36s_material_names_land_where_they_should() {
        for (name, want) in [
            ("BMWE36_paint", Category::Body),
            ("BMWE36_glass", Category::Window),
            ("BMWE36_chrom", Category::Chrome),
            ("BMWE36_felgen", Category::Chrome),
            ("BMWE36_fara", Category::Light),
            ("BMWE36_brakelight", Category::Light),
            ("BMWE36_foglight", Category::Light),
            ("E36_lights.001", Category::Light),
            ("BMWE36_signal_L", Category::Light),
            ("BMWE36_seat_tex", Category::Interior),
            ("BMWE36_dash_tex", Category::Interior),
            ("BMWE36_carpet_tex", Category::Interior),
            ("bump_leather.002", Category::Interior),
            ("BMWE36_int", Category::Interior),
        ] {
            assert_eq!(category(&material(name), ""), want, "material `{name}`");
        }
    }

    /// The bug this rule exists to prevent: `int` is the interior material, and it is also inside
    /// `paint`, which is the largest surface on the car.
    #[test]
    fn paint_does_not_read_as_interior() {
        assert_eq!(category(&material("BMWE36_paint"), ""), Category::Body);
        assert_eq!(category(&material("paint.004"), ""), Category::Body);
        assert_eq!(category(&material("BMWE36_int"), ""), Category::Interior);
    }

    /// `Light_Map` is a baked texture and one of the commonest material names there is. Reading it
    /// as a lamp gave the VW's boot lid a lamp's share of the triangle budget, and then put its tail
    /// lights at bumper height, because a lamp goes where its lens is.
    #[test]
    fn a_lightmap_is_not_a_lamp() {
        for name in ["Light_Map", "lightMap", "Golf_LightMap_01", "light map", "Baked_Light"] {
            assert_eq!(category(&material(name), ""), Category::Body, "material `{name}`");
        }
        // And the lamps themselves still are. `light_glass` is not among them: glass is asked
        // about first, deliberately, so a lens named for both is a window here — `lamps.rs` is
        // where that one is picked up.
        for name in ["BMWE36_fara", "taillight", "rear_light", "TL_Lamp"] {
            assert_eq!(category(&material(name), ""), Category::Light, "material `{name}`");
        }
    }

    #[test]
    fn transparency_and_emission_are_believed_over_names() {
        let mut m = material("BMWE36_paint");
        m.transparent = true;
        assert_eq!(category(&m, ""), Category::Window);

        let mut m = material("some_panel");
        m.emissive = [1.0, 0.9, 0.7];
        assert_eq!(category(&m, ""), Category::Light);
    }

    /// The engine block: metallic and smooth, but black. Chrome is bright, and a car covered in
    /// dark metallic panels is just a car.
    #[test]
    fn dark_metal_is_not_chrome() {
        let mut m = material("BMWE36_metal");
        m.base_color = [0.01, 0.01, 0.01, 1.0];
        m.metallic = 1.0;
        m.roughness = 0.0;
        assert_eq!(category(&m, ""), Category::Body);

        m.base_color = [0.9, 0.9, 0.9, 1.0];
        assert_eq!(category(&m, ""), Category::Chrome);
    }

    #[test]
    fn the_config_overrules_everything_else() {
        let mut config = CarConfig::unconfigured("T");
        config.materials.tyre = vec!["Scene_-_Root".into()];
        let (c, why) = decide(
            usize::MAX,
            "Object_4.001",
            &material("Scene_-_Root.002"),
            &config,
            &Found::default(),
        );
        assert_eq!(c, Category::Tyre);
        assert_eq!(why, "named in the config");
    }

    /// A node name is as good as a material name — some models put all the meaning in one and
    /// none in the other.
    #[test]
    fn a_node_name_counts_when_the_material_says_nothing() {
        assert_eq!(
            category(&material("Scene_-_Root.002"), "front_windshield"),
            Category::Window
        );
    }
}
