//! Skill table and learning rules (COMBAT.md §1.4, §3, §4.3).
//!
//! The table is data: `<dist>/server/skills.json`, baked and validated by
//! the asset pipeline. This module only knows generic effect kinds (damage,
//! heal, move, ...) and target archetypes; which skill does what lives in
//! the data. Distances in the file are metres; here they are cells (0.5 m).

use super::dice::Dice;
use super::rules::DamageType;
use super::{Class, Condition};
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    #[default]
    Circle,
    Line,
    Arc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MoveMode {
    Slide,
    Blink,
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
    pub count: Lin,
    /// Ticks between hits (0 = all at once).
    pub every: f32,
    pub shape: Shape,
    pub radius: f32,
    pub width: f32,
    /// Cosine of half the arc.
    pub arc_cos: f32,
    pub scatter: f32,
    pub spread: bool,
    /// A failed save also leaves this condition.
    pub save: Option<SaveFx>,
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
    pub radius: f32,
    pub count: Lin,
    pub every: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Damage(DamageFx),
    Heal(HealFx),
    Move { mode: MoveMode, distance: Lin },
    /// Passive: + value % of max SP per second.
    SpRegen(Lin),
    /// Save or the condition, no damage (in `radius` of the point).
    Condition { save: SaveFx, radius: f32 },
    /// Not cast by the server yet (conditions, zones, buffs...).
    Pending(String),
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

/// Every skill, indexed by position (the order of the file).
#[derive(Clone, Debug, Default)]
pub struct SkillBook {
    skills: Vec<SkillDef>,
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
            let effects = s.effects.iter().map(RawEffect::parse).collect::<Result<Vec<_>, String>>().map_err(bad)?;
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
                target: s.target,
                range: s.range * CELLS,
                cast: s.cast,
                cooldown: s.cooldown,
                sp: s.sp,
                effects,
            });
        }
        Ok(Self { skills })
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
    count: Option<Lin>,
    #[serde(default)]
    every: f32,
    #[serde(default)]
    shape: Shape,
    #[serde(default)]
    radius: f32,
    #[serde(default)]
    width: f32,
    #[serde(default)]
    arc: f32,
    #[serde(default)]
    scatter: f32,
    #[serde(default)]
    spread: bool,
    mode: Option<MoveMode>,
    distance: Option<Lin>,
    value: Option<Lin>,
    save: Option<RawSave>,
}

#[derive(Deserialize)]
struct RawSave {
    condition: String,
    secs: Option<Lin>,
}

/// Timings stay in seconds; the caller turns them into ticks.
impl RawEffect {
    fn parse(v: &serde_json::Value) -> Result<Effect, String> {
        let kind = v.get("type").and_then(|t| t.as_str()).ok_or("effect without a type")?;
        if !matches!(kind, "damage" | "heal" | "move" | "sp_regen" | "condition") {
            return Ok(Effect::Pending(kind.to_string()));
        }
        let raw: RawEffect = serde_json::from_value(v.clone()).map_err(|e| format!("{kind} effect: {e}"))?;
        raw.build()
    }

    fn build(&self) -> Result<Effect, String> {
        // A condition the server does not apply yet keeps the skill pending.
        let save = match &self.save {
            None => None,
            Some(s) => match Condition::from_id(&s.condition) {
                Some(condition) => Some(SaveFx { condition, secs: s.secs.unwrap_or(Lin(1.0, 0.0)) }),
                None => return Ok(Effect::Pending(format!("condition {}", s.condition))),
            },
        };
        let mult = self.mult.unwrap_or(Lin(1.0, 0.0));
        let count = self.count.unwrap_or(Lin(1.0, 0.0));
        Ok(match self.kind.as_str() {
            "damage" => Effect::Damage(DamageFx {
                dice: match self.dice.as_str() {
                    "weapon" => EffectDice::Weapon,
                    d => EffectDice::Fixed(parse_dice(d)?),
                },
                extra: (!self.extra.is_empty()).then(|| parse_dice(&self.extra)).transpose()?,
                scale_k: self.scale == "k",
                mult,
                bonus: self.bonus.unwrap_or_default(),
                kind: match self.damage.as_str() {
                    "" => DamageType::Physical,
                    d => DamageType::from_id(d).ok_or_else(|| format!("unknown damage type {d}"))?,
                },
                count,
                every: self.every,
                shape: self.shape,
                radius: self.radius * CELLS,
                width: self.width * CELLS,
                arc_cos: if self.arc > 0.0 { (self.arc.to_radians() / 2.0).cos() } else { -1.0 },
                scatter: self.scatter * CELLS,
                spread: self.spread,
                save,
            }),
            "heal" => Effect::Heal(HealFx {
                dice: parse_dice(&self.dice)?,
                scale_k: self.scale == "k",
                mult,
                radius: self.radius * CELLS,
                count,
                every: self.every,
            }),
            "move" => Effect::Move {
                mode: self.mode.ok_or("move without a mode")?,
                distance: Lin(self.distance.unwrap_or_default().0 * CELLS, self.distance.unwrap_or_default().1 * CELLS),
            },
            "sp_regen" => Effect::SpRegen(self.value.unwrap_or_default()),
            "condition" => Effect::Condition {
                save: save.ok_or("condition without a save")?,
                radius: self.radius * CELLS,
            },
            other => Effect::Pending(other.to_string()),
        })
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
       "requires":{"bolt":1},"effects":[{"type":"zone"}]},
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
       "effects":[{"type":"damage","dice":"1d4","save":{"condition":"burning"}}]}
    ]}"#;

    pub fn book() -> SkillBook {
        SkillBook::from_json(BOOK).unwrap()
    }

    #[test]
    fn parses_the_table_in_cells() {
        let b = book();
        assert_eq!(b.len(), 11);
        let (_, bolt) = b.find("bolt").unwrap();
        assert_eq!(bolt.range, 20.0);
        assert_eq!(bolt.class, Some(Class::Mage));
        assert!(bolt.ready());
        let Effect::Damage(fx) = &bolt.effects[0] else { panic!() };
        assert_eq!((fx.radius, fx.kind, fx.count.at(3)), (2.0, DamageType::Fire, 3.0));
        assert!(!b.find("storm").unwrap().1.ready(), "zones are not cast yet");
        assert_eq!(b.find("slide").unwrap().1.class, None);
        assert_eq!(bolt.sp_cost(3), 3);
        assert!((bolt.cast_secs(10) - 0.9).abs() < 1e-6);
    }

    #[test]
    fn saves_carry_conditions() {
        let b = book();
        let Effect::Damage(fx) = &b.find("chill").unwrap().1.effects[0] else { panic!() };
        let save = fx.save.unwrap();
        assert_eq!((save.condition, save.secs.at(2)), (Condition::Frozen, 2.0));
        let curse = b.find("curse").unwrap().1;
        assert!(curse.ready());
        assert!(matches!(curse.effects[0], Effect::Condition { save: SaveFx { condition: Condition::Petrified, secs: Lin(3.0, 0.0) }, .. }));
        assert!(!b.find("scorch").unwrap().1.ready(), "a condition not applied yet keeps the skill pending");
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
