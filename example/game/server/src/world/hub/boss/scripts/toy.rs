//! Toy Factory's scripted elites and boss (docs/locations/toy_factory.md §2):
//! the Foreman (3:00), Hatii (5:00) and the Stormy Knight (10:00).
//! Distances are cells (2 per metre), times seconds, as in the parent
//! module.

use super::{ability, hurt, Ability, Action, Anchor, Hurt, Pattern, Phase, Rank, Script, Traits, EVERYONE, NO_HURT};
use crate::world::combat::{Condition, DamageType};
use crate::world::protocol::look;

/// Bodies with traits beyond their rotation (reflection, a cracked shell,
/// spilled coins); read by `scripts::traits`.
pub(super) const TRAITS: &[Traits] = &[];

const SLOWED: Option<(Condition, f32)> = Some((Condition::Slowed, 2.0));

// --- The Foreman: the Clock Tower Manager, brass and cogs. Clock hands that
// sweep the floor, punch clocks under your feet, overtime and the shift,
// and Midnight (hub/toy.rs blows the shift-change whistle under 25 %). ----

pub(super) const FOREMAN: Script = Script {
    monster: "foreman",
    rank: Rank::Elite,
    aggro: 16.0,
    intro: "Clock in! Nobody leaves this floor before the whistle!",
    enrage_secs: 150.0,
    enrage: "Overtime! EVERYBODY works overtime!",
    death: "The clock... stops...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 2, 1, 0], gap: 3.2, haste: 1.0 },
        Phase { below: 60, shout: "Behind schedule! Double time!", opener: Some(3), rotation: &[5, 2, 4, 0], gap: 2.7, haste: 0.9 },
        Phase { below: 25, shout: "It's almost MIDNIGHT!", opener: Some(4), rotation: &[5, 6, 2, 5], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: minute hand: one long bar turning round him like a clock hand.
        ability(
            "boss_foreman_minute",
            "Minute Hand",
            1.1,
            Anchor::Boss,
            Pattern::Cross { arms: 1, len: 12.0, width: 2.0, count: 6, step: 30.0, every: 0.35 },
            hurt(2, 8, DamageType::Force),
            look::GOLD,
        ),
        // 1: hour hand: short, thick, slow and the other way round.
        ability(
            "boss_foreman_hour",
            "Hour Hand",
            1.2,
            Anchor::Boss,
            Pattern::Cross { arms: 1, len: 7.0, width: 3.0, count: 3, step: -60.0, every: 0.7 },
            Hurt { rider: SLOWED, ..hurt(3, 6, DamageType::Force) },
            look::GOLD,
        ),
        // 2: punch clock: a clock face under each of you, stamped twice.
        ability(
            "boss_foreman_punch",
            "Punch Clock",
            1.0,
            Anchor::Each(EVERYONE),
            Pattern::Chase { count: 2, r: 2.0, every: 0.7 },
            Hurt { rider: Some((Condition::Stunned, 0.6)), ..hurt(2, 6, DamageType::Lightning) },
            look::GLYPH,
        ),
        // 3: overtime: he winds his gears and the floor's toys back up;
        //    burst him to stop it.
        Ability {
            shout: "Overtime! Wind them up!",
            action: Action::Channel { secs: 4.0, heal: 10, rally: 25, rally_r: 14.0, break_pct: 8 },
            ..ability("boss_foreman_overtime", "Overtime", 0.0, Anchor::Boss, Pattern::None, NO_HURT, look::HEAL)
        },
        // 4: call the shift: alarms drop from the cranes.
        Ability {
            shout: "Next shift, on the floor!",
            shake: 30,
            action: Action::Summon { kind: "alarm", count: 2, per_player: 1 },
            ..ability("boss_foreman_shift", "Call the Shift", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 5: both hands: a plus that turns; keep to a quadrant and walk
        //    with it.
        ability(
            "boss_foreman_hands",
            "Both Hands",
            1.2,
            Anchor::Boss,
            Pattern::Cross { arms: 2, len: 12.0, width: 2.0, count: 6, step: 30.0, every: 0.35 },
            hurt(2, 8, DamageType::Force),
            look::GOLD,
        ),
        // 6: midnight: twelve chimes roll out in rings; stand in the gaps
        //    (the shift change it calls is hub/toy.rs's).
        Ability {
            shout: "MIDNIGHT! Shift change!",
            shake: 80,
            ..ability(
                "boss_foreman_midnight",
                "Midnight",
                1.6,
                Anchor::Boss,
                Pattern::Rings { count: 4, start: 2.0, width: 2.0, step: 4.0, every: 0.4 },
                hurt(3, 6, DamageType::Force),
                look::GOLD,
            )
        },
    ],
};

// --- Hatii: the ice wolf. Fangs, frost novas, rushes that leave rime, a
// howl that sends you running, a blizzard, and her litter. ----------------

pub(super) const HATII: Script = Script {
    monster: "hatii",
    rank: Rank::Elite,
    aggro: 18.0,
    intro: "Grrrrr... AWOOOOO!",
    enrage_secs: 150.0,
    enrage: "The whole factory freezes over!",
    death: "Awoo... ooo...",
    phases: &[
        Phase { below: 100, shout: "", opener: Some(0), rotation: &[1, 2, 3, 7, 1], gap: 3.0, haste: 1.0 },
        Phase { below: 55, shout: "The winter wind howls with me!", opener: Some(4), rotation: &[5, 3, 7, 2, 1], gap: 2.6, haste: 0.9 },
        Phase { below: 20, shout: "My pups! Hunt them down!", opener: Some(0), rotation: &[5, 6, 2, 4], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: call the litter: Baby Hatii run in.
        Ability {
            shake: 30,
            action: Action::Summon { kind: "garm_baby", count: 3, per_player: 1 },
            ..ability("boss_hatii_litter", "Call the Litter", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 1: ice fang: two snapping bites in front of her; get behind.
        ability(
            "boss_hatii_fang",
            "Ice Fang",
            0.9,
            Anchor::Nearest,
            Pattern::Sweep { count: 2, r: 5.0, arc: 100.0, step: 100.0, every: 0.3 },
            Hurt { rider: SLOWED, ..hurt(3, 6, DamageType::Cold) },
            look::COLD,
        ),
        // 2: frost nova: rime rings roll out; stand in the gaps.
        Ability {
            shake: 70,
            hurt: Hurt { rider: Some((Condition::Frozen, 1.2)), ..hurt(2, 8, DamageType::Cold) },
            ..ability(
                "boss_hatii_nova",
                "Frost Nova",
                1.3,
                Anchor::Boss,
                Pattern::Rings { count: 3, start: 0.0, width: 2.5, step: 4.5, every: 0.5 },
                NO_HURT,
                look::COLD,
            )
        },
        // 3: glacial rush at the farthest of you; the rime patch where she
        //    stops is ability 7, next in line.
        Ability {
            action: Action::Charge,
            shake: 40,
            ..ability("boss_hatii_rush", "Glacial Rush", 1.1, Anchor::Farthest, Pattern::Line { len: 16.0, width: 2.4 }, hurt(3, 8, DamageType::Cold), look::COLD)
        },
        // 4: howl: the cold ring all round her; hug her.
        Ability {
            shout: "AWOOOOOOO!",
            shake: 60,
            hurt: Hurt { rider: Some((Condition::Feared, 1.5)), ..hurt(2, 6, DamageType::Force) },
            ..ability("boss_hatii_howl", "Howl", 1.5, Anchor::Boss, Pattern::Donut { inner: 3.0, outer: 16.0 }, NO_HURT, look::COLD)
        },
        // 5: blizzard: icicles rain round one of you; keep moving.
        Ability {
            shake: 30,
            hurt: Hurt { rider: SLOWED, ..hurt(2, 8, DamageType::Cold) },
            ..ability(
                "boss_hatii_blizzard",
                "Blizzard",
                1.2,
                Anchor::Marked,
                Pattern::Rain { count: 10, per_player: 1, r: 2.0, spread: 10.0, every: 0.25 },
                NO_HURT,
                look::COLD,
            )
        },
        // 6: triple rush: she runs through three of you in turn.
        Ability {
            action: Action::Charge,
            stagger: 0.7,
            shake: 40,
            ..ability("boss_hatii_triple", "Triple Rush", 1.0, Anchor::Each(3), Pattern::Line { len: 16.0, width: 2.4 }, hurt(3, 8, DamageType::Cold), look::COLD)
        },
        // 7: rime: the ice her rush left under her; do not fight in it.
        ability(
            "boss_hatii_rime",
            "Rime",
            0.8,
            Anchor::Boss,
            Pattern::Pool { r: 2.5, secs: 6.0, pulse: 1.0, pull: 0.0 },
            Hurt { rider: Some((Condition::Slowed, 1.5)), save: false, ..hurt(1, 6, DamageType::Cold) },
            look::COLD,
        ),
    ],
};

// --- Stormy Knight: the wind that moves you. Lance charges that throw you
// aside, storm gusts, a gale into the belts, ranks of ice lances, the toy
// parade, a whirlwind, the eye of the storm and Absolute Zero. -----------

pub(super) const STORMY_KNIGHT: Script = Script {
    monster: "knight_of_windstorm",
    rank: Rank::Boss,
    aggro: 20.0,
    intro: "Who winds the toys while I ride? Stand before the storm!",
    enrage_secs: 300.0,
    enrage: "The storm has no end! Be swept away!",
    death: "The wind... falls... still...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 3], gap: 3.0, haste: 1.0 },
        Phase { below: 70, shout: "Soldiers! Parade for your general!", opener: Some(4), rotation: &[5, 1, 0, 2, 3], gap: 2.7, haste: 0.9 },
        Phase { below: 40, shout: "Into the eye of my storm!", opener: Some(6), rotation: &[7, 5, 1, 2], gap: 2.4, haste: 0.85 },
        Phase { below: 15, shout: "Freeze. All of it. FOREVER.", opener: Some(8), rotation: &[7, 1, 5, 3, 8], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: lance charge at the farthest of you; it throws you aside.
        Ability {
            action: Action::Charge,
            shake: 60,
            hurt: Hurt { push: 3.0, ..hurt(4, 8, DamageType::Physical) },
            ..ability("boss_stormy_lance", "Lance Charge", 1.2, Anchor::Farthest, Pattern::Line { len: 20.0, width: 2.6 }, NO_HURT, look::COLD)
        },
        // 1: storm gust: a storm of ice round each of you; never stand still.
        Ability {
            shake: 40,
            hurt: Hurt { rider: Some((Condition::Frozen, 1.0)), ..hurt(3, 6, DamageType::Cold) },
            ..ability(
                "boss_stormy_gust",
                "Storm Gust",
                1.2,
                Anchor::Each(EVERYONE),
                Pattern::Rain { count: 5, per_player: 0, r: 2.2, spread: 6.0, every: 0.3 },
                NO_HURT,
                look::COLD,
            )
        },
        // 2: gale: a wall of wind that throws you far back; keep a wall
        //    behind you, never a belt.
        Ability {
            shout: "Be gone with the wind!",
            shake: 50,
            hurt: Hurt { push: 6.0, ..hurt(2, 6, DamageType::Force) },
            ..ability(
                "boss_stormy_gale",
                "Gale",
                1.4,
                Anchor::Boss,
                Pattern::Sweep { count: 1, r: 14.0, arc: 70.0, step: 0.0, every: 0.0 },
                NO_HURT,
                look::RIPPLE,
            )
        },
        // 3: frozen ranks: bars of ice lances march out, each with a gap.
        ability(
            "boss_stormy_ranks",
            "Frozen Ranks",
            1.3,
            Anchor::Boss,
            Pattern::Bars { count: 4, len: 24.0, width: 2.5, gap: 3.0, first: 3.0, advance: 4.0, every: 0.55 },
            hurt(3, 8, DamageType::Cold),
            look::COLD,
        ),
        // 4: toy parade: soldiers march out of the crates.
        Ability {
            shake: 30,
            action: Action::Summon { kind: "cruiser", count: 3, per_player: 2 },
            ..ability("boss_stormy_parade", "Toy Parade", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 5: whirlwind: three arms of lightning unroll round one of you.
        ability(
            "boss_stormy_whirlwind",
            "Whirlwind",
            1.2,
            Anchor::Marked,
            Pattern::Spiral { arms: 3, steps: 7, first: 2.0, step: 2.2, turn: 28.0, r: 1.6, every: 0.12 },
            hurt(2, 8, DamageType::Lightning),
            look::LIGHTNING,
        ),
        // 6: eye of the storm: everything white but his feet; hug him.
        Ability {
            shout: "Only the eye is calm!",
            shake: 120,
            hurt: Hurt { rider: Some((Condition::Slowed, 3.0)), ..hurt(4, 6, DamageType::Cold) },
            ..ability("boss_stormy_eye", "Eye of the Storm", 1.6, Anchor::Boss, Pattern::Donut { inner: 3.5, outer: 18.0 }, NO_HURT, look::COLD)
        },
        // 7: double lance: he charges two of you in turn.
        Ability {
            action: Action::Charge,
            stagger: 0.8,
            shake: 60,
            hurt: Hurt { push: 3.0, ..hurt(4, 8, DamageType::Physical) },
            ..ability("boss_stormy_double", "Double Lance", 1.1, Anchor::Each(2), Pattern::Line { len: 20.0, width: 2.6 }, NO_HURT, look::COLD)
        },
        // 8: absolute zero: the hall freezes over but for three steam vents;
        //    break his shield or stand on a vent.
        Ability {
            shout: "ABSOLUTE ZERO!",
            shake: 200,
            action: Action::Shield { pct: 10 },
            hurt: Hurt { save: false, ..hurt(10, 10, DamageType::Cold) },
            ..ability(
                "boss_stormy_zero",
                "Absolute Zero",
                5.0,
                Anchor::Boss,
                Pattern::Sanctuary { r: 24.0, safe: 3, safe_r: 3.0, dist: 10.0 },
                NO_HURT,
                look::COLD,
            )
        },
    ],
};

#[cfg(test)]
#[path = "../toy_tests.rs"]
mod tests;
