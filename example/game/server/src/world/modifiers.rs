//! Mission modifiers (COMBAT.md "Mission modifiers and rewards"): the
//! difficulty slider and the curses a party leader picks in the lobby, what
//! they do to the run, and what a cleared run pays for the risk.
//!
//! - **One table.** [`CURSES`] (id ↔ bit, risk, level requirement, text,
//!   icon, locked) and [`TIERS`] are the only place modifiers are defined.
//!   The client draws its window from what [`defs_msg`] sends; nothing is
//!   repeated there.
//! - **Bits, not ids, at run time.** A selection is a tier and a
//!   [`ModFlags`] mask, validated once in the lobby ([`Modifiers::validate`]).
//!   Hot paths (damage, potions, movement, fog, light) test one bit.
//! - **Loot stays bounded.** Modifiers make monsters *tougher* first. Where
//!   they add *more* monsters ([`MonsterScale::count`], more elites), each
//!   kill's drop chances, zeny and experience shrink by most of that factor
//!   ([`MonsterScale::loot_permille`]): a run with c× the monsters drops
//!   c^[`COUNT_LOOT_GROWTH`] of a plain run (Swarm ×1.5 → about +18 %),
//!   more but never a multiple. The pay for the risk is a separate clear bonus
//!   ([`Rewards`]): victory only, sized by the risk score, never by kills.

use super::combat::{Archetype, MonsterBlock};
use super::missions::FogRules;
use super::protocol::{ModifierInfo, ModifierStep, RewardStep, ServerMsg};

/// On/off modifiers (curses) of a run, one bit each ([`CURSES`] maps ids
/// to bits). Hand-rolled: two dozen bits never need a crate.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ModFlags(pub u32);

impl ModFlags {
    pub const NONE: ModFlags = ModFlags(0);
    /// Any blow that reaches HP knocks the player down.
    pub const ONE_HIT: ModFlags = ModFlags(1 << 0);
    /// Players walk 20 % slower.
    pub const SLOW: ModFlags = ModFlags(1 << 1);
    /// Potions cannot be drunk.
    pub const NO_POTIONS: ModFlags = ModFlags(1 << 2);
    /// The light radius shrinks by a quarter.
    pub const DARKNESS: ModFlags = ModFlags(1 << 3);
    /// The overtime fog rises three minutes earlier.
    pub const EARLY_FOG: ModFlags = ModFlags(1 << 4);
    /// Monsters move 25 % faster.
    pub const FRENZY: ModFlags = ModFlags(1 << 5);
    /// Half again as many monsters (each one worth less).
    pub const SWARM: ModFlags = ModFlags(1 << 6);
    // Designed, not built yet (locked in the table).
    pub const VOLATILE: ModFlags = ModFlags(1 << 7);
    pub const TRAPS: ModFlags = ModFlags(1 << 8);
    pub const WITHERED: ModFlags = ModFlags(1 << 9);
    pub const NO_REVIVE: ModFlags = ModFlags(1 << 10);

    #[inline]
    pub const fn bits(self) -> u32 {
        self.0
    }

    #[inline]
    pub const fn contains(self, other: ModFlags) -> bool {
        self.0 & other.0 == other.0
    }

    #[inline]
    pub const fn intersects(self, other: ModFlags) -> bool {
        self.0 & other.0 != 0
    }

    #[inline]
    pub fn insert(&mut self, other: ModFlags) {
        self.0 |= other.0;
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The set bits, lowest first.
    pub fn iter(self) -> impl Iterator<Item = ModFlags> {
        (0..32).map(|b| ModFlags(1 << b)).filter(move |f| self.contains(*f))
    }
}

/// One curse of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurseDef {
    pub id: &'static str,
    /// Its single bit.
    pub flag: ModFlags,
    pub name: &'static str,
    /// Tooltip text (`**bold**`, `[bad]…[/]` markup as item tooltips).
    pub desc: &'static str,
    /// Icon ref (`kit/icons.ts`: `skill:<id>`, `item:<id>`, ...).
    pub icon: &'static str,
    /// Risk score it adds.
    pub risk: u32,
    /// Every party member must be at least this level (0 = anyone).
    pub min_level: u32,
    /// Designed but not built: shown as "coming soon", refused.
    pub locked: bool,
}

/// Every curse: the single source of truth (ids, bits, risk, text).
pub const CURSES: &[CurseDef] = &[
    CurseDef {
        id: "one_hit",
        flag: ModFlags::ONE_HIT,
        name: "Glass Bones",
        desc: "Any blow that gets through your shields and temporary HP **knocks you down**. Allies can still revive you.",
        icon: "skill:nv_trickdead",
        risk: 30,
        min_level: 10,
        locked: false,
    },
    CurseDef {
        id: "slow",
        flag: ModFlags::SLOW,
        name: "Leaden Feet",
        desc: "You walk **20% slower**. Movement skills are unaffected.",
        icon: "skill:al_decagi",
        risk: 6,
        min_level: 0,
        locked: false,
    },
    CurseDef {
        id: "no_potions",
        flag: ModFlags::NO_POTIONS,
        name: "Abstinence",
        desc: "**No potions** on this run. They stay in your bag.",
        icon: "item:red_potion",
        risk: 8,
        min_level: 0,
        locked: false,
    },
    CurseDef {
        id: "darkness",
        flag: ModFlags::DARKNESS,
        name: "Darkness",
        desc: "Your light radius is **25% smaller**: monsters show up closer.",
        icon: "skill:mg_sight",
        risk: 8,
        min_level: 0,
        locked: false,
    },
    CurseDef {
        id: "early_fog",
        flag: ModFlags::EARLY_FOG,
        name: "Creeping Fog",
        desc: "The poison fog rises **3 minutes earlier** (1 minute after the boss shows up).",
        icon: "skill:wz_quagmire",
        risk: 5,
        min_level: 0,
        locked: false,
    },
    CurseDef {
        id: "frenzy",
        flag: ModFlags::FRENZY,
        name: "Frenzy",
        desc: "Monsters move **25% faster**.",
        icon: "skill:al_incagi",
        risk: 8,
        min_level: 0,
        locked: false,
    },
    CurseDef {
        id: "swarm",
        flag: ModFlags::SWARM,
        name: "Swarm",
        desc: "**+50% monsters**. Each kill drops proportionally less (chances, zeny, experience), so a run drops about what it would without it.",
        icon: "monster:poring",
        risk: 6,
        min_level: 0,
        locked: false,
    },
    CurseDef {
        id: "volatile",
        flag: ModFlags::VOLATILE,
        name: "Volatile",
        desc: "Monsters **explode** a moment after they die.",
        icon: "skill:ht_blastmine",
        risk: 10,
        min_level: 0,
        locked: true,
    },
    CurseDef {
        id: "traps",
        flag: ModFlags::TRAPS,
        name: "Trapped Ground",
        desc: "Hidden **traps** are scattered over the field.",
        icon: "skill:ht_landmine",
        risk: 8,
        min_level: 0,
        locked: true,
    },
    CurseDef {
        id: "withered",
        flag: ModFlags::WITHERED,
        name: "Withered",
        desc: "All healing is **halved**.",
        icon: "skill:al_heal",
        risk: 8,
        min_level: 0,
        locked: true,
    },
    CurseDef {
        id: "no_revive",
        flag: ModFlags::NO_REVIVE,
        name: "No Second Chances",
        desc: "Downed players **cannot be revived**.",
        icon: "skill:all_resurrection",
        risk: 12,
        min_level: 0,
        locked: true,
    },
];

/// Difficulty tiers (the slider), tier 0 = the mission as designed.
pub const TIERS: [&str; 11] =
    ["Normal", "Hard I", "Hard II", "Hard III", "Expert I", "Expert II", "Expert III", "Nightmare I", "Nightmare II", "Nightmare III", "Hell"];
pub const MAX_TIER: u8 = (TIERS.len() - 1) as u8;
/// Risk per difficulty tier.
pub const TIER_RISK: u32 = 4;
/// Risk beyond this pays nothing more.
pub const RISK_CAP: u32 = 100;
/// Walking pace under Leaden Feet.
pub const SLOW_PACE: f32 = 0.8;
/// The difficulty slider's id in the table sent to clients.
pub const DIFFICULTY_ID: &str = "difficulty";

/// Every bit the table knows.
pub fn known() -> ModFlags {
    ModFlags(CURSES.iter().fold(0, |m, c| m | c.flag.0))
}

pub fn curse(flag: ModFlags) -> Option<&'static CurseDef> {
    CURSES.iter().find(|c| c.flag == flag)
}

pub fn by_id(id: &str) -> Option<&'static CurseDef> {
    CURSES.iter().find(|c| c.id == id)
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ModError {
    #[error("difficulty tier {0} does not exist")]
    Tier(u8),
    #[error("unknown modifier")]
    Unknown,
    #[error("{0} is not available yet")]
    Locked(&'static str),
    #[error("{name} needs level {level}")]
    Level { name: &'static str, level: u32 },
}

/// A run's modifiers: the difficulty tier and the curses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Modifiers {
    pub tier: u8,
    pub flags: ModFlags,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers { tier: 0, flags: ModFlags::NONE };

    /// Checks a client's choice: the tier in range, every bit a known,
    /// built curse, and `level` (each party member's) high enough.
    pub fn validate(tier: u8, mask: u32, level: u32) -> Result<Modifiers, ModError> {
        if tier > MAX_TIER {
            return Err(ModError::Tier(tier));
        }
        let flags = ModFlags(mask);
        if mask & !known().0 != 0 {
            return Err(ModError::Unknown);
        }
        let m = Modifiers { tier, flags };
        for c in m.curses() {
            if c.locked {
                return Err(ModError::Locked(c.name));
            }
        }
        m.check_level(level)?;
        Ok(m)
    }

    /// Whether a party member of `level` may play it.
    pub fn check_level(&self, level: u32) -> Result<(), ModError> {
        match self.curses().find(|c| level < c.min_level) {
            Some(c) => Err(ModError::Level { name: c.name, level: c.min_level }),
            None => Ok(()),
        }
    }

    #[inline]
    pub fn has(&self, flag: ModFlags) -> bool {
        self.flags.contains(flag)
    }

    pub fn is_plain(&self) -> bool {
        self.tier == 0 && self.flags.is_empty()
    }

    /// Table rows of the active curses.
    pub fn curses(&self) -> impl Iterator<Item = &'static CurseDef> + '_ {
        CURSES.iter().filter(|c| self.flags.contains(c.flag))
    }

    /// Risk score: tiers plus curses.
    pub fn risk(&self) -> u32 {
        self.tier as u32 * TIER_RISK + self.curses().map(|c| c.risk).sum::<u32>()
    }

    pub fn rewards(&self) -> Rewards {
        Rewards::for_risk(self.risk())
    }

    pub fn monsters(&self) -> MonsterScale {
        MonsterScale::of(*self)
    }

    /// Light radius factor (Darkness).
    pub fn light(&self) -> f32 {
        if self.has(ModFlags::DARKNESS) { 0.75 } else { 1.0 }
    }

    /// Player walking pace factor (Leaden Feet).
    pub fn pace(&self) -> f32 {
        if self.has(ModFlags::SLOW) { SLOW_PACE } else { 1.0 }
    }

    /// The mission's fog under Creeping Fog: three minutes earlier at the
    /// latest, one minute after the boss instead of two (never before the
    /// first minute).
    pub fn fog(&self, rules: FogRules) -> FogRules {
        if !self.has(ModFlags::EARLY_FOG) {
            return rules;
        }
        FogRules {
            after_boss_secs: (rules.after_boss_secs - 60.0).max(30.0),
            latest_secs: (rules.latest_secs - 180.0).max(60.0),
            ..rules
        }
    }

    /// The `MODIFIERS` state of this selection (`scope`: [`scope`]).
    pub fn state_msg(&self, scope: u8) -> ServerMsg {
        let r = self.rewards();
        let s = self.monsters();
        ServerMsg::Modifiers {
            scope,
            tier: self.tier,
            mask: self.flags.0,
            risk: self.risk(),
            zeny_pct: r.zeny_pct,
            exp_pct: r.exp_pct,
            chest: r.chest,
            loot_pct: (s.loot_permille(Archetype::Swarm) + 5) / 10,
        }
    }
}

/// [`ServerMsg::Modifiers`] scopes.
pub mod scope {
    /// The recipient's lobby (the leader may change it).
    pub const LOBBY: u8 = 0;
    /// The recipient's stage (the HUD icons).
    pub const STAGE: u8 = 1;
}

/// What a selection does to monsters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonsterScale {
    /// Hit points, percent.
    pub hp_pct: u32,
    pub ac: i32,
    /// Saving throws against players' conditions (resistance).
    pub save: i32,
    /// Added to damage dice.
    pub damage: i32,
    /// Movement speed factor.
    pub speed: f32,
    /// Weight factor of elites in the game master's picks.
    pub elite_weight: f32,
    /// Wave budget factor: how many more monsters.
    pub count: f32,
}

impl MonsterScale {
    pub const PLAIN: MonsterScale = MonsterScale { hp_pct: 100, ac: 0, save: 0, damage: 0, speed: 1.0, elite_weight: 1.0, count: 1.0 };

    /// Tier `t`: +15 % HP, +3 % speed, +20 % elite weight and +4 % monsters
    /// per tier; +1 armour class per 2 tiers, +1 save and damage per 3.
    pub fn of(m: Modifiers) -> MonsterScale {
        let t = m.tier.min(MAX_TIER) as i32;
        let mut s = MonsterScale {
            hp_pct: 100 + 15 * t as u32,
            ac: t / 2,
            save: t / 3,
            damage: t / 3,
            speed: 1.0 + 0.03 * t as f32,
            elite_weight: 1.0 + 0.2 * t as f32,
            count: 1.0 + 0.04 * t as f32,
        };
        if m.has(ModFlags::FRENZY) {
            s.speed *= 1.25;
        }
        if m.has(ModFlags::SWARM) {
            s.count *= 1.5;
        }
        s
    }

    /// A level-and-archetype block made tougher.
    pub fn apply(&self, mut b: MonsterBlock) -> MonsterBlock {
        if self.hp_pct != 100 {
            b.hp = ((b.hp as i64 * self.hp_pct as i64 + 50) / 100).max(1) as i32;
        }
        b.ac += self.ac;
        b.save += self.save;
        b.damage.bonus += self.damage;
        b
    }

    /// Each kill's share of the loot, per mille: drop chances, kill zeny and
    /// kill experience shrink as the count grows (elites also by their
    /// extra weight) by f^-(1 - [`COUNT_LOOT_GROWTH`]), so a run with f×
    /// the monsters yields f^[`COUNT_LOOT_GROWTH`] of a plain run's loot.
    /// Bosses come one at a time whatever the budget, plants are a fixed
    /// set: never scaled.
    pub fn loot_permille(&self, archetype: Archetype) -> u32 {
        let f = match archetype {
            // Plants are a fixed small set per stage, whatever the budget.
            Archetype::Boss | Archetype::Plant => 1.0,
            Archetype::Elite => self.count * self.elite_weight,
            _ => self.count,
        };
        ((1000.0 * f.max(1.0).powf(COUNT_LOOT_GROWTH - 1.0)).round() as u32).clamp(1, 1000)
    }

    /// The text of tier `t` (the slider's step).
    fn detail(&self) -> String {
        if self.hp_pct == 100 {
            return "The mission as designed.".into();
        }
        let mut s = format!("Monsters: **+{}% HP**", self.hp_pct - 100);
        if self.ac > 0 {
            s += &format!(", +{} AC", self.ac);
        }
        if self.save > 0 {
            s += &format!(", +{} saves and damage", self.save);
        }
        s += &format!(", **+{}% speed**.", ((self.speed - 1.0) * 100.0).round());
        s += &format!(
            "\nElites ×{:.1} as often; +{}% monsters, each kill worth {}% (drops, zeny, exp).",
            self.elite_weight,
            ((self.count - 1.0) * 100.0).round(),
            (self.loot_permille(Archetype::Swarm) + 5) / 10
        );
        s
    }
}

/// How much of the extra bodies' loot a run keeps: a run with c× the
/// monsters yields c^this of a plain run's drops, kill zeny and kill exp
/// (0: none, flat loot per run; 1: all of it, linear farming). 0.4 lets a
/// swarm pay a little more (×1.5 monsters → +18 %, ×2 → +32 %), never a
/// multiple; the real pay for risk is the clear bonus ([`Rewards`]).
pub const COUNT_LOOT_GROWTH: f32 = 0.4;

/// What a cleared run pays for its risk, on top of everything it dropped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rewards {
    /// Bonus zeny: this percent of the run's zeny (clear and kills).
    pub zeny_pct: u32,
    /// Bonus experience: this percent of the run's experience.
    pub exp_pct: u32,
    /// Extra rolls on an elite drop table (the reward chest).
    pub chest: u32,
}

impl Rewards {
    /// Risk `R` (capped at [`RISK_CAP`]): zeny +R %, experience +R/2 %,
    /// a chest roll per 20 risk.
    pub const fn for_risk(risk: u32) -> Rewards {
        let r = if risk > RISK_CAP { RISK_CAP } else { risk };
        Rewards { zeny_pct: r, exp_pct: r / 2, chest: r / 20 }
    }
}

/// The table for the client: the slider (bit 0xFF, one step per tier) and
/// every curse, plus the reward by risk, so the window can show the
/// bonus of a choice before the server echoes it.
pub fn defs_msg() -> ServerMsg {
    let mut defs = Vec::with_capacity(CURSES.len() + 1);
    defs.push(ModifierInfo {
        id: DIFFICULTY_ID.into(),
        bit: ModifierInfo::SLIDER,
        name: "Difficulty".into(),
        desc: "Tougher monsters: more HP, armour, saves and speed, and more elites. A little more of them too, each worth less, so the loot of a run stays the same; the reward comes from clearing it.".into(),
        icon: "skill:sm_provoke".into(),
        risk: TIER_RISK,
        min_level: 0,
        locked: false,
        steps: TIERS
            .iter()
            .enumerate()
            .map(|(t, &label)| ModifierStep {
                label: label.into(),
                detail: MonsterScale::of(Modifiers { tier: t as u8, flags: ModFlags::NONE }).detail(),
            })
            .collect(),
    });
    defs.extend(CURSES.iter().map(|c| ModifierInfo {
        id: c.id.into(),
        bit: c.flag.0.trailing_zeros() as u8,
        name: c.name.into(),
        desc: c.desc.into(),
        icon: c.icon.into(),
        risk: c.risk,
        min_level: c.min_level,
        locked: c.locked,
        steps: Vec::new(),
    }));
    let rewards = (0..=RISK_CAP)
        .map(|r| {
            let w = Rewards::for_risk(r);
            RewardStep { zeny_pct: w.zeny_pct, exp_pct: w.exp_pct, chest: w.chest }
        })
        .collect();
    ServerMsg::ModifierDefs { defs, rewards }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::combat::loot::{self, Drop};
    use crate::world::combat::Rng;
    use crate::world::horde::catalog::{test_def, MonsterCatalog};
    use crate::world::horde::director::{Director, WaveRules};
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn bits_are_unique_and_the_table_round_trips() {
        let mut seen = 0u32;
        for c in CURSES {
            assert_eq!(c.flag.0.count_ones(), 1, "{} is one bit", c.id);
            assert_eq!(seen & c.flag.0, 0, "{} shares a bit", c.id);
            seen |= c.flag.0;
            assert_eq!(by_id(c.id).unwrap().flag, c.flag);
            assert_eq!(curse(c.flag).unwrap().id, c.id);
        }
        assert_eq!(known().0, seen);
        let ids: std::collections::HashSet<_> = CURSES.iter().map(|c| c.id).collect();
        assert_eq!(ids.len(), CURSES.len(), "ids unique");
        // The wire table carries the same bits and the slider.
        let ServerMsg::ModifierDefs { defs, rewards } = defs_msg() else { panic!() };
        assert_eq!(defs[0].bit, ModifierInfo::SLIDER);
        assert_eq!(defs[0].steps.len(), TIERS.len());
        for (d, c) in defs[1..].iter().zip(CURSES) {
            assert_eq!((d.id.as_str(), 1u32 << d.bit, d.locked), (c.id, c.flag.0, c.locked));
        }
        assert_eq!(rewards.len() as u32, RISK_CAP + 1);
        let back = ServerMsg::decode(&defs_msg().encode()).unwrap();
        assert_eq!(back, defs_msg());
        let m = Modifiers { tier: 3, flags: ModFlags(ModFlags::SLOW.0 | ModFlags::SWARM.0) };
        assert_eq!(ServerMsg::decode(&m.state_msg(scope::LOBBY).encode()).unwrap(), m.state_msg(scope::LOBBY));
        assert_eq!(m.flags.iter().collect::<Vec<_>>(), vec![ModFlags::SLOW, ModFlags::SWARM]);
    }

    #[test]
    fn validation() {
        assert_eq!(Modifiers::validate(0, 0, 1), Ok(Modifiers::NONE));
        assert_eq!(Modifiers::validate(MAX_TIER + 1, 0, 50), Err(ModError::Tier(MAX_TIER + 1)));
        assert_eq!(Modifiers::validate(0, 1 << 30, 50), Err(ModError::Unknown));
        assert_eq!(Modifiers::validate(0, ModFlags::VOLATILE.0, 50), Err(ModError::Locked("Volatile")));
        assert_eq!(Modifiers::validate(2, ModFlags::ONE_HIT.0, 9), Err(ModError::Level { name: "Glass Bones", level: 10 }));
        let m = Modifiers::validate(MAX_TIER, ModFlags::ONE_HIT.0 | ModFlags::NO_POTIONS.0, 10).unwrap();
        assert!(m.has(ModFlags::ONE_HIT) && m.has(ModFlags::NO_POTIONS) && !m.has(ModFlags::SLOW));
        assert_eq!(m.risk(), 10 * TIER_RISK + 30 + 8);
    }

    #[test]
    fn monsters_get_tougher_with_the_tier() {
        let base = MonsterBlock::new(10, Archetype::Tank, 1);
        assert_eq!(MonsterScale::PLAIN.apply(base), base);
        assert_eq!(MonsterScale::of(Modifiers::NONE), MonsterScale::PLAIN);
        let s = MonsterScale::of(Modifiers { tier: 4, flags: ModFlags::FRENZY });
        let b = s.apply(base);
        assert_eq!(b.hp, (base.hp as f32 * 1.6).round() as i32);
        assert_eq!((b.ac, b.save, b.damage.bonus), (base.ac + 2, base.save + 1, base.damage.bonus + 1));
        assert!((s.speed - 1.12 * 1.25).abs() < 1e-5);
        assert_eq!(MonsterScale::of(Modifiers { tier: 0, flags: ModFlags::SWARM }).loot_permille(Archetype::Swarm), 784);
        assert_eq!(s.loot_permille(Archetype::Boss), 1000);
    }

    #[test]
    fn darkness_slow_and_fog() {
        let m = Modifiers { tier: 0, flags: ModFlags(ModFlags::DARKNESS.0 | ModFlags::SLOW.0 | ModFlags::EARLY_FOG.0) };
        assert_eq!((m.light(), m.pace()), (0.75, 0.8));
        let f = m.fog(FogRules::NORMAL);
        assert_eq!((f.after_boss_secs, f.latest_secs), (60.0, 540.0));
        assert_eq!(Modifiers::NONE.fog(FogRules::NORMAL), FogRules::NORMAL);
    }

    #[test]
    fn rewards_follow_risk_only() {
        assert_eq!(Rewards::for_risk(0), Rewards::default());
        assert_eq!(Rewards::for_risk(25), Rewards { zeny_pct: 25, exp_pct: 12, chest: 1 });
        assert_eq!(Rewards::for_risk(500), Rewards::for_risk(RISK_CAP), "capped");
    }

    /// The farming guard: the game master buys the same 15 minutes with
    /// and without count modifiers; with each kill's loot scaled, the
    /// expected item drops, kill zeny and kill experience of the whole run
    /// (everything killed, as a perfect area build would) stay within a few
    /// percent of the plain run, while the monster count grows.
    #[test]
    fn expected_loot_per_run_stays_bounded_when_the_count_grows() {
        let mut swarm = test_def("a_swarm", 2.0, 1, 0.0, false);
        swarm.pack = (4, 10);
        swarm.drops = vec![Drop::new("jellopy", 30.0, None).unwrap(), Drop::new("knife", 0.5, None).unwrap()];
        let mut tank = test_def("b_tank", 2.0, 4, 60.0, false);
        tank.pack = (1, 2);
        tank.drops = vec![Drop::new("jellopy", 60.0, None).unwrap()];
        let mut elite = test_def("c_elite", 2.0, 12, 120.0, false);
        elite.archetype = Archetype::Elite;
        elite.pack = (1, 1);
        elite.drops = vec![Drop::new("knife", 20.0, None).unwrap()];
        let test = MonsterCatalog::new(vec![swarm, tank, elite]).unwrap();
        bounded_on(&test);
        // The real roster and drop tables, when the asset pack is baked.
        let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
        if let Ok(baked) = MonsterCatalog::load(&dist) {
            bounded_on(&baked);
        }
    }

    fn bounded_on(c: &MonsterCatalog) {
        // (monsters, expected item drops, kill zeny) over a run, averaged.
        let run = |m: Modifiers| {
            let s = m.monsters();
            let rules = WaveRules { budget: WaveRules::NORMAL.budget * s.count, elite_weight: s.elite_weight, ..WaveRules::NORMAL };
            let (mut monsters, mut drops, mut zeny) = (0.0f64, 0.0f64, 0.0f64);
            for seed in 0..8 {
                let mut d = Director::new(rules.clone());
                let mut rng = StdRng::seed_from_u64(seed);
                let mut out = Vec::new();
                for _ in 0..(900 * 20) {
                    d.tick(0.05, 1, c, &mut rng, |_| false, &mut out);
                }
                for p in &out {
                    let def = c.get(p.kind);
                    if def.boss {
                        continue; // one at a time, ends the run: not farmable
                    }
                    let share = s.loot_permille(def.archetype) as f64 / 1000.0;
                    let n = p.count as f64;
                    monsters += n;
                    drops += n * share * def.drops.iter().map(|r| r.rate as f64 / loot::ALWAYS as f64).sum::<f64>();
                    zeny += n * share * crate::world::combat::items::kill_zeny(10, def.archetype) as f64;
                }
            }
            (monsters / 8.0, drops / 8.0, zeny / 8.0)
        };
        let plain = run(Modifiers::NONE);
        println!("plain: {:.0} monsters, {:.1} item drops, {:.0} kill zeny", plain.0, plain.1, plain.2);
        for m in [
            Modifiers { tier: 0, flags: ModFlags::SWARM },
            Modifiers { tier: 5, flags: ModFlags::NONE },
            Modifiers { tier: MAX_TIER, flags: ModFlags::SWARM },
        ] {
            let got = run(m);
            println!("{m:?}: {:.0} monsters, {:.1} item drops, {:.0} kill zeny", got.0, got.1, got.2);
            // Swarm adds bodies; tiers mostly trade them for elites.
            if m.has(ModFlags::SWARM) {
                assert!(got.0 > plain.0 * 1.3, "{m:?}: more monsters ({} vs {})", got.0, plain.0);
            }
            // More loot with more bodies, but sub-linearly: at most
            // count^COUNT_LOOT_GROWTH (plus noise), far below the count.
            let cap = (m.monsters().count as f64).powf(COUNT_LOOT_GROWTH as f64) * 1.08;
            assert!(got.1 <= plain.1 * cap, "{m:?}: item drops {} vs {} (cap ×{cap:.2})", got.1, plain.1);
            assert!(got.2 <= plain.2 * cap, "{m:?}: kill zeny {} vs {} (cap ×{cap:.2})", got.2, plain.2);
            if m == (Modifiers { tier: 0, flags: ModFlags::SWARM }) {
                assert!(got.1 > plain.1 * 1.05, "{m:?}: a swarm pays somewhat more ({} vs {})", got.1, plain.1);
                assert!(got.1 / plain.1 < got.0 / plain.0 * 0.85, "{m:?}: loot grows slower than the count");
            }
        }

        // Without the scaling Swarm would pay ×1.5; with it ×1.5^0.4 ≈ 1.18.
        let s = MonsterScale::of(Modifiers { tier: 0, flags: ModFlags::SWARM });
        assert!((1.5 * s.loot_permille(Archetype::Swarm) as f64 / 1000.0 - 1.5f64.powf(0.4)).abs() < 0.01);
    }

    #[test]
    fn scaled_rolls_hit_the_scaled_rate() {
        let t = [Drop::new("jellopy", 30.0, None).unwrap()];
        let mut rng = Rng::new(3);
        let (mut full, mut scaled) = (0, 0);
        for _ in 0..40_000 {
            loot::roll_scaled(&t, 1000, &mut rng, |_, _| full += 1);
            loot::roll_scaled(&t, 500, &mut rng, |_, _| scaled += 1);
        }
        assert!((11_400..12_600).contains(&full), "{full}");
        assert!((5_600..6_400).contains(&scaled), "{scaled}");
        let mut total = 0u64;
        for _ in 0..10_000 {
            total += loot::scale_amount(3, 667, &mut rng);
        }
        assert!((19_700..20_300).contains(&total), "stochastic rounding keeps the mean: {total}");
    }
}
