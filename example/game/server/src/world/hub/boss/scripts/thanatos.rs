//! Thanatos Tower's scripted fights (example/docs/locations/thanatos_tower.md
//! §2.3-2.5): two fragments of the dead hero's mind guarding the stairs,
//! Odium (hatred, 3:00) and Despero (despair, 5:00), and Thanatos himself
//! (10:00), whose kit changes with every phase: a recital of the ladder's
//! bosses in four movements. Distances are cells (2 per metre), times
//! seconds, as in the parent module.

use super::{ability, hurt, Ability, Action, Anchor, Hurt, Pattern, Phase, Rank, Script, Traits, EVERYONE, NO_HURT};
use crate::world::combat::{Condition, DamageType};
use crate::world::protocol::look;

// --- Odium of Thanatos: hatred. It does not chase; it stands off and
// shoots, recoils from melee, and burns behind a shield. ---------------------

pub(super) const ODIUM: Script = Script {
    monster: "tha_odium",
    rank: Rank::Elite,
    aggro: 20.0,
    intro: "You... you are the ones who left me here. I HATE you.",
    enrage_secs: 150.0,
    enrage: "Hate is all that is left of me!",
    death: "Even hatred... fades...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 0], gap: 3.2, haste: 1.0 },
        Phase { below: 60, shout: "Burn with me!", opener: Some(3), rotation: &[4, 0, 2, 1], gap: 2.7, haste: 0.9 },
        Phase { below: 25, shout: "I will not be forgotten!", opener: Some(3), rotation: &[4, 5, 1, 2], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: Hate Ray: a red beam at the farthest of you (it prefers the
        //    back line); sidestep.
        Ability {
            hurt: Hurt { ranged: true, ..hurt(3, 8, DamageType::Force) },
            ..ability("boss_hate_ray", "Hate Ray", 1.1, Anchor::Farthest, Pattern::Line { len: 22.0, width: 2.0 }, NO_HURT, look::DAMAGE)
        },
        // 1: Spite: a curse circle under each of you; step off.
        ability("boss_spite", "Spite", 1.0, Anchor::Each(EVERYONE), Pattern::Circle { r: 2.0 }, hurt(2, 8, DamageType::Necrotic), look::CURSE),
        // 2: Recoil: it blinks away from melee; its landing bursts.
        Ability {
            action: Action::Retreat { dist: 12.0 },
            hurt: Hurt { push: 3.0, ..hurt(2, 8, DamageType::Force) },
            ..ability("boss_recoil", "Recoil", 0.8, Anchor::Boss, Pattern::Circle { r: 4.0 }, NO_HURT, look::DARK)
        },
        // 3: Burning Hatred: a shield, then rings of fire; break it.
        Ability {
            shout: "Feel what I feel!",
            action: Action::Shield { pct: 12 },
            shake: 90,
            hurt: Hurt { rider: Some((Condition::Burning, 3.0)), ..hurt(3, 8, DamageType::Fire) },
            ..ability(
                "boss_burning_hatred",
                "Burning Hatred",
                3.0,
                Anchor::Boss,
                Pattern::Rings { count: 3, start: 0.0, width: 2.5, step: 4.5, every: 0.5 },
                NO_HURT,
                look::FIRE,
            )
        },
        // 4: Cross of Spite: red beams turning; walk with them.
        ability(
            "boss_cross_of_spite",
            "Cross of Spite",
            1.3,
            Anchor::Boss,
            Pattern::Cross { arms: 2, len: 16.0, width: 2.0, count: 4, step: 22.5, every: 0.45 },
            hurt(3, 8, DamageType::Force),
            look::DAMAGE,
        ),
        // 5: Triple Ray: a beam at three of you, one after another.
        Ability {
            stagger: 0.5,
            hurt: Hurt { ranged: true, ..hurt(3, 8, DamageType::Force) },
            ..ability("boss_triple_ray", "Triple Ray", 1.0, Anchor::Each(3), Pattern::Line { len: 22.0, width: 2.0 }, NO_HURT, look::DAMAGE)
        },
    ],
};

// --- Despero of Thanatos: despair. Fear and weight, walls closing in, and
// a kneeling channel that rallies the hall. ---------------------------------

pub(super) const DESPERO: Script = Script {
    monster: "tha_despero",
    rank: Rank::Elite,
    aggro: 18.0,
    intro: "Why do you still climb? There is nothing at the top.",
    enrage_secs: 150.0,
    enrage: "Give up. Everyone does, in the end.",
    death: "Perhaps... there was... hope...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 0], gap: 3.2, haste: 1.0 },
        Phase { below: 55, shout: "The walls are closing in... can you feel them?", opener: Some(3), rotation: &[1, 4, 2, 5], gap: 2.7, haste: 0.9 },
        Phase { below: 20, shout: "No way out. No way up. No way at all.", opener: Some(3), rotation: &[4, 0, 1, 5], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: Wave of Despair: rings of dread; stand in the gaps.
        Ability {
            hurt: Hurt { rider: Some((Condition::Feared, 1.5)), ..hurt(2, 8, DamageType::Necrotic) },
            ..ability(
                "boss_wave_of_despair",
                "Wave of Despair",
                1.3,
                Anchor::Boss,
                Pattern::Rings { count: 3, start: 2.0, width: 2.4, step: 4.4, every: 0.45 },
                NO_HURT,
                look::CURSE,
            )
        },
        // 1: Heavy Heart: grey pools under each of you; leave them.
        Ability {
            hurt: Hurt { rider: Some((Condition::Slowed, 1.0)), save: false, ..hurt(1, 8, DamageType::Necrotic) },
            ..ability("boss_heavy_heart", "Heavy Heart", 1.1, Anchor::Each(EVERYONE), Pattern::Pool { r: 2.4, secs: 8.0, pulse: 1.0, pull: 0.0 }, NO_HURT, look::DARK)
        },
        // 2: Crushing Blow: it leaps onto one of you.
        Ability {
            action: Action::Leap,
            shake: 100,
            hurt: Hurt { rider: Some((Condition::Stunned, 1.0)), ..hurt(4, 6, DamageType::Physical) },
            ..ability("boss_crushing_blow", "Crushing Blow", 1.3, Anchor::Marked, Pattern::Circle { r: 3.4 }, NO_HURT, look::DAMAGE)
        },
        // 3: Hopelessness: a grey ring closing; hug it.
        Ability {
            shout: "Come closer. It is warmer here.",
            hurt: Hurt { rider: Some((Condition::Feared, 2.0)), ..hurt(4, 6, DamageType::Necrotic) },
            ..ability("boss_hopelessness", "Hopelessness", 1.6, Anchor::Boss, Pattern::Donut { inner: 3.5, outer: 18.0 }, NO_HURT, look::CURSE)
        },
        // 4: Closing Walls: walls march in toward it, a gap in each.
        ability(
            "boss_closing_walls",
            "Closing Walls",
            1.3,
            Anchor::Boss,
            Pattern::Bars { count: 5, len: 24.0, width: 2.5, gap: 3.0, first: 14.0, advance: -3.0, every: 0.5 },
            hurt(3, 8, DamageType::Force),
            look::DARK,
        ),
        // 5: Give Up: it kneels and the hall mends round it; burst it.
        Ability {
            shout: "Rest. Just... rest.",
            action: Action::Channel { secs: 4.0, heal: 12, rally: 20, rally_r: 14.0, break_pct: 8 },
            ..ability("boss_give_up", "Give Up", 0.6, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
    ],
};

// --- Thanatos: the phantom of the tower. Four phases, one memory each: the
// Swordsman, the Mage, the Burden, Oblivion. Two fragments take a body when
// he calls them (he mends while one lives: hub/thanatos.rs). ------------------

pub(super) const THANATOS: Script = Script {
    monster: "thanatos",
    rank: Rank::Boss,
    aggro: 24.0,
    intro: "Who climbs my tower? ...I knew your faces once.",
    enrage_secs: 330.0,
    enrage: "I remember NOTHING but the end!",
    death: "...I remember... now. Thank you.",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 0], gap: 3.0, haste: 1.0 },
        Phase { below: 75, shout: "I was a mage once... I remember fire. And frost.", opener: Some(3), rotation: &[5, 6, 7, 1, 5], gap: 2.8, haste: 0.9 },
        Phase { below: 50, shout: "So heavy... carry it with me!", opener: Some(4), rotation: &[8, 9, 2, 7, 10], gap: 2.6, haste: 0.85 },
        Phase { below: 25, shout: "Forget. Forget it all. OBLIVION!", opener: Some(12), rotation: &[11, 5, 9, 0, 12], gap: 2.4, haste: 0.8 },
    ],
    abilities: &[
        // 0: Blade Sweep: a pale greatsword arc turning; stay behind its edge.
        Ability {
            hurt: Hurt { push: 2.0, ..hurt(4, 6, DamageType::Physical) },
            ..ability(
                "boss_blade_sweep",
                "Blade Sweep",
                1.1,
                Anchor::Boss,
                Pattern::Sweep { count: 4, r: 8.0, arc: 110.0, step: 90.0, every: 0.3 },
                NO_HURT,
                look::DAMAGE,
            )
        },
        // 1: Hero's Charge: he charges two of you in turn.
        Ability {
            stagger: 0.8,
            action: Action::Charge,
            shake: 60,
            ..ability("boss_heros_charge", "Hero's Charge", 1.2, Anchor::Each(2), Pattern::Line { len: 20.0, width: 2.8 }, hurt(4, 8, DamageType::Physical), look::DAMAGE)
        },
        // 2: Shockwave: gold rings roll out; stand in the gaps.
        Ability {
            shake: 140,
            hurt: Hurt { rider: Some((Condition::Stunned, 1.0)), ..hurt(3, 8, DamageType::Force) },
            ..ability(
                "boss_memory_shockwave",
                "Shockwave",
                1.5,
                Anchor::Boss,
                Pattern::Rings { count: 4, start: 0.0, width: 3.0, step: 5.0, every: 0.45 },
                NO_HURT,
                look::GOLD,
            )
        },
        // 3: Dolor: the mask of agony leaves his orbit and takes a body.
        Ability {
            shout: "Agony... go.",
            action: Action::Summon { kind: "tha_dolor", count: 1, per_player: 0 },
            ..ability("boss_call_dolor", "Dolor", 1.2, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 4: Maero: the mask of sorrow takes a body.
        Ability {
            shout: "Sorrow... go.",
            action: Action::Summon { kind: "tha_maero", count: 1, per_player: 0 },
            ..ability("boss_call_maero", "Maero", 1.2, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 5: Meteor Storm: meteors rain round each of you; watch the shadows.
        Ability {
            shake: 70,
            ..ability(
                "boss_meteor_storm",
                "Meteor Storm",
                1.3,
                Anchor::Each(EVERYONE),
                Pattern::Rain { count: 4, per_player: 0, r: 2.6, spread: 7.0, every: 0.4 },
                hurt(3, 8, DamageType::Fire),
                look::FIRE,
            )
        },
        // 6: Frost Rings: rings of frost round one of you; that one runs out.
        Ability {
            hurt: Hurt { rider: Some((Condition::Frozen, 1.2)), ..hurt(3, 6, DamageType::Cold) },
            ..ability(
                "boss_frost_rings",
                "Frost Rings",
                1.2,
                Anchor::Marked,
                Pattern::Rings { count: 3, start: 0.0, width: 2.5, step: 4.0, every: 0.5 },
                NO_HURT,
                look::COLD,
            )
        },
        // 7: Thunder Cross: six bolts turning; walk with them.
        ability(
            "boss_thunder_cross",
            "Thunder Cross",
            1.3,
            Anchor::Boss,
            Pattern::Cross { arms: 3, len: 18.0, width: 2.0, count: 4, step: 20.0, every: 0.4 },
            hurt(3, 8, DamageType::Lightning),
            look::LIGHTNING,
        ),
        // 8: Grasping Memories: hands of light follow each of you; keep moving.
        Ability {
            hurt: Hurt { rider: Some((Condition::Rooted, 0.8)), ..hurt(2, 8, DamageType::Necrotic) },
            ..ability("boss_grasping_memories", "Grasping Memories", 0.9, Anchor::Each(EVERYONE), Pattern::Chase { count: 4, r: 1.8, every: 0.7 }, NO_HURT, look::CURSE)
        },
        // 9: Burden: a mask descends on one of you; everyone inside shares
        //    the blow (alone it is still heavy).
        Ability {
            shout: "Who will carry this with you?",
            shake: 160,
            hurt: Hurt { split: 8, save: false, ..hurt(30, 6, DamageType::Force) },
            ..ability("boss_burden", "Burden", 3.5, Anchor::Marked, Pattern::Circle { r: 4.5 }, NO_HURT, look::GOLD)
        },
        // 10: Trail of Sorrow: tears fall behind each of you; run straight.
        Ability {
            hurt: Hurt { rider: Some((Condition::Slowed, 1.0)), save: false, ..hurt(2, 6, DamageType::Cold) },
            ..ability(
                "boss_trail_of_sorrow",
                "Trail of Sorrow",
                1.0,
                Anchor::Each(EVERYONE),
                Pattern::Trail { count: 6, r: 1.6, every: 0.45, fuse: 1.0, secs: 6.0, pulse: 1.0 },
                NO_HURT,
                look::COLD,
            )
        },
        // 11: Blink Strikes: he is on each of you in turn; watch the order.
        Ability {
            stagger: 0.6,
            action: Action::Leap,
            ..ability("boss_blink_strikes", "Blink Strikes", 1.0, Anchor::Each(EVERYONE), Pattern::Circle { r: 2.8 }, hurt(3, 8, DamageType::Force), look::DARK)
        },
        // 12: Oblivion: the hall goes white; break his shield, or stand in a
        //     memory (the statues judge meanwhile: hub/thanatos.rs).
        Ability {
            shout: "Let it all be forgotten!",
            action: Action::Shield { pct: 10 },
            shake: 200,
            hurt: Hurt { save: false, ..hurt(12, 10, DamageType::Force) },
            ..ability(
                "boss_oblivion",
                "Oblivion",
                5.0,
                Anchor::Boss,
                Pattern::Sanctuary { r: 28.0, safe: 4, safe_r: 3.0, dist: 11.0 },
                NO_HURT,
                look::HOLY,
            )
        },
    ],
};

/// Thanatos is a ghost (physical halved) whose armour breaks as his
/// memories do: from phase 3 he takes 20 % more, so melee is not shut out
/// of its own finale.
pub(super) const TRAITS: &[Traits] = &[Traits {
    monster: "thanatos",
    reflect_until: 0,
    vulnerable_from: 2,
    vulnerable_pct: 20,
    burst_from: 0,
    burst_every: 0,
    burst_zeny: (0, 0),
}];
