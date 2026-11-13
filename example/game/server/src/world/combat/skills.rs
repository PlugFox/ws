//! Skill table and learning rules (COMBAT.md §1.4, §3, §4.3).
//!
//! The table is data: `<dist>/server/skills.json`, baked and validated by
//! the asset pipeline. This module only knows generic effect kinds (damage,
//! heal, move, ...) and target archetypes; which skill does what lives in
//! the data. Distances in the file are metres; here they are cells (0.5 m).

use super::dice::Dice;
use super::rules::DamageType;
use super::{Class, Condition, Mods};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

/// Cells per metre.
const CELLS: f32 = 2.0;
/// Slots on the hotbar (two pages of five, COMBAT.md §4.2).
pub const SLOTS: usize = 10;
/// The row every class shares (the kiting kit).
pub const BASIC: &str = "basic";

/// `a + b·slv`; in the file `[a, b]` or a plain number `a`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Deserialize)]
#[serde(from = "RawLin")]
pub struct Lin(pub f32, pub f32);

#[derive(Deserialize)]
#[serde(untagged)]
enum RawLin {
    Flat(f32),
    Pair([f32; 2]),
}

impl From<RawLin> for Lin {
    fn from(v: RawLin) -> Self {
        match v {
            RawLin::Flat(a) => Lin(a, 0.0),
            RawLin::Pair([a, b]) => Lin(a, b),
        }
    }
}

impl Lin {
    pub fn at(self, slv: u8) -> f32 {
        self.0 + self.1 * slv as f32
    }
}

/// How a skill picks where it lands (COMBAT.md §4.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    #[serde(rename = "self", alias = "")]
    Caster,
    Front,
    Aim,
    /// Along the aim, or away from the nearby monsters (slides, blinks).
    Escape,
    Line,
    Nearest,
    NearestGroup,
    Densest,
    Spread,
    Strongest,
    Allies,
    MostWoundedAlly,
    MostThreatenedAlly,
    DensestAllies,
    DownedAlly,
}

/// Shape of an effect's area (COMBAT.md §4.3). Sizes grow with the skill
/// level (`Lin`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    /// `radius` around the point.
    #[default]
    Circle,
    /// Centred on the point: `length` along the aim × `width` across (a
    /// wall is short and wide, a square has both equal).
    Rect,
    /// From the caster along the aim: `length` (the skill's range when
    /// 0) × `width` (piercing arrows, beams, dashes).
    Line,
    /// In front of the caster: `radius` and `arc` degrees (sword swings).
    Arc,
}

/// What an area moves with while it lasts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Follow {
    /// Stays where it landed.
    #[default]
    None,
    /// An aura around the caster.
    Caster,
    /// Homing: the hit goes wherever the picked monster (or ally) goes and
    /// cannot miss it.
    Target,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveMode {
    Slide,
    Blink,
}

impl<'de> Deserialize<'de> for MoveMode {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match String::deserialize(d)?.as_str() {
            "slide" | "dash" => Ok(MoveMode::Slide),
            "blink" => Ok(MoveMode::Blink),
            other => Err(serde::de::Error::custom(format!("unknown move mode {other}"))),
        }
    }
}

/// Where an effect lands and whom it picks (cells).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AreaFx {
    pub shape: Shape,
    pub radius: Lin,
    pub length: Lin,
    pub width: Lin,
    /// Cosine of half the arc.
    pub arc_cos: f32,
    /// Around the caster instead of the target point.
    pub at_caster: bool,
    pub follow: Follow,
    /// At most this many targets, nearest the origin first (a line keeps
    /// the first ones along it); 0 = everything inside.
    pub limit: Lin,
    /// Hops after the first target, each to the nearest one not hit yet
    /// within `jump` (bouncing shields, chain spells).
    pub chain: Lin,
    pub jump: f32,
    /// Random offset of each hit.
    pub scatter: f32,
    /// Each hit on a different monster, nearest the point first.
    pub spread: bool,
}

impl AreaFx {
    /// A single point (no area): the payload lands on the picked target only.
    pub fn is_point(&self, slv: u8) -> bool {
        self.shape == Shape::Circle && self.radius.at(slv) <= 0.0
    }
}

/// When an effect acts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TimingFx {
    /// Repeated hits, `every` seconds apart (bolts, storms).
    pub count: Lin,
    pub every: f32,
    /// Projectile speed, cells per second: the hit lands after its flight
    /// (0 = at once).
    pub speed: f32,
    /// Seconds the area lasts on the ground (0 = it acts once).
    pub secs: Lin,
    /// Seconds between the actions of a lasting area.
    pub pulse: f32,
    /// A trap: waits (up to `secs`) until a monster comes within `trigger`
    /// cells, then acts once.
    pub trigger: f32,
    /// A trap also goes off by itself after this many seconds (0 = never).
    pub fuse: f32,
}

impl TimingFx {
    pub fn lasting(&self, slv: u8) -> bool {
        self.secs.at(slv) > 0.0
    }
}

/// Dice of an effect: fixed, or the held weapon's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EffectDice {
    Fixed(Dice),
    Weapon,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DamageFx {
    pub dice: EffectDice,
    /// Added on top of `dice`, its count scaled by `mult` (weapon skills).
    pub extra: Option<Dice>,
    /// Multiply the dice count by the cantrip multiplier `k`.
    pub scale_k: bool,
    pub mult: Lin,
    pub bonus: Lin,
    pub kind: DamageType,
    /// A failed save also leaves this condition.
    pub save: Option<SaveFx>,
    /// Cells pushed away from the area's origin (along the aim for lines
    /// and rects); negative pulls in.
    pub knockback: Lin,
}

/// The condition a failed saving throw leaves, and for how long.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SaveFx {
    pub condition: Condition,
    /// Seconds.
    pub secs: Lin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HealFx {
    pub dice: Dice,
    pub scale_k: bool,
    pub mult: Lin,
}

/// A buff on allies (or a passive on the caster when `secs` is 0).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BuffFx {
    /// Seconds; 0 = for as long as the skill is learned (passives).
    pub secs: Lin,
    pub ac: Lin,
    pub to_hit: Lin,
    pub to_hit_die: u32,
    /// Per weapon hit: `damage` dice, count × `mult`, plus `damage_flat`.
    pub damage: Option<Dice>,
    pub mult: Lin,
    pub damage_flat: Lin,
    pub haste: Lin,
    pub speed: Lin,
    pub resist: Lin,
    pub reduce: Lin,
    /// Percent of melee hits blocked.
    pub block: Lin,
    /// Cells.
    pub light: Lin,
    /// Fractions of the maximum per second.
    pub sp_regen: Lin,
    pub hp_regen: Lin,
    pub crit: Lin,
    /// Temporary HP: `temp_hp` dice (count × `mult`, × k when `scale_k`)
    /// plus `shield` × max HP.
    pub temp_hp: Option<Dice>,
    pub scale_k: bool,
    pub shield: Lin,
    pub advantage: bool,
    pub hidden: bool,
    pub steady: bool,
    pub mana_shield: bool,
    pub evil_resist: Lin,
    pub evil_damage: Lin,
    pub soul_drain: Lin,
    /// On until cast again (Energy Coat).
    pub toggle: bool,
}

impl BuffFx {
    /// The modifiers at skill level `slv`.
    pub fn mods(&self, slv: u8) -> Mods {
        let n = |l: Lin| l.at(slv);
        let i = |l: Lin| l.at(slv).round() as i32;
        Mods {
            ac: i(self.ac),
            to_hit: i(self.to_hit),
            to_hit_die: self.to_hit_die,
            damage: self.damage.and_then(|d| {
                let count = (d.count as f32 * self.mult.at(slv)).round() as u32;
                (count > 0).then_some(Dice { count, ..d })
            }),
            damage_flat: i(self.damage_flat),
            haste: n(self.haste),
            speed: n(self.speed),
            resist: n(self.resist),
            reduce: i(self.reduce),
            block: i(self.block).clamp(0, 75) as u32,
            light: n(self.light),
            sp_regen: n(self.sp_regen) / 100.0,
            hp_regen: n(self.hp_regen) / 100.0,
            crit: i(self.crit).max(0) as u32,
            advantage: self.advantage,
            hidden: self.hidden,
            steady: self.steady,
            mana_shield: self.mana_shield,
            evil_resist: n(self.evil_resist),
            evil_damage: i(self.evil_damage),
            soul_drain: i(self.soul_drain),
        }
    }

    pub fn grants_temp_hp(&self) -> bool {
        self.temp_hp.is_some() || self.shield != Lin::default()
    }
}

/// What an effect does to each target it picks.
#[derive(Clone, Debug, PartialEq)]
pub enum Payload {
    /// Monsters: damage (a save for half unless it swings the weapon).
    Damage(DamageFx),
    /// Allies.
    Heal(HealFx),
    /// Monsters: a save or `condition` (Taunted makes them hunt the
    /// caster). Without a save the condition is kept while inside a lasting
    /// area; `mired` adds Mired (swamps).
    Condition { condition: Condition, save: bool, secs: Lin, mired: bool },
    /// Monsters: pushed `distance` cells away from the origin (negative
    /// pulls), no damage.
    Push(Lin),
    /// Allies.
    Buff(BuffFx),
    /// Allies inside ignore the next `hits` melee hits (shared); a ranged
    /// ward makes monsters' ranged skills miss instead.
    Ward { hits: Lin, ranged: bool },
    /// Monsters cannot stay inside: pushed out every tick (ice walls,
    /// sanctums).
    Barrier,
    /// A downed ally stands up with `hp` × max HP.
    Revive { hp: Lin },
    /// Allies inside lose their conditions (Status Recovery).
    Cure,
    /// Monster ground inside ends (Ganbantein).
    Dispel,
}

impl Payload {
    /// Lands on monsters (or else on allies).
    pub fn hostile(&self) -> bool {
        matches!(self, Payload::Damage(_) | Payload::Condition { .. } | Payload::Push(_) | Payload::Barrier | Payload::Dispel)
    }

    /// Seconds between the actions of a lasting area of this payload.
    fn default_pulse(&self) -> f32 {
        match self {
            Payload::Damage(_) | Payload::Push(_) => 0.5,
            Payload::Condition { .. } => 0.25,
            Payload::Heal(_) | Payload::Buff(_) | Payload::Revive { .. } => 1.0,
            Payload::Barrier | Payload::Ward { .. } | Payload::Cure | Payload::Dispel => 0.0,
        }
    }
}

/// One cast effect: an area, its timing and its payload.
#[derive(Clone, Debug, PartialEq)]
pub struct Act {
    pub area: AreaFx,
    pub timing: TimingFx,
    pub payload: Payload,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Act(Box<Act>),
    /// The caster moves (slides keep the facing, blinks skip the ground in
    /// between).
    Move { mode: MoveMode, distance: Lin },
    /// Always on while learned.
    Passive(BuffFx),
    /// Not cast by the server yet.
    Pending(String),
}

impl Effect {
    pub fn act(&self) -> Option<&Act> {
        match self {
            Effect::Act(a) => Some(a),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SkillDef {
    pub id: String,
    pub name: String,
    /// `None` for the basic row.
    pub class: Option<Class>,
    pub tier: u8,
    pub max_level: u8,
    pub requires: Vec<(u16, u8)>,
    pub min_level: u32,
    pub passive: bool,
    pub mobile: bool,
    /// Its ground lasts only while the slot is held and the caster stands
    /// (Gravitation Field, Battle Chant, Basilica).
    pub channel: bool,
    pub target: Target,
    /// Cells.
    pub range: f32,
    pub cast: Lin,
    pub cooldown: Lin,
    pub sp: Lin,
    pub effects: Vec<Effect>,
}

impl SkillDef {
    /// Every effect is one the server casts.
    pub fn ready(&self) -> bool {
        !self.effects.is_empty() && self.effects.iter().all(|e| !matches!(e, Effect::Pending(_)))
    }

    pub fn sp_cost(&self, slv: u8) -> i32 {
        self.sp.at(slv).round().max(0.0) as i32
    }

    pub fn cast_secs(&self, slv: u8) -> f32 {
        self.cast.at(slv).max(0.0)
    }

    pub fn cooldown_secs(&self, slv: u8) -> f32 {
        self.cooldown.at(slv).max(0.0)
    }

    pub fn usable_by(&self, class: Class) -> bool {
        self.class.is_none_or(|c| c == class)
    }
}

/// Learned levels and the hotbar of one character (stored as JSON).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Loadout {
    #[serde(default)]
    pub skills: BTreeMap<String, u8>,
    /// Skill id per slot; empty = free.
    #[serde(default)]
    pub hotbar: Vec<String>,
}

impl Loadout {
    pub fn spent(&self) -> u32 {
        self.skills.values().map(|&l| l as u32).sum()
    }

    pub fn level(&self, id: &str) -> u8 {
        self.skills.get(id).copied().unwrap_or(0)
    }

    pub fn slot(&self, i: usize) -> &str {
        self.hotbar.get(i).map_or("", String::as_str)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum LearnError {
    #[error("no such skill")]
    Unknown,
    #[error("another class's skill")]
    WrongClass,
    #[error("already at its maximum level")]
    Maxed,
    #[error("needs character level {0}")]
    Level(u32),
    #[error("needs {0} level {1}")]
    Requires(String, u8),
    #[error("no skill points left")]
    NoPoints,
    #[error("no such slot")]
    BadSlot,
    #[error("passive skills need no slot")]
    Passive,
    #[error("not learned yet")]
    NotLearned,
}

impl LearnError {
    pub fn code(&self) -> &'static str {
        match self {
            LearnError::Unknown => "no_such_skill",
            LearnError::WrongClass => "wrong_class",
            LearnError::Maxed => "skill_maxed",
            LearnError::Level(_) => "level_too_low",
            LearnError::Requires(..) => "requires",
            LearnError::NoPoints => "no_skill_points",
            LearnError::BadSlot => "bad_slot",
            LearnError::Passive => "passive_skill",
            LearnError::NotLearned => "not_learned",
        }
    }
}

#[derive(Debug, Error)]
pub enum SkillBookError {
    #[error("skills.json: {0}")]
    Io(#[from] std::io::Error),
    #[error("skills.json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("skill {0}: {1}")]
    Bad(String, String),
}

/// How a monster picks where its skill lands (COMBAT.md §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobTarget {
    /// Around the monster (novas, splashes).
    #[serde(rename = "self")]
    Caster,
    /// The nearest standing player.
    Nearest,
    /// The point covering the most players.
    Densest,
    /// An arc toward the nearest player.
    Front,
}

/// A monster skill: the same effects as a player's, landing on players.
#[derive(Clone, Debug)]
pub struct MonsterSkill {
    pub id: String,
    pub name: String,
    pub target: MobTarget,
    /// Cells: used when a player is this close.
    pub range: f32,
    /// Seconds of telegraph; the monster stands still.
    pub cast: f32,
    pub cooldown: f32,
    /// Percent per check while ready.
    pub chance: u32,
    /// Only below this fraction of its HP.
    pub hp_below: f32,
    /// A ranged ward over the player stops it.
    pub ranged: bool,
    pub effects: Vec<Effect>,
}

/// Every skill, indexed by position (the order of the file).
#[derive(Clone, Debug, Default)]
pub struct SkillBook {
    skills: Vec<SkillDef>,
    monster: Vec<MonsterSkill>,
}

impl SkillBook {
    /// Loads `<dist>/server/skills.json`.
    pub fn load(dist_root: &Path) -> Result<Self, SkillBookError> {
        let text = std::fs::read_to_string(dist_root.join("server/skills.json"))?;
        Self::from_json(&text)
    }

    pub fn from_json(text: &str) -> Result<Self, SkillBookError> {
        let raw: RawBook = serde_json::from_str(text)?;
        let index: BTreeMap<&str, u16> = raw.skills.iter().enumerate().map(|(i, s)| (s.id.as_str(), i as u16)).collect();
        let mut skills = Vec::with_capacity(raw.skills.len());
        for s in &raw.skills {
            let bad = |why: String| SkillBookError::Bad(s.id.clone(), why);
            let class = match s.class.as_str() {
                BASIC => None,
                c => Some(Class::from_id(c).ok_or_else(|| bad(format!("unknown class {c}")))?),
            };
            let mut requires = Vec::new();
            for (req, &lvl) in &s.requires {
                let i = index.get(req.as_str()).ok_or_else(|| bad(format!("requires unknown {req}")))?;
                requires.push((*i, lvl));
            }
            let effects = s.effects.iter().map(|e| RawEffect::parse(e, s.passive)).collect::<Result<Vec<_>, String>>().map_err(bad)?;
            skills.push(SkillDef {
                id: s.id.clone(),
                name: s.name.clone(),
                class,
                tier: s.tier,
                max_level: s.max_level.max(1),
                requires,
                min_level: s.min_level,
                passive: s.passive,
                mobile: s.mobile,
                channel: s.channel,
                target: s.target,
                range: s.range * CELLS,
                cast: s.cast,
                cooldown: s.cooldown,
                sp: s.sp,
                effects,
            });
        }
        let mut monster = Vec::with_capacity(raw.monster_skills.len());
        for m in &raw.monster_skills {
            let bad = |why: String| SkillBookError::Bad(m.id.clone(), why);
            let effects = m.effects.iter().map(|e| RawEffect::parse(e, false)).collect::<Result<Vec<_>, String>>().map_err(bad)?;
            if effects.is_empty() || effects.iter().any(|e| matches!(e, Effect::Pending(_) | Effect::Passive(_))) {
                return Err(bad("a monster skill needs castable effects".into()));
            }
            monster.push(MonsterSkill {
                id: m.id.clone(),
                name: m.name.clone(),
                target: m.target,
                range: m.range * CELLS,
                cast: m.cast.max(0.0),
                cooldown: m.cooldown.max(0.0),
                chance: m.chance.clamp(1, 100),
                hp_below: m.hp_below,
                ranged: m.ranged,
                effects,
            });
        }
        Ok(Self { skills, monster })
    }

    /// Monster skill by id.
    pub fn find_monster(&self, id: &str) -> Option<u16> {
        self.monster.iter().position(|m| m.id == id).map(|i| i as u16)
    }

    pub fn monster(&self, i: u16) -> &MonsterSkill {
        &self.monster[i as usize]
    }

    pub fn len(&self) -> usize {
        self.skills.len()
    }

    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    pub fn get(&self, i: u16) -> &SkillDef {
        &self.skills[i as usize]
    }

    pub fn find(&self, id: &str) -> Option<(u16, &SkillDef)> {
        self.skills.iter().enumerate().find(|(_, s)| s.id == id).map(|(i, s)| (i as u16, s))
    }

    pub fn iter(&self) -> impl Iterator<Item = (u16, &SkillDef)> {
        self.skills.iter().enumerate().map(|(i, s)| (i as u16, s))
    }

    /// Unspent points of a character.
    pub fn points_left(&self, level: u32, loadout: &Loadout) -> u32 {
        Class::skill_points(level).saturating_sub(loadout.spent())
    }

    /// Checks whether `id` can go up one level.
    pub fn can_learn(&self, class: Class, level: u32, loadout: &Loadout, id: &str) -> Result<(), LearnError> {
        let (_, s) = self.find(id).ok_or(LearnError::Unknown)?;
        if !s.usable_by(class) {
            return Err(LearnError::WrongClass);
        }
        if loadout.level(id) >= s.max_level {
            return Err(LearnError::Maxed);
        }
        if level < s.min_level {
            return Err(LearnError::Level(s.min_level));
        }
        for &(req, lvl) in &s.requires {
            let r = self.get(req);
            if loadout.level(&r.id) < lvl {
                return Err(LearnError::Requires(r.name.clone(), lvl));
            }
        }
        if self.points_left(level, loadout) == 0 {
            return Err(LearnError::NoPoints);
        }
        Ok(())
    }

    /// Raises `id` by one level.
    pub fn learn(&self, class: Class, level: u32, loadout: &mut Loadout, id: &str) -> Result<u8, LearnError> {
        self.can_learn(class, level, loadout, id)?;
        let l = loadout.skills.entry(id.to_string()).or_insert(0);
        *l += 1;
        Ok(*l)
    }

    /// Puts a learned active skill in a slot (empty `id` clears it). The
    /// same skill moves rather than appearing twice.
    pub fn set_slot(&self, loadout: &mut Loadout, slot: usize, id: &str) -> Result<(), LearnError> {
        if slot >= SLOTS {
            return Err(LearnError::BadSlot);
        }
        if !id.is_empty() {
            let (_, s) = self.find(id).ok_or(LearnError::Unknown)?;
            if s.passive {
                return Err(LearnError::Passive);
            }
            if loadout.level(id) == 0 {
                return Err(LearnError::NotLearned);
            }
        }
        loadout.hotbar.resize(SLOTS, String::new());
        for s in loadout.hotbar.iter_mut().filter(|s| s.as_str() == id) {
            s.clear();
        }
        loadout.hotbar[slot] = id.to_string();
        Ok(())
    }

    /// Drops unknown, foreign or over-max skills and slots that no longer
    /// hold a learned active skill (a table change between sessions).
    pub fn sanitize(&self, class: Class, loadout: &mut Loadout) {
        loadout.skills.retain(|id, lvl| match self.find(id) {
            Some((_, s)) if s.usable_by(class) && *lvl > 0 => {
                *lvl = (*lvl).min(s.max_level);
                true
            }
            _ => false,
        });
        loadout.hotbar.resize(SLOTS, String::new());
        for slot in loadout.hotbar.iter_mut() {
            let keep = self.find(slot).is_some_and(|(_, s)| !s.passive) && loadout.skills.contains_key(slot.as_str());
            if !keep {
                slot.clear();
            }
        }
    }
}

// ----- file format -----------------------------------------------------------

#[derive(Deserialize)]
struct RawBook {
    skills: Vec<RawSkill>,
    #[serde(default)]
    monster_skills: Vec<RawMonsterSkill>,
}

#[derive(Deserialize)]
struct RawMonsterSkill {
    id: String,
    name: String,
    target: MobTarget,
    range: f32,
    #[serde(default)]
    cast: f32,
    cooldown: f32,
    #[serde(default = "hundred")]
    chance: u32,
    #[serde(default = "whole")]
    hp_below: f32,
    #[serde(default)]
    ranged: bool,
    effects: Vec<serde_json::Value>,
}

fn hundred() -> u32 {
    100
}

fn whole() -> f32 {
    1.0
}

#[derive(Deserialize)]
struct RawSkill {
    id: String,
    name: String,
    class: String,
    #[serde(default = "one")]
    tier: u8,
    #[serde(default = "one")]
    max_level: u8,
    #[serde(default)]
    requires: BTreeMap<String, u8>,
    #[serde(default = "one_u32")]
    min_level: u32,
    #[serde(default)]
    passive: bool,
    #[serde(default)]
    mobile: bool,
    #[serde(default)]
    channel: bool,
    #[serde(default = "caster")]
    target: Target,
    #[serde(default)]
    range: f32,
    #[serde(default)]
    cast: Lin,
    #[serde(default)]
    cooldown: Lin,
    #[serde(default)]
    sp: Lin,
    /// Parsed per type: effects the server does not cast yet may use any
    /// fields.
    #[serde(default)]
    effects: Vec<serde_json::Value>,
}

fn one() -> u8 {
    1
}

fn one_u32() -> u32 {
    1
}

fn caster() -> Target {
    Target::Caster
}

#[derive(Deserialize)]
struct RawEffect {
    #[serde(rename = "type")]
    kind: String,
    // Payload.
    #[serde(default)]
    dice: String,
    #[serde(default)]
    extra: String,
    #[serde(default)]
    scale: String,
    mult: Option<Lin>,
    bonus: Option<Lin>,
    #[serde(default)]
    damage: String,
    save: Option<RawSave>,
    #[serde(default)]
    condition: String,
    #[serde(default)]
    ac: Option<Lin>,
    knockback: Option<Lin>,
    distance: Option<Lin>,
    mode: Option<MoveMode>,
    value: Option<Lin>,
    hits: Option<Lin>,
    #[serde(default)]
    ranged: bool,
    hp: Option<Lin>,
    // Buff numbers.
    to_hit: Option<Lin>,
    #[serde(default)]
    bless: String,
    damage_flat: Option<Lin>,
    haste: Option<Lin>,
    speed: Option<Lin>,
    resist: Option<Lin>,
    reduce: Option<Lin>,
    block: Option<Lin>,
    light: Option<Lin>,
    sp_regen: Option<Lin>,
    hp_regen: Option<Lin>,
    crit: Option<Lin>,
    #[serde(default)]
    temp_hp: String,
    shield: Option<Lin>,
    #[serde(default)]
    advantage: bool,
    #[serde(default)]
    hidden: bool,
    #[serde(default)]
    steady: bool,
    #[serde(default)]
    mana_shield: bool,
    evil_resist: Option<Lin>,
    evil_damage: Option<Lin>,
    soul_drain: Option<Lin>,
    #[serde(default)]
    toggle: bool,
    // Area.
    #[serde(default)]
    shape: Shape,
    radius: Option<Lin>,
    length: Option<Lin>,
    width: Option<Lin>,
    #[serde(default)]
    arc: f32,
    #[serde(default)]
    at: String,
    #[serde(default)]
    follow: Follow,
    limit: Option<Lin>,
    chain: Option<Lin>,
    #[serde(default)]
    jump: f32,
    #[serde(default)]
    scatter: f32,
    #[serde(default)]
    spread: bool,
    // Timing.
    count: Option<Lin>,
    #[serde(default)]
    every: f32,
    #[serde(default)]
    projectile: f32,
    secs: Option<Lin>,
    pulse: Option<f32>,
    #[serde(default)]
    trigger: f32,
    #[serde(default)]
    fuse: f32,
}

#[derive(Deserialize)]
struct RawSave {
    condition: String,
    secs: Option<Lin>,
}

/// Effect types the server casts.
const CAST: &[&str] =
    &["damage", "heal", "condition", "push", "buff", "ward", "barrier", "revive", "move", "sp_regen", "cure", "dispel"];

fn cells(l: Option<Lin>) -> Lin {
    let l = l.unwrap_or_default();
    Lin(l.0 * CELLS, l.1 * CELLS)
}

/// Timings stay in seconds; the caller turns them into ticks.
impl RawEffect {
    fn parse(v: &serde_json::Value, passive: bool) -> Result<Effect, String> {
        let kind = v.get("type").and_then(|t| t.as_str()).ok_or("effect without a type")?;
        if !CAST.contains(&kind) {
            return Ok(Effect::Pending(kind.to_string()));
        }
        let raw: RawEffect = serde_json::from_value(v.clone()).map_err(|e| format!("{kind} effect: {e}"))?;
        raw.build(passive)
    }

    fn build(&self, passive: bool) -> Result<Effect, String> {
        // A condition the server does not apply yet keeps the skill pending.
        let save = match &self.save {
            None => None,
            Some(s) => match Condition::from_id(&s.condition) {
                Some(condition) => Some(SaveFx { condition, secs: s.secs.unwrap_or(Lin(1.0, 0.0)) }),
                None => return Ok(Effect::Pending(format!("condition {}", s.condition))),
            },
        };
        let condition = match self.condition.as_str() {
            "" => None,
            c => match Condition::from_id(c) {
                Some(c) => Some(c),
                None => return Ok(Effect::Pending(format!("condition {c}"))),
            },
        };
        let dice_or = |s: &str| (!s.is_empty()).then(|| parse_dice(s)).transpose();
        let one = Lin(1.0, 0.0);
        let buff = || -> Result<BuffFx, String> {
            Ok(BuffFx {
                secs: self.secs.unwrap_or_default(),
                ac: self.ac.unwrap_or_default(),
                to_hit: self.to_hit.unwrap_or_default(),
                to_hit_die: dice_or(&self.bless)?.map_or(0, |d| d.sides),
                damage: dice_or(&self.extra)?,
                mult: self.mult.unwrap_or(one),
                damage_flat: self.damage_flat.unwrap_or_default(),
                haste: self.haste.unwrap_or_default(),
                speed: self.speed.unwrap_or_default(),
                resist: self.resist.unwrap_or_default(),
                reduce: self.reduce.unwrap_or_default(),
                block: self.block.unwrap_or_default(),
                light: cells(self.light),
                sp_regen: self.sp_regen.or(self.value).unwrap_or_default(),
                hp_regen: self.hp_regen.unwrap_or_default(),
                crit: self.crit.unwrap_or_default(),
                temp_hp: dice_or(&self.temp_hp)?,
                scale_k: self.scale == "k",
                shield: self.shield.unwrap_or_default(),
                advantage: self.advantage,
                hidden: self.hidden,
                steady: self.steady,
                mana_shield: self.mana_shield,
                evil_resist: self.evil_resist.unwrap_or_default(),
                evil_damage: self.evil_damage.unwrap_or_default(),
                soul_drain: self.soul_drain.unwrap_or_default(),
                toggle: self.toggle,
            })
        };
        let payload = match self.kind.as_str() {
            "move" => {
                return Ok(Effect::Move { mode: self.mode.ok_or("move without a mode")?, distance: cells(self.distance) });
            }
            "sp_regen" | "buff" if passive => return Ok(Effect::Passive(buff()?)),
            "sp_regen" => return Err("sp_regen is a passive".into()),
            "damage" => Payload::Damage(DamageFx {
                dice: match self.dice.as_str() {
                    "weapon" => EffectDice::Weapon,
                    d => EffectDice::Fixed(parse_dice(d)?),
                },
                extra: dice_or(&self.extra)?,
                scale_k: self.scale == "k",
                mult: self.mult.unwrap_or(one),
                bonus: self.bonus.unwrap_or_default(),
                kind: match self.damage.as_str() {
                    "" => DamageType::Physical,
                    d => DamageType::from_id(d).ok_or_else(|| format!("unknown damage type {d}"))?,
                },
                save,
                knockback: cells(self.knockback),
            }),
            "heal" => Payload::Heal(HealFx { dice: parse_dice(&self.dice)?, scale_k: self.scale == "k", mult: self.mult.unwrap_or(one) }),
            "condition" => {
                let mired = self.ac.is_some_and(|a| a.0 < 0.0);
                match (save, condition) {
                    (Some(s), _) => Payload::Condition { condition: s.condition, save: true, secs: s.secs, mired },
                    (None, Some(c)) => Payload::Condition { condition: c, save: false, secs: Lin::default(), mired },
                    (None, None) if mired => Payload::Condition { condition: Condition::Mired, save: false, secs: Lin::default(), mired },
                    (None, None) => return Err("condition without a condition".into()),
                }
            }
            "push" => Payload::Push(cells(self.distance)),
            "buff" => Payload::Buff(buff()?),
            // A ranged ward holds for its whole time.
            "ward" if self.ranged => Payload::Ward { hits: Lin(f32::from(u16::MAX), 0.0), ranged: true },
            "ward" => Payload::Ward { hits: self.hits.unwrap_or(one), ranged: false },
            "barrier" => Payload::Barrier,
            "revive" => Payload::Revive { hp: self.hp.unwrap_or(Lin(0.25, 0.0)) },
            "cure" => Payload::Cure,
            "dispel" => Payload::Dispel,
            other => return Ok(Effect::Pending(other.to_string())),
        };
        // A buff's `secs` is how long it lasts on whoever gets it; only a
        // pulsing buff (an aura) is ground that lasts.
        let ground = match payload {
            Payload::Buff(_) => self.pulse.is_some(),
            _ => true,
        };
        let secs = if ground { self.secs.unwrap_or_default() } else { Lin::default() };
        let at_caster = match self.at.as_str() {
            "" | "target" => false,
            "caster" => true,
            other => return Err(format!("unknown anchor {other}")),
        };
        if self.trigger > 0.0 && secs.at(1) <= 0.0 {
            return Err("a trap needs secs (how long it waits)".into());
        }
        Ok(Effect::Act(Box::new(Act {
            area: AreaFx {
                shape: self.shape,
                radius: cells(self.radius),
                length: cells(self.length),
                width: match self.width {
                    Some(w) => cells(Some(w)),
                    None if self.shape == Shape::Line || self.shape == Shape::Rect => Lin(1.0, 0.0),
                    None => Lin::default(),
                },
                arc_cos: if self.arc > 0.0 { (self.arc.min(360.0).to_radians() / 2.0).cos() } else { -1.0 },
                at_caster,
                follow: self.follow,
                limit: self.limit.unwrap_or_default(),
                chain: self.chain.unwrap_or_default(),
                jump: if self.jump > 0.0 { self.jump * CELLS } else { 6.0 },
                scatter: self.scatter * CELLS,
                spread: self.spread,
            },
            timing: TimingFx {
                count: self.count.unwrap_or(one),
                every: self.every,
                speed: self.projectile * CELLS,
                secs,
                pulse: self.pulse.unwrap_or_else(|| payload.default_pulse()),
                trigger: self.trigger * CELLS,
                fuse: self.fuse,
            },
            payload,
        })))
    }
}

/// `"2d6"`, `"1d4+1"`.
pub fn parse_dice(s: &str) -> Result<Dice, String> {
    let bad = || format!("bad dice {s:?}");
    let (count, rest) = s.split_once('d').ok_or_else(bad)?;
    let (sides, bonus) = match rest.split_once('+') {
        Some((a, b)) => (a, b.parse::<i32>().map_err(|_| bad())?),
        None => (rest, 0),
    };
    let count = count.parse::<u32>().map_err(|_| bad())?;
    let sides = sides.parse::<u32>().map_err(|_| bad())?;
    if count == 0 || sides == 0 {
        return Err(bad());
    }
    Ok(Dice::new(count, sides, bonus))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A small neutral table: two basics, a mage line with a prerequisite,
    /// a passive and a tier-2 node.
    pub const BOOK: &str = r#"{"skills":[
      {"id":"slide","name":"Slide","class":"basic","max_level":5,"target":"escape","mobile":true,
       "cooldown":[6,-0.6],"sp":[2,0],"effects":[{"type":"move","mode":"slide","distance":[2,0.4]}]},
      {"id":"mend","name":"Mend","class":"basic","target":"self","mobile":true,"cooldown":[20,0],"sp":[3,0],
       "effects":[{"type":"heal","dice":"1d6","scale":"k"}]},
      {"id":"bolt","name":"Bolt","class":"mage","max_level":10,"target":"densest","range":10,
       "cast":[0.3,0.06],"cooldown":[1,0],"sp":[1,0.5],
       "effects":[{"type":"damage","dice":"1d4","scale":"k","damage":"fire","count":[0,1],"radius":1}]},
      {"id":"burst","name":"Burst","class":"mage","max_level":10,"target":"nearest_group","range":8,
       "requires":{"bolt":2},"sp":[1,0.4],
       "effects":[{"type":"damage","dice":"1d6","scale":"k","damage":"force","radius":1.5}]},
      {"id":"focus","name":"Focus","class":"mage","max_level":10,"passive":true,
       "effects":[{"type":"sp_regen","value":[0,0.2]}]},
      {"id":"storm","name":"Storm","class":"mage","tier":2,"min_level":30,"target":"densest","range":16,
       "requires":{"bolt":1},"effects":[{"type":"aura"}]},
      {"id":"cleave","name":"Cleave","class":"swordsman","target":"front","effects":[]},
      {"id":"mending","name":"Mending","class":"acolyte","max_level":10,"target":"most_wounded_ally","range":10,
       "cast":[0.2,0],"cooldown":[1.5,0],"sp":[2,0.6],
       "effects":[{"type":"heal","dice":"1d8","scale":"k","mult":[0,1],"radius":1.5}]},
      {"id":"chill","name":"Chill","class":"mage","max_level":10,"target":"nearest","range":10,
       "effects":[{"type":"damage","dice":"1d4","damage":"cold","radius":1,
                   "save":{"ability":"body","condition":"frozen","secs":[1,0.5]}}]},
      {"id":"curse","name":"Curse","class":"mage","target":"strongest","range":10,
       "effects":[{"type":"condition","save":{"condition":"petrified","secs":3}}]},
      {"id":"scorch","name":"Scorch","class":"mage","target":"nearest","range":10,
       "effects":[{"type":"damage","dice":"1d4","save":{"condition":"bleeding"}}]},
      {"id":"swamp","name":"Swamp","class":"mage","target":"densest","range":10,
       "effects":[{"type":"condition","radius":1.5,"secs":4,"condition":"slowed","ac":-2}]},
      {"id":"flames","name":"Flames","class":"mage","target":"aim","range":6,
       "effects":[{"type":"damage","shape":"rect","length":0.5,"width":4,"secs":[2,1],"dice":"1d6","scale":"k",
                   "damage":"fire","knockback":1,"save":{"condition":"burning","secs":3}}]},
      {"id":"shield","name":"Shield","class":"mage","target":"densest_allies","range":8,
       "effects":[{"type":"ward","radius":2,"secs":10,"hits":[1,1]}]},
      {"id":"pneuma","name":"Pneuma","class":"mage","target":"densest_allies","range":8,
       "effects":[{"type":"ward","radius":2,"secs":10,"ranged":true}]},
      {"id":"taunt","name":"Taunt","class":"swordsman","target":"self",
       "effects":[{"type":"condition","radius":4,"save":{"condition":"taunted","secs":5}}]},
      {"id":"chain","name":"Chain","class":"mage","target":"nearest","range":10,
       "effects":[{"type":"damage","dice":"1d4","chain":2,"jump":1.5}]},
      {"id":"arrow","name":"Arrow","class":"archer","target":"aim","range":8,
       "effects":[{"type":"damage","dice":"1d4","shape":"line","width":0.5,"limit":1,"knockback":1}]},
      {"id":"snare","name":"Snare","class":"archer","target":"aim","range":6,
       "effects":[{"type":"condition","radius":1,"trigger":0.5,"secs":10,"save":{"condition":"rooted","secs":3}}]},
      {"id":"icewall","name":"Ice Wall","class":"mage","target":"aim","range":6,
       "effects":[{"type":"barrier","shape":"rect","length":0.5,"width":3,"secs":5}]},
      {"id":"chant","name":"Chant","class":"acolyte","target":"self",
       "effects":[{"type":"buff","radius":2,"secs":4,"pulse":0.5,"follow":"caster","ac":2}]},
      {"id":"haste","name":"Haste","class":"acolyte","max_level":5,"target":"allies","range":5,
       "effects":[{"type":"buff","radius":5,"secs":10,"haste":0.5,"speed":0.25,"ac":[0,1]}]},
      {"id":"endure","name":"Endure","class":"swordsman","max_level":5,"target":"self",
       "effects":[{"type":"buff","secs":10,"temp_hp":"1d10","mult":[0,1],"steady":true}]},
      {"id":"homing","name":"Homing","class":"mage","target":"nearest","range":10,
       "effects":[{"type":"damage","dice":"1d4","follow":"target","projectile":5}]},
      {"id":"raise","name":"Raise","class":"acolyte","target":"downed_ally","range":10,
       "effects":[{"type":"revive","hp":0.5}]},
      {"id":"mastery","name":"Mastery","class":"swordsman","max_level":10,"passive":true,
       "effects":[{"type":"buff","to_hit":[0,0.5],"damage_flat":[0,1]}]},
      {"id":"gravity","name":"Gravity","class":"mage","target":"densest","range":8,"channel":true,
       "effects":[{"type":"damage","dice":"1d4","radius":2,"secs":4,"pulse":0.5}]},
      {"id":"pull","name":"Pull","class":"mage","target":"self",
       "effects":[{"type":"push","radius":4,"distance":-2}]}
    ],"monster_skills":[
      {"id":"splash","name":"Splash","target":"self","range":1.5,"cast":0.5,"cooldown":4,
       "effects":[{"type":"damage","dice":"2d6","damage":"cold","radius":1.5,"at":"caster"}]},
      {"id":"cloud","name":"Cloud","target":"nearest","range":4,"cast":0.5,"cooldown":10,"ranged":true,
       "effects":[{"type":"damage","dice":"1d4","damage":"poison","radius":2,"secs":3,"pulse":1},
                  {"type":"condition","radius":2,"save":{"condition":"poisoned","secs":4}}]},
      {"id":"cleave","name":"Cleave","target":"front","range":3,"cast":0.5,"cooldown":6,"hp_below":0.5,
       "effects":[{"type":"damage","dice":"4d10","shape":"arc","radius":3,"arc":120,"at":"caster","knockback":1.5}]},
      {"id":"hop","name":"Hop","target":"nearest","range":4,"cast":0.2,"cooldown":6,"chance":50,
       "effects":[{"type":"move","mode":"dash","distance":2.5},{"type":"damage","dice":"1d4","radius":0.8,"at":"caster"}]}
    ]}"#;

    pub fn book() -> SkillBook {
        SkillBook::from_json(BOOK).unwrap()
    }

    #[test]
    fn parses_the_table_in_cells() {
        let b = book();
        assert_eq!(b.len(), 28);
        let (_, bolt) = b.find("bolt").unwrap();
        assert_eq!(bolt.range, 20.0);
        assert_eq!(bolt.class, Some(Class::Mage));
        assert!(bolt.ready());
        let act = bolt.effects[0].act().unwrap();
        let Payload::Damage(fx) = &act.payload else { panic!() };
        assert_eq!((act.area.radius.at(1), fx.kind, act.timing.count.at(3)), (2.0, DamageType::Fire, 3.0));
        assert!(!b.find("storm").unwrap().1.ready(), "unknown effects are not cast");
        assert_eq!(b.find("slide").unwrap().1.class, None);
        assert_eq!(bolt.sp_cost(3), 3);
        assert!((bolt.cast_secs(10) - 0.9).abs() < 1e-6);
    }

    #[test]
    fn saves_carry_conditions() {
        let b = book();
        let Payload::Damage(fx) = &b.find("chill").unwrap().1.effects[0].act().unwrap().payload else { panic!() };
        let save = fx.save.unwrap();
        assert_eq!((save.condition, save.secs.at(2)), (Condition::Frozen, 2.0));
        let curse = b.find("curse").unwrap().1;
        assert!(curse.ready());
        let act = curse.effects[0].act().unwrap();
        assert!(act.area.is_point(1), "no radius: only the picked monster");
        assert_eq!(act.payload, Payload::Condition { condition: Condition::Petrified, save: true, secs: Lin(3.0, 0.0), mired: false });
        assert!(!b.find("scorch").unwrap().1.ready(), "an unknown condition keeps the skill pending");
    }

    #[test]
    fn ground_effects_parse_in_cells() {
        let b = book();
        let z = b.find("swamp").unwrap().1.effects[0].act().unwrap();
        assert_eq!((z.area.radius.at(1), z.timing.secs.at(1), z.timing.pulse), (3.0, 4.0, 0.25));
        assert_eq!(z.payload, Payload::Condition { condition: Condition::Slowed, save: false, secs: Lin::default(), mired: true });
        let w = b.find("flames").unwrap().1.effects[0].act().unwrap();
        let Payload::Damage(d) = &w.payload else { panic!() };
        assert_eq!((w.area.shape, w.area.width.at(1), w.area.length.at(1)), (Shape::Rect, 8.0, 1.0));
        assert_eq!((d.knockback.at(1), w.timing.secs.at(2), w.timing.pulse, d.kind), (2.0, 4.0, 0.5, DamageType::Fire));
        assert_eq!(d.save.map(|s| s.condition), Some(Condition::Burning));
        let ward = b.find("shield").unwrap().1.effects[0].act().unwrap();
        assert_eq!((ward.area.radius.at(1), &ward.payload), (4.0, &Payload::Ward { hits: Lin(1.0, 1.0), ranged: false }));
        let pneuma = b.find("pneuma").unwrap().1;
        assert!(pneuma.ready());
        assert!(matches!(pneuma.effects[0].act().unwrap().payload, Payload::Ward { ranged: true, .. }));
    }

    #[test]
    fn generic_pieces_parse() {
        let b = book();
        let arrow = b.find("arrow").unwrap().1.effects[0].act().unwrap();
        assert_eq!((arrow.area.shape, arrow.area.limit.at(1), arrow.area.width.at(1)), (Shape::Line, 1.0, 1.0));
        let snare = b.find("snare").unwrap().1.effects[0].act().unwrap();
        assert_eq!((snare.timing.trigger, snare.timing.secs.at(1)), (1.0, 10.0));
        let chant = b.find("chant").unwrap().1.effects[0].act().unwrap();
        assert_eq!((chant.area.follow, chant.timing.lasting(1), chant.timing.pulse), (Follow::Caster, true, 0.5));
        let haste = b.find("haste").unwrap().1.effects[0].act().unwrap();
        assert!(!haste.timing.lasting(1), "a buff's secs is its duration on the ally");
        let Payload::Buff(h) = &haste.payload else { panic!() };
        assert_eq!((h.mods(3).ac, h.mods(3).haste, h.secs.at(1)), (3, 0.5, 10.0));
        let homing = b.find("homing").unwrap().1.effects[0].act().unwrap();
        assert_eq!((homing.area.follow, homing.timing.speed), (Follow::Target, 10.0));
        assert!(matches!(b.find("mastery").unwrap().1.effects[0], Effect::Passive(BuffFx { .. })));
        assert_eq!(b.find("pull").unwrap().1.effects[0].act().unwrap().payload, Payload::Push(Lin(-4.0, 0.0)));
        assert!(SkillBook::from_json(r#"{"skills":[{"id":"t","name":"T","class":"basic","effects":[{"type":"damage","dice":"1d4","trigger":1}]}]}"#).is_err(), "a trap needs secs");
    }

    #[test]
    fn dice_strings() {
        assert_eq!(parse_dice("2d6").unwrap(), Dice::new(2, 6, 0));
        assert_eq!(parse_dice("1d4+1").unwrap(), Dice::new(1, 4, 1));
        assert!(parse_dice("d6").is_err());
        assert!(parse_dice("weapon").is_err());
    }

    #[test]
    fn learning_follows_points_prerequisites_and_gates() {
        let b = book();
        let mut l = Loadout::default();
        assert_eq!(b.learn(Class::Mage, 1, &mut l, "bolt"), Err(LearnError::NoPoints), "level 1 has no points");
        assert_eq!(b.learn(Class::Mage, 3, &mut l, "burst"), Err(LearnError::Requires("Bolt".into(), 2)));
        assert_eq!(b.learn(Class::Mage, 3, &mut l, "bolt"), Ok(1));
        assert_eq!(b.learn(Class::Mage, 3, &mut l, "bolt"), Ok(2));
        assert_eq!(b.learn(Class::Mage, 3, &mut l, "burst"), Err(LearnError::NoPoints));
        assert_eq!(b.learn(Class::Mage, 4, &mut l, "burst"), Ok(1));
        assert_eq!(b.learn(Class::Mage, 99, &mut l, "cleave"), Err(LearnError::WrongClass));
        assert_eq!(b.learn(Class::Mage, 20, &mut l, "storm"), Err(LearnError::Level(30)));
        assert_eq!(b.learn(Class::Swordsman, 2, &mut Loadout::default(), "slide"), Ok(1), "basics fit every class");
        let mut full = Loadout::default();
        full.skills.insert("mend".into(), 1);
        assert_eq!(b.learn(Class::Mage, 50, &mut full, "mend"), Err(LearnError::Maxed));
        assert_eq!(b.points_left(4, &l), 0);
    }

    #[test]
    fn slots_hold_learned_actives_once() {
        let b = book();
        let mut l = Loadout::default();
        l.skills.insert("bolt".into(), 1);
        l.skills.insert("focus".into(), 1);
        assert_eq!(b.set_slot(&mut l, 0, "bolt"), Ok(()));
        assert_eq!(b.set_slot(&mut l, 3, "bolt"), Ok(()));
        assert_eq!((l.slot(0), l.slot(3)), ("", "bolt"), "a skill moves between slots");
        assert_eq!(b.set_slot(&mut l, 1, "focus"), Err(LearnError::Passive));
        assert_eq!(b.set_slot(&mut l, 1, "burst"), Err(LearnError::NotLearned));
        assert_eq!(b.set_slot(&mut l, SLOTS, "bolt"), Err(LearnError::BadSlot));
        assert_eq!(b.set_slot(&mut l, 3, ""), Ok(()));
        assert_eq!(l.slot(3), "");
    }

    #[test]
    fn sanitize_drops_what_the_table_no_longer_allows() {
        let b = book();
        let mut l = Loadout::default();
        l.skills.insert("bolt".into(), 40);
        l.skills.insert("gone".into(), 3);
        l.skills.insert("cleave".into(), 1);
        l.hotbar = vec!["bolt".into(), "gone".into(), "focus".into()];
        b.sanitize(Class::Mage, &mut l);
        assert_eq!(l.skills.len(), 1);
        assert_eq!(l.level("bolt"), 10);
        assert_eq!(l.hotbar.len(), SLOTS);
        assert_eq!((l.slot(0), l.slot(1), l.slot(2)), ("bolt", "", ""));
    }
}
