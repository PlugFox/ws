//! Thor Volcano's scripted elites and boss (docs/locations/thor_volcano.md §2):
//! Salamander (3:00), Kasa (5:00) and Ifrit (10:00). Distances are cells
//! (2 per metre), times seconds, as in the parent module.

use super::{ability, hurt, Ability, Action, Anchor, Hurt, Pattern, Phase, Rank, Script, Traits, EVERYONE, NO_HURT};
use crate::world::combat::{Condition, DamageType};
use crate::world::protocol::look;

/// Bodies with traits beyond their rotation (reflection, a cracked shell,
/// spilled coins); read by `scripts::traits`.
pub(super) const TRAITS: &[Traits] = &[];

const BURN: Option<(Condition, f32)> = Some((Condition::Burning, 2.0));

// --- Salamander: the lava lizard. A flamethrower that pans, a tail that
// throws you, pools of magma piling up, a shell of molten rock. ----------

pub(super) const SALAMANDER: Script = Script {
    monster: "salamander",
    rank: Rank::Elite,
    aggro: 16.0,
    intro: "Hssss... fresh meat walks into my furnace!",
    enrage_secs: 150.0,
    enrage: "The whole mountain is MY nest!",
    death: "The fire... goes... out...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 0], gap: 3.0, haste: 1.0 },
        Phase { below: 60, shout: "My skin turns to magma!", opener: Some(3), rotation: &[4, 2, 1, 5], gap: 2.6, haste: 0.9 },
        Phase { below: 25, shout: "Hatch, my little ones! Feed!", opener: Some(5), rotation: &[4, 6, 2, 3], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: flame breath: a cone of fire that pans across the chamber.
        Ability {
            hurt: Hurt { rider: BURN, ..hurt(2, 8, DamageType::Fire) },
            ..ability(
                "boss_salamander_breath",
                "Flame Breath",
                1.1,
                Anchor::Nearest,
                Pattern::Sweep { count: 5, r: 8.0, arc: 50.0, step: 25.0, every: 0.25 },
                NO_HURT,
                look::FIRE,
            )
        },
        // 1: tail lash: a sweep all round it that throws you back.
        Ability {
            hurt: Hurt { push: 3.0, ..hurt(3, 6, DamageType::Physical) },
            ..ability("boss_tail_lash", "Tail Lash", 0.9, Anchor::Boss, Pattern::Circle { r: 5.0 }, NO_HURT, look::DAMAGE)
        },
        // 2: magma spit: a pool of magma under everyone; they pile up.
        ability(
            "boss_magma_spit",
            "Magma Spit",
            1.2,
            Anchor::Each(EVERYONE),
            Pattern::Pool { r: 2.2, secs: 8.0, pulse: 1.0, pull: 0.0 },
            Hurt { rider: Some((Condition::Burning, 1.5)), save: false, ..hurt(2, 6, DamageType::Fire) },
            look::FIRE,
        ),
        // 3: molten skin: a shell of magma, then two rings of fire burst
        //    out of it unless it is broken (cold counts double on it).
        Ability {
            shout: "Break my shell? HAH!",
            action: Action::Shield { pct: 10 },
            ..ability(
                "boss_molten_skin",
                "Molten Skin",
                3.0,
                Anchor::Boss,
                Pattern::Rings { count: 2, start: 0.0, width: 3.0, step: 4.0, every: 0.6 },
                hurt(3, 8, DamageType::Fire),
                look::FIRE,
            )
        },
        // 4: burning run: it charges the farthest of you.
        Ability {
            action: Action::Charge,
            shake: 40,
            ..ability("boss_burning_run", "Burning Run", 1.1, Anchor::Farthest, Pattern::Line { len: 18.0, width: 2.4 }, hurt(3, 8, DamageType::Fire), look::FIRE)
        },
        // 5: brood: Magmarings crawl out of the vents.
        Ability {
            action: Action::Summon { kind: "magmaring", count: 5, per_player: 2 },
            ..ability("boss_salamander_brood", "Brood", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 6: fire ring: a closing ring of flame; hug it.
        Ability {
            shout: "Burn, all of you!",
            shake: 90,
            ..ability("boss_fire_ring", "Fire Ring", 1.5, Anchor::Boss, Pattern::Donut { inner: 3.0, outer: 14.0 }, hurt(4, 6, DamageType::Fire), look::FIRE)
        },
    ],
};

// --- Kasa: the wildfire spirit, burning blue-white. Pillars of fire, wisps
// that follow you, it blinks away, splits into embers and draws the air
// in before the blast. ------------------------------------------------------

pub(super) const KASA: Script = Script {
    monster: "kasa",
    rank: Rank::Elite,
    aggro: 18.0,
    intro: "Flicker, flicker... dance with the wildfire!",
    enrage_secs: 150.0,
    enrage: "I will burn this whole mountain white!",
    death: "Snuffed... out...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 0], gap: 3.0, haste: 1.0 },
        Phase { below: 55, shout: "One flame becomes three!", opener: Some(3), rotation: &[4, 1, 2, 0], gap: 2.6, haste: 0.9 },
        Phase { below: 20, shout: "Breathe in... and BURN!", opener: Some(3), rotation: &[4, 5, 1, 2], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: fire pillars: columns of fire rain round each of you.
        Ability {
            shake: 30,
            ..ability(
                "boss_fire_pillars",
                "Fire Pillars",
                1.1,
                Anchor::Each(EVERYONE),
                Pattern::Rain { count: 4, per_player: 0, r: 1.8, spread: 5.0, every: 0.35 },
                hurt(3, 6, DamageType::Fire),
                look::FIRE,
            )
        },
        // 1: will-o'-wisps: blue flames that follow you and bite.
        ability(
            "boss_wisps",
            "Will-o'-Wisps",
            0.9,
            Anchor::Each(EVERYONE),
            Pattern::Chase { count: 4, r: 1.5, every: 0.8 },
            Hurt { rider: BURN, ..hurt(2, 6, DamageType::Fire) },
            look::FIRE,
        ),
        // 2: flicker: it goes out and relights elsewhere, scorching the spot.
        Ability {
            action: Action::Retreat { dist: 10.0 },
            ..ability("boss_flicker", "Flicker", 0.8, Anchor::Boss, Pattern::Circle { r: 3.5 }, hurt(2, 8, DamageType::Fire), look::FIRE)
        },
        // 3: split: two embers of itself; while they burn it cannot be hurt.
        Ability {
            shout: "Douse us all, if you can!",
            action: Action::Mirror { kind: "kasa_ember", count: 2, per_player: 0 },
            ..ability("boss_kasa_split", "Split", 1.2, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 4: wildfire: fire runs between it and its embers.
        ability("boss_wildfire", "Wildfire", 1.3, Anchor::Boss, Pattern::Crossfire { width: 2.0, len: 12.0 }, hurt(3, 8, DamageType::Fire), look::FIRE),
        // 5: backdraft: rings of air rush inward, dragging you in.
        Ability {
            shout: "Come closer to the flame...",
            shake: 70,
            hurt: Hurt { push: -3.0, ..hurt(3, 8, DamageType::Fire) },
            ..ability(
                "boss_backdraft",
                "Backdraft",
                1.4,
                Anchor::Boss,
                Pattern::Rings { count: 3, start: 9.0, width: 2.5, step: -4.0, every: 0.5 },
                NO_HURT,
                look::FIRE,
            )
        },
    ],
};

// --- Ifrit: the lord of the volcano. A whip of flame, meteor storms, the
// mountain quaking, a vortex that drags you in before it bursts, children
// of fire, a meteor to share, and the Eruption. ---------------------------

pub(super) const IFRIT: Script = Script {
    monster: "ifrit",
    rank: Rank::Boss,
    aggro: 22.0,
    intro: "Who dares to stand in MY fire? Kneel, and burn!",
    enrage_secs: 300.0,
    enrage: "The mountain answers only to me! ERUPT!",
    death: "The fire... sleeps... for now...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 3], gap: 3.0, haste: 1.0 },
        Phase { below: 75, shout: "Into the heart of the flame!", opener: Some(4), rotation: &[5, 1, 0, 6, 2, 4], gap: 2.6, haste: 0.9 },
        Phase { below: 50, shout: "Children of fire, rise!", opener: Some(7), rotation: &[8, 9, 3, 1, 6, 7], gap: 2.4, haste: 0.85 },
        Phase { below: 25, shout: "The mountain ERUPTS!", opener: Some(10), rotation: &[9, 0, 4, 5, 1, 10], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: flame whip: a lash of fire sweeping a full turn round him.
        Ability {
            hurt: Hurt { rider: BURN, ..hurt(4, 6, DamageType::Fire) },
            ..ability(
                "boss_flame_whip",
                "Flame Whip",
                1.2,
                Anchor::Nearest,
                Pattern::Sweep { count: 6, r: 10.0, arc: 60.0, step: 60.0, every: 0.3 },
                NO_HURT,
                look::FIRE,
            )
        },
        // 1: pyroclasm: a storm of burning rock round a marked player.
        Ability {
            shake: 60,
            ..ability(
                "boss_pyroclasm",
                "Pyroclasm",
                1.3,
                Anchor::Marked,
                Pattern::Rain { count: 9, per_player: 2, r: 2.6, spread: 10.0, every: 0.35 },
                hurt(3, 8, DamageType::Fire),
                look::FIRE,
            )
        },
        // 2: earthquake: shockwaves roll out; stand in the gaps.
        Ability {
            shout: "The mountain quakes at my step!",
            shake: 150,
            hurt: Hurt { rider: Some((Condition::Stunned, 1.0)), push: 2.0, ..hurt(3, 8, DamageType::Earth) },
            ..ability(
                "boss_ifrit_quake",
                "Earthquake",
                1.5,
                Anchor::Boss,
                Pattern::Rings { count: 4, start: 0.0, width: 3.0, step: 5.0, every: 0.5 },
                NO_HURT,
                look::EARTH,
            )
        },
        // 3: scorching charge at the farthest of you; it throws you aside.
        Ability {
            action: Action::Charge,
            shake: 60,
            hurt: Hurt { push: 2.0, ..hurt(4, 8, DamageType::Fire) },
            ..ability("boss_scorching_charge", "Scorching Charge", 1.2, Anchor::Farthest, Pattern::Line { len: 20.0, width: 3.0 }, NO_HURT, look::FIRE)
        },
        // 4: infernal vortex: a whirl of fire drags everything toward him
        //    for 5 s (run outward); its blast is ability 5, next in line.
        Ability {
            shout: "Come to me... into the fire!",
            ..ability(
                "boss_infernal_vortex",
                "Infernal Vortex",
                1.4,
                Anchor::Boss,
                Pattern::Pool { r: 12.0, secs: 5.0, pulse: 1.0, pull: 3.0 },
                Hurt { save: false, ..hurt(1, 6, DamageType::Fire) },
                look::FIRE,
            )
        },
        // 5: the vortex bursts: whoever it dragged in burns.
        Ability {
            shake: 120,
            hurt: Hurt { rider: BURN, ..hurt(6, 6, DamageType::Fire) },
            ..ability("boss_vortex_blast", "Vortex Blast", 2.5, Anchor::Boss, Pattern::Circle { r: 5.0 }, NO_HURT, look::FIRE)
        },
        // 6: lava geysers: the ground cracks and spouts behind each of you.
        ability(
            "boss_lava_geysers",
            "Lava Geysers",
            1.0,
            Anchor::Each(EVERYONE),
            Pattern::Trail { count: 5, r: 1.8, every: 0.5, fuse: 1.0, secs: 5.0, pulse: 1.0 },
            hurt(2, 8, DamageType::Fire),
            look::FIRE,
        ),
        // 7: children of fire: two embers of Kasa ...
        Ability {
            action: Action::Summon { kind: "kasa_ember", count: 2, per_player: 1 },
            ..ability("boss_children_of_fire", "Children of Fire", 1.2, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 8: ... and Magmarings out of the vents, right after.
        Ability {
            action: Action::Summon { kind: "magmaring", count: 4, per_player: 2 },
            ..ability("boss_fire_brood", "Brood of Magma", 0.8, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 9: meteor: one huge shadow on a marked player; its blow is shared
        //    by everyone under it (stack up: alone it hits for 8 dice).
        Ability {
            shout: "A star falls for you!",
            shake: 200,
            hurt: Hurt { split: 8, ..hurt(28, 6, DamageType::Fire) },
            ..ability("boss_meteor", "Meteor", 3.5, Anchor::Marked, Pattern::Circle { r: 4.5 }, NO_HURT, look::FIRE)
        },
        // 10: eruption: the floor becomes lava but for three obsidian
        //     islands (and any obsidian a cold spell made); break his
        //     shield or stand on rock. Both surges flood with it (thor.rs).
        Ability {
            shout: "ERUPTION! Let the mountain swallow you!",
            shake: 220,
            action: Action::Shield { pct: 10 },
            hurt: Hurt { save: false, ..hurt(10, 10, DamageType::Fire) },
            ..ability(
                "boss_eruption",
                "Eruption",
                5.0,
                Anchor::Boss,
                Pattern::LitSanctuary { r: 26.0, lit_r: 3.0, min: 3, safe_r: 3.0, dist: 10.0 },
                NO_HURT,
                look::FIRE,
            )
        },
    ],
};
