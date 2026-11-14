//! Class profiles: what the character creation screen shows about each
//! class line (`GET /api/classes`).
//!
//! Everything a player can check in game comes from the real tables: hit
//! dice, HP / SP, armour class and saves ([`Class`]), the weapons it may
//! hold ([`WEAPONS`]), the armour and shields it may wear ([`ITEMS`]) and
//! its skills (the [`SkillBook`]). Only the judgement calls live here, in
//! [`PROFILES`]: the party role, the radar ratings, the difficulty, a one-
//! line pitch and which skills to show first. Lines not playable yet
//! (Merchant, Super Novice, Ninja, Gunslinger, Taekwon) are listed
//! too, locked, with their ratings and planned titles only (ROADMAP
//! "Classes").

use super::items::{Kind, Weight, ITEMS};
use super::weapon::{Reach, WeaponKind, WEAPONS};
use super::{Class, Save, SkillBook, MAX_LEVEL};
use serde_json::{json, Value};

/// Party roles (the creation screen's role badge; the first is the main one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Tank,
    Melee,
    Ranged,
    Caster,
    Healer,
    Support,
    Hybrid,
}

impl Role {
    pub const fn id(self) -> &'static str {
        match self {
            Role::Tank => "tank",
            Role::Melee => "melee",
            Role::Ranged => "ranged",
            Role::Caster => "caster",
            Role::Healer => "healer",
            Role::Support => "support",
            Role::Hybrid => "hybrid",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Role::Tank => "Tank",
            Role::Melee => "Melee damage",
            Role::Ranged => "Ranged damage",
            Role::Caster => "Area caster",
            Role::Healer => "Healer",
            Role::Support => "Support",
            Role::Hybrid => "Jack of all trades",
        }
    }
}

/// Radar axes, 1 (weak) to 5 (best of the classes). Relative to each
/// other, not absolute numbers: a hand-made reading of the skill lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ratings {
    /// Damage output (single target and areas together).
    pub damage: u8,
    /// HP, armour and self-protection.
    pub toughness: u8,
    /// Dashes, slides, blinks and speed buffs.
    pub mobility: u8,
    /// How far it fights from.
    pub range: u8,
    /// Heals, buffs and protection for the party.
    pub support: u8,
    /// Crowd control and area: slows, freezes, taunts, walls, big circles.
    pub control: u8,
}

impl Ratings {
    /// Axis ids and values in radar order (clockwise from the top).
    pub const fn axes(&self) -> [(&'static str, u8); 6] {
        [
            ("damage", self.damage),
            ("toughness", self.toughness),
            ("mobility", self.mobility),
            ("range", self.range),
            ("support", self.support),
            ("control", self.control),
        ]
    }
}

/// The hand-authored part of a class line.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    /// Class id (the wire `archetype`; locked lines have their own).
    pub id: &'static str,
    pub name: &'static str,
    /// The playable class, `None` while the line is locked.
    pub class: Option<Class>,
    pub roles: &'static [Role],
    pub ratings: Ratings,
    /// 1 (forgiving) to 5 (demanding).
    pub difficulty: u8,
    /// One line on the creation screen.
    pub pitch: &'static str,
    /// Skills shown first, in this order (ids of the skill book).
    pub signature: &'static [&'static str],
    /// Locked lines: the planned titles (playable lines use [`Class::title`]).
    pub planned: &'static [&'static str],
}

const fn r(damage: u8, toughness: u8, mobility: u8, range: u8, support: u8, control: u8) -> Ratings {
    Ratings { damage, toughness, mobility, range, support, control }
}

/// Every class line in creation-screen order: the playable ones first.
pub const PROFILES: &[Profile] = &[
    Profile {
        id: "swordsman",
        name: "Swordsman",
        class: Some(Class::Swordsman),
        roles: &[Role::Tank, Role::Melee],
        // Heaviest armour and dice; Provoke and Magnum Break hold the horde.
        ratings: r(3, 5, 2, 1, 2, 3),
        difficulty: 1,
        pitch: "Front line: taunts the horde, blocks with a shield, smites with a holy blade.",
        signature: &["bash", "magnum_break", "provoke", "auto_guard", "grand_cross"],
        planned: &[],
    },
    Profile {
        id: "archer",
        name: "Archer",
        class: Some(Class::Archer),
        roles: &[Role::Ranged],
        // Longest reach and burst on one target; traps for control.
        ratings: r(4, 2, 4, 5, 1, 3),
        difficulty: 3,
        pitch: "Extreme range, traps and quick feet: bursts the biggest target.",
        signature: &["double_strafe", "arrow_shower", "ankle_snare", "claymore_trap", "sharp_shooting"],
        planned: &[],
    },
    Profile {
        id: "mage",
        name: "Mage",
        class: Some(Class::Mage),
        roles: &[Role::Caster, Role::Ranged],
        // The largest circles and the most crowd control; paper armour.
        ratings: r(5, 1, 2, 4, 1, 5),
        difficulty: 4,
        pitch: "Storms of fire, frost and lightning over the horde; fragile.",
        signature: &["fire_bolt", "frost_diver", "fire_wall", "storm_gust", "meteor_storm"],
        planned: &[],
    },
    Profile {
        id: "acolyte",
        name: "Acolyte",
        class: Some(Class::Acolyte),
        roles: &[Role::Healer, Role::Support],
        ratings: r(2, 3, 3, 3, 5, 2),
        difficulty: 2,
        pitch: "Heals the most wounded, blesses the party, smites the undead.",
        signature: &["heal", "blessing", "increase_agi", "sanctuary", "magnus_exorcismus"],
        planned: &[],
    },
    Profile {
        id: "thief",
        name: "Thief",
        class: Some(Class::Thief),
        roles: &[Role::Melee],
        // The quickest blades and Sonic Blow; Cloaking and Back Stab move
        // it through the horde; poison ground for control; no heals.
        ratings: r(4, 2, 5, 1, 1, 2),
        difficulty: 4,
        pitch: "Poison, stealth and the fastest blades: in and out of the horde.",
        signature: &["sonic_blow", "envenom", "cloaking", "venom_dust", "meteor_assault"],
        planned: &[],
    },
    Profile {
        id: "merchant",
        name: "Merchant",
        class: None,
        roles: &[Role::Melee, Role::Support],
        ratings: r(3, 4, 2, 2, 3, 2),
        difficulty: 2,
        pitch: "Cart crashes, forged weapons and potions thrown to the party.",
        signature: &[],
        planned: &["Merchant", "Blacksmith · Alchemist", "Whitesmith · Creator"],
    },
    Profile {
        id: "super_novice",
        name: "Super Novice",
        class: None,
        roles: &[Role::Hybrid],
        ratings: r(3, 2, 3, 3, 3, 3),
        difficulty: 5,
        pitch: "A Novice who never chose: picks skills from every first class.",
        signature: &[],
        planned: &["Novice", "Super Novice", "Expanded Super Novice"],
    },
    Profile {
        id: "ninja",
        name: "Ninja",
        class: None,
        roles: &[Role::Melee, Role::Ranged],
        ratings: r(4, 2, 5, 3, 1, 3),
        difficulty: 4,
        pitch: "Shuriken, shadow steps and ninjutsu scrolls of fire, ice and wind.",
        signature: &[],
        planned: &["Ninja", "Kagerou · Oboro", "Shinkiro · Shiranui"],
    },
    Profile {
        id: "gunslinger",
        name: "Gunslinger",
        class: None,
        roles: &[Role::Ranged],
        ratings: r(5, 2, 3, 5, 1, 3),
        difficulty: 3,
        pitch: "Revolvers, rifles, shotguns and a gatling: coins for crits.",
        signature: &[],
        planned: &["Gunslinger", "Rebellion", "Night Watch"],
    },
    Profile {
        id: "taekwon",
        name: "Taekwon",
        class: None,
        roles: &[Role::Melee, Role::Support],
        ratings: r(4, 3, 5, 1, 2, 2),
        difficulty: 4,
        pitch: "Kick chains and stances; grows into a Star Gladiator or a Soul Linker.",
        signature: &[],
        planned: &["Taekwon", "Star Gladiator · Soul Linker", "Sky Emperor · Soul Reaper"],
    },
];

/// The profile of a class id.
pub fn find(id: &str) -> Option<&'static Profile> {
    PROFILES.iter().find(|p| p.id == id)
}

fn save_id(s: Save) -> &'static str {
    match s {
        Save::Body => "body",
        Save::Reflex => "reflex",
        Save::Mind => "mind",
    }
}

fn weight_id(w: Weight) -> &'static str {
    match w {
        Weight::Light => "light",
        Weight::Medium => "medium",
        Weight::Heavy => "heavy",
    }
}

fn die(count: u32, sides: u32) -> String {
    format!("{count}d{sides}")
}

/// The weapons a class may hold: per family, its swing and the catalogue's
/// weapons of it (icon ids are item ids).
fn weapons(class: Class) -> Vec<Value> {
    WeaponKind::ALL
        .iter()
        .filter(|&&k| class.can_hold(k))
        .map(|&k| {
            let items: Vec<&_> = WEAPONS.iter().filter(|w| w.kind == k).collect();
            let reach = match items.first().map(|w| w.reach) {
                Some(Reach::Ranged { .. }) => "ranged",
                _ => "melee",
            };
            json!({
                "kind": k.id(),
                "label": k.label(),
                "two_handed": k.two_handed(),
                "reach": reach,
                "items": items.iter().map(|w| json!({"id": w.id, "name": w.name, "die": die(w.die.count, w.die.sides), "level": w.level})).collect::<Vec<_>>(),
            })
        })
        .collect()
}

/// Body armour and shields a class may wear: the heaviest weight, every
/// piece (lightest first) and the starting kit.
fn armour(class: Class) -> Value {
    let wearable = |kind: fn(&Kind) -> bool| ITEMS.iter().filter(move |i| kind(&i.kind) && i.classes & super::items::class_bit(class) != 0);
    let mut body: Vec<_> = wearable(|k| matches!(k, Kind::Armour(..))).collect();
    body.sort_by_key(|i| match i.kind {
        Kind::Armour(w, base) => (w as u8, base, i.price),
        _ => (0, 0, 0),
    });
    let heaviest = body.iter().filter_map(|i| if let Kind::Armour(w, _) = i.kind { Some(w) } else { None }).max_by_key(|w| *w as u8);
    let shields: Vec<_> = wearable(|k| matches!(k, Kind::Shield)).collect();
    json!({
        "weight": heaviest.map(weight_id),
        "defence": class.defence(),
        "body": body.iter().map(|i| json!({"id": i.id, "name": i.name})).collect::<Vec<_>>(),
        "shields": shields.iter().map(|i| json!({"id": i.id, "name": i.name})).collect::<Vec<_>>(),
        "start": class.starting_kit(),
    })
}

/// The class's skills in tree order (tier, then file order); signature
/// skills carry their rank in [`Profile::signature`].
fn skills(class: Class, profile: &Profile, book: &SkillBook) -> Vec<Value> {
    let mut list: Vec<_> = book.iter().filter(|(_, s)| s.class == Some(class)).collect();
    list.sort_by_key(|(i, s)| (s.tier, *i));
    list.into_iter()
        .map(|(_, s)| {
            let mut v = json!({
                "id": s.id,
                "name": s.name,
                "icon": s.icon,
                "text": s.text,
                "tier": s.tier,
                "passive": s.passive,
                "max_level": s.max_level,
            });
            if let Some(rank) = profile.signature.iter().position(|&id| id == s.id) {
                v["signature"] = json!(rank);
            }
            v
        })
        .collect()
}

/// One class line for the creation screen.
pub fn describe(p: &Profile, book: &SkillBook) -> Value {
    let mut v = json!({
        "id": p.id,
        "name": p.name,
        "locked": p.class.is_none(),
        "roles": p.roles.iter().map(|r| json!({"id": r.id(), "label": r.label()})).collect::<Vec<_>>(),
        "ratings": p.ratings.axes().iter().map(|(k, v)| json!({"id": k, "value": v})).collect::<Vec<_>>(),
        "difficulty": p.difficulty,
        "pitch": p.pitch,
    });
    let Some(c) = p.class else {
        v["titles"] = json!(p.planned);
        return v;
    };
    // Titles and hit dice of the three tiers (levels 1, 30, 60).
    let tiers = [1, 30, 60];
    v["titles"] = json!(tiers.map(|l| c.title(l)));
    v["hit_die"] = json!(tiers.map(|l| format!("d{}", c.hit_die_at(l))));
    v["hp"] = json!([c.max_hp(1), c.max_hp(MAX_LEVEL)]);
    v["sp"] = json!([c.max_sp(1), c.max_sp(MAX_LEVEL)]);
    v["ac"] = json!(c.base_ac(1));
    v["save"] = json!(save_id(c.proficient_save()));
    v["attacks"] = json!(tiers.map(|l| c.attacks_per_action(l)));
    v["start_weapon"] = json!(c.starting_weapon());
    v["weapons"] = json!(weapons(c));
    v["armour"] = armour(c);
    v["skills"] = json!(skills(c, p, book));
    v
}

/// `GET /api/classes`: every line, playable first.
pub fn catalogue(book: &SkillBook) -> Value {
    json!({ "classes": PROFILES.iter().map(|p| describe(p, book)).collect::<Vec<_>>() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::combat::skills::tests::book;

    #[test]
    fn every_class_has_one_profile_and_locked_lines_are_not_classes() {
        for c in Class::ALL {
            assert_eq!(PROFILES.iter().filter(|p| p.class == Some(c)).count(), 1, "{c:?}");
            assert_eq!(find(c.id()).and_then(|p| p.class), Some(c), "profile id is the class id");
        }
        for (i, p) in PROFILES.iter().enumerate() {
            assert!(PROFILES[..i].iter().all(|o| o.id != p.id), "duplicate {}", p.id);
            assert!(!p.roles.is_empty(), "{} has a role", p.id);
            assert!(p.ratings.axes().iter().all(|(_, v)| (1..=5).contains(v)), "{} ratings 1..5", p.id);
            assert!((1..=5).contains(&p.difficulty));
            if p.class.is_none() {
                assert!(Class::from_id(p.id).is_none(), "{} is locked, the server must not create it", p.id);
                assert!(!p.planned.is_empty() && p.signature.is_empty());
            }
        }
        // Playable lines come first (the screen's default pick).
        let first_locked = PROFILES.iter().position(|p| p.class.is_none()).unwrap();
        assert!(PROFILES[first_locked..].iter().all(|p| p.class.is_none()));
    }

    /// The signature skills name real skills of their class in the baked
    /// skill table (when the asset pack is there).
    #[test]
    fn signature_skills_exist_in_the_pack() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
        let Ok(book) = SkillBook::load(&path) else { return };
        for p in PROFILES {
            let Some(c) = p.class else { continue };
            for id in p.signature {
                let (_, s) = book.find(id).unwrap_or_else(|| panic!("{}: unknown skill {id}", p.id));
                assert_eq!(s.class, Some(c), "{id} is a {} skill", p.id);
                assert!(!s.icon.is_empty(), "{id} has an icon");
            }
        }
    }

    #[test]
    fn describes_from_the_real_tables() {
        let book = book();
        let mage = describe(find("mage").unwrap(), &book);
        assert_eq!(mage["locked"], false);
        assert_eq!(mage["hit_die"], json!(["d6", "d6", "d8"]));
        assert_eq!(mage["hp"], json!([Class::Mage.max_hp(1), Class::Mage.max_hp(99)]));
        assert_eq!(mage["titles"], json!(["Mage", "Wizard", "High Wizard"]));
        let kinds: Vec<_> = mage["weapons"].as_array().unwrap().iter().map(|w| w["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["dagger", "staff"]);
        assert_eq!(mage["armour"]["weight"], "light");
        assert!(mage["armour"]["shields"].as_array().unwrap().iter().any(|s| s["id"] == "guard"));
        // The test book's mage line, in tier order.
        let skills = mage["skills"].as_array().unwrap();
        assert!(!skills.is_empty());
        assert!(skills.windows(2).all(|w| w[0]["tier"].as_u64() <= w[1]["tier"].as_u64()));
        assert!(skills.iter().all(|s| s["id"] != "slide"), "no basic row");

        let sword = describe(find("swordsman").unwrap(), &book);
        assert_eq!(sword["armour"]["weight"], "heavy");
        assert_eq!(sword["armour"]["start"], json!(["cotton_shirt", "guard"]));
        assert_eq!(sword["weapons"][0]["items"][0]["id"], "knife");
        assert_eq!(sword["roles"][0]["id"], "tank");
        assert_eq!(sword["ratings"].as_array().unwrap().len(), 6);

        let thief = describe(find("thief").unwrap(), &book);
        assert_eq!(thief["locked"], false, "the Thief line is playable");
        assert_eq!(thief["titles"], json!(["Thief", "Assassin", "Assassin Cross"]));
        assert_eq!(thief["start_weapon"], "knife");
        let kinds: Vec<_> = thief["weapons"].as_array().unwrap().iter().map(|w| w["kind"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["dagger", "sword", "katar"]);
        assert_eq!(thief["save"], "reflex");

        let ninja = describe(find("ninja").unwrap(), &book);
        assert_eq!(ninja["locked"], true);
        assert!(ninja.get("skills").is_none() && ninja.get("hit_die").is_none());
        assert_eq!(catalogue(&book)["classes"].as_array().unwrap().len(), PROFILES.len());
    }
}
