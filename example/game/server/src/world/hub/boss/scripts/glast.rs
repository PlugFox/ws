//! Glast Heim Churchyard's scripted fights (example/docs/locations/glast_heim.md
//! §2.3-2.5): Dark Illusion, the Bloody Knight and the Dark Lord. Distances
//! are cells (2 per metre), times seconds, as in the parent module.
//!
//! Hell's Judgment silences (no skills, basic attacks only).

use super::{ability, hurt, Ability, Action, Anchor, Hurt, Pattern, Phase, Rank, Script, Traits, EVERYONE, NO_HURT};
use crate::world::combat::{Condition, DamageType};
use crate::world::protocol::look;

// --- Dark Illusion: Moonlight Flower's machinery in black. Shadow bolts,
// rings of dread, images and the fire between them, an eclipse ring. -----

pub(super) const DARK_ILLUSION: Script = Script {
    monster: "dark_illusion",
    rank: Rank::Elite,
    aggro: 16.0,
    intro: "You walk among my dead... then you will stay with them.",
    enrage_secs: 150.0,
    enrage: "Enough games. Let the dark take you!",
    death: "I am... only a shadow of him...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 0], gap: 3.2, haste: 1.0 },
        Phase { below: 60, shout: "Which of us is real? None of us!", opener: Some(3), rotation: &[4, 1, 2, 5], gap: 2.7, haste: 0.9 },
        Phase { below: 25, shout: "The moon is gone. Only the dark remains.", opener: Some(3), rotation: &[4, 5, 1, 6], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: shadow bolt at each of you.
        ability(
            "boss_illusion_bolt",
            "Shadow Bolt",
            1.0,
            Anchor::Each(EVERYONE),
            Pattern::Line { len: 14.0, width: 1.8 },
            Hurt { ranged: true, ..hurt(2, 8, DamageType::Necrotic) },
            look::DARK,
        ),
        // 1: dread: three rings roll out; a failed save sends you running.
        Ability {
            shout: "Fear me!",
            shake: 40,
            ..ability(
                "boss_illusion_dread",
                "Dread",
                1.3,
                Anchor::Boss,
                Pattern::Rings { count: 3, start: 2.0, width: 2.2, step: 4.2, every: 0.45 },
                Hurt { rider: Some((Condition::Feared, 1.5)), ..hurt(2, 6, DamageType::Necrotic) },
                look::CURSE,
            )
        },
        // 2: dark step: it is suddenly beside whoever stands farthest.
        Ability {
            action: Action::Leap,
            ..ability("boss_illusion_step", "Dark Step", 0.8, Anchor::Farthest, Pattern::Circle { r: 3.0 }, hurt(2, 8, DamageType::Force), look::DARK)
        },
        // 3: illusions: two images; it takes no harm while they stand.
        Ability {
            action: Action::Mirror { kind: "dark_illusion_image", count: 2, per_player: 0 },
            ..ability("boss_illusion_images", "Illusions", 1.2, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 4: shadowfire between it and its images, and at you.
        ability(
            "boss_illusion_shadowfire",
            "Shadowfire",
            1.3,
            Anchor::Boss,
            Pattern::Crossfire { width: 2.2, len: 12.0 },
            hurt(3, 8, DamageType::Fire),
            look::FIRE,
        ),
        // 5: grasp: ghost hands follow each of you and bite twice.
        ability(
            "boss_illusion_grasp",
            "Grasp",
            0.9,
            Anchor::Each(EVERYONE),
            Pattern::Chase { count: 2, r: 1.8, every: 0.8 },
            Hurt { rider: Some((Condition::Rooted, 1.0)), ..hurt(2, 8, DamageType::Necrotic) },
            look::CURSE,
        ),
        // 6: eclipse ring: a black sun over it; only its side is safe.
        Ability {
            shout: "Behold... the black sun!",
            shake: 90,
            ..ability(
                "boss_illusion_eclipse",
                "Eclipse Ring",
                1.6,
                Anchor::Boss,
                Pattern::Donut { inner: 3.5, outer: 16.0 },
                hurt(4, 6, DamageType::Necrotic),
                look::DARK,
            )
        },
    ],
};

// --- Bloody Knight: a storm of swords. Charges, a turning cross of
// blades, swords raining point-down, ghost ranks, a pact of blood. -------

pub(super) const BLOODY_KNIGHT: Script = Script {
    monster: "bloody_knight",
    rank: Rank::Elite,
    aggro: 18.0,
    intro: "Intruders in the king's churchyard. Your blood will do.",
    enrage_secs: 150.0,
    enrage: "BLOOD! MORE BLOOD!",
    death: "My oath... was to the king...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 0], gap: 3.2, haste: 1.0 },
        Phase { below: 55, shout: "Guard! To me!", opener: Some(3), rotation: &[4, 1, 5, 2], gap: 2.7, haste: 0.9 },
        Phase { below: 20, shout: "Your strength is mine!", opener: Some(6), rotation: &[4, 7, 5, 1], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: bloody charge down a lane at whoever stands farthest.
        Ability {
            action: Action::Charge,
            shake: 40,
            ..ability(
                "boss_knight_charge",
                "Bloody Charge",
                1.1,
                Anchor::Farthest,
                Pattern::Line { len: 18.0, width: 2.6 },
                Hurt { push: 2.0, ..hurt(3, 8, DamageType::Physical) },
                look::DAMAGE,
            )
        },
        // 1: sword cross: two blades through him turning three times.
        ability(
            "boss_knight_cross",
            "Sword Cross",
            1.2,
            Anchor::Boss,
            Pattern::Cross { arms: 2, len: 12.0, width: 2.2, count: 3, step: 45.0, every: 0.5 },
            hurt(3, 8, DamageType::Physical),
            look::DAMAGE,
        ),
        // 2: rend: three steel arcs at the nearest.
        ability(
            "boss_knight_rend",
            "Rend",
            0.9,
            Anchor::Nearest,
            Pattern::Sweep { count: 3, r: 6.0, arc: 90.0, step: 70.0, every: 0.3 },
            hurt(3, 6, DamageType::Physical),
            look::DAMAGE,
        ),
        // 3: blood guard: Raydrics march in.
        Ability {
            shout: "Guard! To me!",
            action: Action::Summon { kind: "raydric", count: 2, per_player: 1 },
            ..ability("boss_knight_guard", "Blood Guard", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 4: blade storm: swords fall point-down round each of you.
        Ability {
            shake: 30,
            ..ability(
                "boss_knight_storm",
                "Blade Storm",
                1.2,
                Anchor::Each(EVERYONE),
                Pattern::Rain { count: 4, per_player: 0, r: 1.8, spread: 6.0, every: 0.3 },
                hurt(3, 6, DamageType::Physical),
                look::DAMAGE,
            )
        },
        // 5: phantom ranks: ghost knights ride across in four ranks, each
        //    with one gap.
        ability(
            "boss_knight_ranks",
            "Phantom Ranks",
            1.3,
            Anchor::Boss,
            Pattern::Bars { count: 4, len: 22.0, width: 2.5, gap: 3.0, first: 3.0, advance: 4.0, every: 0.55 },
            hurt(3, 8, DamageType::Necrotic),
            look::DARK,
        ),
        // 6: blood pact: he drinks his guard; kill them first.
        Ability {
            shout: "Your strength is mine!",
            action: Action::Absorb { secs: 4.0, heal: 8, radius: 14.0 },
            ..ability("boss_knight_pact", "Blood Pact", 0.0, Anchor::Boss, Pattern::None, NO_HURT, look::HEAL)
        },
        // 7: double charge: at two of you in turn.
        Ability {
            action: Action::Charge,
            stagger: 0.8,
            shake: 40,
            ..ability(
                "boss_knight_double",
                "Double Charge",
                1.0,
                Anchor::Each(2),
                Pattern::Line { len: 18.0, width: 2.6 },
                Hurt { push: 2.0, ..hurt(3, 8, DamageType::Physical) },
                look::DAMAGE,
            )
        },
    ],
};

// --- Dark Lord: the master of the fallen capital. A meteor storm, rings of
// judgment, hands out of the graves, fire pillars, the graves opening, a
// soul drain and the Hell's Gate the party must share. -------------------

pub(super) const DARK_LORD: Script = Script {
    monster: "dark_lord",
    rank: Rank::Boss,
    aggro: 22.0,
    intro: "Kneel. This churchyard is my kingdom, and you are its newest dead.",
    enrage_secs: 300.0,
    enrage: "I have waited a thousand years. I will not wait for you!",
    death: "The capital... falls... again...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 3], gap: 3.2, haste: 1.0 },
        Phase { below: 75, shout: "Shadows of mine, rise!", opener: Some(4), rotation: &[5, 0, 1, 3, 2], gap: 2.8, haste: 0.9 },
        Phase { below: 50, shout: "Graves! Give back your dead!", opener: Some(6), rotation: &[0, 5, 1, 7], gap: 2.5, haste: 0.85 },
        Phase { below: 25, shout: "The gate of hell opens for you!", opener: Some(9), rotation: &[8, 1, 5, 2, 9], gap: 2.0, haste: 0.8 },
    ],
    abilities: &[
        // 0: meteor storm round one of you (+2 meteors per extra player).
        Ability {
            shout: "Fall, stars!",
            shake: 60,
            ..ability(
                "boss_lord_meteor",
                "Meteor Storm",
                1.4,
                Anchor::Marked,
                Pattern::Rain { count: 7, per_player: 2, r: 3.0, spread: 9.0, every: 0.45 },
                Hurt { rider: Some((Condition::Burning, 2.0)), ..hurt(3, 8, DamageType::Fire) },
                look::FIRE,
            )
        },
        // 1: hell's judgment: four rings; the gaps are safe. A failed save
        //    silences: casters must not eat it.
        Ability {
            shake: 120,
            ..ability(
                "boss_lord_judgment",
                "Hell's Judgment",
                1.6,
                Anchor::Boss,
                Pattern::Rings { count: 4, start: 0.0, width: 3.0, step: 5.0, every: 0.5 },
                Hurt { rider: Some((Condition::Silenced, 3.0)), ..hurt(3, 8, DamageType::Necrotic) },
                look::CURSE,
            )
        },
        // 2: grave hands follow each of you and bite three times.
        ability(
            "boss_lord_hands",
            "Grave Hands",
            0.9,
            Anchor::Each(EVERYONE),
            Pattern::Chase { count: 3, r: 1.8, every: 0.7 },
            Hurt { rider: Some((Condition::Rooted, 1.0)), ..hurt(2, 8, DamageType::Necrotic) },
            look::EARTH,
        ),
        // 3: dark bolt at each of you.
        ability(
            "boss_lord_bolt",
            "Dark Bolt",
            1.0,
            Anchor::Each(EVERYONE),
            Pattern::Line { len: 16.0, width: 2.0 },
            Hurt { ranged: true, ..hurt(3, 6, DamageType::Necrotic) },
            look::DARK,
        ),
        // 4: summon illusions: images that bolt on their own.
        Ability {
            action: Action::Summon { kind: "dark_illusion_image", count: 1, per_player: 1 },
            ..ability("boss_lord_illusions", "Summon Illusions", 1.2, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 5: fire pillars: glyph traps that arm behind each of you.
        ability(
            "boss_lord_pillars",
            "Fire Pillars",
            1.0,
            Anchor::Each(EVERYONE),
            Pattern::Trail { count: 5, r: 1.6, every: 0.5, fuse: 1.0, secs: 6.0, pulse: 1.0 },
            Hurt { rider: Some((Condition::Burning, 2.0)), ..hurt(2, 6, DamageType::Fire) },
            look::FIRE,
        ),
        // 6: open the graves: every standing grave near him opens at once,
        //    and he heals for each; burst him to stop it.
        Ability {
            shout: "Graves! Give back your dead!",
            shake: 70,
            action: Action::HatchAll { secs: 4.0, radius: 40.0, heal: 2, break_pct: 8 },
            ..ability("boss_lord_graves", "Open the Graves", 0.6, Anchor::Boss, Pattern::None, NO_HURT, look::DARK)
        },
        // 7: soul drain: he drinks from everyone within reach; get out of
        //    it or break it.
        Ability {
            shout: "Your souls are mine!",
            action: Action::Channel { secs: 3.0, heal: 6, rally: 0, rally_r: 0.0, break_pct: 6 },
            ..ability("boss_lord_drain", "Soul Drain", 1.2, Anchor::Boss, Pattern::Circle { r: 9.0 }, hurt(3, 6, DamageType::Necrotic), look::CURSE)
        },
        // 8: twin meteor: a storm round two of you.
        Ability {
            shake: 60,
            ..ability(
                "boss_lord_twin",
                "Twin Meteor",
                1.3,
                Anchor::Each(2),
                Pattern::Rain { count: 6, per_player: 1, r: 3.0, spread: 8.0, every: 0.45 },
                Hurt { rider: Some((Condition::Burning, 2.0)), ..hurt(3, 8, DamageType::Fire) },
                look::FIRE,
            )
        },
        // 9: hell's gate: one great blow on one of you, shared among all
        //    who stand in it (alone: capped at 8 of its 24 dice).
        Ability {
            shout: "The gate of hell opens! Who will share the toll?",
            shake: 160,
            ..ability(
                "boss_lord_gate",
                "Hell's Gate",
                3.5,
                Anchor::Marked,
                Pattern::Circle { r: 4.0 },
                Hurt { save: false, split: 8, ..hurt(24, 6, DamageType::Fire) },
                look::GLYPH,
            )
        },
    ],
};

/// No traited bodies here.
pub(super) const TRAITS: &[Traits] = &[];

#[cfg(test)]
#[path = "../glast_tests.rs"]
mod tests;
