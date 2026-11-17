//! Toy Factory (mission `toy_factory`, docs/locations/toy_factory.md §3): the
//! location's own mechanics, on top of the mission platform (`env.rs` runs
//! the belts' drift, the shift's events and crates, and plants the
//! switches):
//!
//! * **Conveyor belts**: the baked lanes (`hazards.json`, ids
//!   [`BELT_PREFIX`]`<n>`) carry players and monsters along
//!   (`Hazard::drift`). This module sets each lane's drift scale: normal
//!   (1), reversed and faster while a **shift change** runs
//!   ([`SHIFT_SCALE`]), stopped (0) while a switch near it holds it.
//! * **Shift change** (`missions.rs` `SHIFT_CHANGE`): the whistle; the
//!   belts turn round, the cranes drop crates (event strikes, hook
//!   [`CRATE_HOOK`]: [`CRATE_MYST_PCT`] % of them leave a Myst Case), and
//!   from [`WIND_UP_FROM`] on the plain toys are wound up (+[`WIND_UP_SPEED`] %
//!   speed while it runs).
//! * **Switches**: the map's ten switches are switch objects (a plant
//!   monster, two blows, hook [`SWITCH_HOOK`]). Throwing one stops every
//!   belt within [`SWITCH_R`] cells for [`STOP_SECS`]; a thrown switch
//!   stands there for [`SWITCH_COOLDOWN`] before it can be thrown again.
//! * **Midnight**: the Foreman under [`MIDNIGHT_BELOW`] % HP blows the
//!   whistle off schedule (a shift change at once, `env_force_event`).
//!
//! The state is a few small vectors; a quiet tick allocates nothing.

use crate::world::environment::{HazardShape, ObjectEffect};
use crate::world::horde::storage::Rally;
use crate::world::protocol::{look, zone_flags, BossZone, ServerMsg, Telegraph};
use super::cast::ticks_of;
use super::{HubEvent, HubInstance, MapIdx, Outgoing};
use crate::world::combat::Archetype;
use tracing::info;

/// The mission this module runs on.
pub(super) const MISSION: &str = "toy_factory";
/// The baked belt lanes' ids start with this.
pub(super) const BELT_PREFIX: &str = "belt_";
/// The shift change map event (`missions.rs`).
pub(super) const SHIFT: &str = "shift_change";
/// A belt's drift during a shift: reversed, half as fast again.
pub(super) const SHIFT_SCALE: f32 = -1.5;
/// Object hook id of a thrown switch (`missions.rs`).
pub(super) const SWITCH_HOOK: &str = "switch";
/// The live switch and the thrown one standing while it resets.
pub(super) const SWITCH_MONSTER: &str = "factory_switch";
pub(super) const SWITCH_OFF: &str = "factory_switch_off";
/// Effects key of the clunk (effects.toml `[skills.toy_switch]`).
pub(super) const SWITCH_ABILITY: &str = "toy_switch";
/// A switch stops the belts within this many cells (20 m) ...
pub(super) const SWITCH_R: f32 = 40.0;
/// ... for this long, and stands thrown this long.
pub(super) const STOP_SECS: f32 = 30.0;
pub(super) const SWITCH_COOLDOWN: f32 = 60.0;
/// Strike hook id of a crate landing (`missions.rs` `CRATES`).
pub(super) const CRATE_HOOK: &str = "crate";
/// Percent of crates that leave a Myst Case.
pub(super) const CRATE_MYST_PCT: u32 = 30;
const MYST_CASE: &str = "mystcase";
/// From this run second a shift winds the plain toys up ...
pub(super) const WIND_UP_FROM: f32 = 360.0;
/// ... this many percent faster while it runs.
pub(super) const WIND_UP_SPEED: u8 = 20;
/// The Foreman blows the whistle under this much HP (%).
pub(super) const MIDNIGHT_BELOW: i64 = 25;
const FOREMAN: &str = "foreman";

/// One belt lane and the scale it runs at.
#[derive(Clone, Copy, Debug)]
struct Lane {
    id: &'static str,
    shape: HazardShape,
    /// A switch holds it still until this run second.
    stopped_until: f32,
    /// The drift scale last set.
    scale: f32,
}

/// A thrown switch resetting.
#[derive(Clone, Copy, Debug)]
struct Thrown {
    x: f32,
    y: f32,
    until: f32,
    slot: u16,
    generation: u8,
}

/// The belts, the switches and the wind-up of a Toy Factory stage.
#[derive(Debug, Default)]
pub(super) struct Factory {
    lanes: Vec<Lane>,
    thrown: Vec<Thrown>,
    started: bool,
    /// Next run second the slow checks run (switches, wind-up, Foreman).
    next_check: f32,
    /// The Foreman that blew the whistle (slot, generation).
    midnight: Option<(u16, u8)>,
    /// Scratch: lane changes of a step (index, scale).
    changes: Vec<(usize, f32)>,
}

impl Factory {
    /// The scale lane `id` runs at now (tests).
    #[cfg(test)]
    pub fn scale(&self, id: &str) -> Option<f32> {
        self.lanes.iter().find(|l| l.id == id).map(|l| l.scale)
    }

    /// How many lanes the stage has (tests).
    #[cfg(test)]
    pub fn lanes(&self) -> usize {
        self.lanes.len()
    }
}

/// Distance from `(x, y)` to a hazard's shape (0 inside).
fn reach(shape: &HazardShape, x: f32, y: f32) -> f32 {
    match *shape {
        HazardShape::Circle { x: cx, y: cy, r } => (((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - r).max(0.0),
        HazardShape::Rect { x: cx, y: cy, w, h } => {
            let dx = ((x - cx).abs() - w / 2.0).max(0.0);
            let dy = ((y - cy).abs() - h / 2.0).max(0.0);
            (dx * dx + dy * dy).sqrt()
        }
    }
}

impl HubInstance {
    /// Once per tick on a Toy Factory stage: each belt's scale (normal,
    /// reversed in a shift, stopped by a switch), then once a second the
    /// switches resetting, the wind-up and the Foreman's whistle.
    pub(super) fn toy_step(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        if !self.env.toy.started {
            self.env.toy.started = true;
            self.env.toy.lanes = self
                .env_drifting()
                .into_iter()
                .filter(|(id, _)| id.starts_with(BELT_PREFIX))
                .map(|(id, shape)| Lane { id, shape, stopped_until: 0.0, scale: 1.0 })
                .collect();
            info!("{}: {} belt lanes", self.name, self.env.toy.lanes.len());
        }
        let shift = self.env_event_start(SHIFT).is_some();
        let mut changes = std::mem::take(&mut self.env.toy.changes);
        changes.clear();
        for (i, l) in self.env.toy.lanes.iter().enumerate() {
            let want = if l.stopped_until > now {
                0.0
            } else if shift {
                SHIFT_SCALE
            } else {
                1.0
            };
            if want != l.scale {
                changes.push((i, want));
            }
        }
        // One message per target scale (a shift turns every lane at once).
        for want in [0.0, SHIFT_SCALE, 1.0] {
            let ids: Vec<&'static str> = changes.iter().filter(|c| c.1 == want).map(|c| self.env.toy.lanes[c.0].id).collect();
            if !ids.is_empty() {
                self.env_set_drift(map, |h| ids.contains(&h.id), want, out);
            }
        }
        for &(i, want) in &changes {
            self.env.toy.lanes[i].scale = want;
        }
        self.env.toy.changes = changes;
        if now >= self.env.toy.next_check {
            self.env.toy.next_check = now + 1.0;
            self.toy_switches(now);
            if shift && now >= WIND_UP_FROM {
                self.toy_wind_up(now);
            }
            self.toy_midnight(map, now, out);
        }
    }

    /// A switch was thrown at `(x, y)`: the belts near it stop, and a
    /// thrown switch stands there while it resets.
    pub(super) fn toy_switch(&mut self, map: MapIdx, x: f32, y: f32, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_ref() else { return };
        let now = horde.director_elapsed();
        let mut stopped = 0;
        for l in self.env.toy.lanes.iter_mut() {
            if reach(&l.shape, x, y) <= SWITCH_R {
                l.stopped_until = l.stopped_until.max(now + STOP_SECS);
                stopped += 1;
            }
        }
        // The clunk and the steam: a small gold mark on the switch.
        let area = Telegraph { id: 0, shape: 0, look: look::GOLD, x, y, dir: 0, a: 1.6, b: 0.0, span_ms: 0, follow: 0 };
        let msg = ServerMsg::BossZones { caster: 0, ability: SWITCH_ABILITY.into(), zones: vec![BossZone { area, delay_ms: 0, flags: zone_flags::SAFE }] };
        self.broadcast(map, &msg, out);
        self.env_shake(map, 30, 400, out);
        let text = if stopped > 0 { "Clunk! The belts nearby grind to a halt." } else { "Clunk! No belt runs near this switch." };
        for p in self.players.iter().filter(|p| p.map == map) {
            out.push((p.profile.user_id, HubEvent::Control(ServerMsg::Notification { message: text.into() })));
        }
        match self.env_plant(SWITCH_OFF, x, y, ObjectEffect::None) {
            Some((slot, generation)) => self.env.toy.thrown.push(Thrown { x, y, until: now + SWITCH_COOLDOWN, slot, generation }),
            // No room for the thrown one: the switch is ready again at once.
            None => {
                let _ = self.env_plant(SWITCH_MONSTER, x, y, ObjectEffect::Hook(SWITCH_HOOK));
            }
        }
        info!("{}: a switch is thrown at ({x:.0}, {y:.0}): {stopped} belts stop", self.name);
    }

    /// A crate from the cranes landed at `(x, y)`: now and then a Myst Case
    /// climbs out of it.
    pub(super) fn toy_crate(&mut self, x: f32, y: f32) {
        if self.dice.die(100) > CRATE_MYST_PCT {
            return;
        }
        let Some(horde) = self.horde.as_mut() else { return };
        if horde.spawning_closed() {
            return;
        }
        if let Some(kind) = horde.catalog().kind_of(MYST_CASE) {
            let _ = horde.spawn(kind, x, y);
        }
    }

    /// Reset switches stand live again (the thrown one goes).
    fn toy_switches(&mut self, now: f32) {
        let mut k = 0;
        while k < self.env.toy.thrown.len() {
            let t = self.env.toy.thrown[k];
            if now < t.until {
                k += 1;
                continue;
            }
            self.env.toy.thrown.swap_remove(k);
            if let Some(horde) = self.horde.as_mut() {
                let store = horde.store();
                if store.is_alive(t.slot) && store.generation[t.slot as usize] == t.generation {
                    horde.despawn(t.slot);
                }
            }
            let _ = self.env_plant(SWITCH_MONSTER, t.x, t.y, ObjectEffect::Hook(SWITCH_HOOK));
        }
    }

    /// A shift late in the run winds the plain toys up: faster until it
    /// ends (renewed each second for the ones that came since).
    fn toy_wind_up(&mut self, now: f32) {
        let Some(start) = self.env_event_start(SHIFT) else { return };
        let Some(m) = crate::world::missions::find(MISSION) else { return };
        let Some(ev) = m.env.event(SHIFT) else { return };
        let left = (start + ev.duration_secs - now).max(0.0);
        let hz = self.config.tick_hz.max(1) as f32;
        let ticks = ticks_of(left, hz).min(u16::MAX as u32) as u16;
        if ticks == 0 {
            return;
        }
        let Some(horde) = self.horde.as_mut() else { return };
        let slots: Vec<u16> = horde.store().alive().to_vec();
        for s in slots {
            let def = horde.catalog().get(horde.store().kind[s as usize]);
            if horde.is_plant(s) || def.boss || def.archetype == Archetype::Elite {
                continue;
            }
            horde.store_mut().rally(s, Rally { to_hit: 0, speed: WIND_UP_SPEED, taken: 0, ticks });
        }
    }

    /// The Foreman under a quarter of its HP: the whistle blows now.
    fn toy_midnight(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_ref() else { return };
        let Some(foreman) = horde.catalog().kind_of(FOREMAN) else { return };
        let store = horde.store();
        let Some(s) = store.alive().iter().copied().find(|&s| store.kind[s as usize] == foreman && store.hp[s as usize] > 0) else { return };
        let g = store.generation[s as usize];
        let max = horde.block(s).hp.max(1) as i64;
        if self.env.toy.midnight == Some((s, g)) || store.hp[s as usize] as i64 * 100 > MIDNIGHT_BELOW * max {
            return;
        }
        self.env.toy.midnight = Some((s, g));
        if self.env_force_event(map, SHIFT, now, out) {
            info!("{}: the Foreman blows the whistle at midnight", self.name);
        }
    }
}

#[cfg(test)]
#[path = "toy_tests.rs"]
mod tests;
