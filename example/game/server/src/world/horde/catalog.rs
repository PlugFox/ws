//! Monster definitions baked to `<dist>/server/monsters.json`.
//!
//! The kind index (`u8`) is the position in [`MonsterCatalog::defs`], sorted
//! by id so it is stable across restarts; it is what snapshots carry and
//! what `stage_joined.monsters` names.

use crate::world::combat::loot::Drop;
use crate::world::combat::{Archetype, Element, Size};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("monsters.json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("monster {id}: {message}")]
    Invalid { id: String, message: String },
}

#[derive(Deserialize)]
struct RawDirector {
    archetype: String,
    spawn_cost: u32,
    min_wave_time: f32,
    weight: u32,
    #[serde(default)]
    xp_reward: u32,
    /// Missions whose roster it belongs to (`missions.rs`); absent: the
    /// original Poring Island roster ([`crate::world::missions::LEGACY_ROSTER`]).
    #[serde(default)]
    missions: Vec<String>,
}

#[derive(Deserialize)]
struct RawStats {
    move_speed: f32,
    /// RO level (rAthena); places the kind inside a mission's level band.
    #[serde(default = "one")]
    level: u32,
    #[serde(default)]
    element: String,
    #[serde(default)]
    size: String,
    #[serde(default)]
    race: String,
    /// Plants: hits it takes to fell, `[min, max]` (rolled per plant).
    #[serde(default)]
    hits: Option<[u16; 2]>,
}

fn one() -> u32 {
    1
}

#[derive(Deserialize)]
struct RawSkillRef {
    id: String,
}

/// A drop table row (`drops.toml`): chance in percent, count `[min, max]`.
#[derive(Deserialize)]
struct RawDrop {
    item: String,
    chance: f64,
    #[serde(default)]
    count: Option<[u32; 2]>,
}

#[derive(Deserialize)]
struct RawMonster {
    id: String,
    name: String,
    radius: f32,
    director: RawDirector,
    stats: RawStats,
    #[serde(default)]
    skills: Vec<RawSkillRef>,
    #[serde(default)]
    drops: Vec<RawDrop>,
}

/// Resolves a baked drop table; rows naming items the server does not
/// know are skipped with a warning (the asset pack may run ahead).
fn drops_of(id: &str, raw: Vec<RawDrop>) -> Vec<Drop> {
    raw.into_iter()
        .filter_map(|d| match Drop::new(&d.item, d.chance, d.count) {
            Ok(d) => Some(d),
            Err(e) => {
                tracing::warn!("monster {id}: drop skipped: {e}");
                None
            }
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonsterDef {
    pub id: String,
    pub name: String,
    /// Body radius in cells (separation and contact).
    pub radius: f32,
    /// Cells per second.
    pub speed: f32,
    /// RO level; combat numbers come from the level it spawns at
    /// ([`crate::world::combat::MonsterBlock`]).
    pub level: u32,
    pub archetype: Archetype,
    pub element: Element,
    pub size: Size,
    /// Director credits one spawn costs.
    pub spawn_cost: u32,
    /// Seconds into the stage before the director may spawn it.
    pub unlock_secs: f32,
    /// Relative pick weight among affordable kinds.
    pub weight: u32,
    pub xp: u32,
    /// One of its kind alive at a time; always simulated at full rate.
    pub boss: bool,
    /// Pack size range the game master buys this kind in (by archetype:
    /// swarms by the dozen, chasers in small groups, tanks in ones to
    /// threes, elites and bosses alone).
    pub pack: (u16, u16),
    /// Monster skill ids (`skills.json` `monster_skills`).
    pub skills: Vec<String>,
    /// Undead or demon (Divine Protection, Demon Bane).
    pub evil: bool,
    /// What a kill may drop (rolled per kill; per party member for elites
    /// and bosses).
    pub drops: Vec<Drop>,
    /// Plants: the hits it takes to fell one, `(min, max)`, rolled per
    /// plant; (0, 0) for everything else.
    pub hits: (u16, u16),
    /// Mission ids whose roster it is in (recipe `director.missions`);
    /// empty: the legacy Poring Island roster (`missions::roster`).
    pub missions: Vec<String>,
}

impl MonsterDef {
    /// Planted at stage creation, never bought by the director.
    pub fn is_plant(&self) -> bool {
        self.archetype == Archetype::Plant
    }
}

/// Pack size range of a director archetype.
pub fn pack_of(archetype: &str) -> (u16, u16) {
    match archetype {
        "swarm" => (6, 12),
        "chaser" => (3, 6),
        "tank" => (1, 3),
        // Elites (mini-bosses) are rare and come alone; plants are planted.
        "elite" | "boss" | "plant" => (1, 1),
        _ => (1, 3),
    }
}

#[derive(Clone, Debug, Default)]
pub struct MonsterCatalog {
    defs: Vec<MonsterDef>,
}

/// Fastest monster the simulation accepts (cells/s): a tick step must stay
/// below half a cell at 20 Hz so axis-separated wall checks cannot tunnel.
pub const MAX_SPEED: f32 = 9.0;

impl MonsterCatalog {
    pub fn new(mut defs: Vec<MonsterDef>) -> Result<Self, CatalogError> {
        defs.sort_by(|a, b| a.id.cmp(&b.id));
        if defs.len() > u8::MAX as usize {
            return Err(CatalogError::Invalid { id: "*".into(), message: "more than 255 kinds".into() });
        }
        for d in &defs {
            let bad = |message: &str| Err(CatalogError::Invalid { id: d.id.clone(), message: message.into() });
            if !(d.radius.is_finite() && d.radius > 0.0 && d.radius <= 4.0) {
                return bad("radius must be in (0, 4] cells");
            }
            if !(d.speed.is_finite() && d.speed > 0.0 && d.speed <= MAX_SPEED) {
                return bad("move_speed must be in (0, 9] cells/s");
            }
            if !d.unlock_secs.is_finite() || d.unlock_secs < 0.0 {
                return bad("min_wave_time must be >= 0");
            }
            if d.pack.0 == 0 || d.pack.0 > d.pack.1 {
                return bad("pack range must be 1 <= min <= max");
            }
            if d.is_plant() {
                if d.hits.0 == 0 || d.hits.0 > d.hits.1 {
                    return bad("plants need stats.hits = [min, max] with 1 <= min <= max");
                }
            } else if !d.boss && (d.spawn_cost == 0 || d.weight == 0) {
                return bad("regular monsters need a positive spawn_cost and weight");
            }
        }
        Ok(Self { defs })
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, CatalogError> {
        let raw: BTreeMap<String, RawMonster> = serde_json::from_slice(bytes)?;
        let defs = raw
            .into_values()
            .map(|m| MonsterDef {
                drops: drops_of(&m.id, m.drops),
                boss: m.director.archetype == "boss",
                pack: pack_of(&m.director.archetype),
                archetype: Archetype::from_id(&m.director.archetype),
                level: m.stats.level.max(1),
                element: Element::from_id(&m.stats.element),
                size: Size::from_id(&m.stats.size),
                id: m.id,
                name: m.name,
                radius: m.radius,
                speed: m.stats.move_speed,
                spawn_cost: m.director.spawn_cost,
                unlock_secs: m.director.min_wave_time,
                weight: m.director.weight,
                xp: m.director.xp_reward,
                skills: m.skills.into_iter().map(|s| s.id).collect(),
                evil: matches!(m.stats.race.as_str(), "undead" | "demon"),
                hits: m.stats.hits.map_or((0, 0), |[a, b]| (a, b)),
                missions: m.director.missions,
            })
            .collect();
        Self::new(defs)
    }

    /// Loads `<dist>/server/monsters.json`.
    pub fn load(dist_root: &Path) -> Result<Self, CatalogError> {
        let path = dist_root.join("server/monsters.json");
        let bytes =
            std::fs::read(&path).map_err(|source| CatalogError::Io { path: path.display().to_string(), source })?;
        Self::parse(&bytes)
    }

    pub fn defs(&self) -> &[MonsterDef] {
        &self.defs
    }

    pub fn get(&self, kind: u8) -> &MonsterDef {
        &self.defs[kind as usize]
    }

    pub fn kind_of(&self, id: &str) -> Option<u8> {
        self.defs.iter().position(|d| d.id == id).map(|i| i as u8)
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Kind ids in index order (sent in `stage_joined`).
    pub fn ids(&self) -> Vec<String> {
        self.defs.iter().map(|d| d.id.clone()).collect()
    }
}

#[cfg(test)]
pub(crate) fn test_def(id: &str, speed: f32, cost: u32, unlock: f32, boss: bool) -> MonsterDef {
    MonsterDef {
        id: id.into(),
        name: id.into(),
        radius: if boss { 1.2 } else { 0.35 },
        speed,
        level: 1,
        archetype: if boss { Archetype::Boss } else { Archetype::Swarm },
        element: Element::Neutral,
        size: if boss { Size::Large } else { Size::Small },
        spawn_cost: cost,
        unlock_secs: unlock,
        weight: 10,
        xp: 1,
        boss,
        pack: if boss { (1, 1) } else { (1, 3) },
        skills: Vec::new(),
        evil: false,
        drops: Vec::new(),
        hits: (0, 0),
        missions: Vec::new(),
    }
}

/// A plant kind for tests: felled in `hits` blows.
#[cfg(test)]
pub(crate) fn test_plant(id: &str, hits: (u16, u16)) -> MonsterDef {
    MonsterDef {
        archetype: Archetype::Plant,
        spawn_cost: 0,
        weight: 0,
        xp: 0,
        pack: (1, 1),
        radius: 0.3,
        hits,
        ..test_def(id, 1.0, 0, 0.0, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_baked_shape_and_sorts_kinds() {
        let json = br#"{
          "willow": {"id":"willow","name":"Willow","radius":0.5,
            "director":{"archetype":"tank","spawn_cost":6,"min_wave_time":150,"tier":2,"weight":30,"xp_reward":25},
            "stats":{"max_hp":200,"move_speed":1.8,"touch_damage":12,"defense":20,"level":7,"element":"earth","size":"medium"}, "skills": []},
          "vocal": {"id":"vocal","name":"Vocal","radius":0.55,
            "director":{"archetype":"elite","spawn_cost":20,"min_wave_time":420,"weight":6,"xp_reward":80},
            "stats":{"move_speed":3.4,"level":13}},
          "angeling": {"id":"angeling","name":"Angeling","radius":0.8,
            "director":{"archetype":"boss","spawn_cost":50,"min_wave_time":600,"tier":3,"weight":5,"xp_reward":500},
            "stats":{"max_hp":8000,"move_speed":4.0,"touch_damage":40}}
        }"#;
        let c = MonsterCatalog::parse(json).unwrap();
        assert_eq!(c.ids(), vec!["angeling", "vocal", "willow"]);
        assert!(c.get(0).boss);
        assert_eq!(c.kind_of("willow"), Some(2));
        assert_eq!(c.get(2).unlock_secs, 150.0);
        assert_eq!(c.get(2).pack, (1, 3), "tanks come alone or in twos and threes");
        assert_eq!(c.get(0).pack, (1, 1));
        assert_eq!((c.get(1).pack, c.get(1).archetype, c.get(1).boss), ((1, 1), Archetype::Elite, false), "elites come alone");
        assert_eq!((c.get(2).level, c.get(2).element, c.get(2).archetype), (7, Element::Earth, Archetype::Tank));
        assert_eq!((c.get(0).level, c.get(0).element), (1, Element::Neutral), "defaults");
    }

    #[test]
    fn drop_tables_resolve_and_skip_unknown_items() {
        let json = br#"{
          "poring": {"id":"poring","name":"Poring","radius":0.35,
            "director":{"archetype":"swarm","spawn_cost":1,"min_wave_time":0,"weight":100},
            "stats":{"move_speed":2.5},
            "drops":[{"item":"jellopy","chance":12},{"item":"no_such_thing","chance":5},
                     {"item":"apple","chance":3,"count":[1,2]},{"item":"zeny","chance":100,"count":[5,9]}]}
        }"#;
        let c = MonsterCatalog::parse(json).unwrap();
        let d = &c.get(0).drops;
        assert_eq!(d.iter().map(|d| d.item).collect::<Vec<_>>(), vec!["jellopy", "apple", "zeny"]);
        assert_eq!((d[0].rate, d[1].max, d[2].min), (1200, 2, 5));
    }

    #[test]
    fn plants_parse_with_their_hit_range() {
        let json = br#"{
          "green_plant": {"id":"green_plant","name":"Green Plant","radius":0.3,
            "director":{"archetype":"plant","spawn_cost":0,"min_wave_time":0,"weight":0},
            "stats":{"move_speed":1.0,"hits":[3,6],"race":"plant"}}
        }"#;
        let c = MonsterCatalog::parse(json).unwrap();
        let d = c.get(0);
        assert!(d.is_plant() && !d.boss);
        assert_eq!((d.hits, d.pack, d.archetype.id()), ((3, 6), (1, 1), "plant"));
        assert!(MonsterCatalog::new(vec![test_plant("p", (0, 2))]).is_err(), "a plant needs hits");
        assert!(MonsterCatalog::new(vec![test_plant("p", (4, 2))]).is_err());
        assert!(MonsterCatalog::new(vec![test_plant("p", (2, 4))]).is_ok(), "plants cost nothing");
    }

    #[test]
    fn rejects_bad_values() {
        assert!(MonsterCatalog::new(vec![test_def("a", 0.0, 1, 0.0, false)]).is_err());
        assert!(MonsterCatalog::new(vec![test_def("a", 20.0, 1, 0.0, false)]).is_err());
        assert!(MonsterCatalog::new(vec![test_def("a", 2.0, 0, 0.0, false)]).is_err());
        assert!(MonsterCatalog::new(vec![test_def("a", 2.0, 0, 0.0, true)]).is_ok(), "bosses may be free");
    }
}
