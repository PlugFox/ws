//! Monster affixes ("stars", COMBAT.md "Monster affixes"): now and then a
//! monster, an elite or a boss spawns with one to three affixes that make
//! it tougher, give it a special trick, and raise what it pays.
//!
//! - **One table.** [`AFFIXES`] (id ↔ bit, name, text, colour, where it may
//!   roll) is the only place affixes are defined; clients draw plates,
//!   auras and tooltips from what [`defs_msg`] sends.
//! - **Bits at run time.** A monster carries an [`AffixFlags`] mask in the
//!   horde's struct-of-arrays storage (`HordeStorage::affix`); hot paths
//!   (movement, damage, melee) test one bit, and only when the mask is not
//!   empty.
//! - **Odds follow the run.** [`AffixRules::new`] scales the chance with
//!   the mission's location level and its modifiers' risk score (the
//!   difficulty tier and curses), capped per archetype, and the horde caps
//!   how many affixed monsters live at once ([`AffixRules::cap`]): a
//!   crowd is never all rares.
//! - **Rewards stay bounded.** Each star adds experience, zeny and drop
//!   odds ([`AffixReward`]), sized to the extra effort. Since affixed
//!   monsters are a capped few percent of a horde, a whole run pays at
//!   most about a quarter more at the hardest settings (`tests`), never a
//!   multiple; minions and mirror copies pay nothing, so a summoner is
//!   no farm.

use super::monster::Archetype;
use crate::world::modifiers::Modifiers;
use crate::world::protocol::{AffixInfo, ServerMsg};
use rand::Rng;

/// The affixes of one monster, one bit each ([`AFFIXES`] maps ids to bits).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AffixFlags(pub u16);

impl AffixFlags {
    pub const NONE: AffixFlags = AffixFlags(0);
    /// Moves 30 % and attacks 25 % faster.
    pub const HASTED: AffixFlags = AffixFlags(1 << 0);
    /// Takes 40 % less physical damage.
    pub const ARMOURED: AffixFlags = AffixFlags(1 << 1);
    /// Heals a share of its HP each second unless burning or poisoned.
    pub const REGENERATING: AffixFlags = AffixFlags(1 << 2);
    /// Its melee hits heal it for the damage dealt.
    pub const VAMPIRIC: AffixFlags = AffixFlags(1 << 3);
    /// Fire melee rider; a fire nova on death.
    pub const FIRE: AffixFlags = AffixFlags(1 << 4);
    /// Chilling melee rider; a frost nova on death.
    pub const COLD: AffixFlags = AffixFlags(1 << 5);
    /// Lightning melee rider; lightning strikes on death.
    pub const LIGHTNING: AffixFlags = AffixFlags(1 << 6);
    /// Explodes after a fuse when it dies.
    pub const VOLATILE: AffixFlags = AffixFlags(1 << 7);
    /// Faster at low HP.
    pub const FRENZIED: AffixFlags = AffixFlags(1 << 8);
    /// Blinks next to the player it hunts.
    pub const TELEPORTING: AffixFlags = AffixFlags(1 << 9);
    /// Calls minions.
    pub const SUMMONER: AffixFlags = AffixFlags(1 << 10);
    /// An absorb shield that recharges.
    pub const SHIELDED: AffixFlags = AffixFlags(1 << 11);
    /// Reflects damage onto players inside its ring.
    pub const THORNS: AffixFlags = AffixFlags(1 << 12);
    /// Leaves poison pools behind it.
    pub const PLAGUED: AffixFlags = AffixFlags(1 << 13);
    /// Sweeps a telegraphed arcane beam around.
    pub const ARCANE: AffixFlags = AffixFlags(1 << 14);
    /// Splits once at half HP.
    pub const MIRROR: AffixFlags = AffixFlags(1 << 15);

    /// The elemental enchantments: one per monster.
    pub const ENCHANTS: AffixFlags = AffixFlags(Self::FIRE.0 | Self::COLD.0 | Self::LIGHTNING.0);
    /// Everything that blows up when it dies: one per monster.
    pub const DEATH_BLASTS: AffixFlags = AffixFlags(Self::ENCHANTS.0 | Self::VOLATILE.0);

    #[inline]
    pub const fn bits(self) -> u16 {
        self.0
    }

    #[inline]
    pub const fn contains(self, other: AffixFlags) -> bool {
        self.0 & other.0 == other.0
    }

    #[inline]
    pub const fn intersects(self, other: AffixFlags) -> bool {
        self.0 & other.0 != 0
    }

    #[inline]
    pub fn insert(&mut self, other: AffixFlags) {
        self.0 |= other.0;
    }

    #[inline]
    pub const fn without(self, other: AffixFlags) -> AffixFlags {
        AffixFlags(self.0 & !other.0)
    }

    #[inline]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Stars: how many affixes (the rarity tier).
    #[inline]
    pub const fn stars(self) -> u32 {
        self.0.count_ones()
    }

    /// The set bits, lowest first.
    pub fn iter(self) -> impl Iterator<Item = AffixFlags> {
        (0..16).map(|b| AffixFlags(1 << b)).filter(move |f| self.contains(*f))
    }
}

/// One affix of the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AffixDef {
    pub id: &'static str,
    /// Its single bit.
    pub flag: AffixFlags,
    pub name: &'static str,
    /// Tooltip text (`**bold**` markup as item tooltips): the numbers of
    /// `world::hub::affix`, and the counterplay.
    pub desc: &'static str,
    /// Plate and aura colour (RGB).
    pub color: u32,
    /// Bosses may roll it (only the readable, script-safe ones).
    pub boss: bool,
    /// Rolls only on missions whose location level is at least this.
    pub min_level: u32,
}

/// Every affix: the single source of truth (ids, bits, text, colours).
pub const AFFIXES: &[AffixDef] = &[
    AffixDef {
        id: "hasted",
        flag: AffixFlags::HASTED,
        name: "Hasted",
        desc: "Moves **30% faster** and attacks **25% faster**. Slows and roots hold it as any other.",
        color: 0x9fe8ff,
        boss: false,
        min_level: 0,
    },
    AffixDef {
        id: "armoured",
        flag: AffixFlags::ARMOURED,
        name: "Armoured",
        desc: "Takes **40% less physical damage**. Fire, cold, lightning and other magic get through in full.",
        color: 0xb8c0cc,
        boss: true,
        min_level: 0,
    },
    AffixDef {
        id: "regenerating",
        flag: AffixFlags::REGENERATING,
        name: "Regenerating",
        desc: "Heals **2% of its HP** each second (elites 1%, bosses 0.3%), **not while burning or poisoned**.",
        color: 0x7dff8a,
        boss: true,
        min_level: 0,
    },
    AffixDef {
        id: "vampiric",
        flag: AffixFlags::VAMPIRIC,
        name: "Vampiric",
        desc: "Its melee hits **heal it** for the damage they deal. Keep out of its reach.",
        color: 0xd0303a,
        boss: true,
        min_level: 0,
    },
    AffixDef {
        id: "fire_enchanted",
        flag: AffixFlags::FIRE,
        name: "Fire-enchanted",
        desc: "Melee hits add **fire** damage. When it dies a **fire nova** (3 cells) bursts **1.2 s** later: step out of the ring.",
        color: 0xff7a2a,
        boss: true,
        min_level: 0,
    },
    AffixDef {
        id: "cold_enchanted",
        flag: AffixFlags::COLD,
        name: "Cold-enchanted",
        desc: "Melee hits add **cold** damage and **slow** (save). When it dies a **frost nova** (3 cells) bursts **1.2 s** later and slows.",
        color: 0x7ec8ff,
        boss: true,
        min_level: 0,
    },
    AffixDef {
        id: "lightning_enchanted",
        flag: AffixFlags::LIGHTNING,
        name: "Lightning-enchanted",
        desc: "Melee hits add **lightning** damage. When it dies **three bolts** strike around its body, each marked first.",
        color: 0xfff27a,
        boss: true,
        min_level: 0,
    },
    AffixDef {
        id: "volatile",
        flag: AffixFlags::VOLATILE,
        name: "Volatile",
        desc: "**Explodes 2 s after it dies** (3.5 cells, heavy damage, save for half). The fuse burns on the ground: walk away.",
        color: 0xff4d2e,
        boss: false,
        min_level: 0,
    },
    AffixDef {
        id: "frenzied",
        flag: AffixFlags::FRENZIED,
        name: "Frenzied",
        desc: "Below **35% HP** it flies into a frenzy: **+40%** speed and attack speed. Finish it fast or keep it slowed.",
        color: 0xff5050,
        boss: false,
        min_level: 0,
    },
    AffixDef {
        id: "teleporting",
        flag: AffixFlags::TELEPORTING,
        name: "Teleporting",
        desc: "Every **8 s** it **blinks next to** the player it hunts; the arrival spot glows **0.8 s** before.",
        color: 0xc58cff,
        boss: false,
        min_level: 15,
    },
    AffixDef {
        id: "summoner",
        flag: AffixFlags::SUMMONER,
        name: "Summoner",
        desc: "Calls **2 minions** every **10 s** (at most 10 on the field). Minions give **no experience or loot**: kill the summoner first.",
        color: 0x9a7cff,
        boss: false,
        min_level: 15,
    },
    AffixDef {
        id: "shielded",
        flag: AffixFlags::SHIELDED,
        name: "Shielded",
        desc: "An absorb shield of **30% of its HP** takes damage first; it recharges in full after **5 s** without being hit.",
        color: 0x6fb7ff,
        boss: false,
        min_level: 0,
    },
    AffixDef {
        id: "thorns",
        flag: AffixFlags::THORNS,
        name: "Thorns",
        desc: "Once a second, players **inside its ring of spikes** (2.5 cells) take **25%** of the damage it took. Fight it from outside the ring.",
        color: 0xc8a050,
        boss: true,
        min_level: 0,
    },
    AffixDef {
        id: "plagued",
        flag: AffixFlags::PLAGUED,
        name: "Plagued",
        desc: "Leaves **poison pools** where it walks (4 s; standing in one poisons, save to resist). Do not chase it through them.",
        color: 0x9bd94a,
        boss: false,
        min_level: 15,
    },
    AffixDef {
        id: "arcane",
        flag: AffixFlags::ARCANE,
        name: "Arcane-beam",
        desc: "Every **7 s** it roots itself and **sweeps an arcane beam** half around itself; each sweep is marked before it lands (save for half).",
        color: 0xe070ff,
        boss: false,
        min_level: 30,
    },
    AffixDef {
        id: "mirror",
        flag: AffixFlags::MIRROR,
        name: "Mirror",
        desc: "At **half HP** it **splits** into a copy of itself with the same HP. The copy gives **no experience or loot**.",
        color: 0xe8f4ff,
        boss: false,
        min_level: 30,
    },
];

/// Every bit the table knows.
pub fn known() -> AffixFlags {
    AffixFlags(AFFIXES.iter().fold(0, |m, a| m | a.flag.0))
}

pub fn affix(flag: AffixFlags) -> Option<&'static AffixDef> {
    AFFIXES.iter().find(|a| a.flag == flag)
}

pub fn by_id(id: &str) -> Option<&'static AffixDef> {
    AFFIXES.iter().find(|a| a.id == id)
}

/// Rarity names by stars (index = stars; 0 = a plain monster).
pub const RANKS: [&str; 4] = ["", "Magic", "Rare", "Exalted"];
pub const MAX_STARS: u32 = 3;

/// Extra HP per star, percent (bosses: half).
pub const HP_PCT_PER_STAR: u32 = 20;

/// Base chance per archetype (per mille) at location level 0 and risk 0,
/// and the most any run can raise it to.
const fn base(archetype: Archetype) -> (u32, u32) {
    match archetype {
        Archetype::Swarm => (12, 110),
        Archetype::Chaser => (20, 140),
        Archetype::Tank => (35, 200),
        Archetype::Elite => (250, 700),
        Archetype::Boss => (150, 750),
        Archetype::Plant => (0, 0),
    }
}

/// Most stars an archetype may carry (bosses: one, two from Expert I).
const fn max_stars(archetype: Archetype, tier: u8) -> u32 {
    match archetype {
        Archetype::Swarm => 1,
        Archetype::Chaser => 2,
        Archetype::Tank | Archetype::Elite => 3,
        Archetype::Boss => {
            if tier >= 4 {
                2
            } else {
                1
            }
        }
        Archetype::Plant => 0,
    }
}

/// The odds of one run: how often affixes roll and how many.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AffixRules {
    /// The mission's location level (`Horde::location_level`).
    pub level: u32,
    pub tier: u8,
    /// Chance factor: `(1 + level / 40) × (1 + risk / 50)`.
    pub factor: f32,
    /// Per mille: a rolled monster gets a second star ...
    pub second: u32,
    /// ... and, past that, a third (0 below level 20 and tier 3).
    pub third: u32,
}

impl AffixRules {
    /// A plain Poring Island run: rare stars, never three.
    pub const NONE: AffixRules = AffixRules { level: 0, tier: 0, factor: 0.0, second: 0, third: 0 };

    /// Odds at location level `level` under `mods`: the chance grows with
    /// the level (deeper missions, more rares) and the risk score (tiers
    /// and curses, capped as rewards are); each extra star with the tier
    /// and the level.
    pub fn new(level: u32, mods: Modifiers) -> AffixRules {
        let risk = mods.risk().min(crate::world::modifiers::RISK_CAP);
        let factor = (1.0 + level as f32 / 40.0) * (1.0 + risk as f32 / 50.0);
        let second = (200 + 20 * mods.tier as u32 + 3 * level).min(550);
        let third = if level >= 20 || mods.tier >= 3 { second / 2 } else { 0 };
        AffixRules { level, tier: mods.tier, factor, second, third }
    }

    /// Chance (per mille) that a fresh monster of `archetype` rolls.
    pub fn chance(&self, archetype: Archetype) -> u32 {
        let (b, cap) = base(archetype);
        ((b as f32 * self.factor).round() as u32).min(cap)
    }

    /// Affixed monsters (bosses aside) alive at most with `alive` monsters
    /// on the field: two, plus one per ten.
    pub fn cap(&self, alive: usize) -> usize {
        2 + alive / 10
    }

    /// Rolls a fresh monster's affixes (none most of the time).
    pub fn roll(&self, archetype: Archetype, rng: &mut impl Rng) -> AffixFlags {
        let chance = self.chance(archetype);
        if chance == 0 || rng.gen_range(0..1000) >= chance {
            return AffixFlags::NONE;
        }
        let most = max_stars(archetype, self.tier);
        let mut stars = 1;
        if most >= 2 && rng.gen_range(0..1000) < self.second {
            stars = 2;
            if most >= 3 && rng.gen_range(0..1000) < self.third {
                stars = 3;
            }
        }
        self.pick(archetype == Archetype::Boss, stars, rng)
    }

    /// `stars` different affixes allowed here, never two enchantments or
    /// two death blasts.
    pub fn pick(&self, boss: bool, stars: u32, rng: &mut impl Rng) -> AffixFlags {
        let mut got = AffixFlags::NONE;
        for _ in 0..stars {
            let open = |a: &&AffixDef| {
                (!boss || a.boss)
                    && self.level >= a.min_level
                    && !got.intersects(a.flag)
                    && !(got.intersects(AffixFlags::DEATH_BLASTS) && AffixFlags::DEATH_BLASTS.intersects(a.flag))
            };
            let n = AFFIXES.iter().filter(open).count();
            if n == 0 {
                break;
            }
            let k = rng.gen_range(0..n);
            if let Some(a) = AFFIXES.iter().filter(open).nth(k) {
                got.insert(a.flag);
            }
        }
        got
    }
}

/// What a kill of an affixed monster pays on top of a plain one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AffixReward {
    /// Experience and kill zeny, per mille of a plain kill.
    pub permille: u32,
    /// Drop chances, per mille of a plain kill's.
    pub drop_permille: u32,
    /// Percent chance of a second roll on its whole drop table.
    pub extra_roll_pct: u32,
}

impl AffixReward {
    pub const PLAIN: AffixReward = AffixReward { permille: 1000, drop_permille: 1000, extra_roll_pct: 0 };
    /// A minion or a mirror copy: nothing.
    pub const NOTHING: AffixReward = AffixReward { permille: 0, drop_permille: 0, extra_roll_pct: 0 };

    /// Per star: +75 % experience and zeny, +50 % drop chances, and a
    /// 20 % chance of an extra roll on the drop table.
    pub const fn of(flags: AffixFlags) -> AffixReward {
        let s = flags.stars();
        AffixReward { permille: 1000 + 750 * s, drop_permille: 1000 + 500 * s, extra_roll_pct: 20 * s }
    }
}

/// HP percent of a monster with `flags` (+20 % per star, bosses +10 %).
pub const fn hp_pct(flags: AffixFlags, boss: bool) -> u32 {
    100 + flags.stars() * if boss { HP_PCT_PER_STAR / 2 } else { HP_PCT_PER_STAR }
}

/// The table for the client: every affix, the rank names and the reward
/// per star (the plate tooltip shows it).
pub fn defs_msg() -> ServerMsg {
    let r = AffixReward::of(AffixFlags(1));
    ServerMsg::AffixDefs {
        defs: AFFIXES
            .iter()
            .map(|a| AffixInfo {
                id: a.id.into(),
                bit: a.flag.0.trailing_zeros() as u8,
                name: a.name.into(),
                desc: a.desc.into(),
                color: a.color,
            })
            .collect(),
        ranks: RANKS[1..].iter().map(|s| s.to_string()).collect(),
        exp_pct: (r.permille - 1000) / 10,
        drop_pct: (r.drop_permille - 1000) / 10,
        extra_pct: r.extra_roll_pct,
        hp_pct: HP_PCT_PER_STAR,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::modifiers::{ModFlags, MAX_TIER};
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn bits_are_unique_and_the_table_round_trips() {
        let mut seen = 0u16;
        for a in AFFIXES {
            assert_eq!(a.flag.0.count_ones(), 1, "{} is one bit", a.id);
            assert_eq!(seen & a.flag.0, 0, "{} shares a bit", a.id);
            seen |= a.flag.0;
            assert_eq!(by_id(a.id).unwrap().flag, a.flag);
            assert_eq!(affix(a.flag).unwrap().id, a.id);
        }
        assert_eq!(known().0, seen);
        assert!((12..=16).contains(&AFFIXES.len()));
        let ServerMsg::AffixDefs { defs, ranks, .. } = defs_msg() else { panic!() };
        assert_eq!(ranks, vec!["Magic", "Rare", "Exalted"]);
        for (d, a) in defs.iter().zip(AFFIXES) {
            assert_eq!((d.id.as_str(), 1u16 << d.bit), (a.id, a.flag.0));
        }
        assert_eq!(ServerMsg::decode(&defs_msg().encode()).unwrap(), defs_msg());
        let f = AffixFlags(AffixFlags::FIRE.0 | AffixFlags::THORNS.0);
        assert_eq!(f.iter().collect::<Vec<_>>(), vec![AffixFlags::FIRE, AffixFlags::THORNS]);
        assert_eq!(f.stars(), 2);
    }

    /// Stars per thousand rolls of `archetype` under `rules`:
    /// `[none, 1, 2, 3]`, and every roll checked for legality.
    fn histogram(rules: AffixRules, archetype: Archetype, n: u32) -> [u32; 4] {
        let mut rng = StdRng::seed_from_u64(11);
        let mut h = [0u32; 4];
        for _ in 0..n {
            let f = rules.roll(archetype, &mut rng);
            assert!((f.0 & !known().0) == 0);
            assert!((f.0 & AffixFlags::DEATH_BLASTS.0).count_ones() <= 1, "one death blast: {f:?}");
            for a in f.iter() {
                let d = affix(a).unwrap();
                assert!(rules.level >= d.min_level, "{} below its level", d.id);
                assert!(archetype != Archetype::Boss || d.boss, "{} on a boss", d.id);
            }
            h[f.stars() as usize] += 1;
        }
        h
    }

    #[test]
    fn the_rolling_distribution_follows_level_and_risk() {
        let plain = AffixRules::new(8, Modifiers::NONE);
        let n = 100_000;
        let h = histogram(plain, Archetype::Swarm, n);
        // Poring Island, plain: ~1.4 % of swarm monsters, one star only.
        let rate = (n - h[0]) as f32 / n as f32;
        assert!((0.011..0.018).contains(&rate), "{rate}");
        assert_eq!((h[2], h[3]), (0, 0), "swarm monsters carry one star");
        // No third star on an easy early map; elites roll often.
        let e = histogram(plain, Archetype::Elite, n);
        assert_eq!(e[3], 0);
        assert!(e[2] > 0 && (n - e[0]) as f32 / n as f32 > 0.25);
        // Deep and hard: more often, more stars, still capped.
        let hell = AffixRules::new(52, Modifiers { tier: MAX_TIER, flags: ModFlags(ModFlags::SWARM.0 | ModFlags::FRENZY.0) });
        assert!(hell.chance(Archetype::Swarm) > plain.chance(Archetype::Swarm) * 4);
        assert!(hell.chance(Archetype::Swarm) <= 110, "capped");
        assert_eq!(AffixRules::new(99, Modifiers { tier: MAX_TIER, flags: crate::world::modifiers::known() }).chance(Archetype::Elite), 700);
        let t = histogram(hell, Archetype::Tank, n);
        assert!(t[3] > 0 && t[2] > t[3], "{t:?}");
        // A rising tier alone raises the odds.
        let hard = AffixRules::new(8, Modifiers { tier: 3, flags: ModFlags::NONE });
        assert!(hard.chance(Archetype::Chaser) > plain.chance(Archetype::Chaser));
        // Bosses: one star, then two from Expert I, only boss-safe affixes.
        let b = histogram(plain, Archetype::Boss, 20_000);
        assert_eq!((b[2], b[3]), (0, 0));
        let b = histogram(AffixRules::new(52, Modifiers { tier: 6, flags: ModFlags::NONE }), Archetype::Boss, 20_000);
        assert!(b[2] > 0 && b[3] == 0);
        // Plants never.
        assert_eq!(histogram(hell, Archetype::Plant, 1000)[0], 1000);
        // Level-gated affixes stay off early maps.
        let mut rng = StdRng::seed_from_u64(5);
        for _ in 0..2000 {
            let f = plain.pick(false, 3, &mut rng);
            assert!(!f.intersects(AffixFlags(AffixFlags::ARCANE.0 | AffixFlags::MIRROR.0 | AffixFlags::SUMMONER.0)));
            assert_eq!(f.stars(), 3);
        }
    }

    #[test]
    fn rewards_grow_per_star() {
        assert_eq!(AffixReward::of(AffixFlags::NONE), AffixReward::PLAIN);
        let r = AffixReward::of(AffixFlags(0b101));
        assert_eq!((r.permille, r.drop_permille, r.extra_roll_pct), (2500, 2000, 40));
        assert_eq!(hp_pct(AffixFlags(0b111), false), 160);
        assert_eq!(hp_pct(AffixFlags(0b1), true), 110);
    }

    /// The farming guard: over a run's spawns (a swarm-heavy roster, as
    /// the real ones are), the expected experience of every kill with
    /// affixes stays within a few percent of a plain run on early maps and
    /// within about a fifth at the hardest settings, whatever the density
    /// cap leaves on top.
    #[test]
    fn a_whole_run_pays_a_bounded_bonus() {
        let mix = [(Archetype::Swarm, 60), (Archetype::Chaser, 25), (Archetype::Tank, 12), (Archetype::Elite, 3)];
        let run = |rules: AffixRules| {
            let mut rng = StdRng::seed_from_u64(3);
            let (mut plain, mut paid) = (0.0f64, 0.0f64);
            for _ in 0..2000 {
                for &(a, n) in &mix {
                    let value = crate::world::combat::kill_value(20, a) as f64;
                    for _ in 0..n {
                        plain += value;
                        paid += value * AffixReward::of(rules.roll(a, &mut rng)).permille as f64 / 1000.0;
                    }
                }
            }
            paid / plain
        };
        let poring = run(AffixRules::new(8, Modifiers::NONE));
        assert!((1.0..1.08).contains(&poring), "plain early run ×{poring:.3}");
        let worst = run(AffixRules::new(65, Modifiers { tier: MAX_TIER, flags: crate::world::modifiers::known() }));
        assert!(worst < 1.35 && worst > poring, "hardest run ×{worst:.3}");
        println!("affix exp bonus per run: plain ×{poring:.3}, hardest ×{worst:.3}");
    }
}
