//! Thor Volcano (mission `thor_volcano`, docs/locations/thor_volcano.md §3): the
//! location's own mechanics, on top of the mission platform (`env.rs` runs
//! the vents, the Lava Surges of the two halves with their volcanic bombs,
//! the tremor that warns of Ifrit; `missions.rs` THOR_ENV):
//!
//! * **Obsidian**: a cold spell landing on lava (a vent, a surge, a bomb's
//!   embers: hazards of look `fire`) quenches it into calm ground for a few
//!   seconds (`env.rs` `GROUND_MEETS`, shared with the Culvert's ice); the
//!   calm discs also shelter from Ifrit's Eruption (`boss.rs` lights).
//! * **Ifrit rises from his chamber**: the first time the boss is on the
//!   field he is moved into the south-east chamber and the ground shakes.
//! * **Eruption floods both halves**: when Ifrit falls under a quarter of
//!   his HP (his last phase opens with the Eruption) both Lava Surges are
//!   forced at once (`env_force_event`).
//! * **Byorgues travel hidden** (the generic burrowed state: untargetable,
//!   drawn as a heat-warped ripple of rock): they surface next to a player
//!   to strike from behind (their Ambush skill) and slip away again once
//!   nobody is close.
//!
//! The state is a few small vectors scanned every few ticks.

use super::{HubInstance, MapIdx, Outgoing};
use crate::world::protocol::look;
use tracing::info;

/// The mission this module runs on.
pub(super) const MISSION: &str = "thor_volcano";
/// Hazard looks that are lava (cold quenches them into obsidian).
pub(super) const LAVA: &[u8] = &[look::FIRE];
/// Ifrit's magma chamber (cells; the south-east group's first chamber).
pub(super) const IFRIT_CHAMBER: (f32, f32) = (222.5, 193.5);
/// Byorgues surface within this many cells of a standing player, stay up
/// at least `HIDE_UP_SECS`, and slip away again when nobody is within
/// `HIDE_AGAIN`.
const SURFACE: f32 = 3.5;
const HIDE_AGAIN: f32 = 7.0;
const HIDE_UP_SECS: f32 = 5.0;
/// Ifrit's Eruption phase starts under this share of his HP (%).
const ERUPTION_BELOW: i64 = 25;
/// Scan the field for Byorgues and the boss this often (ticks).
const SCAN_TICKS: u32 = 3;

/// The volcano's state on one stage.
#[derive(Debug, Default)]
pub(super) struct Volcano {
    /// The boss has risen (he is moved into his chamber once).
    risen: bool,
    /// Byorgues on the field: (slot, generation, run second it may hide again).
    hidden: Vec<(u16, u8, f32)>,
    /// The Ifrit whose Eruption already flooded both halves.
    erupted: Option<(u16, u8)>,
}

impl HubInstance {
    /// Once per tick from `env_step` (the `thor_volcano` fence).
    pub(super) fn thor_step(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        if !self.tick.is_multiple_of(SCAN_TICKS) {
            return;
        }
        let mut v = std::mem::take(&mut self.env.thor);
        self.ifrit_rises(&mut v, map, out);
        self.eruption(&mut v, map, now, out);
        self.byorgues(&mut v, map, now);
        self.env.thor = v;
    }

    /// Ifrit comes up out of his chamber the first time he is on the field.
    fn ifrit_rises(&mut self, v: &mut Volcano, map: MapIdx, out: &mut Outgoing) {
        if v.risen {
            return;
        }
        let Some(horde) = self.horde.as_mut() else { return };
        let Some(kind) = horde.catalog().kind_of("ifrit") else {
            v.risen = true;
            return;
        };
        let store = horde.store();
        let Some(slot) = store.alive().iter().copied().find(|&s| store.kind[s as usize] == kind) else { return };
        v.risen = true;
        if self.world.map(map).walk.is_walkable(IFRIT_CHAMBER.0, IFRIT_CHAMBER.1) {
            horde.place(slot, IFRIT_CHAMBER.0, IFRIT_CHAMBER.1);
        }
        info!("{}: Ifrit rises from his chamber", self.name);
        self.env_shake(map, 230, 2200, out);
    }

    /// Ifrit's last phase: both halves of the volcano flood at once.
    fn eruption(&mut self, v: &mut Volcano, map: MapIdx, now: f32, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_ref() else { return };
        let Some(kind) = horde.catalog().kind_of("ifrit") else { return };
        let store = horde.store();
        let Some(s) = store.alive().iter().copied().find(|&s| store.kind[s as usize] == kind) else { return };
        let g = store.generation[s as usize];
        let max = horde.block(s).hp.max(1) as i64;
        if v.erupted == Some((s, g)) || store.hp[s as usize] as i64 * 100 > ERUPTION_BELOW * max {
            return;
        }
        v.erupted = Some((s, g));
        let west = self.env_force_event(map, "surge_west", now, out);
        let east = self.env_force_event(map, "surge_east", now, out);
        info!("{}: Ifrit erupts (west {west}, east {east})", self.name);
    }

    /// Byorgues: hidden while they travel, out to strike.
    fn byorgues(&mut self, v: &mut Volcano, map: MapIdx, now: f32) {
        let Some(horde) = self.horde.as_mut() else { return };
        let Some(kind) = horde.catalog().kind_of("byorgue") else { return };
        let ups: Vec<(f32, f32)> =
            self.players.iter().filter(|p| p.map == map && p.fighter.as_ref().is_some_and(|f| f.is_up())).map(|p| (p.x, p.y)).collect();
        let store = horde.store();
        v.hidden.retain(|&(s, g, _)| store.is_alive(s) && store.generation[s as usize] == g);
        let fresh: Vec<u16> = store
            .alive()
            .iter()
            .copied()
            .filter(|&s| store.kind[s as usize] == kind && !v.hidden.iter().any(|h| h.0 == s))
            .collect();
        for s in fresh {
            let g = horde.store().generation[s as usize];
            horde.set_burrowed(s, true);
            v.hidden.push((s, g, 0.0));
        }
        for h in v.hidden.iter_mut() {
            let s = h.0 as usize;
            let (x, y) = (horde.store().x[s], horde.store().y[s]);
            let near = ups.iter().map(|&(px, py)| (px - x).powi(2) + (py - y).powi(2)).fold(f32::INFINITY, f32::min).sqrt();
            let under = horde.store().burrowed(h.0);
            if under && near <= SURFACE {
                horde.set_burrowed(h.0, false);
                h.2 = now + HIDE_UP_SECS;
            } else if !under && now >= h.2 && near > HIDE_AGAIN {
                horde.set_burrowed(h.0, true);
            }
        }
    }
}

#[cfg(test)]
#[path = "thor_tests.rs"]
mod tests;
