//! Encounter scripts of the scripted elites and bosses (COMBAT.md "Boss
//! encounters"): pure data, read by `hub::boss`.
//!
//! A script is phases by HP threshold; each phase runs a fixed rotation of
//! abilities (readable, learnable: never a random pick), with a gap between
//! them and a haste on wind-ups. An ability is an anchor (where), a pattern
//! (what shape, in which beats), a hurt (what a beat does to a player in
//! it) and an action (what the monster itself does: charge, leap, channel,
//! summon, shield). Distances are cells (2 per metre), times seconds.

use crate::world::combat::{Condition, DamageType};
use crate::world::protocol::look;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Rank {
    Elite,
    Boss,
}

#[derive(Debug)]
pub(crate) struct Script {
    /// Monster id of the catalogue.
    pub monster: &'static str,
    pub rank: Rank,
    /// The fight starts when a standing player comes this close.
    pub aggro: f32,
    /// Yelled when the fight starts.
    pub intro: &'static str,
    /// Soft enrage: after this long, wind-ups shorten and beats hit harder.
    pub enrage_secs: f32,
    pub enrage: &'static str,
    /// Yelled as it falls (the log line outlives its bubble).
    pub death: &'static str,
    /// First phase at 100 %; each later one starts at its `below` HP %.
    pub phases: &'static [Phase],
    pub abilities: &'static [Ability],
}

#[derive(Debug)]
pub(crate) struct Phase {
    pub below: u8,
    /// Yelled on entering the phase (empty: silent).
    pub shout: &'static str,
    /// Cast first on entering the phase.
    pub opener: Option<u8>,
    /// Ability indices, in order, round and round.
    pub rotation: &'static [u8],
    /// Seconds between the end of one ability and the next.
    pub gap: f32,
    /// Wind-up multiplier (0.8 = 20 % faster).
    pub haste: f32,
}

#[derive(Debug)]
pub(crate) struct Ability {
    /// Effects key (effects.toml `[skills.<id>]`) and log id.
    pub id: &'static str,
    /// The cast bar's label.
    pub name: &'static str,
    /// Wind-up before the first beat.
    pub wind: f32,
    /// Yelled as the cast starts (empty: silent).
    pub shout: &'static str,
    pub anchor: Anchor,
    /// Between successive anchors of [`Anchor::Each`] (a chain of leaps).
    pub stagger: f32,
    pub pattern: Pattern,
    pub hurt: Hurt,
    /// Telegraph look of its zones.
    pub look: u8,
    pub action: Action,
}

/// Where an ability lands.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Anchor {
    /// On the monster, aimed at the nearest player.
    Boss,
    /// On the nearest standing player.
    Nearest,
    /// On one standing player picked by the stage's dice.
    Marked,
    /// On each standing player, up to this many (scales with the party).
    Each(u8),
}

/// The shape of an ability and the beats it resolves in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Pattern {
    /// No zone (summons, channels without a blast).
    None,
    Circle { r: f32 },
    /// Safe inside `inner` (stand close), hurts out to `outer`.
    Donut { inner: f32, outer: f32 },
    /// Expanding waves: ring `i` spans `start + i·step .. + width` and
    /// resolves `i·every` after the first; the gaps between rings are safe.
    Rings { count: u8, start: f32, width: f32, step: f32, every: f32 },
    /// `arms` bars through the anchor, evenly turned (2 = a plus); the cross
    /// repeats `count` times, turned `step` degrees, `every` seconds apart.
    Cross { arms: u8, len: f32, width: f32, count: u8, step: f32, every: f32 },
    /// A cone of `arc` degrees that sweeps: `count` beats turning `step`
    /// degrees each, `every` seconds apart, starting at the aim.
    Sweep { count: u8, r: f32, arc: f32, step: f32, every: f32 },
    /// A bar from the monster toward the anchor (charges, bolts).
    Line { len: f32, width: f32 },
    /// Bounding hops from the monster toward the anchor: `count` landing
    /// circles, the first `first` away, each hop `grow` longer.
    Hops { count: u8, first: f32, grow: f32, r: f32, every: f32 },
    /// Lasting ground: bites every `pulse` for `secs`.
    Pool { r: f32, secs: f32, pulse: f32 },
    /// A nova of radius `r` on the monster; `safe` sanctuaries of `safe_r`
    /// stand `dist` away around it, and whoever is inside one is spared.
    Sanctuary { r: f32, safe: u8, safe_r: f32, dist: f32 },
}

/// What a beat does to a player inside.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Hurt {
    /// Dice count and sides (0 = no damage); + level / 4.
    pub dice: (u8, u8),
    pub kind: DamageType,
    /// A condition for `secs` (a successful save resists it).
    pub rider: Option<(Condition, f32)>,
    /// A save halves the damage (mechanics you fail take it all).
    pub save: bool,
    /// A projectile: a ranged ward (Pneuma) turns it aside.
    pub ranged: bool,
}

/// What the monster itself does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Action {
    None,
    /// It slides along its line to the far end when the line resolves.
    Charge,
    /// It lands on each beat's circle in turn (leaps, hops, chains).
    Leap,
    /// It blinks `dist` away from the nearest player as the wind-up ends;
    /// the pattern is centred on where it lands.
    Retreat { dist: f32 },
    /// It calls adds of `kind` around itself: `count` + `per_player` each.
    Summon { kind: &'static str, count: u8, per_player: u8 },
    /// It stands and channels for `secs`: at the end it heals `heal` % of
    /// its HP, and monsters within `rally_r` heal `rally` % and shed their
    /// conditions. Burst damage of `break_pct` % of its HP (per player)
    /// breaks the channel and stuns it.
    Channel { secs: f32, heal: u8, rally: u8, rally_r: f32, break_pct: u8 },
    /// It channels for `secs`, then swallows its own adds within `radius`,
    /// healing `heal` % each: kill them first.
    Absorb { secs: f32, heal: u8, radius: f32 },
    /// A shield absorbing `pct` % of its HP (per player) through the
    /// wind-up; breaking it cancels the beats.
    Shield { pct: u8 },
}

const NO_HURT: Hurt = Hurt { dice: (0, 0), kind: DamageType::Physical, rider: None, save: false, ranged: false };

const fn hurt(n: u8, s: u8, kind: DamageType) -> Hurt {
    Hurt { dice: (n, s), kind, rider: None, save: true, ranged: false }
}

const fn ability(id: &'static str, name: &'static str, wind: f32, anchor: Anchor, pattern: Pattern, hurt: Hurt, look: u8) -> Ability {
    Ability { id, name, wind, shout: "", anchor, stagger: 0.0, pattern, hurt, look, action: Action::None }
}

/// The scripted monsters.
pub(crate) static SCRIPTS: &[Script] = &[ECLIPSE, VOCAL, MASTERING, ANGELING];

pub(crate) fn find(monster: &str) -> Option<&'static Script> {
    SCRIPTS.iter().find(|s| s.monster == monster)
}

// --- Eclipse: the moon rabbit. Charges, pounces and bounding runs. --------

const ECLIPSE: Script = Script {
    monster: "eclipse",
    rank: Rank::Elite,
    aggro: 16.0,
    intro: "A full moon tonight... Catch me if you can!",
    enrage_secs: 150.0,
    enrage: "Too slow! The moon won't wait for you!",
    death: "The moon... sets...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 0, 2], gap: 1.6, haste: 1.0 },
        Phase { below: 60, shout: "Faster! FASTER!", opener: Some(2), rotation: &[3, 1, 0, 2, 0], gap: 1.2, haste: 0.9 },
        Phase { below: 25, shout: "Lunar frenzy! Hop, hop, HOP!", opener: Some(4), rotation: &[4, 3, 0, 2], gap: 0.9, haste: 0.8 },
    ],
    abilities: &[
        // 0: a telegraphed line charge at the nearest player.
        Ability {
            action: Action::Charge,
            ..ability("boss_moon_charge", "Moon Charge", 1.1, Anchor::Nearest, Pattern::Line { len: 16.0, width: 2.4 }, hurt(3, 6, DamageType::Physical), look::DAMAGE)
        },
        // 1: a leap slam on a marked player.
        Ability {
            shout: "You! Stay right there!",
            action: Action::Leap,
            hurt: Hurt { rider: Some((Condition::Stunned, 1.0)), ..hurt(3, 8, DamageType::Physical) },
            ..ability("boss_moon_pounce", "Pounce", 1.4, Anchor::Marked, Pattern::Circle { r: 3.2 }, NO_HURT, look::DAMAGE)
        },
        // 2: a bounding run: hops that grow longer, each a landing circle.
        Ability {
            action: Action::Leap,
            ..ability(
                "boss_moon_bound",
                "Bounding Run",
                0.9,
                Anchor::Nearest,
                Pattern::Hops { count: 4, first: 4.0, grow: 2.0, r: 2.4, every: 0.45 },
                hurt(2, 6, DamageType::Physical),
                look::DAMAGE,
            )
        },
        // 3: moonfall: shockwaves rolling out from where it stands.
        Ability {
            shout: "Moonfall!",
            ..ability(
                "boss_moonfall",
                "Moonfall",
                1.6,
                Anchor::Boss,
                Pattern::Rings { count: 4, start: 0.0, width: 3.0, step: 5.0, every: 0.5 },
                hurt(3, 6, DamageType::Force),
                look::DARK,
            )
        },
        // 4: lunar frenzy: it leaps onto every player in turn.
        Ability {
            stagger: 0.7,
            action: Action::Leap,
            ..ability("boss_lunar_frenzy", "Lunar Frenzy", 1.2, Anchor::Each(4), Pattern::Circle { r: 2.8 }, hurt(3, 6, DamageType::Physical), look::DARK)
        },
    ],
};

// --- Vocal: the singing grasshopper. Rhythm rings, songs, a lullaby. ------

const VOCAL: Script = Script {
    monster: "vocal",
    rank: Rank::Elite,
    aggro: 16.0,
    intro: "Mi-mi-mi... Ahem! This concert is MANDATORY!",
    enrage_secs: 150.0,
    enrage: "The finale goes on forever!",
    death: "My... last... note...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 1, 0, 3], gap: 1.8, haste: 1.0 },
        Phase { below: 60, shout: "Encore! Everybody, sing along!", opener: Some(2), rotation: &[0, 4, 3, 2, 1], gap: 1.4, haste: 0.9 },
        Phase { below: 25, shout: "FORTISSIMO!", opener: Some(4), rotation: &[0, 4, 3, 0, 2, 1], gap: 1.0, haste: 0.8 },
    ],
    abilities: &[
        // 0: sound waves on the beat; stand in the quiet between them.
        ability(
            "boss_sound_wave",
            "Sound Wave",
            1.0,
            Anchor::Boss,
            Pattern::Rings { count: 4, start: 2.0, width: 2.2, step: 4.4, every: 0.55 },
            hurt(2, 6, DamageType::Force),
            look::LIGHTNING,
        ),
        // 1: shriek: get out of its reach.
        Ability {
            shout: "LA-LA-LAAAA!",
            hurt: Hurt { rider: Some((Condition::Stunned, 1.5)), ..hurt(3, 6, DamageType::Force) },
            ..ability("boss_shriek", "Shriek", 1.3, Anchor::Boss, Pattern::Circle { r: 5.0 }, NO_HURT, look::LIGHTNING)
        },
        // 2: a rousing song: heals and frees the monsters around; burst it.
        Ability {
            shout: "Sing with me, friends!",
            action: Action::Channel { secs: 4.0, heal: 10, rally: 35, rally_r: 14.0, break_pct: 8 },
            ..ability("boss_song_of_rousing", "Song of Rousing", 0.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 3: a lullaby sweeping round: slows whoever it brushes.
        Ability {
            hurt: Hurt { rider: Some((Condition::Slowed, 3.0)), save: false, ..hurt(1, 6, DamageType::Force) },
            ..ability(
                "boss_lullaby",
                "Lullaby",
                1.2,
                Anchor::Boss,
                Pattern::Sweep { count: 6, r: 12.0, arc: 60.0, step: 50.0, every: 0.4 },
                NO_HURT,
                look::CURSE,
            )
        },
        // 4: crescendo: the hall shakes, only its feet are quiet.
        Ability {
            shout: "Crescendo! Come closer, my audience!",
            ..ability("boss_crescendo", "Crescendo", 1.5, Anchor::Boss, Pattern::Donut { inner: 3.5, outer: 16.0 }, hurt(4, 8, DamageType::Force), look::LIGHTNING)
        },
    ],
};

// --- Mastering: the big poring. Adds, bounces, slime; it eats its young. --

const MASTERING: Script = Script {
    monster: "mastering",
    rank: Rank::Elite,
    aggro: 16.0,
    intro: "Boing! Boing! Who wants a big squishy hug?",
    enrage_secs: 150.0,
    enrage: "I'm getting BIGGER!",
    death: "Pop...",
    phases: &[
        Phase { below: 100, shout: "", opener: Some(1), rotation: &[0, 3, 4, 2, 1], gap: 1.8, haste: 1.0 },
        Phase { below: 55, shout: "Everyone, gather round!", opener: Some(1), rotation: &[4, 3, 0, 2, 1], gap: 1.4, haste: 0.9 },
        Phase { below: 25, shout: "Pop me? I'll just split!", opener: Some(5), rotation: &[3, 0, 2, 4, 1], gap: 1.1, haste: 0.85 },
    ],
    abilities: &[
        // 0: a jelly slam on a marked player.
        Ability {
            action: Action::Leap,
            hurt: Hurt { rider: Some((Condition::Slowed, 2.0)), ..hurt(3, 8, DamageType::Physical) },
            ..ability("boss_jelly_slam", "Jelly Slam", 1.4, Anchor::Marked, Pattern::Circle { r: 3.6 }, NO_HURT, look::DAMAGE)
        },
        // 1: it calls its porings.
        Ability {
            shout: "Little ones, to me!",
            action: Action::Summon { kind: "poring", count: 3, per_player: 2 },
            ..ability("boss_call_porings", "Call Porings", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 2: it swallows the porings that still live, healing on each.
        Ability {
            shout: "Come home, my jellies!",
            action: Action::Absorb { secs: 3.0, heal: 6, radius: 20.0 },
            ..ability("boss_reabsorb", "Reabsorb", 0.0, Anchor::Boss, Pattern::None, NO_HURT, look::HEAL)
        },
        // 3: slime rain: a sticky pool under every player.
        Ability {
            hurt: Hurt { rider: Some((Condition::Slowed, 1.5)), save: false, ..hurt(1, 6, DamageType::Poison) },
            ..ability("boss_slime_rain", "Slime Rain", 1.2, Anchor::Each(4), Pattern::Pool { r: 2.6, secs: 8.0, pulse: 1.0 }, NO_HURT, look::SWAMP)
        },
        // 4: big bounce: three heavy hops toward the nearest player.
        Ability {
            action: Action::Leap,
            ..ability(
                "boss_big_bounce",
                "Big Bounce",
                1.0,
                Anchor::Nearest,
                Pattern::Hops { count: 3, first: 5.0, grow: 1.5, r: 3.2, every: 0.6 },
                hurt(3, 6, DamageType::Physical),
                look::DAMAGE,
            )
        },
        // 5: split: a swarm of porings at once.
        Ability {
            action: Action::Summon { kind: "poring", count: 4, per_player: 3 },
            ..ability("boss_split", "Split", 1.2, Anchor::Boss, Pattern::Circle { r: 3.0 }, hurt(2, 6, DamageType::Physical), look::BUFF)
        },
    ],
};

// --- Angeling: the island's boss. Holy patterns, prayers, a shield. -------

const ANGELING: Script = Script {
    monster: "angeling",
    rank: Rank::Boss,
    aggro: 22.0,
    intro: "Mortals, on MY island? How... adorable.",
    enrage_secs: 300.0,
    enrage: "Enough! Heaven's patience is at an end!",
    death: "This... is not... the end...",
    phases: &[
        Phase { below: 100, shout: "", opener: None, rotation: &[0, 2, 0, 1, 5], gap: 1.6, haste: 1.0 },
        Phase { below: 70, shout: "Rise, my children! Defend your angel!", opener: Some(4), rotation: &[0, 3, 6, 2, 5, 1], gap: 1.4, haste: 1.0 },
        Phase { below: 40, shout: "Behold the light of heaven!", opener: Some(7), rotation: &[8, 0, 1, 4, 2, 6, 3, 7], gap: 1.2, haste: 0.9 },
        Phase { below: 15, shout: "I will NOT fall to the likes of you!", opener: Some(8), rotation: &[8, 0, 2, 0, 1, 6], gap: 0.9, haste: 0.8 },
    ],
    abilities: &[
        // 0: holy bolts: a line at every player; step aside.
        Ability {
            hurt: Hurt { ranged: true, ..hurt(2, 8, DamageType::Radiant) },
            ..ability("boss_holy_bolts", "Holy Bolts", 0.9, Anchor::Each(4), Pattern::Line { len: 24.0, width: 1.6 }, NO_HURT, look::HOLY)
        },
        // 1: halo of judgement: only its feet are safe.
        Ability {
            shout: "Come closer, child.",
            hurt: Hurt { rider: Some((Condition::Blinded, 3.0)), ..hurt(4, 8, DamageType::Radiant) },
            ..ability("boss_halo", "Halo of Judgement", 1.6, Anchor::Boss, Pattern::Donut { inner: 3.5, outer: 15.0 }, NO_HURT, look::HOLY)
        },
        // 2: judgement cross: a plus, then an X.
        ability(
            "boss_judgement_cross",
            "Judgement Cross",
            1.3,
            Anchor::Boss,
            Pattern::Cross { arms: 2, len: 36.0, width: 3.0, count: 2, step: 45.0, every: 0.9 },
            hurt(3, 8, DamageType::Radiant),
            look::HOLY,
        ),
        // 3: a prayer that heals it; burst it down to break it.
        Ability {
            shout: "Heavens, mend me!",
            action: Action::Channel { secs: 4.5, heal: 15, rally: 0, rally_r: 0.0, break_pct: 6 },
            ..ability("boss_prayer", "Prayer", 0.0, Anchor::Boss, Pattern::None, NO_HURT, look::HEAL)
        },
        // 4: angelic choir: porings answer the call.
        Ability {
            action: Action::Summon { kind: "poring", count: 2, per_player: 2 },
            ..ability("boss_angelic_choir", "Angelic Choir", 1.0, Anchor::Boss, Pattern::None, NO_HURT, look::BUFF)
        },
        // 5: wing retreat: it flies off and the landing burns.
        Ability {
            action: Action::Retreat { dist: 10.0 },
            ..ability("boss_wing_retreat", "Wing Retreat", 0.8, Anchor::Boss, Pattern::Circle { r: 3.5 }, hurt(2, 8, DamageType::Radiant), look::HOLY)
        },
        // 6: consecrated ground under every player.
        ability(
            "boss_consecrate",
            "Consecrate",
            1.2,
            Anchor::Each(4),
            Pattern::Pool { r: 3.0, secs: 10.0, pulse: 1.0 },
            Hurt { save: false, ..hurt(1, 8, DamageType::Radiant) },
            look::HOLY,
        ),
        // 7: divine shield and holy nova: hide in a sanctuary, or break
        //    the shield before the light comes down.
        Ability {
            shout: "Kneel before the light!",
            action: Action::Shield { pct: 10 },
            hurt: Hurt { save: false, ..hurt(10, 10, DamageType::Radiant) },
            ..ability(
                "boss_holy_nova",
                "Holy Nova",
                6.0,
                Anchor::Boss,
                Pattern::Sanctuary { r: 40.0, safe: 3, safe_r: 2.6, dist: 9.0 },
                NO_HURT,
                look::HOLY,
            )
        },
        // 8: radiant sweep: a beam turning full circle.
        ability(
            "boss_radiant_sweep",
            "Radiant Sweep",
            1.2,
            Anchor::Boss,
            Pattern::Sweep { count: 8, r: 16.0, arc: 50.0, step: 45.0, every: 0.35 },
            hurt(3, 8, DamageType::Radiant),
            look::HOLY,
        ),
    ],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_are_consistent() {
        for s in SCRIPTS {
            assert!(!s.phases.is_empty() && s.phases[0].below == 100, "{}: the first phase starts at full HP", s.monster);
            for w in s.phases.windows(2) {
                assert!(w[1].below < w[0].below, "{}: phases go down in HP", s.monster);
            }
            for p in s.phases {
                assert!(!p.rotation.is_empty() && p.haste > 0.0 && p.gap >= 0.0);
                for &i in p.rotation.iter().chain(p.opener.iter()) {
                    assert!((i as usize) < s.abilities.len(), "{}: ability {i} out of range", s.monster);
                }
            }
            for a in s.abilities {
                assert!(a.id.starts_with("boss_") && !a.name.is_empty());
                let channel = matches!(a.action, Action::Channel { .. } | Action::Absorb { .. });
                assert!(a.wind > 0.0 || channel, "{}: {} needs a wind-up", s.monster, a.id);
            }
        }
        assert!(find("angeling").is_some_and(|s| s.rank == Rank::Boss));
        assert!(find("poring").is_none());
    }
}
