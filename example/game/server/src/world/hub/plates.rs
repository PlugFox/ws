//! The bars under the special monsters: every live elite or boss (by
//! archetype) on a stage map, its HP as a byte, to everyone on the map
//! (`MONSTER_HP`, 0xA5).
//!
//! The encounter bar at the top (`boss.rs` for scripted fights,
//! `mission.rs` for a plain boss) carries exact numbers, phases and the
//! shield, but only for the monster in the fight; this cheap feed covers
//! the bar drawn under each of them, scripted or not, engaged or not.
//! Ordinary monsters (starred ones too) have no bar and cost nothing here
//! but the scan's archetype test.
//!
//! A handful of monsters at most: a flat list of `(slot, generation)` with
//! the byte last sent. Changes go out at most every [`SEND_EVERY`] ticks,
//! a dead one as 0 once, and the whole set every [`REFRESH_SECS`] (late
//! joiners, a tab coming back).

use super::{HubEvent, HubInstance, MapIdx, Outgoing};
use crate::world::combat::Archetype;
use crate::world::horde::Horde;
use crate::world::protocol::{ServerMsg, MONSTER_ENTITY_BASE};

/// New elites and bosses are looked for this often (ticks).
const SCAN_TICKS: u32 = 10;
/// HP changes are batched this many ticks.
const SEND_EVERY: u32 = 3;
/// The whole set again this often, changed or not.
const REFRESH_SECS: u32 = 3;

#[derive(Clone, Copy, Debug)]
struct Tracked {
    slot: u16,
    generation: u8,
    sent: u8,
}

/// The stage's tracked elites and bosses.
#[derive(Clone, Debug, Default)]
pub(super) struct Plates {
    tracked: Vec<Tracked>,
    /// Per monster kind: elite or boss (built from the catalogue once).
    special: Vec<bool>,
}

/// HP as the bar's byte: 1..=255 while alive (a sliver never reads as
/// dead), 0 only for gone.
pub(super) fn hp_byte(hp: i32, max: i32) -> u8 {
    if hp <= 0 {
        return 0;
    }
    let max = i64::from(max.max(hp).max(1));
    ((i64::from(hp) * 255 + max - 1) / max).clamp(1, 255) as u8
}

impl Plates {
    /// One tick of the feed: the bytes that changed (or all of them on a
    /// refresh), `None` when there is nothing to say.
    fn step(&mut self, horde: &Horde, tick: u32, hz: u32) -> Option<Vec<(u32, u8)>> {
        let store = horde.store();
        let catalog = horde.catalog();
        if self.special.is_empty() {
            self.special = (0..=u8::MAX as usize)
                .map(|k| catalog.defs().get(k).is_some_and(|d| matches!(d.archetype, Archetype::Elite | Archetype::Boss)))
                .collect();
        }
        if tick.is_multiple_of(SCAN_TICKS) {
            for &s in store.alive() {
                let i = s as usize;
                if self.special[store.kind[i] as usize] && store.hp[i] > 0 && !self.tracked.iter().any(|t| t.slot == s && t.generation == store.generation[i]) {
                    // `sent` 0: the first pass below sends it.
                    self.tracked.push(Tracked { slot: s, generation: store.generation[i], sent: 0 });
                }
            }
        }
        let refresh = tick.is_multiple_of(REFRESH_SECS * hz.max(1));
        if !refresh && !tick.is_multiple_of(SEND_EVERY) {
            return None;
        }
        let mut bars = Vec::new();
        self.tracked.retain_mut(|t| {
            let i = t.slot as usize;
            let alive = store.is_alive(t.slot) && store.generation[i] == t.generation;
            let now = if alive { hp_byte(store.hp[i], horde.block(t.slot).hp) } else { 0 };
            if now != t.sent || refresh {
                bars.push((MONSTER_ENTITY_BASE + u32::from(t.slot), now));
                t.sent = now;
            }
            now != 0
        });
        (!bars.is_empty()).then_some(bars)
    }
}

impl HubInstance {
    /// The elites' and bosses' HP bytes to everyone on `map` (stages only).
    pub(super) fn monster_plates(&mut self, map: MapIdx, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_ref() else { return };
        let Some(bars) = self.plates.step(horde, self.tick, self.config.tick_hz) else { return };
        let msg = ServerMsg::MonsterHp { bars };
        for p in self.players.iter().filter(|p| p.map == map) {
            out.push((p.profile.user_id, HubEvent::Control(msg.clone())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::hp_byte;
    use crate::world::combat::Archetype;
    use crate::world::horde::catalog::{test_def, MonsterCatalog, MonsterDef};
    use crate::world::horde::{Horde, HordeConfig};
    use crate::world::hub::test_support::{map, portal};
    use crate::world::hub::{HubEvent, HubInstance, HubSimConfig, HubWorld, PlayerProfile};
    use crate::world::map::{CameraConstraint, LocationKind};
    use crate::world::protocol::{Look, ServerMsg, MONSTER_ENTITY_BASE};
    use std::sync::Arc;
    use uuid::Uuid;

    /// A quiet 40 × 30 field (no director) with a boss, an elite and a
    /// poring in the catalogue; player 1 on it.
    fn field() -> HubInstance {
        let row = format!("#{}#", ".".repeat(38));
        let wall = "#".repeat(40);
        let mut rows = vec![wall.as_str()];
        rows.extend(std::iter::repeat_n(row.as_str(), 28));
        rows.push(wall.as_str());
        let free = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
        let field = map("field", LocationKind::Field, free, &rows, vec![portal("to_town", [1, 1, 1], "town", [3, 3])]);
        let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
        let catalog = Arc::new(
            MonsterCatalog::new(vec![
                test_def("boss", 0.1, 50, 0.0, true),
                MonsterDef { archetype: Archetype::Elite, ..test_def("elite", 0.1, 12, 0.0, false) },
                test_def("poring", 0.1, 1, 0.0, false),
            ])
            .unwrap(),
        );
        let cfg = HordeConfig { light_radius: 30.0, director_enabled: false, seed: Some(3), ..HordeConfig::default() };
        let horde = Horde::new(Arc::clone(&world), catalog, cfg);
        let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde);
        let profile = PlayerProfile {
            user_id: 1,
            character_id: 101,
            name: "p1".into(),
            archetype: "swordsman".into(),
            look: Look::default(),
            level: 40,
            exp: 0,
            weapon: String::new(),
            loadout: Default::default(),
            bag: Default::default(),
        };
        stage.join(profile, Uuid::nil(), None, 0, &mut Vec::new());
        stage
    }

    /// Every `MONSTER_HP` pair player 1 got over `ticks`.
    fn bars(stage: &mut HubInstance, ticks: u32) -> Vec<(u32, u8)> {
        let mut out = Vec::new();
        for _ in 0..ticks {
            stage.tick(&mut out);
        }
        out.iter()
            .filter_map(|(u, e)| match e {
                HubEvent::Control(ServerMsg::MonsterHp { bars }) if *u == 1 => Some(bars.clone()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn elites_and_bosses_get_bars_ordinary_monsters_do_not() {
        let mut stage = field();
        let horde = stage.horde_mut().unwrap();
        let boss = horde.spawn(0, 30.0, 24.0).unwrap();
        let elite = horde.spawn(1, 33.0, 24.0).unwrap();
        let poring = horde.spawn(2, 36.0, 24.0).unwrap();
        let id = |s: u16| MONSTER_ENTITY_BASE + u32::from(s);
        let first = bars(&mut stage, 12);
        assert!(first.contains(&(id(boss), 255)) && first.contains(&(id(elite), 255)), "full bars on sight: {first:?}");
        assert!(!first.iter().any(|&(e, _)| e == id(poring)), "no bar for a poring");

        // Half its HP gone: one update, then quiet until the refresh.
        let horde = stage.horde_mut().unwrap();
        let max = horde.block(elite).hp;
        horde.store_mut().hp[elite as usize] = max / 2;
        let next = bars(&mut stage, 6);
        assert_eq!(next, vec![(id(elite), hp_byte(max / 2, max))], "only the change");

        // Dead: a single 0, then it is forgotten.
        stage.horde_mut().unwrap().store_mut().hp[elite as usize] = 0;
        let gone = bars(&mut stage, 20);
        assert_eq!(gone.iter().filter(|&&b| b == (id(elite), 0)).count(), 1, "gone once: {gone:?}");
        // The refresh brings the whole set back (the boss alone now).
        let later = bars(&mut stage, 60);
        assert!(later.contains(&(id(boss), 255)) && !later.iter().any(|&(e, _)| e == id(elite)), "{later:?}");
    }

    #[test]
    fn hp_bytes_round_up_and_keep_zero_for_gone() {
        assert_eq!(hp_byte(100, 100), 255);
        assert_eq!(hp_byte(50, 100), 128);
        assert_eq!(hp_byte(1, 100_000), 1, "a sliver is still alive");
        assert_eq!(hp_byte(0, 100), 0);
        assert_eq!(hp_byte(-5, 100), 0);
        assert_eq!(hp_byte(120, 100), 255, "over max (a heal past it) caps");
    }
}
