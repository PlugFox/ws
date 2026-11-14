//! Mission catalogue offered by mission-board NPCs.
//!
//! Static for now. A selection forms a lobby (`lobby.rs`); starting it
//! launches a stage instance of `map` (`stage.rs`) whose monsters come in
//! waves bought under the mission's [`WaveRules`].

use super::combat::MonsterBlock;
use super::horde::catalog::MonsterCatalog;
use super::horde::director::WaveRules;
use super::modifiers::MonsterScale;

#[derive(Clone, Debug, PartialEq)]
pub struct MissionInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// Stage location the mission is played on.
    pub map: &'static str,
    pub max_players: u8,
    /// Surviving this long wins even if the boss lives (seconds).
    pub time_limit_secs: f32,
    /// Difficulty: what the wave game master gets to spend and where packs
    /// land. Server-side only.
    pub waves: WaveRules,
    /// Overtime poison fog (`hub/fog.rs`); `None`: the run never fogs.
    pub fog: Option<FogRules>,
}

/// When a dragged-out run starts to close in (COMBAT.md §4.7): a poison fog
/// rises at the edge of the field and its safe circle shrinks to a small
/// walkable spot. Server-side only; the wire carries the circle (`FOG`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FogRules {
    /// The fog rises once a boss has been on the field this long (s) ...
    pub after_boss_secs: f32,
    /// ... and at this point of the run at the latest, boss or not (s).
    pub latest_secs: f32,
    /// The safe circle closes from the edge of the reachable field to
    /// `final_radius` over this long (s), then holds.
    pub shrink_secs: f32,
    /// Radius of the last safe circle (cells).
    pub final_radius: f32,
}

impl FogRules {
    /// Poring Island: Angeling unlocks at 10:00 and the run is won at
    /// 15:00. The fog rises two minutes after the boss shows up (12:00 at
    /// the latest), closes by 14:30 and holds a 5 m circle to the end.
    pub const NORMAL: FogRules = FogRules { after_boss_secs: 120.0, latest_secs: 720.0, shrink_secs: 150.0, final_radius: 10.0 };
}

pub const MISSIONS: &[MissionInfo] = &[MissionInfo {
    id: "poring_island_survival",
    name: "Poring Island Survival",
    description: "Hold out against growing waves on Poring Island until the boss shows up.",
    map: "prt_fild08",
    max_players: 4,
    time_limit_secs: 900.0,
    waves: WaveRules::NORMAL,
    fog: Some(FogRules::NORMAL),
}];

pub fn find(id: &str) -> Option<&'static MissionInfo> {
    MISSIONS.iter().find(|m| m.id == id)
}

/// What the mission window and the in-run timeline show about a mission
/// (`MISSION_INFO`), derived from the live tables only: the mission's
/// [`WaveRules`] and time limit, and the monster catalogue the stage's horde
/// spawns from (`monsters.json`: roster, unlock times, levels, drop tables).
/// Nothing here is written by hand, so a changed roster or drop table shows
/// up in the window as it plays.
#[derive(Clone, Debug, PartialEq)]
pub struct MissionBrief {
    pub mission: String,
    /// Stage map; the client finds the illustration by it.
    pub map: String,
    pub time_limit_secs: u32,
    /// Monster level band (`WaveRules::levels`); also the recommended
    /// player levels.
    pub levels: (u32, u32),
    /// Monsters gain a level this often (seconds).
    pub level_every_secs: u32,
    /// Every kind that can spawn before the time limit, by unlock time.
    pub monsters: Vec<MonsterBrief>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MonsterBrief {
    pub id: String,
    pub name: String,
    /// `swarm`, `chaser`, `tank`, `elite`, `boss`.
    pub archetype: String,
    pub element: String,
    pub size: String,
    /// First moment the game master may send it (seconds).
    pub unlock_secs: u32,
    /// Levels it spawns at between its unlock and the time limit.
    pub levels: (u32, u32),
    /// Hit points at those levels (one player).
    pub hp: (u32, u32),
    /// Pack size range the game master sends it in.
    pub pack: (u32, u32),
    pub drops: Vec<DropBrief>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DropBrief {
    /// Item id, or `zeny`.
    pub item: String,
    /// Chance in hundredths of a percent (10000 = always).
    pub rate: u32,
    pub min: u32,
    pub max: u32,
}

/// Builds the brief of `mission` from the catalogue its stages spawn from.
pub fn brief(mission: &MissionInfo, rules: &WaveRules, catalog: &MonsterCatalog) -> MissionBrief {
    brief_with(mission, rules, catalog, &MonsterScale::PLAIN)
}

/// [`brief`] of a run under mission modifiers: hit points as `scale`
/// makes them (a stage's own brief).
pub fn brief_with(mission: &MissionInfo, rules: &WaveRules, catalog: &MonsterCatalog, scale: &MonsterScale) -> MissionBrief {
    let limit = mission.time_limit_secs;
    let mut monsters: Vec<MonsterBrief> = catalog
        .defs()
        .iter()
        // Plants are scenery the stage plants, not waves the brief lists.
        .filter(|d| d.unlock_secs < limit && !d.is_plant())
        .map(|d| {
            let lo = rules.level_at(d.level, d.boss, d.unlock_secs);
            let hi = rules.level_at(d.level, d.boss, limit);
            let hp = |level: u32| scale.apply(MonsterBlock::new(level, d.archetype, 1)).hp.max(0) as u32;
            MonsterBrief {
                id: d.id.clone(),
                name: d.name.clone(),
                archetype: d.archetype.id().into(),
                element: d.element.id().into(),
                size: d.size.id().into(),
                unlock_secs: d.unlock_secs.round() as u32,
                levels: (lo, hi),
                hp: (hp(lo), hp(hi)),
                pack: (d.pack.0.into(), d.pack.1.into()),
                drops: d.drops.iter().map(|r| DropBrief { item: r.item.into(), rate: r.rate, min: r.min, max: r.max }).collect(),
            }
        })
        .collect();
    monsters.sort_by(|a, b| a.unlock_secs.cmp(&b.unlock_secs).then_with(|| a.id.cmp(&b.id)));
    MissionBrief {
        mission: mission.id.into(),
        map: mission.map.into(),
        time_limit_secs: limit.round() as u32,
        levels: (rules.levels.0.max(1), rules.levels.1.max(rules.levels.0.max(1))),
        level_every_secs: rules.level_every_secs.round() as u32,
        monsters,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::combat::loot::Drop;
    use crate::world::horde::catalog::{test_def, MonsterCatalog, MonsterDef};

    fn catalog(slime_drops: Vec<Drop>) -> MonsterCatalog {
        MonsterCatalog::new(vec![
            MonsterDef { level: 3, drops: slime_drops, ..test_def("slime", 2.0, 1, 0.0, false) },
            MonsterDef { level: 9, archetype: crate::world::combat::Archetype::Elite, ..test_def("knight", 2.0, 12, 240.0, false) },
            test_def("king", 3.0, 50, 600.0, true),
            test_def("never", 2.0, 1, 5000.0, false),
        ])
        .unwrap()
    }

    #[test]
    fn brief_comes_from_the_live_catalogue_and_rules() {
        let m = &MISSIONS[0];
        let rules = &m.waves;
        let c = catalog(vec![Drop::new("jellopy", 12.0, None).unwrap()]);
        let b = brief(m, rules, &c);
        assert_eq!((b.mission.as_str(), b.map.as_str()), (m.id, m.map));
        assert_eq!(b.time_limit_secs, m.time_limit_secs as u32);
        assert_eq!(b.levels, rules.levels);
        let ids: Vec<&str> = b.monsters.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, ["slime", "knight", "king"], "by unlock time; kinds unlocking after the limit never spawn");
        let (slime, knight, king) = (&b.monsters[0], &b.monsters[1], &b.monsters[2]);
        // Levels follow the spawn formula at unlock and at the time limit.
        assert_eq!(slime.levels, (rules.level_at(3, false, 0.0), rules.level_at(3, false, m.time_limit_secs)));
        assert_eq!(knight.levels.0, rules.level_at(9, false, 240.0));
        assert_eq!((knight.archetype.as_str(), knight.unlock_secs), ("elite", 240));
        assert_eq!((king.archetype.as_str(), king.levels.0), ("boss", rules.levels.1 + 5));
        assert_eq!(slime.hp.0, MonsterBlock::new(slime.levels.0, slime_def(&c).archetype, 1).hp as u32);
        assert_eq!(slime.drops, vec![DropBrief { item: "jellopy".into(), rate: 1200, min: 1, max: 1 }]);

        // A changed drop table or roster shows up as is.
        let c2 = catalog(vec![Drop::new("apple", 50.0, Some([2, 3])).unwrap()]);
        let b2 = brief(m, rules, &c2);
        assert_eq!(b2.monsters[0].drops, vec![DropBrief { item: "apple".into(), rate: 5000, min: 2, max: 3 }]);
        let faster = WaveRules { levels: (5, 30), level_every_secs: 60.0, ..rules.clone() };
        let b3 = brief(m, &faster, &c);
        assert_eq!((b3.levels, b3.level_every_secs), ((5, 30), 60));
        assert_eq!(b3.monsters[2].levels.0, 35);
    }

    fn slime_def(c: &MonsterCatalog) -> &MonsterDef {
        c.get(c.kind_of("slime").unwrap())
    }

    /// The real pack (when the asset submodule is baked): every monster of
    /// `monsters.json` that can spawn is in the brief with its baked table.
    #[test]
    fn brief_matches_the_baked_pack() {
        let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
        let Ok(c) = MonsterCatalog::load(&dist) else { return };
        let m = &MISSIONS[0];
        let b = brief(m, &m.waves, &c);
        for d in c.defs().iter().filter(|d| d.unlock_secs < m.time_limit_secs && !d.is_plant()) {
            let got = b.monsters.iter().find(|x| x.id == d.id).unwrap_or_else(|| panic!("{} missing", d.id));
            assert_eq!(got.unlock_secs, d.unlock_secs.round() as u32);
            assert_eq!(got.drops.len(), d.drops.len());
            for (g, r) in got.drops.iter().zip(&d.drops) {
                assert_eq!((g.item.as_str(), g.rate, g.min, g.max), (r.item, r.rate, r.min, r.max));
            }
        }
        assert_eq!(b.monsters.iter().filter(|x| x.archetype == "boss").count(), c.defs().iter().filter(|d| d.boss && d.unlock_secs < m.time_limit_secs).count());
    }
}
