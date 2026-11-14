//! Payon Caves' scripted fights (example/docs/locations/payon.md §5-6): the
//! Weeping Bride, the Jiangshi General and Moonlight Flower. Distances are
//! cells (2 per metre), times seconds, as in the parent module.

use super::{ability, hurt, Ability, Action, Anchor, Hurt, Pattern, Phase, Rank, Script, EVERYONE, NO_HURT};
use crate::world::combat::{Condition, DamageType};
use crate::world::protocol::look;

// --- Weeping Bride: a Sohee in a white veil. Wails that terrify, ghost
// hands that follow you, tears and petals, a procession of brides. ------

pub(super) const WEEPING_BRIDE: Script = Script {
    monster: "weeping_bride",
    rank: Rank::Elite,
    aggro: 16.0,
    intro: "Have you seen my groom? ...You will stay with me instead.",
    enrage_secs: 150.0,
    enrage: "Why does everyone LEAVE me?!",
    death: "At last... the wedding bells...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 0], gap: 3.2, haste: 1.0 },
        Phase { below: 60, shout: "My bridesmaids! Come and greet our guests!", opener: Some(4), rotation: &[2, 3, 0, 1, 5], gap: 2.7, haste: 0.9 },
        Phase { below: 25, shout: "Nobody leaves my wedding!", opener: Some(6), rotation: &[0, 3, 1, 2, 5], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: wail: three rings of sorrow roll out; a failed Mind save
        //    sends you running from her.
        Ability {
            shout: "Waaaaah...!",
            shake: 40,
            hurt: Hurt { rider: Some((Condition::Feared, 1.5)), ..hurt(2, 6, DamageType::Necrotic) },
            ..ability(
                "boss_bride_wail",
                "Wail",
                1.3,
                Anchor::Boss,
                Pattern::Rings { count: 3, start: 2.0, width: 2.5, step: 3.5, every: 0.45 },
                NO_HURT,
                look::CURSE,
            )
        },
        // 1: veil drift: ghost hands rise under one of you and follow.
        ability(
            "boss_bride_veil",
            "Veil Drift",
            0.8,
            Anchor::Marked,
            Pattern::Chase { count: 3, r: 1.6, every: 1.0 },
            hurt(3, 6, DamageType::Necrotic),
            look::DARK,
        ),
        // 2: tears: a cold pool under everyone.
        ability(
            "boss_bride_tears",
            "Tears",
            1.2,
            Anchor::Each(EVERYONE),
            Pattern::Pool { r: 2.4, secs: 8.0, pulse: 1.0 },
            Hurt { rider: Some((Condition::Slowed, 1.5)), save: false, ..hurt(1, 6, DamageType::Cold) },
            look::COLD,
        ),
        // 3: bouquet: lily petals spiral down around one of you.
        Ability {
            shout: "Catch!",
            ..ability(
                "boss_bride_bouquet",
                "Bouquet",
                1.0,
                Anchor::Marked,
                Pattern::Spiral { arms: 1, steps: 5, first: 0.0, step: 1.3, turn: 72.0, r: 1.6, every: 0.25 },
                hurt(2, 6, DamageType::Force),
                look::PETALS,
            )
        },
        // 4: procession: her bridesmaids (Sohee) walk in.
        Ability {
            action: Action::Summon { kind: "sohee", count: 3, per_player: 1 },
            ..ability("boss_bride_procession", "Procession", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 5: lament: she weeps her dead back to strength; burst her to stop it.
        Ability {
            shout: "Rise, all of you... do not leave me alone...",
            action: Action::Channel { secs: 4.0, heal: 12, rally: 30, rally_r: 14.0, break_pct: 8 },
            ..ability("boss_bride_lament", "Lament", 0.0, Anchor::Boss, Pattern::None, NO_HURT, look::HEAL)
        },
        // 6: wail of the abandoned: only her side is quiet.
        Ability {
            shout: "STAY WITH ME!",
            shake: 90,
            hurt: Hurt { rider: Some((Condition::Feared, 2.0)), ..hurt(4, 6, DamageType::Necrotic) },
            ..ability(
                "boss_bride_abandoned",
                "Wail of the Abandoned",
                1.6,
                Anchor::Boss,
                Pattern::Donut { inner: 3.5, outer: 18.0 },
                NO_HURT,
                look::CURSE,
            )
        },
    ],
};

// --- Jiangshi General: a giant Bongun. Talisman seals, stiff leaps, a
// drilled squad of hopping dead marching in ranks. -----------------------

pub(super) const JIANGSHI_GENERAL: Script = Script {
    monster: "jiangshi_general",
    rank: Rank::Elite,
    aggro: 16.0,
    intro: "Squad! Fall in! Fresh recruits for the coffins!",
    enrage_secs: 150.0,
    enrage: "Enough drills! CHARGE!",
    death: "Dis... missed...",
    phases: &[
        Phase { below: 100, shout: "", opener: Some(2), rotation: &[0, 1, 3, 0], gap: 3.2, haste: 1.0 },
        Phase { below: 55, shout: "By the seal of the emperor, HALT!", opener: Some(4), rotation: &[1, 3, 0, 5], gap: 2.6, haste: 0.9 },
        Phase { below: 20, shout: "Reinforcements! Out of your coffins!", opener: Some(2), rotation: &[3, 0, 1, 5, 3], gap: 2.2, haste: 0.8 },
    ],
    abilities: &[
        // 0: talisman seal: a paper glyph under each of you; step off it.
        Ability {
            stagger: 0.3,
            hurt: Hurt { rider: Some((Condition::Rooted, 2.0)), ..hurt(2, 8, DamageType::Necrotic) },
            ..ability("boss_general_seal", "Talisman Seal", 1.4, Anchor::Each(EVERYONE), Pattern::Circle { r: 1.8 }, NO_HURT, look::GLYPH)
        },
        // 1: stiff leap: four heavy hops at one of you.
        Ability {
            action: Action::Leap,
            shake: 50,
            ..ability(
                "boss_general_leap",
                "Stiff Leap",
                1.0,
                Anchor::Marked,
                Pattern::Hops { count: 4, first: 5.0, grow: 1.0, r: 2.2, every: 0.45 },
                hurt(3, 6, DamageType::Physical),
                look::EARTH,
            )
        },
        // 2: raise the squad: Bongun climb out of the floor.
        Ability {
            shout: "Squad, RISE!",
            shake: 30,
            action: Action::Summon { kind: "bongun", count: 3, per_player: 2 },
            ..ability("boss_general_squad", "Raise the Squad", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 3: rank and file: four ranks of talismans march across, each with
        //    one gap.
        Ability {
            shout: "Left! Right! Left!",
            ..ability(
                "boss_general_ranks",
                "Rank and File",
                1.2,
                Anchor::Boss,
                Pattern::Bars { count: 4, len: 30.0, width: 3.0, gap: 4.0, first: 3.0, advance: 4.0, every: 0.6 },
                hurt(3, 8, DamageType::Physical),
                look::GLYPH,
            )
        },
        // 4: sealed command: a talisman shield, then a burst of seals round
        //    him; break the shield and the burst never comes.
        Ability {
            shout: "The emperor's seal protects me!",
            action: Action::Shield { pct: 12 },
            shake: 60,
            hurt: Hurt { rider: Some((Condition::Stunned, 1.0)), ..hurt(3, 8, DamageType::Necrotic) },
            ..ability(
                "boss_general_command",
                "Sealed Command",
                3.0,
                Anchor::Boss,
                Pattern::Rings { count: 3, start: 0.0, width: 3.0, step: 4.5, every: 0.4 },
                NO_HURT,
                look::GLYPH,
            )
        },
        // 5: coffin slam: the ground around him caves in.
        Ability {
            shake: 120,
            ..ability("boss_general_slam", "Coffin Slam", 1.4, Anchor::Boss, Pattern::Circle { r: 4.5 }, hurt(3, 8, DamageType::Physical), look::EARTH)
        },
    ],
};

// --- Moonlight Flower: the nine-tailed fox maiden. Fox-fire spirals, her
// bell, moon petals, mirror images, the pack, a night without lanterns. ---

pub(super) const MOONLIGHT_FLOWER: Script = Script {
    monster: "moonlight_flower",
    rank: Rank::Boss,
    aggro: 22.0,
    intro: "Tee-hee! Visitors, this deep? Come, let us play under the moon~",
    enrage_secs: 300.0,
    enrage: "Playtime is OVER. Burn, little moths!",
    death: "Ah... the moon... is setting...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 2, 3, 0, 1], gap: 3.2, haste: 1.0 },
        Phase { below: 75, shout: "Which of us is the real one? Guess!", opener: Some(4), rotation: &[5, 2, 1, 3, 5], gap: 2.8, haste: 0.9 },
        Phase { below: 50, shout: "My foxes! Come out and play!", opener: Some(6), rotation: &[0, 7, 1, 2, 6], gap: 2.5, haste: 0.85 },
        Phase { below: 25, shout: "Let the lanterns go out...", opener: Some(8), rotation: &[7, 5, 1, 0, 8], gap: 2.0, haste: 0.8 },
    ],
    abilities: &[
        // 0: fox-fire spiral: three arms of fox fire unroll from her.
        ability(
            "boss_moon_spiral",
            "Fox Fire Spiral",
            1.2,
            Anchor::Boss,
            Pattern::Spiral { arms: 3, steps: 8, first: 2.5, step: 2.0, turn: 25.0, r: 1.4, every: 0.12 },
            hurt(2, 8, DamageType::Fire),
            look::FIRE,
        ),
        // 1: bell toll: three rings of sound; stand in a gap, or right under
        //    her bell.
        Ability {
            shout: "Listen to my bell~",
            shake: 90,
            hurt: Hurt { rider: Some((Condition::Stunned, 1.5)), ..hurt(3, 6, DamageType::Force) },
            ..ability(
                "boss_moon_bell",
                "Bell Toll",
                1.6,
                Anchor::Boss,
                Pattern::Rings { count: 3, start: 2.0, width: 3.0, step: 5.0, every: 0.5 },
                NO_HURT,
                look::GOLD,
            )
        },
        // 2: moon petals: crescent petals follow each of you and fall twice.
        ability(
            "boss_moon_petals",
            "Moon Petals",
            1.0,
            Anchor::Each(EVERYONE),
            Pattern::Chase { count: 2, r: 1.8, every: 0.6 },
            hurt(2, 6, DamageType::Cold),
            look::PETALS,
        ),
        // 3: blink: she is suddenly beside whoever stands farthest.
        Ability {
            action: Action::Leap,
            ..ability("boss_moon_blink", "Blink", 0.8, Anchor::Farthest, Pattern::Circle { r: 3.0 }, hurt(2, 8, DamageType::Force), look::HOLY)
        },
        // 4: mirror image: two copies; she takes no harm while they stand.
        Ability {
            action: Action::Mirror { kind: "moonlight_mirror", count: 2, per_player: 0 },
            ..ability("boss_moon_mirror", "Mirror Image", 1.2, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 5: crossfire: fox fire between her and her images, and at you.
        ability(
            "boss_moon_crossfire",
            "Crossfire",
            1.3,
            Anchor::Boss,
            Pattern::Crossfire { width: 2.4, len: 24.0 },
            hurt(3, 8, DamageType::Fire),
            look::FIRE,
        ),
        // 6: call the pack: Nine Tails run in.
        Ability {
            shake: 40,
            action: Action::Summon { kind: "nine_tail", count: 3, per_player: 1 },
            ..ability("boss_moon_pack", "Call the Pack", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 7: blink chain: she lands by three of you in turn, ringing her bell.
        Ability {
            action: Action::Leap,
            stagger: 0.7,
            hurt: Hurt { rider: Some((Condition::Stunned, 1.0)), ..hurt(2, 8, DamageType::Force) },
            ..ability("boss_moon_chain", "Blink Chain", 0.9, Anchor::Each(3), Pattern::Circle { r: 2.6 }, NO_HURT, look::GOLD)
        },
        // 8: lanternless night: the light dies; only the lit braziers (and
        //    a moonlit circle or two when too few burn) are safe, unless her
        //    shield breaks first.
        Ability {
            shout: "Hush now... the lanterns are going out...",
            action: Action::Shield { pct: 10 },
            shake: 150,
            hurt: Hurt { save: false, ..hurt(10, 10, DamageType::Cold) },
            ..ability(
                "boss_moon_night",
                "Lanternless Night",
                6.0,
                Anchor::Boss,
                Pattern::LitSanctuary { r: 40.0, lit_r: 4.0, min: 2, safe_r: 2.6, dist: 9.0 },
                NO_HURT,
                look::DARK,
            )
        },
    ],
};
