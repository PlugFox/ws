//! Prontera Culvert's scripted fights (example/docs/locations/culvert.md §4):
//! the Thief Bug Queen, the Rat King and the Golden Thief Bug. Distances are
//! cells (2 per metre), times seconds, as in the parent module.

use super::{ability, hurt, Ability, Action, Anchor, Hurt, Pattern, Phase, Rank, Script, Traits, NO_HURT};
use crate::world::combat::{Condition, DamageType};
use crate::world::protocol::look;

// --- Thief Bug Queen: guards the egg clusters. Acid sprays, skitter dives,
// fresh eggs, a brood call that hatches them all, a swarm spiral. --------

pub(super) const THIEF_BUG_QUEEN: Script = Script {
    monster: "thief_bug_queen",
    rank: Rank::Elite,
    aggro: 16.0,
    intro: "Intruders in my nursery? My children are HUNGRY!",
    enrage_secs: 150.0,
    enrage: "The whole brood rises! You cannot crush us all!",
    death: "My eggs... my children...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 0, 3], gap: 3.2, haste: 1.0 },
        Phase { below: 50, shout: "Smell that? Everyone, to me!", opener: Some(4), rotation: &[1, 0, 2, 3, 0], gap: 2.6, haste: 0.9 },
        Phase { below: 20, shout: "Swarm! SWARM!", opener: Some(5), rotation: &[5, 1, 0, 3], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: Acid Spray: a cone of acid swung round; walk behind the beam.
        Ability {
            hurt: Hurt { rider: Some((Condition::Poisoned, 3.0)), ..hurt(2, 6, DamageType::Poison) },
            ..ability(
                "boss_acid_spray",
                "Acid Spray",
                1.1,
                Anchor::Boss,
                Pattern::Sweep { count: 5, r: 9.0, arc: 60.0, step: 40.0, every: 0.4 },
                NO_HURT,
                look::POISON,
            )
        },
        // 1: Skitter Dive: three hops at a marked player, each landing a ring.
        Ability {
            action: Action::Leap,
            shake: 30,
            ..ability(
                "boss_skitter_dive",
                "Skitter Dive",
                0.9,
                Anchor::Marked,
                Pattern::Hops { count: 3, first: 4.0, grow: 1.5, r: 2.2, every: 0.45 },
                hurt(2, 6, DamageType::Physical),
                look::EARTH,
            )
        },
        // 2: Lay Eggs: fresh eggs round her; break them before they hatch.
        Ability {
            shout: "Grow, my children. Grow!",
            action: Action::Lay { kind: "thief_bug_egg", count: 3, per_player: 1, ring: 3.0, hatch: "thief_bug_larva", hatch_count: 2, hatch_secs: 30.0 },
            ..ability("boss_lay_eggs", "Lay Eggs", 1.2, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 3: Brood Call: a channel; done, every egg nearby hatches and feeds her.
        Ability {
            shout: "Hatch! HATCH!",
            action: Action::HatchAll { secs: 3.0, radius: 20.0, heal: 3, break_pct: 8 },
            ..ability("boss_brood_call", "Brood Call", 0.8, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 4: Pheromone: the bugs around her mend and shake off what ails them.
        Ability {
            action: Action::Channel { secs: 1.0, heal: 0, rally: 15, rally_r: 14.0, break_pct: 100 },
            ..ability("boss_pheromone", "Pheromone", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 5: Swarm Spiral: three arms of larva dives unrolling round her.
        Ability {
            hurt: Hurt { rider: Some((Condition::Poisoned, 2.0)), ..hurt(2, 6, DamageType::Poison) },
            ..ability(
                "boss_swarm_spiral",
                "Swarm Spiral",
                1.3,
                Anchor::Boss,
                Pattern::Spiral { arms: 3, steps: 8, first: 2.0, step: 2.0, turn: 25.0, r: 1.6, every: 0.3 },
                NO_HURT,
                look::POISON,
            )
        },
    ],
};

// --- Rat King: a Tarou with a crown. Stampedes down lanes, a plague trail,
// his crown thrown, the pack called, the pipes gnawed through. -------------

pub(super) const RAT_KING: Script = Script {
    monster: "rat_king",
    rank: Rank::Elite,
    aggro: 16.0,
    intro: "Bow to the King of the Gutter!",
    enrage_secs: 150.0,
    enrage: "Every rat in the city answers to ME!",
    death: "My crown... my... gutter...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 2, 3, 1], gap: 3.2, haste: 1.0 },
        Phase { below: 55, shout: "Hear that? The pipes are full of my children!", opener: Some(4), rotation: &[0, 1, 2, 3, 0], gap: 2.6, haste: 0.9 },
        Phase { below: 25, shout: "SQUEEEEEE!", opener: Some(5), rotation: &[0, 5, 1, 2, 3], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: Rat Tide: ranks of rats stampeding across, a gap in each; step into it.
        Ability {
            shout: "Stampede!",
            shake: 40,
            hurt: Hurt { rider: Some((Condition::Slowed, 1.0)), ..hurt(2, 6, DamageType::Physical) },
            ..ability(
                "boss_rat_tide",
                "Rat Tide",
                1.4,
                Anchor::Boss,
                Pattern::Bars { count: 4, len: 24.0, width: 3.0, gap: 4.5, first: 2.0, advance: 4.0, every: 0.4 },
                NO_HURT,
                look::EARTH,
            )
        },
        // 1: Plague Trail: puddles of filth dropped behind a marked player.
        Ability {
            hurt: Hurt { rider: Some((Condition::Poisoned, 2.0)), save: false, ..hurt(1, 6, DamageType::Poison) },
            ..ability(
                "boss_plague_trail",
                "Plague Trail",
                0.8,
                Anchor::Marked,
                Pattern::Trail { count: 8, r: 1.4, every: 0.45, fuse: 0.6, secs: 5.0, pulse: 1.0 },
                NO_HURT,
                look::SLUDGE,
            )
        },
        // 2: Crown Toss: his crown flies down a line.
        Ability {
            shout: "Catch!",
            ..ability("boss_crown_toss", "Crown Toss", 0.9, Anchor::Nearest, Pattern::Line { len: 16.0, width: 1.6 }, hurt(2, 8, DamageType::Physical), look::GOLD)
        },
        // 3: Call the Pack: rats pour out of the banks.
        Ability {
            shout: "Children, FEAST!",
            action: Action::Summon { kind: "tarou", count: 4, per_player: 2 },
            ..ability("boss_rat_pack", "Call the Pack", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 4: Gnaw the Pipes: a long wind-up behind a shield; break it or the
        //    sewage gushes up round him.
        Ability {
            shout: "Chew, chew, CHEW!",
            action: Action::Shield { pct: 6 },
            shake: 80,
            hurt: Hurt { rider: Some((Condition::Slowed, 1.5)), save: false, ..hurt(1, 6, DamageType::Poison) },
            ..ability("boss_gnaw_pipes", "Gnaw the Pipes", 2.6, Anchor::Boss, Pattern::Pool { r: 6.0, secs: 8.0, pulse: 1.0 }, NO_HURT, look::SLUDGE)
        },
        // 5: Dark Squeal: a shriek that blinds; hug him.
        Ability {
            hurt: Hurt { rider: Some((Condition::Blinded, 2.0)), ..hurt(3, 6, DamageType::Necrotic) },
            ..ability("boss_dark_squeal", "Dark Squeal", 1.4, Anchor::Boss, Pattern::Donut { inner: 3.0, outer: 12.0 }, NO_HURT, look::DARK)
        },
    ],
};

// --- Golden Thief Bug: the hoard's guardian. A shell that turns spells
// back, coins and fire, then burrowing; cracked, it bleeds gold. ---------

pub(super) const GOLDEN_THIEF_BUG: Script = Script {
    monster: "golden_bug",
    rank: Rank::Boss,
    aggro: 22.0,
    intro: "Who dares come near MY gold?!",
    enrage_secs: 300.0,
    enrage: "Mine! All of it is MINE!",
    death: "My gold... my beautiful... gold...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 2, 3, 1, 2], gap: 3.3, haste: 1.0 },
        Phase { below: 70, shout: "You can't reflect what you can't see!", opener: Some(4), rotation: &[6, 0, 5, 4, 2, 7, 1], gap: 2.7, haste: 0.9 },
        Phase { below: 35, shout: "My shell... you CRACKED my shell!", opener: Some(8), rotation: &[6, 0, 1, 2, 8, 6, 5], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: Mammonite: a fist of gold coins smashed down on a marked player.
        Ability {
            shout: "Pay up!",
            shake: 90,
            ..ability("boss_mammonite", "Mammonite", 1.3, Anchor::Marked, Pattern::Circle { r: 4.0 }, hurt(3, 8, DamageType::Physical), look::GOLD)
        },
        // 1: Magnum Break: fire bursts off its shell, then an outer ring.
        Ability {
            shake: 60,
            ..ability(
                "boss_magnum_break",
                "Magnum Break",
                1.2,
                Anchor::Boss,
                Pattern::Rings { count: 2, start: 0.0, width: 4.5, step: 4.5, every: 0.5 },
                hurt(3, 6, DamageType::Fire),
                look::FIRE,
            )
        },
        // 2: Fireball Volley: one at each of you.
        Ability {
            hurt: Hurt { ranged: true, ..hurt(2, 8, DamageType::Fire) },
            ..ability("boss_fireball_volley", "Fireball Volley", 0.9, Anchor::Each(4), Pattern::Line { len: 20.0, width: 1.6 }, NO_HURT, look::FIRE)
        },
        // 3: Gilded Charge: it skids down a lane, leaving gold dust.
        Ability {
            action: Action::Charge,
            shake: 50,
            ..ability("boss_gilded_charge", "Gilded Charge", 1.1, Anchor::Nearest, Pattern::Line { len: 18.0, width: 2.6 }, hurt(3, 6, DamageType::Physical), look::GOLD)
        },
        // 4: Burrow: under the sewage after a marked player; a ripple
        //    follows them and it bursts up where the ripple stands.
        Ability {
            action: Action::Burrow { speed: 10.0 },
            shake: 150,
            hurt: Hurt { rider: Some((Condition::Stunned, 0.8)), ..hurt(4, 8, DamageType::Physical) },
            ..ability("boss_burrow", "Burrow", 3.0, Anchor::Marked, Pattern::Chase { count: 1, r: 3.5, every: 0.0 }, NO_HURT, look::RIPPLE)
        },
        // 5: Escort: Female Thief Bugs crawl out to guard it.
        Ability {
            action: Action::Summon { kind: "thief_bug_female", count: 2, per_player: 1 },
            ..ability("boss_bug_escort", "Escort", 0.8, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 6: Coin Spiral: four arms of falling coins unrolling round it.
        ability(
            "boss_coin_spiral",
            "Coin Spiral",
            1.4,
            Anchor::Boss,
            Pattern::Spiral { arms: 4, steps: 10, first: 2.0, step: 1.8, turn: 20.0, r: 1.4, every: 0.25 },
            hurt(2, 6, DamageType::Force),
            look::GOLD,
        ),
        // 7: Golden Assumptio: a channel that mends it; burst it to break it.
        Ability {
            shout: "My gold protects me!",
            action: Action::Channel { secs: 4.0, heal: 10, rally: 0, rally_r: 0.0, break_pct: 6 },
            ..ability("boss_golden_assumptio", "Golden Assumptio", 0.8, Anchor::Boss, Pattern::None, NO_HURT, look::HOLY)
        },
        // 8: Frenzied Burrow: it bursts up under three of you in turn.
        Ability {
            stagger: 1.0,
            action: Action::Burrow { speed: 12.0 },
            shake: 150,
            hurt: Hurt { rider: Some((Condition::Stunned, 0.8)), ..hurt(4, 8, DamageType::Physical) },
            ..ability("boss_frenzied_burrow", "Frenzied Burrow", 2.2, Anchor::Each(3), Pattern::Chase { count: 1, r: 3.0, every: 0.0 }, NO_HURT, look::RIPPLE)
        },
    ],
};

/// The Golden Thief Bug's shell: spells turn back while it is whole
/// (phases 1 and 2); cracked (phase 3) it takes 25 % more and every 10 % of
/// HP it loses spills zeny to everyone fighting it.
pub(super) const TRAITS: &[Traits] = &[Traits {
    monster: "golden_bug",
    reflect_until: 2,
    vulnerable_from: 2,
    vulnerable_pct: 25,
    burst_from: 2,
    burst_every: 10,
    burst_zeny: (40, 90),
}];
