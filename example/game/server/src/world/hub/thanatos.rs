//! Thanatos Tower (mission `thanatos_tower`, docs/locations/thanatos_tower.md
//! §3): the location's own mechanics, on top of the mission platform
//! (`env.rs` runs the three stairs, the Judgment and its beams, the
//! specimen tubes; `missions.rs` TOWER_ENV and the acts' roster rows):
//!
//! * **Thanatos forms in the nave**: the first time the boss is on the
//!   field he is moved into the great nave and the ground shakes.
//! * **Fragments mend him**: while Dolor or Maero (the adds his openers
//!   call) stands, Thanatos regenerates [`MEND_PCT_TENTHS`] tenths of a
//!   percent of his HP a second, shown as a pale link: kill them.
//! * **Oblivion forces a Judgment**: from his last phase on, the statues
//!   open their eyes whenever no Judgment runs and the last forced one is
//!   [`OBLIVION_EVERY`] s ago (the first one as the phase begins).
//! * **Judged worthy**: a beam of the Judgment landing on angels mends
//!   them [`JUDGED_PCT`] % (drag the fight away from them); beams never
//!   land beside a statue (the strikes' mask, `missions.rs`).
//!
//! The state is a few numbers; the scan runs every few ticks.

use super::area::Area;
use super::{HubInstance, MapIdx, Outgoing};
use crate::world::missions::tower;
use crate::world::protocol::{look, zone_flags, BossZone, ServerMsg};
use tracing::info;

/// The mission this module runs on.
pub(super) const MISSION: &str = "thanatos_tower";
/// Where Thanatos forms: the great nave (cells, clearance 25).
pub(super) const NAVE: (f32, f32) = (148.5, 95.5);
/// The boss and his adds (monster ids).
pub(super) const BOSS: &str = "thanatos";
pub(super) const FRAGMENTS: [&str; 2] = ["tha_dolor", "tha_maero"];
/// The angels a beam mends.
pub(super) const ANGELS: [&str; 4] = ["retribution", "solace", "shelter", "observation"];
/// Thanatos mends this many tenths of a percent of his HP a second while
/// a fragment stands.
pub(super) const MEND_PCT_TENTHS: i64 = 5;
/// His last phase begins at this share of his HP (%; `scripts/thanatos.rs`).
pub(super) const OBLIVION_BELOW: i64 = 25;
/// Oblivion forces a Judgment at most this often (s).
pub(super) const OBLIVION_EVERY: f32 = 45.0;
/// A beam landing mends the angels within this many cells of it this
/// share of their HP (%).
pub(super) const JUDGED_R: f32 = 3.0;
pub(super) const JUDGED_PCT: i32 = 10;
/// The statues' calm ring while they judge (cells; the strikes' mask is 3).
pub(super) const STATUE_RING: f32 = 2.6;
/// Scan the field this often (ticks).
const SCAN_TICKS: u32 = 3;

/// The tower's state on one stage.
#[derive(Debug, Default)]
pub(super) struct Tower {
    /// The boss has formed (he is moved into the nave once).
    risen: bool,
    /// Next mending second (run seconds).
    mend_at: f32,
    /// The run second Oblivion last forced a Judgment (negative: never).
    forced_at: Option<f32>,
    /// The Judgment whose eyes were shown (its start, run seconds).
    eyes_for: Option<f32>,
}

impl HubInstance {
    /// Once per tick from `env_step` (the `thanatos_tower` fence).
    pub(super) fn tower_step(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        if !self.tick.is_multiple_of(SCAN_TICKS) {
            return;
        }
        let mut t = std::mem::take(&mut self.env.tower);
        self.thanatos_forms(&mut t, map, out);
        self.fragments_mend(&mut t, map, now, out);
        self.oblivion(&mut t, map, now, out);
        self.statues_watch(&mut t, map, now, out);
        self.env.tower = t;
    }

    /// The live boss: (slot, HP, max HP).
    fn tower_boss(&self) -> Option<(u16, i32, i32)> {
        let horde = self.horde.as_ref()?;
        let kind = horde.catalog().kind_of(BOSS)?;
        let store = horde.store();
        let s = store.alive().iter().copied().find(|&s| store.kind[s as usize] == kind)?;
        Some((s, store.hp[s as usize], horde.block(s).hp.max(1)))
    }

    /// Thanatos forms in the nave the first time he is on the field.
    fn thanatos_forms(&mut self, t: &mut Tower, map: MapIdx, out: &mut Outgoing) {
        if t.risen {
            return;
        }
        let Some((slot, ..)) = self.tower_boss() else { return };
        t.risen = true;
        if self.world.map(map).walk.is_walkable(NAVE.0, NAVE.1)
            && let Some(horde) = self.horde.as_mut()
        {
            horde.place(slot, NAVE.0, NAVE.1);
        }
        info!("{}: Thanatos forms in the nave", self.name);
        self.env_shake(map, 230, 2200, out);
    }

    /// While a fragment stands, Thanatos mends a little every second; a
    /// pale ring on each fragment shows the link.
    fn fragments_mend(&mut self, t: &mut Tower, map: MapIdx, now: f32, out: &mut Outgoing) {
        if now < t.mend_at {
            return;
        }
        t.mend_at = now + 1.0;
        let Some((boss, hp, max)) = self.tower_boss() else { return };
        let Some(horde) = self.horde.as_mut() else { return };
        let kinds: Vec<u8> = FRAGMENTS.iter().filter_map(|id| horde.catalog().kind_of(id)).collect();
        let store = horde.store();
        let links: Vec<(f32, f32)> =
            store.alive().iter().filter(|&&s| kinds.contains(&store.kind[s as usize])).map(|&s| (store.x[s as usize], store.y[s as usize])).collect();
        if links.is_empty() || hp >= max {
            return;
        }
        let mend = ((max as i64 * MEND_PCT_TENTHS + 999) / 1000).max(1) as i32;
        horde.store_mut().hp[boss as usize] = (hp + mend).min(max);
        let zones = links
            .iter()
            .map(|&(x, y)| {
                let id = self.next_zone;
                self.next_zone = self.next_zone.wrapping_add(1).max(1);
                BossZone { area: Area::Circle { x, y, r: 1.4 }.telegraph(id, look::HEAL, 900, 0), delay_ms: 0, flags: 0 }
            })
            .collect();
        self.broadcast(map, &ServerMsg::BossZones { caster: 0, ability: "tower_mend".into(), zones }, out);
    }

    /// In his last phase, Oblivion calls the statues' Judgment.
    fn oblivion(&mut self, t: &mut Tower, map: MapIdx, now: f32, out: &mut Outgoing) {
        let Some((_, hp, max)) = self.tower_boss() else { return };
        if hp as i64 * 100 > OBLIVION_BELOW * max as i64 || t.forced_at.is_some_and(|at| now - at < OBLIVION_EVERY) {
            return;
        }
        if self.env_force_event(map, tower::JUDGMENT, now, out) {
            t.forced_at = Some(now);
            info!("{}: Oblivion calls the Judgment at {now:.0} s", self.name);
        }
    }

    /// While a Judgment runs the statues' eyes glow: a gold ring of calm
    /// ground at each statue's base (no beam lands there).
    fn statues_watch(&mut self, t: &mut Tower, map: MapIdx, now: f32, out: &mut Outgoing) {
        let Some(start) = self.env_event_start(tower::JUDGMENT) else { return };
        if t.eyes_for == Some(start) {
            return;
        }
        t.eyes_for = Some(start);
        let left = crate::world::missions::find(MISSION).and_then(|m| m.env.event(tower::JUDGMENT)).map_or(0.0, |e| start + e.duration_secs - now).max(0.0);
        let span = (left * 1000.0) as u32;
        let zones = tower::STATUES
            .iter()
            .map(|&(x, y)| {
                let id = self.next_zone;
                self.next_zone = self.next_zone.wrapping_add(1).max(1);
                BossZone { area: Area::Circle { x, y, r: STATUE_RING }.telegraph(id, look::GOLD, span, 0), delay_ms: 0, flags: zone_flags::SAFE }
            })
            .collect();
        self.broadcast(map, &ServerMsg::BossZones { caster: 0, ability: "tower_eyes".into(), zones }, out);
    }

    /// A beam of the Judgment landed at `(x, y)`: the angels it caught are
    /// judged worthy and mend.
    pub(super) fn tower_judged(&mut self, map: MapIdx, x: f32, y: f32, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_mut() else { return };
        let kinds: Vec<u8> = ANGELS.iter().filter_map(|id| horde.catalog().kind_of(id)).collect();
        let caught: Vec<u16> = {
            let store = horde.store();
            store
                .alive()
                .iter()
                .copied()
                .filter(|&s| {
                    let i = s as usize;
                    kinds.contains(&store.kind[i]) && (store.x[i] - x).powi(2) + (store.y[i] - y).powi(2) <= JUDGED_R * JUDGED_R
                })
                .collect()
        };
        if caught.is_empty() {
            return;
        }
        let mut spots = Vec::with_capacity(caught.len());
        for s in caught {
            let max = horde.block(s).hp.max(1);
            let store = horde.store_mut();
            let i = s as usize;
            store.hp[i] = (store.hp[i] + max * JUDGED_PCT / 100).min(max);
            spots.push((store.x[i], store.y[i]));
        }
        let zones = spots
            .into_iter()
            .map(|(x, y)| BossZone { area: Area::Circle { x, y, r: 1.2 }.telegraph(0, look::HEAL, 0, 0), delay_ms: 0, flags: 0 })
            .collect();
        self.broadcast(map, &ServerMsg::BossZones { caster: 0, ability: "tower_judged".into(), zones }, out);
    }
}

#[cfg(test)]
#[path = "thanatos_tests.rs"]
mod tests;
