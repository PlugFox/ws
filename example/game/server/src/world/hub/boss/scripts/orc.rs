//! Orc Village's scripted fights (example/docs/locations/orc_village.md
//! §2.3-2.5): the Goblin Leader, the Orc Hero and the Orc Lord. Distances
//! are cells (2 per metre), times seconds, as in the parent module.

use super::{ability, hurt, Ability, Action, Anchor, Hurt, Pattern, Phase, Rank, Script, Traits, NO_HURT};
use crate::world::combat::{Condition, DamageType};
use crate::world::hub::orc::CALL_THE_CLANS;
use crate::world::protocol::look;

/// Bodies with traits beyond their rotation (reflection, a cracked shell,
/// spilled coins); read by `scripts::traits`.
pub(super) const TRAITS: &[Traits] = &[];

// --- Goblin Leader: the mercenaries' boss. Bombs, smoke, a knife dance and
// a gang that comes when he whistles. -------------------------------------

pub(super) const GOBLIN_LEADER: Script = Script {
    monster: "goblin_leader",
    rank: Rank::Elite,
    aggro: 16.0,
    intro: "Oi! Fresh marks! Boys, get 'em!",
    enrage_secs: 150.0,
    enrage: "Enough games! Light ALL the fuses!",
    death: "Not... the face...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 0], gap: 3.2, haste: 1.0 },
        Phase { below: 60, shout: "Smoke 'em out!", opener: Some(3), rotation: &[4, 2, 1, 0], gap: 2.7, haste: 0.9 },
        Phase { below: 25, shout: "Everybody, PILE ON!", opener: Some(1), rotation: &[4, 3, 2, 4], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: Bomb Toss: a lit bomb lands on each of you; step off your circle.
        Ability {
            shake: 40,
            ..ability("boss_goblin_bombs", "Bomb Toss", 1.2, Anchor::Each(8), Pattern::Circle { r: 2.2 }, hurt(2, 8, DamageType::Fire), look::FIRE)
        },
        // 1: Gang Up: a whistle, and the goblins come running.
        Ability {
            shout: "Boys! Over here!",
            action: Action::Summon { kind: "goblin_1", count: 4, per_player: 2 },
            ..ability("boss_goblin_gang", "Gang Up", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 2: Knife Dance: three bounding stabs at the nearest of you.
        Ability {
            action: Action::Leap,
            ..ability(
                "boss_goblin_dance",
                "Knife Dance",
                0.9,
                Anchor::Nearest,
                Pattern::Hops { count: 3, first: 4.0, grow: 1.5, r: 2.0, every: 0.4 },
                hurt(2, 6, DamageType::Physical),
                look::DAMAGE,
            )
        },
        // 3: Smoke Bomb: he jumps back into a cloud that blinds whoever
        //    stands in it; walk out of the smoke.
        Ability {
            shout: "Now you see me...",
            action: Action::Retreat { dist: 8.0 },
            hurt: Hurt { rider: Some((Condition::Blinded, 1.5)), save: false, ..hurt(0, 0, DamageType::Poison) },
            ..ability("boss_goblin_smoke", "Smoke Bomb", 0.8, Anchor::Boss, Pattern::Pool { r: 6.0, secs: 8.0, pulse: 1.0, pull: 0.0 }, NO_HURT, look::DARK)
        },
        // 4: Carpet Bombs: three rows of bombs march over one of you, each
        //    with one gap.
        Ability {
            shout: "Fire in the hole!",
            shake: 50,
            ..ability(
                "boss_goblin_carpet",
                "Carpet Bombs",
                1.3,
                Anchor::Marked,
                Pattern::Bars { count: 3, len: 14.0, width: 2.5, gap: 3.0, first: 2.0, advance: 3.5, every: 0.5 },
                hurt(2, 8, DamageType::Fire),
                look::FIRE,
            )
        },
    ],
};

// --- Orc Hero: the vanguard's champion. Charges that throw you aside,
// cleaves, a stomp that stuns, a banner that rallies, a duel. --------------

pub(super) const ORC_HERO: Script = Script {
    monster: "ork_hero",
    rank: Rank::Elite,
    aggro: 18.0,
    intro: "Who dares walk into the warband's camp? Face the Hero!",
    enrage_secs: 150.0,
    enrage: "No more sparring. NOW you die!",
    death: "A hero's... death...",
    phases: &[
        Phase { below: 100, shout: "", opener: Some(0), rotation: &[1, 2, 3, 2], gap: 3.2, haste: 1.0 },
        Phase { below: 55, shout: "To the banner, brothers!", opener: Some(4), rotation: &[5, 1, 3, 7], gap: 2.7, haste: 0.9 },
        Phase { below: 20, shout: "Vanguard! With me!", opener: Some(0), rotation: &[5, 6, 7, 3], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: Vanguard: his banner guard marches in.
        Ability {
            shout: "Vanguard, form up!",
            action: Action::Summon { kind: "ork_warrior", count: 4, per_player: 2 },
            ..ability("boss_hero_vanguard", "Vanguard", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 1: Hero's Charge: at the farthest of you; it throws you aside.
        Ability {
            action: Action::Charge,
            shake: 60,
            hurt: Hurt { push: 3.0, ..hurt(3, 8, DamageType::Physical) },
            ..ability("boss_hero_charge", "Hero's Charge", 1.2, Anchor::Farthest, Pattern::Line { len: 18.0, width: 2.6 }, NO_HURT, look::EARTH)
        },
        // 2: Cleave: three sweeps of the axe at the nearest.
        ability(
            "boss_hero_cleave",
            "Cleave",
            1.0,
            Anchor::Nearest,
            Pattern::Sweep { count: 3, r: 5.0, arc: 90.0, step: 60.0, every: 0.3 },
            hurt(3, 6, DamageType::Physical),
            look::DAMAGE,
        ),
        // 3: Ground Stomp: three cracking rings; stand in the gaps.
        Ability {
            shake: 90,
            hurt: Hurt { rider: Some((Condition::Stunned, 1.0)), ..hurt(3, 6, DamageType::Earth) },
            ..ability(
                "boss_hero_stomp",
                "Ground Stomp",
                1.4,
                Anchor::Boss,
                Pattern::Rings { count: 3, start: 0.0, width: 2.5, step: 4.5, every: 0.5 },
                NO_HURT,
                look::EARTH,
            )
        },
        // 4: War Banner: he plants a banner that rallies the orcs round it
        //    (`hub/orc.rs`) and calls three more unless it is felled.
        Ability {
            shout: "Rally to the banner!",
            action: Action::Lay { kind: "orc_banner", count: 1, per_player: 0, ring: 3.0, hatch: "ork_warrior", hatch_count: 3, hatch_secs: 12.0 },
            ..ability("boss_hero_banner", "War Banner", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 5: Whirlwind: a turning cross of blades; walk with the spin.
        ability(
            "boss_hero_whirlwind",
            "Whirlwind",
            1.3,
            Anchor::Boss,
            Pattern::Cross { arms: 2, len: 9.0, width: 2.4, count: 4, step: 30.0, every: 0.45 },
            hurt(3, 8, DamageType::Physical),
            look::DAMAGE,
        ),
        // 6: Double Charge: two charges in a row.
        Ability {
            stagger: 0.8,
            action: Action::Charge,
            shake: 60,
            hurt: Hurt { push: 3.0, ..hurt(3, 8, DamageType::Physical) },
            ..ability("boss_hero_double", "Double Charge", 1.1, Anchor::Each(2), Pattern::Line { len: 18.0, width: 2.6 }, NO_HURT, look::EARTH)
        },
        // 7: Challenge: a duel ring follows one of you and bites three
        //    times; keep moving, lead it off the others.
        Ability {
            shout: "You! Face me!",
            hurt: Hurt { rider: Some((Condition::Slowed, 1.5)), ..hurt(2, 8, DamageType::Physical) },
            ..ability("boss_hero_challenge", "Challenge", 0.9, Anchor::Marked, Pattern::Chase { count: 3, r: 1.8, every: 0.9 }, NO_HURT, look::GOLD)
        },
    ],
};

// --- Orc Lord: the warlord of the clans. Earthshakers, a cleave that turns
// full circle, boulders from the sky, war drums, ranks of orcs, the clans
// called with a horn, and a last stand. -------------------------------------

pub(super) const ORC_LORD: Script = Script {
    monster: "orc_lord",
    rank: Rank::Boss,
    aggro: 20.0,
    intro: "Who burns my village?!",
    enrage_secs: 300.0,
    enrage: "The clans have waited long enough! CRUSH THEM!",
    death: "The clans... will remember...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 3], gap: 3.2, haste: 1.0 },
        Phase { below: 70, shout: "Beat the drums! Let the clans hear!", opener: Some(4), rotation: &[5, 2, 0, 1, 4], gap: 2.8, haste: 0.9 },
        Phase { below: 40, shout: "Sound the horn! Every clan, to me!", opener: Some(6), rotation: &[3, 5, 2, 0], gap: 2.5, haste: 0.85 },
        Phase { below: 15, shout: "I will not fall in my own village!", opener: Some(7), rotation: &[0, 3, 2, 1, 7], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: Earthshaker: four rings of cracking earth; step through the
        //    gaps in rhythm.
        Ability {
            shout: "The earth obeys ME!",
            shake: 140,
            hurt: Hurt { rider: Some((Condition::Stunned, 1.0)), ..hurt(3, 8, DamageType::Earth) },
            ..ability(
                "boss_orc_earthshaker",
                "Earthshaker",
                1.5,
                Anchor::Boss,
                Pattern::Rings { count: 4, start: 0.0, width: 3.0, step: 5.0, every: 0.5 },
                NO_HURT,
                look::EARTH,
            )
        },
        // 1: Lord's Cleave: the axe turns full circle; keep behind its
        //    leading edge, or out of reach.
        Ability {
            shake: 50,
            hurt: Hurt { push: 2.0, ..hurt(4, 6, DamageType::Physical) },
            ..ability(
                "boss_orc_cleave",
                "Lord's Cleave",
                1.1,
                Anchor::Nearest,
                Pattern::Sweep { count: 4, r: 7.0, arc: 120.0, step: 90.0, every: 0.35 },
                NO_HURT,
                look::DAMAGE,
            )
        },
        // 2: Boulder Rain: rocks fall round one of you; watch the shadows.
        ability(
            "boss_orc_boulders",
            "Boulder Rain",
            1.2,
            Anchor::Marked,
            Pattern::Rain { count: 8, per_player: 2, r: 2.2, spread: 9.0, every: 0.3 },
            hurt(3, 6, DamageType::Earth),
            look::EARTH,
        ),
        // 3: Warlord's Charge: at the farthest of you; it throws you aside.
        Ability {
            action: Action::Charge,
            shake: 120,
            hurt: Hurt { push: 3.0, ..hurt(4, 8, DamageType::Physical) },
            ..ability("boss_orc_charge", "Warlord's Charge", 1.3, Anchor::Farthest, Pattern::Line { len: 20.0, width: 3.0 }, NO_HURT, look::EARTH)
        },
        // 4: War Drums: drums round him; break them in ten seconds or each
        //    calls a High Orc.
        Ability {
            shout: "DRUMS!",
            action: Action::Lay { kind: "orc_drum", count: 3, per_player: 1, ring: 8.0, hatch: "high_orc", hatch_count: 1, hatch_secs: 10.0 },
            ..ability("boss_orc_drums", "War Drums", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 5: Rank and File: phantom ranks of orcs march across, each with
        //    one gap.
        ability(
            "boss_orc_ranks",
            "Rank and File",
            1.3,
            Anchor::Boss,
            Pattern::Bars { count: 4, len: 22.0, width: 3.0, gap: 3.5, first: 3.0, advance: 4.0, every: 0.6 },
            hurt(3, 8, DamageType::Physical),
            look::DAMAGE,
        ),
        // 6: Call the Clans: warriors come running and the war horn sounds
        //    (`hub/orc.rs`: a column, the banners replanted, arrows).
        Ability {
            shake: 60,
            action: Action::Summon { kind: "ork_warrior", count: 5, per_player: 2 },
            ..ability(CALL_THE_CLANS, "Call the Clans", 1.2, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 7: Last Stand: the whole arena reddens; break his shield or stand
        //    in a calm circle.
        Ability {
            shout: "LAST STAND!",
            shake: 160,
            action: Action::Shield { pct: 10 },
            hurt: Hurt { save: false, ..hurt(8, 10, DamageType::Physical) },
            ..ability(
                "boss_orc_last_stand",
                "Last Stand",
                4.0,
                Anchor::Boss,
                Pattern::Sanctuary { r: 22.0, safe: 3, safe_r: 3.0, dist: 9.0 },
                NO_HURT,
                look::DAMAGE,
            )
        },
    ],
};

#[cfg(test)]
#[path = "../orc_tests.rs"]
mod tests;
