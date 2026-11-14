//! Sograt Desert's scripted fights (example/docs/locations/sograt.md §2.3-2.5):
//! Sirocco the pack alpha, the Sand Colossus and Phreeoni. Distances are
//! cells (2 per metre), times seconds, as in the parent module.

use super::{ability, hurt, Ability, Action, Anchor, Hurt, Pattern, Phase, Rank, Script, EVERYONE, NO_HURT};
use crate::world::combat::{Condition, DamageType};
use crate::world::protocol::look;

// --- Sirocco: the Desert Wolf alpha. Calls its pack, rakes zig-zags, the
// pack closes in around one of you. -------------------------------------

pub(super) const SIROCCO: Script = Script {
    monster: "sirocco",
    rank: Rank::Elite,
    aggro: 18.0,
    intro: "AWOOOOO! The pack hunts tonight!",
    enrage_secs: 150.0,
    enrage: "No more games. I'll run you down MYSELF!",
    death: "The pack... will remember...",
    phases: &[
        Phase { below: 100, shout: "", opener: Some(0), rotation: &[1, 3, 2, 1, 3], gap: 3.0, haste: 1.0 },
        Phase { below: 66, shout: "Brothers! To me!", opener: Some(4), rotation: &[1, 2, 3, 1, 0], gap: 2.6, haste: 0.92 },
        Phase { below: 33, shout: "Smell that? Blood on the sand!", opener: Some(4), rotation: &[1, 2, 1, 3, 0], gap: 2.2, haste: 0.82 },
    ],
    abilities: &[
        // 0: the howl: pups lope in from the dunes.
        Ability {
            shout: "AWOOOO!",
            action: Action::Summon { kind: "desert_wolf_b", count: 3, per_player: 1 },
            shake: 40,
            ..ability("boss_pack_howl", "Call the Pack", 1.2, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 1: three raking dashes, each from where the last ended.
        Ability {
            stagger: 0.55,
            action: Action::Charge,
            ..ability("boss_sirocco_rush", "Sirocco Rush", 1.0, Anchor::Each(3), Pattern::Line { len: 14.0, width: 2.4 }, hurt(2, 6, DamageType::Physical), look::SAND)
        },
        // 2: the pack closes in: rings shrinking onto one player; step out
        //    before the last one lands on them.
        Ability {
            shout: "Surround it!",
            hurt: Hurt { rider: Some((Condition::Slowed, 2.0)), ..hurt(2, 6, DamageType::Physical) },
            ..ability(
                "boss_encircle",
                "Encircle",
                1.2,
                Anchor::Marked,
                Pattern::Rings { count: 3, start: 9.0, width: 2.0, step: -3.0, every: 0.45 },
                NO_HURT,
                look::DAMAGE,
            )
        },
        // 3: a pounce on a marked player.
        Ability {
            action: Action::Leap,
            hurt: Hurt { rider: Some((Condition::Stunned, 0.8)), ..hurt(2, 8, DamageType::Physical) },
            ..ability("boss_alpha_pounce", "Pounce", 1.1, Anchor::Marked, Pattern::Circle { r: 2.8 }, NO_HURT, look::SAND)
        },
        // 4: the grown wolves answer.
        Ability {
            shout: "Hunters, ARISE!",
            action: Action::Summon { kind: "desert_wolf", count: 2, per_player: 1 },
            shake: 60,
            ..ability("boss_pack_call", "Call the Hunters", 1.4, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
    ],
};

// --- Sand Colossus: a Sandman the size of a dune. Vortexes, waves of sand,
// golems shed off its body. ---------------------------------------------

pub(super) const SAND_COLOSSUS: Script = Script {
    monster: "sand_colossus",
    rank: Rank::Elite,
    aggro: 18.0,
    intro: "Grrrrrn... You walk... on MY body...",
    enrage_secs: 150.0,
    enrage: "The whole desert... rises!",
    death: "Back... to... dust...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 3, 1, 4], gap: 3.2, haste: 1.0 },
        Phase { below: 75, shout: "Grains... become... golems...", opener: Some(2), rotation: &[1, 0, 3, 4, 0], gap: 2.8, haste: 0.95 },
        Phase { below: 50, shout: "", opener: Some(2), rotation: &[4, 1, 0, 3], gap: 2.6, haste: 0.9 },
        Phase { below: 25, shout: "SINK! SINK INTO ME!", opener: Some(2), rotation: &[1, 0, 4, 1, 3], gap: 2.2, haste: 0.85 },
    ],
    abilities: &[
        // 0: a sand vortex under every player: it sucks at the feet and grinds.
        Ability {
            hurt: Hurt { rider: Some((Condition::Mired, 1.5)), save: false, ..hurt(1, 6, DamageType::Earth) },
            ..ability("boss_sand_vortex", "Sand Vortex", 1.2, Anchor::Each(EVERYONE), Pattern::Pool { r: 5.0, secs: 5.0, pulse: 1.0 }, NO_HURT, look::SAND)
        },
        // 1: three dunes rolling out from it; stand in the troughs.
        Ability {
            shout: "The dunes... MOVE!",
            shake: 70,
            ..ability(
                "boss_dune_wave",
                "Dune Wave",
                1.5,
                Anchor::Boss,
                Pattern::Rings { count: 3, start: 3.0, width: 4.0, step: 7.0, every: 0.6 },
                hurt(2, 6, DamageType::Earth),
                look::SAND,
            )
        },
        // 2: it sheds sand golems.
        Ability {
            action: Action::Summon { kind: "sand_man", count: 2, per_player: 1 },
            ..ability("boss_shed_golems", "Shed Golems", 1.5, Anchor::Boss, Pattern::Circle { r: 4.0 }, hurt(1, 8, DamageType::Earth), look::SAND)
        },
        // 3: a fist of packed sand on the nearest player.
        Ability {
            hurt: Hurt { rider: Some((Condition::Stunned, 1.0)), ..hurt(3, 8, DamageType::Physical) },
            shake: 50,
            ..ability("boss_sand_fist", "Sand Fist", 1.4, Anchor::Nearest, Pattern::Circle { r: 4.0 }, NO_HURT, look::EARTH)
        },
        // 4: sand torrents pouring off its arms: a cross, then turned.
        ability(
            "boss_sand_torrent",
            "Sand Torrent",
            1.3,
            Anchor::Boss,
            Pattern::Cross { arms: 2, len: 30.0, width: 3.0, count: 2, step: 45.0, every: 0.9 },
            hurt(2, 8, DamageType::Earth),
            look::SAND,
        ),
    ],
};

// --- Phreeoni: the beast of the Sograt dunes. Charges, sweeps, slams; it
// calls its chicks; under 40 % it travels under the sand to devour. ------

pub(super) const PHREEONI: Script = Script {
    monster: "phreeoni",
    rank: Rank::Boss,
    aggro: 22.0,
    intro: "GRRRAAAHHH! Fresh meat walks on my sand!",
    enrage_secs: 300.0,
    enrage: "HUNGRY! SO HUNGRY!",
    death: "The sand... takes me... back...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 1, 0, 2], gap: 3.0, haste: 1.0 },
        Phase { below: 70, shout: "Little ones! Dinner is served!", opener: Some(3), rotation: &[4, 2, 1, 3, 0, 2], gap: 2.6, haste: 0.95 },
        Phase { below: 40, shout: "You cannot hide on MY sand!", opener: Some(7), rotation: &[5, 6, 2, 4, 1, 7, 3, 5, 6], gap: 2.3, haste: 0.85 },
    ],
    abilities: &[
        // 0: Gore Charge at whoever stands farthest.
        Ability {
            shout: "GRAAAH!",
            action: Action::Charge,
            hurt: Hurt { rider: Some((Condition::Stunned, 0.8)), ..hurt(3, 8, DamageType::Physical) },
            shake: 90,
            ..ability("boss_gore_charge", "Gore Charge", 1.4, Anchor::Farthest, Pattern::Line { len: 28.0, width: 4.4 }, NO_HURT, look::SAND)
        },
        // 1: Tail Sweep: an arc dragged round from the nearest player.
        Ability {
            hurt: Hurt { rider: Some((Condition::Slowed, 2.0)), ..hurt(2, 10, DamageType::Physical) },
            ..ability(
                "boss_tail_sweep",
                "Tail Sweep",
                0.9,
                Anchor::Boss,
                Pattern::Sweep { count: 5, r: 10.0, arc: 70.0, step: 50.0, every: 0.2 },
                NO_HURT,
                look::SAND,
            )
        },
        // 2: Ground Slam: three shockwave rings; slip through the gaps.
        Ability {
            shout: "",
            shake: 140,
            ..ability(
                "boss_ground_slam",
                "Ground Slam",
                1.5,
                Anchor::Boss,
                Pattern::Rings { count: 3, start: 0.0, width: 5.0, step: 7.0, every: 0.5 },
                hurt(3, 6, DamageType::Force),
                look::EARTH,
            )
        },
        // 3: Picky Brood: it lays eggs around itself; each one left standing
        //    hatches into three Picky (break them).
        Ability {
            shout: "Hatch, my chicks! FEAST!",
            action: Action::Lay { kind: "picky_egg", count: 3, per_player: 1, ring: 8.0, hatch: "picky", hatch_count: 3, hatch_secs: 8.0 },
            ..ability("boss_picky_brood", "Picky Brood", 1.2, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 4: Double Charge: two lines, the second from where the first ends.
        Ability {
            stagger: 0.6,
            action: Action::Charge,
            hurt: Hurt { rider: Some((Condition::Stunned, 0.8)), ..hurt(3, 8, DamageType::Physical) },
            shake: 90,
            ..ability("boss_double_charge", "Double Charge", 1.3, Anchor::Each(2), Pattern::Line { len: 26.0, width: 4.0 }, NO_HURT, look::SAND)
        },
        // 5: Sand Grasp: a sinking patch under every player follows them and
        //    bites three times (keep moving, away from the others).
        Ability {
            hurt: Hurt { rider: Some((Condition::Mired, 1.5)), ..hurt(2, 6, DamageType::Earth) },
            ..ability("boss_burrow_chase", "Sand Grasp", 1.0, Anchor::Each(EVERYONE), Pattern::Chase { count: 3, r: 2.6, every: 0.8 }, NO_HURT, look::SAND)
        },
        // 6: Devour: it dives under the sand, tunnels after a marked player
        //    (a heaving mound, a churning patch following them) and bursts up
        //    under them.
        Ability {
            shout: "DEVOUR!",
            action: Action::Burrow { speed: 9.0 },
            hurt: Hurt { rider: Some((Condition::Stunned, 1.0)), ..hurt(4, 8, DamageType::Physical) },
            shake: 200,
            ..ability("boss_devour", "Devour", 3.0, Anchor::Marked, Pattern::Chase { count: 1, r: 5.0, every: 0.0 }, NO_HURT, look::SAND)
        },
        // 7: Sandstorm Roar: the air itself turns to sand; only its feet are calm.
        Ability {
            shout: "Sand, BURY THEM!",
            shake: 120,
            hurt: Hurt { rider: Some((Condition::Blinded, 2.0)), ..hurt(3, 8, DamageType::Earth) },
            ..ability("boss_sandstorm_roar", "Sandstorm Roar", 1.8, Anchor::Boss, Pattern::Donut { inner: 5.0, outer: 22.0 }, NO_HURT, look::SAND)
        },
    ],
};
