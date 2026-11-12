//! Monster definitions baked to `<dist>/server/monsters.json`.
//!
//! The kind index (`u8`) is the position in [`MonsterCatalog::defs`], sorted
//! by id so it is stable across restarts; it is what snapshots carry and
//! what `stage_joined.monsters` names.

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
}

#[derive(Deserialize)]
struct RawStats {
    max_hp: i32,
    move_speed: f32,
    #[serde(default)]
    touch_damage: i32,
}

#[derive(Deserialize)]
struct RawMonster {
    id: String,
    name: String,
    radius: f32,
    director: RawDirector,
    stats: RawStats,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonsterDef {
    pub id: String,
    pub name: String,
    /// Body radius in cells (separation and contact).
    pub radius: f32,
    /// Cells per second.
    pub speed: f32,
    pub max_hp: i32,
    pub touch_damage: i32,
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
    /// swarms by the dozen, chasers in small groups, tanks alone).
    pub pack: (u16, u16),
}

/// Pack size range of a director archetype.
pub fn pack_of(archetype: &str) -> (u16, u16) {
    match archetype {
        "swarm" => (6, 12),
        "chaser" => (3, 6),
        "tank" => (1, 3),
        "boss" => (1, 1),
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
            if d.max_hp <= 0 {
                return bad("max_hp must be positive");
            }
            if !d.unlock_secs.is_finite() || d.unlock_secs < 0.0 {
                return bad("min_wave_time must be >= 0");
            }
            if d.pack.0 == 0 || d.pack.0 > d.pack.1 {
                return bad("pack range must be 1 <= min <= max");
            }
            if !d.boss && (d.spawn_cost == 0 || d.weight == 0) {
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
                boss: m.director.archetype == "boss",
                pack: pack_of(&m.director.archetype),
                id: m.id,
                name: m.name,
                radius: m.radius,
                speed: m.stats.move_speed,
                max_hp: m.stats.max_hp,
                touch_damage: m.stats.touch_damage,
                spawn_cost: m.director.spawn_cost,
                unlock_secs: m.director.min_wave_time,
                weight: m.director.weight,
                xp: m.director.xp_reward,
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
        max_hp: 50,
        touch_damage: 5,
        spawn_cost: cost,
        unlock_secs: unlock,
        weight: 10,
        xp: 1,
        boss,
        pack: if boss { (1, 1) } else { (1, 3) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_baked_shape_and_sorts_kinds() {
        let json = br#"{
          "zombie": {"id":"zombie","name":"Zombie","radius":0.6,
            "director":{"archetype":"tank","spawn_cost":6,"min_wave_time":90,"tier":2,"weight":30,"xp_reward":25},
            "stats":{"max_hp":204,"move_speed":1.8,"touch_damage":18,"defense":20}, "skills": []},
          "baphomet": {"id":"baphomet","name":"Baphomet","radius":1.2,
            "director":{"archetype":"boss","spawn_cost":50,"min_wave_time":300,"tier":3,"weight":5,"xp_reward":500},
            "stats":{"max_hp":8000,"move_speed":5.0,"touch_damage":65}}
        }"#;
        let c = MonsterCatalog::parse(json).unwrap();
        assert_eq!(c.ids(), vec!["baphomet", "zombie"]);
        assert!(c.get(0).boss);
        assert_eq!(c.kind_of("zombie"), Some(1));
        assert_eq!(c.get(1).unlock_secs, 90.0);
        assert_eq!(c.get(1).pack, (1, 3), "tanks come alone or in twos and threes");
        assert_eq!(c.get(0).pack, (1, 1));
    }

    #[test]
    fn rejects_bad_values() {
        assert!(MonsterCatalog::new(vec![test_def("a", 0.0, 1, 0.0, false)]).is_err());
        assert!(MonsterCatalog::new(vec![test_def("a", 20.0, 1, 0.0, false)]).is_err());
        assert!(MonsterCatalog::new(vec![test_def("a", 2.0, 0, 0.0, false)]).is_err());
        assert!(MonsterCatalog::new(vec![test_def("a", 2.0, 0, 0.0, true)]).is_ok(), "bosses may be free");
    }
}
