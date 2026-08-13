//! Finding the lamps, and working out what each one is for.
//!
//! Most of this question is already answered by the time it is asked. Sorting materials has put
//! every lens in [`Category::Light`], and placement has put the car nose-first at the origin with
//! its wheels on the ground — so a lamp's position, its size and which end of the car it is on are
//! all measurable, and none of them need naming.
//!
//! What is left is what the lens is *for*. A model does not say whether a red lamp comes on under
//! braking, and no amount of geometry will tell you: the E36's rear cluster is one lens doing three
//! jobs. So the kind is taken from the part's name where the name is clear, from which end of the
//! car it is on where it is not, and from the config where neither will do.
//!
//! The rule throughout is the one wheel identification uses: **no lamp rather than the wrong lamp.**
//! A car whose brake lights are an arbitrary mesh is worse than a car with none — the wrong thing
//! lights up under braking and the model looks broken in a way nobody can explain.

use angle_zero::azcar::{Category, LightDef, LightKind, LIGHT_STEERS};

use crate::categorise::Assignment;
use crate::config::{Anchor, CarConfig};
use crate::mat::Bounds;
use crate::model::SourceModel;
use crate::wheels::Found as Wheels;

/// The eight slots a car can fill, in the order they are written.
const SLOTS: [(LightKind, Side); 8] = [
    (LightKind::Head, Side::Left),
    (LightKind::Head, Side::Right),
    (LightKind::Tail, Side::Left),
    (LightKind::Tail, Side::Right),
    (LightKind::Brake, Side::Left),
    (LightKind::Brake, Side::Right),
    (LightKind::Reverse, Side::Left),
    (LightKind::Reverse, Side::Right),
];

/// The four kinds, for the passes that walk them all.
const KINDS: [LightKind; 4] = [
    LightKind::Head,
    LightKind::Tail,
    LightKind::Brake,
    LightKind::Reverse,
];

/// The car faces +Z with +Y up, which in a right-handed frame puts +X on its left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    fn of(x: f32) -> Side {
        if x >= 0.0 {
            Side::Left
        } else {
            Side::Right
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
        }
    }
}

/// How much larger a lamp's glow is than the lens it comes out of.
const GLOW_OVER_LENS: f32 = 1.8;

/// Words that name what a lamp is for, across the models this has been pointed at.
///
/// Ordered by how specific they are, and tested in that order: `brakelight` contains `light`, and
/// `reverse_light` contains both — so the job words have to be asked about before the generic ones,
/// or every lamp on the car is a tail light.
const REVERSE_WORDS: &[&str] = &["reverse", "reversing", "backup", "rueckfahr"];
const BRAKE_WORDS: &[&str] = &["brake", "brakelight", "stop", "stoplight"];
const TAIL_WORDS: &[&str] = &["tail", "taillight", "rear", "rearlight", "rueck"];
const HEAD_WORDS: &[&str] = &["head", "headlight", "headlamp", "fara", "front"];

/// One lens, as the model has it.
struct Candidate {
    /// Node, parent and material names, lowercased and run together — everything a config fragment
    /// could reasonably be pointing at.
    names: String,
    /// Centre of the lens in car space, after placement.
    at: [f32; 3],
    /// Half the larger of the lens's width and height: the size its glow should be.
    radius: f32,
    /// What its name says it is for, if anything.
    named: Option<LightKind>,
    /// Whether this half of the lens sits *on* the car's centreline rather than at a corner.
    ///
    /// This is not a detail. A car's rear cluster is a pair, but the strip across the boot lid and
    /// the high-level lamp in the back window are single lamps in the middle — and a rule that
    /// splits the world into left and right hands one of them arbitrarily to whichever side the
    /// centring left it a millimetre on. That is exactly the "random mesh as a brake light" this
    /// module exists to refuse.
    ///
    /// Asked of the half rather than of the whole part it came from, which sounds like the weaker
    /// test and is the right one. A centre lamp is narrow, so both of its halves are still within a
    /// few centimetres of the axis — the E36's boot-lid strip cuts into halves centred at +0.10 and
    /// -0.09. Asking the *part* whether it has any glass near the axis catches that too, and also
    /// catches every merged mesh that happens to contain a lamp near the middle: the R34 models
    /// every lamp it has as one object, so that test threw away all four of them.
    centred: bool,
}

/// How far from the middle a lens has to be before it counts as belonging to a side: a twelfth of
/// the car's width, which is a few centimetres on a saloon.
fn centre_band(bounds: &Bounds) -> f32 {
    (bounds.max[0] - bounds.min[0]).abs() / 12.0
}

#[derive(Default)]
pub struct Found {
    pub lights: Vec<LightDef>,
    pub warnings: Vec<String>,
    /// Which slots were filled, for the report. `(kind, side, how)`.
    pub filled: Vec<(LightKind, Side, &'static str)>,
}

/// Works out what lamps the car has.
///
/// Runs after placement and after material categorisation, and needs both: the positions have to be
/// the ones the car will be driven in, and the lens parts have to have been identified already.
pub fn identify(
    model: &SourceModel,
    config: &CarConfig,
    assignment: &Assignment,
    wheels: &Wheels,
    strings: &mut crate::compile::Strings,
) -> Found {
    let mut found = Found::default();
    if !config.lights.enabled {
        return found;
    }

    let bounds = car_bounds(model);
    let named: Vec<&str> = config
        .lights
        .slots()
        .iter()
        .filter_map(|(_, a)| a.as_ref().and_then(|a| a.node.as_deref()))
        .collect();
    let candidates = candidates(model, assignment, wheels, &bounds, &named);
    // A car whose lenses are all one part — one rear cluster mesh spanning the whole tail — cannot
    // be split into a left and a right lamp by looking at parts. Saying so is worth more than
    // silently fitting one lamp down the middle of the car.
    if candidates.is_empty() && !config.lights.slots().iter().any(|(_, a)| a.is_some()) {
        found.warnings.push(
            "no lamp lenses found: nothing was sorted into the light category, so the car has no \
             lights. Name them in [materials] light, or place them in [lights]"
                .into(),
        );
        return found;
    }

    for (kind, side) in SLOTS {
        let anchor = slot_config(config, kind, side);
        match resolve(&candidates, kind, side, anchor, config, strings) {
            Ok(Some((light, how))) => {
                found.lights.push(light);
                found.filled.push((kind, side, how));
            }
            Ok(None) => {}
            Err(why) => found.warnings.push(why),
        }
    }

    // A lens in the middle of the car is a real lamp that this has no slot for — the high-level
    // brake lamp, most often. Said once, by name, because the alternative is a car that visibly has
    // one and an asset that says nothing about why it is dark.
    for c in candidates.iter().filter(|c| c.centred) {
        if let Some(kind) = c.named {
            found.warnings.push(format!(
                "the {} lens `{}` sits on the car's centreline, and a lamp here belongs to a side, \
                 so it is left off",
                kind.name(),
                c.names.split_whitespace().next().unwrap_or("?"),
            ));
        }
    }

    // Lamps come in pairs. One of a pair means the detector matched something on one side of the
    // car and not its reflection, which is far more likely to be a lens the categoriser missed than
    // a car with one headlight — so the odd one out goes, and the config is told how to say
    // otherwise. This is the same rule that leaves all four wheels in the body when only three
    // corners can be found.
    prune_asymmetric(&mut found, config, &bounds);
    prune_singletons(&mut found, config);
    // One lens is two candidates now that parts are cut in quarters, so a lens that earns a warning
    // earns it twice.
    found.warnings.dedup();
    found
}

/// Every part that is a lamp lens, with what can be measured off it.
///
/// Each part is cut into quadrants first, and that is not a refinement — it is what makes detection
/// work at all. Five of the seven cars here model several lamps as *one* mesh:
/// `ae86-body_taillights_0` is both rear lenses, the E39's tail and reverse lamps are one part each
/// spanning the whole back of the car, and the R34's single light mesh holds every lamp it has,
/// front and rear together. Measured whole, all of them have their centre in the middle of the car,
/// where no lamp is — so a rule that reads part centres finds a row of lamps down the car's axis
/// and then correctly refuses every one of them.
///
/// The cut is about the car's own origin, which placement has already put on the centreline between
/// the axles — so the four quadrants are the four corners a lamp can be at.
///
/// Cutting is cheap here in a way it is not for wheels, which have to be cut as *geometry* so that
/// each corner can be turned. A lamp needs a position and a size, so the vertices are read twice
/// and nothing is copied or moved. `wheels::split_merged_wheels` is the same idea at a much higher
/// price.
fn candidates(
    model: &SourceModel,
    assignment: &Assignment,
    wheels: &Wheels,
    bounds: &Bounds,
    named: &[&str],
) -> Vec<Candidate> {
    let band = centre_band(bounds);
    let mut out = Vec::new();
    for (i, part) in model.parts.iter().enumerate() {
        // A lens bolted to a wheel is a reflector on a hubcap, not a lamp. It also spins.
        if wheels.corner_of(i).is_some() {
            continue;
        }
        let material = &model.materials[part.material].name;
        let names = format!("{} {} {}", part.node, part.parent, material).to_ascii_lowercase();
        // Sorted as a lens, or named in the config as one. The second is the escape hatch that
        // makes the first overrulable, and it has to reach parts of any category to be worth
        // anything: the 190E's headlights are `HL_Glass_*`, which are transparent, which makes them
        // glass — a perfectly reasonable answer that leaves the car with no headlights.
        let is_lens = assignment.categories[i] == Category::Light
            || named
                .iter()
                .any(|f| !f.is_empty() && names.contains(&f.to_ascii_lowercase()));
        if !is_lens {
            continue;
        }
        let named = named_kind(&part.node, &part.parent, material);

        for side in [Side::Left, Side::Right] {
            for front in [true, false] {
                let mut half = Bounds::EMPTY;
                for p in part
                    .positions
                    .iter()
                    .filter(|p| Side::of(p[0]) == side && (p[2] > 0.0) == front)
                {
                    half.add(*p);
                }
                let size = half.size();
                if size == [0.0; 3] {
                    continue;
                }
                let at = [
                    (half.min[0] + half.max[0]) * 0.5,
                    (half.min[1] + half.max[1]) * 0.5,
                    (half.min[2] + half.max[2]) * 0.5,
                ];
                out.push(Candidate {
                    names: names.clone(),
                    at,
                    // The glow stands in for the lens, so it is the size of the lens as seen from
                    // in front: its width or its height, whichever is larger, halved.
                    radius: size[0].max(size[1]) * 0.5,
                    named,
                    centred: at[0].abs() < band,
                });
            }
        }
    }
    out
}

/// What a part's names say it is for, or `None` if they say nothing that specific.
fn named_kind(node: &str, parent: &str, material: &str) -> Option<LightKind> {
    for (words, kind) in [
        (REVERSE_WORDS, LightKind::Reverse),
        (BRAKE_WORDS, LightKind::Brake),
        (TAIL_WORDS, LightKind::Tail),
        (HEAD_WORDS, LightKind::Head),
    ] {
        for name in [node, parent, material] {
            if any_word(name, words) {
                return Some(kind);
            }
        }
    }
    None
}

/// Whether a name contains one of these words as a word.
///
/// The same rule the material categoriser uses, and it is here for the same reason: `rear` is
/// inside `rearview`, and a wing mirror is not a tail light. Words of five letters or more are
/// distinctive enough to match inside a token, which is what catches `brakelight_l`.
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

/// Fills one slot, or explains why it could not be.
#[allow(clippy::too_many_arguments)]
fn resolve(
    candidates: &[Candidate],
    kind: LightKind,
    side: Side,
    anchor: Option<&Anchor>,
    config: &CarConfig,
    strings: &mut crate::compile::Strings,
) -> Result<Option<(LightDef, &'static str)>, String> {
    // An explicit position needs no part at all, which is the escape hatch for a model whose lamps
    // are painted on rather than modelled.
    if let Some(a) = anchor {
        if let Some(at) = a.at {
            return Ok(Some((
                build(kind, side, at, 0.28, a, config, strings),
                "placed by the config",
            )));
        }
    }

    // Which parts could be this lamp. A named part narrows it to that part; otherwise every lens on
    // the right end of the car and the right side of it is a candidate.
    let front = matches!(kind, LightKind::Head);
    // The car's own origin, which is where the candidates were cut, and which placement has put
    // between the axles. Not the middle of the bounding box: a car with a long boot and a short
    // bonnet has one several centimetres behind the other, and lamps would be sorted about a line
    // that no part of the pipeline agrees is the middle of anything.
    let middle_z = 0.0;
    let mut matched: Vec<&Candidate> = candidates
        .iter()
        .filter(|c| {
            if let Some(fragment) = anchor.and_then(|a| a.node.as_deref()) {
                // Still this side of the car: a fragment names which lens, and the quadrant cut
                // has already said which half of it belongs to which lamp.
                return matches_fragment(c, fragment) && Side::of(c.at[0]) == side;
            }
            !c.centred
                && Side::of(c.at[0]) == side
                && (c.at[2] > middle_z) == front
        })
        .collect();

    if matched.is_empty() {
        // Only a slot the config asked for is worth complaining about. Every car has slots it does
        // not fill — most have no separate reverse lens — and warning about each of them would bury
        // the one warning that matters.
        return match anchor {
            Some(a) => Err(format!(
                "no lens matches the {} {} light{}; it is left off the car",
                side.name(),
                kind.name(),
                a.node
                    .as_deref()
                    .map(|n| format!(" (`{n}`)"))
                    .unwrap_or_default(),
            )),
            None => Ok(None),
        };
    }

    // Of those, the ones whose names say what they are for. A name is better evidence than a
    // position — a brake lens and a tail lens sit in the same cluster — so if any part on this side
    // names this kind, the unnamed ones are not it.
    let named: Vec<&Candidate> = matched
        .iter()
        .copied()
        .filter(|c| c.named == Some(kind))
        .collect();
    let how = if anchor.and_then(|a| a.node.as_deref()).is_some() {
        "named in the config"
    } else if !named.is_empty() {
        matched = named;
        "named in the model"
    } else {
        // Nothing here is named for this kind. A separate brake or reverse lens is only ever
        // identified by its name: guessing that some part of a rear cluster is the reverse lamp is
        // exactly the "never silently assign a random mesh" case.
        if matches!(kind, LightKind::Brake | LightKind::Reverse) && anchor.is_none() {
            return Ok(None);
        }
        // A part named for a *different* kind is not this one either. Left in, the tail lens would
        // be averaged into the headlight.
        matched.retain(|c| c.named.is_none() || c.named == Some(kind));
        if matched.is_empty() {
            return Ok(None);
        }
        "found by where it sits"
    };

    // One lamp per slot, out of however many parts a cluster is modelled as: the largest lens, and
    // not the middle of all of them.
    //
    // The E36 is why. Its front corner is a headlight lens, a pair of indicator lenses and a
    // foglight in the bumper below, and averaging the four puts the "headlight" 10 cm too low and
    // 6 cm too far back — between the lamps rather than on one. The biggest lens is the one that
    // reads as the lamp from any distance the car is seen at, and it is where the light comes from.
    let lens = matched
        .iter()
        .max_by(|a, b| a.radius.total_cmp(&b.radius))
        .expect("matched is not empty");
    let (at, radius) = (lens.at, lens.radius);

    // A headlight behind the middle of the car is not a headlight. The usual cause is a model that
    // faces the wrong way, which `[spawn] yaw = 180` fixes — and which is otherwise invisible until
    // the car is driven down the hill backwards.
    if front && at[2] < middle_z {
        return Err(format!(
            "the {} headlight came out at z = {:.2}, which is the back of the car — the model may \
             face the wrong way, and `[spawn] yaw = 180` is the fix",
            side.name(),
            at[2]
        ));
    }

    Ok(Some((
        build(kind, side, at, radius, anchor.unwrap_or(&NO_ANCHOR), config, strings),
        how,
    )))
}

static NO_ANCHOR: Anchor = Anchor {
    node: None,
    at: None,
    radius: None,
    color: None,
    intensity: None,
};

/// Case-insensitive substring match against the part's names, which is what a config author means
/// by a node fragment — the same rule `[wheels] match` follows.
fn matches_fragment(c: &Candidate, fragment: &str) -> bool {
    !fragment.is_empty() && c.names.contains(&fragment.to_ascii_lowercase())
}

/// Assembles the record, with the config overriding whatever was measured.
fn build(
    kind: LightKind,
    side: Side,
    at: [f32; 3],
    radius: f32,
    anchor: &Anchor,
    config: &CarConfig,
    strings: &mut crate::compile::Strings,
) -> LightDef {
    let color = anchor.color.unwrap_or(default_color(kind));
    let intensity = anchor.intensity.unwrap_or(default_intensity(kind));
    // What is drawn is the bloom around the lens rather than the lens itself, so it is larger than
    // the glass — a lamp glowing exactly to the edge of its own lens and no further reads as a
    // sticker. And a measured size can be silly in both directions: a rear cluster modelled as one
    // strip across the boot is half a metre wide, and a lens modelled as a decal has no size at all.
    let radius = anchor
        .radius
        .unwrap_or_else(|| (radius * GLOW_OVER_LENS).clamp(0.18, 0.55));
    let beam = matches!(kind, LightKind::Head);

    LightDef {
        kind,
        flags: if beam && config.lights.steer {
            LIGHT_STEERS
        } else {
            0
        },
        name: strings.push(&format!("{}_{}", kind_tag(kind), side.name())),
        at,
        color: pack(color, intensity),
        radius,
        range: if beam { config.lights.range } else { 0.0 },
        spread: if beam { config.lights.spread } else { 0.0 },
    }
}

fn kind_tag(kind: LightKind) -> &'static str {
    match kind {
        LightKind::Head => "headlight",
        LightKind::Tail => "tail",
        LightKind::Brake => "brake",
        LightKind::Reverse => "reverse",
    }
}

/// What colour a lamp burns, when the config does not say.
///
/// Not taken from the lens material, deliberately. A lens in a scanned model is dark red glass or
/// smoked plastic — that is what it looks like switched *off*, which is the state the model was
/// built in — so using it would give a car whose brake lights come on nearly black. What a lamp
/// emits and what its lens looks like in daylight are different things, and only one of them is in
/// the file.
fn default_color(kind: LightKind) -> [f32; 3] {
    match kind {
        // Slightly warm, as a halogen headlamp is.
        LightKind::Head => [1.0, 0.95, 0.82],
        LightKind::Tail | LightKind::Brake => [1.0, 0.33, 0.27],
        LightKind::Reverse => [1.0, 1.0, 0.94],
    }
}

/// How bright a lamp is when it is fully on.
///
/// A headlight glow is much dimmer than a brake light and that is not a mistake: it is a bright
/// source pointing away from a camera that sits behind the car, so what is seen of it is spill.
/// The brake lamps point straight at the camera, and are the one thing on a car ahead that has to
/// carry at a hundred metres.
fn default_intensity(kind: LightKind) -> f32 {
    match kind {
        LightKind::Head => 0.30,
        LightKind::Tail | LightKind::Brake => 0.95,
        LightKind::Reverse => 0.70,
    }
}

/// Linear RGB and a brightness into the `GU_COLOR_8888` the record carries.
fn pack(color: [f32; 3], intensity: f32) -> u32 {
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    let a = ch(intensity);
    // ABGR, which is the order the PSP reads a colour in.
    (a << 24) | (ch(color[2]) << 16) | (ch(color[1]) << 8) | ch(color[0])
}

fn slot_config(config: &CarConfig, kind: LightKind, side: Side) -> Option<&Anchor> {
    let l = &config.lights;
    match (kind, side) {
        (LightKind::Head, Side::Left) => l.headlight_left.as_ref(),
        (LightKind::Head, Side::Right) => l.headlight_right.as_ref(),
        (LightKind::Tail, Side::Left) => l.tail_left.as_ref(),
        (LightKind::Tail, Side::Right) => l.tail_right.as_ref(),
        (LightKind::Brake, Side::Left) => l.brake_left.as_ref(),
        (LightKind::Brake, Side::Right) => l.brake_right.as_ref(),
        (LightKind::Reverse, Side::Left) => l.reverse_left.as_ref(),
        (LightKind::Reverse, Side::Right) => l.reverse_right.as_ref(),
    }
}

/// Drops a pair whose two halves are not each other's reflection.
///
/// Lamps are the most symmetric things on a car after the wheels, so a pair that is not a mirror
/// image is not a pair — it is one real lamp and one part that happened to be the largest lens in
/// the opposite quadrant. The 190E is the case: its left "headlight" came out at z = 0.30, the
/// middle of the car, against a right one at z = 1.35, because the largest front-left lens in that
/// model is a side repeater on the wing.
///
/// Dropping both is right even though one of them was correct. Which one was correct is exactly
/// what cannot be known here, and a car with one headlight on the wing is a worse answer than a car
/// whose config has to name them.
fn prune_asymmetric(found: &mut Found, config: &CarConfig, bounds: &Bounds) {
    // A tenth of the car's length. Real pairs are within a centimetre or two; this is loose enough
    // for a model whose two lenses were built independently and never quite matched.
    let tolerance = (bounds.max[2] - bounds.min[2]).abs() * 0.1;
    for kind in KINDS {
        let pair: Vec<LightDef> = found
            .lights
            .iter()
            .copied()
            .filter(|l| l.kind == kind)
            .collect();
        let [a, b] = pair[..] else { continue };
        let mirrored = (a.at[0] + b.at[0]).abs() < tolerance
            && (a.at[1] - b.at[1]).abs() < tolerance
            && (a.at[2] - b.at[2]).abs() < tolerance;
        if mirrored || config_named_both(config, kind) {
            continue;
        }
        found.warnings.push(format!(
            "the two {}s came out at ({:.2}, {:.2}, {:.2}) and ({:.2}, {:.2}, {:.2}), which are not \
             a mirrored pair, so neither is trusted — name them in [lights]",
            kind.name(),
            a.at[0], a.at[1], a.at[2],
            b.at[0], b.at[1], b.at[2],
        ));
        found.lights.retain(|l| l.kind != kind);
        found.filled.retain(|(k, _, _)| *k != kind);
    }
}

fn config_named_both(config: &CarConfig, kind: LightKind) -> bool {
    slot_config(config, kind, Side::Left).is_some() && slot_config(config, kind, Side::Right).is_some()
}

/// Drops any lamp whose opposite number is missing, unless the config asked for it by name.
fn prune_singletons(found: &mut Found, config: &CarConfig) {
    for kind in KINDS {
        let sides: Vec<Side> = found
            .filled
            .iter()
            .filter(|(k, _, _)| *k == kind)
            .map(|(_, s, _)| *s)
            .collect();
        if sides.len() != 1 {
            continue;
        }
        let lone = sides[0];
        if slot_config(config, kind, lone).is_some() {
            continue;
        }
        found.warnings.push(format!(
            "only the {} {} was found, and lamps come in pairs, so it is left off — configure both \
             in [lights] if the car really has one",
            lone.name(),
            kind.name(),
        ));
        found.lights.retain(|l| l.kind != kind);
        found.filled.retain(|(k, _, _)| *k != kind);
    }
}

fn car_bounds(model: &SourceModel) -> Bounds {
    let mut b = Bounds::EMPTY;
    for part in &model.parts {
        let pb = part.bounds();
        b.add(pb.min);
        b.add(pb.max);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile::Strings;
    use crate::model::Material;
    use crate::wheels::tests::box_part;

    /// A material named however the caller likes, and emissive so the categoriser calls it a lamp
    /// without being told to — which is how a real model announces a lens.
    fn lens_material(name: &str) -> Material {
        Material {
            name: name.to_string(),
            base_color: [0.6, 0.1, 0.1, 1.0],
            metallic: 0.0,
            roughness: 1.0,
            emissive: [1.0, 0.4, 0.3],
            image: None,
            double_sided: false,
            transparent: false,
        }
    }

    fn plain_material() -> Material {
        Material {
            name: "paint".into(),
            base_color: [0.4, 0.4, 0.45, 1.0],
            metallic: 0.0,
            roughness: 1.0,
            emissive: [0.0; 3],
            image: None,
            double_sided: false,
            transparent: false,
        }
    }

    /// A car in the space the detector sees it in: already placed, so the origin is on the ground
    /// between the axles and the nose is at +Z.
    fn car(lenses: &[(&str, [f32; 3], [f32; 3])]) -> SourceModel {
        let mut parts = vec![box_part("shell", [0.0, 0.7, 0.0], [1.7, 1.2, 4.2], 2)];
        let mut materials = vec![plain_material()];
        for (name, at, size) in lenses {
            let mut p = box_part(name, *at, *size, 1);
            p.material = materials.len();
            materials.push(lens_material(name));
            parts.push(p);
        }
        SourceModel {
            source: "test".into(),
            credit: Default::default(),
            parts,
            materials,
            images: Vec::new(),
        }
    }

    fn find(model: &SourceModel, config: &CarConfig) -> Found {
        let wheels = Wheels::default();
        let assignment = crate::categorise::assign(model, config, &wheels);
        identify(model, config, &assignment, &wheels, &mut Strings::default())
    }

    /// The E36's shape of model: a lens part per corner, named for what it is.
    fn four_corner_car() -> SourceModel {
        car(&[
            ("headlight_L", [0.6, 0.7, 1.9], [0.3, 0.2, 0.1]),
            ("headlight_R", [-0.6, 0.7, 1.9], [0.3, 0.2, 0.1]),
            ("taillight_L", [0.6, 0.9, -2.0], [0.3, 0.25, 0.1]),
            ("taillight_R", [-0.6, 0.9, -2.0], [0.3, 0.25, 0.1]),
        ])
    }

    fn only(found: &Found, kind: LightKind) -> Vec<LightDef> {
        found.lights.iter().copied().filter(|l| l.kind == kind).collect()
    }

    #[test]
    fn a_lens_at_each_corner_becomes_a_lamp_at_each_corner() {
        let model = four_corner_car();
        let found = find(&model, &CarConfig::unconfigured("Test"));
        assert!(found.warnings.is_empty(), "{:?}", found.warnings);

        assert_eq!(only(&found, LightKind::Head).len(), 2);
        assert_eq!(only(&found, LightKind::Tail).len(), 2);
        // Nothing named a brake or a reverse lens, so the car has neither. Guessing one out of the
        // rear cluster is the mistake this whole module exists to avoid.
        assert_eq!(only(&found, LightKind::Brake).len(), 0);
        assert_eq!(only(&found, LightKind::Reverse).len(), 0);

        for l in only(&found, LightKind::Head) {
            assert!(l.at[2] > 1.0, "a headlight is at the front: {:?}", l.at);
            assert!(l.range > 0.0, "and throws a beam");
        }
        for l in only(&found, LightKind::Tail) {
            assert!(l.at[2] < -1.0, "a tail light is at the back: {:?}", l.at);
            assert_eq!(l.range, 0.0, "and throws none");
        }
        let heads = only(&found, LightKind::Head);
        assert!((heads[0].at[0] + heads[1].at[0]).abs() < 1e-5, "a mirrored pair");
    }

    /// The AE86, the R34 and four others: every lamp on the car is one mesh, so measuring parts
    /// whole puts every lamp on the centreline and finds nothing at all.
    #[test]
    fn lamps_merged_into_one_mesh_are_still_found_at_the_corners() {
        // One part covering all four corners, built by merging four boxes.
        let mut merged = box_part("lights_all", [0.6, 0.7, 1.9], [0.3, 0.2, 0.1], 1);
        for at in [[-0.6f32, 0.7, 1.9], [0.6, 0.9, -2.0], [-0.6, 0.9, -2.0]] {
            let other = box_part("lights_all", at, [0.3, 0.2, 0.1], 1);
            let base = merged.positions.len() as u32;
            merged.positions.extend_from_slice(&other.positions);
            merged.normals.extend_from_slice(&other.normals);
            merged.indices.extend(other.indices.iter().map(|i| i + base));
        }
        merged.material = 1;

        let mut model = car(&[("unused", [0.0, 9.0, 0.0], [0.1, 0.1, 0.1])]);
        model.parts.pop();
        model.parts.push(merged);

        let found = find(&model, &CarConfig::unconfigured("Test"));
        assert_eq!(only(&found, LightKind::Head).len(), 2, "{:?}", found.warnings);
        assert_eq!(only(&found, LightKind::Tail).len(), 2);
        for l in &found.lights {
            assert!(l.at[0].abs() > 0.4, "a lamp landed on the centreline: {:?}", l.at);
        }
    }

    /// The E36's high-level brake lamp. A single lamp in the middle belongs to no side, and handing
    /// it to whichever one the centring left it a millimetre on is exactly the wrong answer.
    #[test]
    fn a_lens_on_the_centreline_is_named_rather_than_given_to_a_side() {
        let mut model = four_corner_car();
        let mut chmsl = box_part("chmsl_brakelight", [0.0, 1.1, -1.6], [0.4, 0.06, 0.05], 2);
        chmsl.material = model.materials.len();
        model.materials.push(lens_material("brakelight"));
        model.parts.push(chmsl);

        let found = find(&model, &CarConfig::unconfigured("Test"));
        assert_eq!(only(&found, LightKind::Brake).len(), 0);
        assert!(
            found.warnings.iter().any(|w| w.contains("centreline") && w.contains("chmsl")),
            "{:?}",
            found.warnings
        );
        // Said once, not once per half.
        assert_eq!(
            found.warnings.iter().filter(|w| w.contains("centreline")).count(),
            1
        );
    }

    /// The 190E: the largest front-left lens in that model is a repeater on the wing, so the pair
    /// comes out crooked. Neither half is trusted, because which one was right is unknowable here.
    #[test]
    fn a_pair_that_is_not_a_mirror_image_is_not_a_pair() {
        let model = car(&[
            ("headlight_L", [0.3, 0.9, 0.3], [0.2, 0.2, 0.1]),
            ("headlight_R", [-0.6, 0.6, 1.9], [0.3, 0.2, 0.1]),
        ]);
        let found = find(&model, &CarConfig::unconfigured("Test"));
        assert_eq!(only(&found, LightKind::Head).len(), 0);
        assert!(
            found.warnings.iter().any(|w| w.contains("not a mirrored pair")),
            "{:?}",
            found.warnings
        );
    }

    /// One of a pair is a lens the categoriser missed on the other side, far more often than it is
    /// a car with one headlight.
    #[test]
    fn one_lamp_of_a_pair_is_left_off() {
        let model = car(&[("headlight_L", [0.6, 0.7, 1.9], [0.3, 0.2, 0.1])]);
        let found = find(&model, &CarConfig::unconfigured("Test"));
        assert_eq!(only(&found, LightKind::Head).len(), 0);
        assert!(
            found.warnings.iter().any(|w| w.contains("lamps come in pairs")),
            "{:?}",
            found.warnings
        );
    }

    /// A separate brake or reverse lens is only ever identified by name. Nothing about a rear
    /// cluster's geometry says which part of it comes on under braking.
    #[test]
    fn brake_and_reverse_lenses_are_never_guessed_from_position() {
        let model = car(&[
            ("cluster_L", [0.6, 0.9, -2.0], [0.3, 0.25, 0.1]),
            ("cluster_R", [-0.6, 0.9, -2.0], [0.3, 0.25, 0.1]),
        ]);
        let found = find(&model, &CarConfig::unconfigured("Test"));
        assert_eq!(only(&found, LightKind::Tail).len(), 2, "an unnamed rear lens is a tail light");
        assert_eq!(only(&found, LightKind::Brake).len(), 0);
        assert_eq!(only(&found, LightKind::Reverse).len(), 0);

        // Named, it is believed.
        let model = car(&[
            ("reverse_light_L", [0.4, 0.8, -2.0], [0.2, 0.15, 0.1]),
            ("reverse_light_R", [-0.4, 0.8, -2.0], [0.2, 0.15, 0.1]),
        ]);
        let found = find(&model, &CarConfig::unconfigured("Test"));
        assert_eq!(only(&found, LightKind::Reverse).len(), 2, "{:?}", found.warnings);
        assert_eq!(only(&found, LightKind::Tail).len(), 0, "and is not also a tail light");
    }

    #[test]
    fn the_config_can_place_a_lamp_the_model_does_not_have() {
        let mut config = CarConfig::unconfigured("Test");
        config.lights.brake_left = Some(Anchor {
            at: Some([0.7, 1.0, -2.1]),
            ..Anchor::default()
        });
        config.lights.brake_right = Some(Anchor {
            at: Some([-0.7, 1.0, -2.1]),
            intensity: Some(0.5),
            color: Some([0.0, 1.0, 0.0]),
            ..Anchor::default()
        });

        let found = find(&four_corner_car(), &config);
        let brakes = only(&found, LightKind::Brake);
        assert_eq!(brakes.len(), 2, "{:?}", found.warnings);
        assert_eq!(brakes[0].at, [0.7, 1.0, -2.1]);
        // Colour and brightness are the config's, in the ABGR the console reads.
        assert_eq!(brakes[1].color >> 24, 128, "half brightness");
        assert_eq!(brakes[1].color & 0xff_ffff, 0x00_FF00, "green");
    }

    /// A car that should have no lamps at all, said outright.
    #[test]
    fn lights_can_be_turned_off_entirely() {
        let mut config = CarConfig::unconfigured("Test");
        config.lights.enabled = false;
        let found = find(&four_corner_car(), &config);
        assert!(found.lights.is_empty());
        assert!(found.warnings.is_empty(), "and quietly: {:?}", found.warnings);
    }

    /// Whether the headlights follow the wheels is per car, and lives nowhere near the renderer.
    #[test]
    fn steering_headlights_are_a_config_decision() {
        let plain = find(&four_corner_car(), &CarConfig::unconfigured("Test"));
        assert!(only(&plain, LightKind::Head).iter().all(|l| !l.steers()));

        let mut config = CarConfig::unconfigured("Test");
        config.lights.steer = true;
        let steered = find(&four_corner_car(), &config);
        assert!(only(&steered, LightKind::Head).iter().all(|l| l.steers()));
        // Only the lamps that light the road: nothing about a tail lamp turns with the wheels.
        assert!(only(&steered, LightKind::Tail).iter().all(|l| !l.steers()));
    }

    /// A model that faces the wrong way puts its headlights at the back, which is invisible on the
    /// title screen and obvious for the whole descent. `[spawn] yaw = 180` is the fix, and this is
    /// where somebody finds that out.
    #[test]
    fn headlights_at_the_back_of_the_car_are_reported_as_a_facing_problem() {
        let mut config = CarConfig::unconfigured("Test");
        config.lights.headlight_left = Some(Anchor {
            node: Some("headlight_L".into()),
            ..Anchor::default()
        });
        config.lights.headlight_right = Some(Anchor {
            node: Some("headlight_R".into()),
            ..Anchor::default()
        });
        // The lenses named as headlights are at the back.
        let model = car(&[
            ("headlight_L", [0.6, 0.7, -1.9], [0.3, 0.2, 0.1]),
            ("headlight_R", [-0.6, 0.7, -1.9], [0.3, 0.2, 0.1]),
        ]);
        let found = find(&model, &config);
        assert!(
            found.warnings.iter().any(|w| w.contains("face the wrong way")),
            "{:?}",
            found.warnings
        );
    }

    /// The config's node fragment wins over where a lens sits, the same way a named wheel corner
    /// wins over the geometry.
    #[test]
    fn a_named_node_picks_the_lens_out_of_several() {
        let model = car(&[
            ("outer_lens_L", [0.7, 0.7, 1.9], [0.4, 0.2, 0.1]),
            ("outer_lens_R", [-0.7, 0.7, 1.9], [0.4, 0.2, 0.1]),
            ("inner_lens_L", [0.3, 0.7, 1.9], [0.2, 0.2, 0.1]),
            ("inner_lens_R", [-0.3, 0.7, 1.9], [0.2, 0.2, 0.1]),
        ]);
        let mut config = CarConfig::unconfigured("Test");
        config.lights.headlight_left = Some(Anchor {
            node: Some("inner_lens_L".into()),
            ..Anchor::default()
        });
        config.lights.headlight_right = Some(Anchor {
            node: Some("inner_lens_R".into()),
            ..Anchor::default()
        });

        let found = find(&model, &config);
        let heads = only(&found, LightKind::Head);
        assert_eq!(heads.len(), 2, "{:?}", found.warnings);
        // The inner pair, not the larger outer one the measurement would have chosen.
        assert!(heads.iter().all(|l| l.at[0].abs() < 0.5), "{:?}", heads);
    }
}
