//! A run's environment (`world::environment`, COMBAT.md "Mission
//! platform"): the mission's hazards and the map's baked ones, its timed
//! map events, its light and its interactive objects, stepped once per
//! tick inside the combat step.
//!
//! * Hazards are lasting ground for clients (`BOSS_ZONES` with caster 0
//!   and ability `hazard:<id>`, `ZONE_END` when they stop). An always-on
//!   one shows from the start; an event's hazard shows with the event's
//!   warning (it fills over the warning, as a boss telegraph does) and
//!   ends with it. Pull acts every tick, damage and the condition every
//!   `every_secs` (all hazards share a phase).
//! * Map events warn, begin and end (`ENVIRONMENT` op 1) on their
//!   schedule; the light radius is the stage's base times every running
//!   event's and every lit object's scale (`ENVIRONMENT` op 0 when it
//!   changes).
//! * Objects are plants planted at fixed spots on the first step; one
//!   gone from its slot was felled and fires its effect once.
//!
//! The state is a few small vectors made once; a quiet tick allocates
//! nothing.

use super::aim::BODY;
use super::cast::ticks_of;
use super::fight::damage_type_bits;
use super::mission::Outcome;
use super::{HubEvent, HubInstance, MapIdx, Outgoing};
use crate::world::combat::rules::saving_throw;
use crate::world::combat::{DamageType, Dice};
use crate::world::environment::{Hazard, HazardShape, MapEvent, ObjectEffect};
use crate::world::missions::{self, MissionInfo};
use crate::world::protocol::{env_phase, hazard_ability, BossZone, HitEvent, ServerMsg, Telegraph};
use tracing::{info, warn};

/// Light changes ease in over this long on clients.
const LIGHT_FADE_MS: u32 = 1500;
/// Hazard zones per `BOSS_ZONES` message.
const ZONES_PER_MSG: usize = 64;
/// Look for felled objects this often (ticks).
const OBJECT_SCAN: u32 = 5;

#[derive(Debug)]
struct LiveHazard {
    def: Hazard,
    /// Zone id on clients while shown (0 = not shown).
    zone: u32,
    /// Biting now.
    on: bool,
    /// Ended for good (an object turned it off).
    gone: bool,
}

#[derive(Clone, Copy, Debug)]
struct LiveEvent {
    def: MapEvent,
    /// Start of the occurrence warned about / running.
    warned: Option<f32>,
    running: Option<f32>,
}

#[derive(Clone, Copy, Debug)]
struct LiveObject {
    slot: u16,
    generation: u8,
    effect: ObjectEffect,
    /// Hatches at this run second into these monsters (kind, count).
    hatch: Option<(f32, u8, u8)>,
}

/// The environment of one stage.
#[derive(Debug, Default)]
pub(super) struct Env {
    mission: Option<&'static MissionInfo>,
    hazards: Vec<LiveHazard>,
    events: Vec<LiveEvent>,
    objects: Vec<LiveObject>,
    /// Light boosts of felled objects: (scale, until run second).
    boosts: Vec<(f32, f32)>,
    /// The stage's light radius before any event or boost (taken on the
    /// first step: the mission's and the modifiers' scale are in it).
    base_light: f32,
    /// The scale last applied.
    light_scale: f32,
    /// Next damage/condition pulse (run seconds) per hazard.
    pulse_at: Vec<f32>,
    /// Calm discs (x, y, r, until run second): hazards spare whoever stands
    /// inside one (sewage frozen over by a cold spell, `hub/culvert.rs`).
    calm: Vec<(f32, f32, f32, f32)>,
    started: bool,
    /// Payon Caves' lit braziers (`payon.rs`).
    pub(super) payon: super::payon::Braziers,
}

impl Env {
    /// The environment of a stage of `mission` on a map with the baked
    /// `map_hazards`.
    pub fn new(mission: Option<&'static MissionInfo>, map_hazards: &[Hazard]) -> Self {
        let Some(m) = mission else { return Self::default() };
        let hazards: Vec<LiveHazard> = m
            .env
            .hazards
            .iter()
            .chain(map_hazards)
            .filter(|h| match h.problem() {
                Some(p) => {
                    warn!("mission {}: hazard skipped: {p}", m.id);
                    false
                }
                None => true,
            })
            .map(|h| LiveHazard { def: *h, zone: 0, on: false, gone: false })
            .collect();
        Self {
            mission,
            pulse_at: vec![0.0; hazards.len()],
            calm: Vec::new(),
            hazards,
            events: m.env.events.iter().map(|&def| LiveEvent { def, warned: None, running: None }).collect(),
            objects: Vec::new(),
            boosts: Vec::new(),
            base_light: 0.0,
            light_scale: 1.0,
            started: false,
            payon: Default::default(),
        }
    }

    /// The light grows `scale` times until run second `until` (a lit
    /// brazier); boosts multiply with each other and with events.
    pub(super) fn boost_light(&mut self, scale: f32, until: f32) {
        self.boosts.push((scale, until));
    }

    /// Takes back one boost pushed with [`Env::boost_light`] (a brazier
    /// snuffed before its time).
    pub(super) fn end_boost(&mut self, scale: f32, until: f32) {
        if let Some(i) = self.boosts.iter().position(|b| b.0 == scale && (b.1 - until).abs() < 1e-3) {
            self.boosts.swap_remove(i);
        }
    }

    /// Whether `(x, y)` lies in a hazard biting now whose look is one of
    /// `looks` (`protocol::look` ids).
    pub fn ground_is(&self, x: f32, y: f32, looks: &[u8]) -> bool {
        self.hazards.iter().any(|h| h.on && looks.contains(&h.def.look_id()) && h.def.shape.contains(x, y, 0.0))
    }

    fn running(&self, event: &str) -> bool {
        self.events.iter().any(|e| e.def.id == event && e.running.is_some())
    }

    /// The light multiplier at `now`: running events times live boosts.
    fn scale_at(&self, now: f32) -> f32 {
        let events: f32 = self.events.iter().filter(|e| e.running.is_some()).map(|e| e.def.light_scale).product();
        let boosts: f32 = self.boosts.iter().filter(|b| b.1 > now).map(|b| b.0).product();
        events * boosts
    }
}

fn telegraph(shape: &HazardShape, id: u32, look: u8, span_ms: u32) -> Telegraph {
    match *shape {
        HazardShape::Circle { x, y, r } => Telegraph { id, shape: 0, look, x, y, dir: 0, a: r, b: 0.0, span_ms, follow: 0 },
        HazardShape::Rect { x, y, w, h } => Telegraph { id, shape: 1, look, x, y, dir: 0, a: w, b: h, span_ms, follow: 0 },
    }
}

/// A hazard's family: its id without a trailing `_<n>` (the boxes of one
/// baked band share it).
fn family(id: &str) -> &str {
    match id.rsplit_once('_') {
        Some((head, n)) if !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()) => head,
        _ => id,
    }
}

fn ms(secs: f32) -> u32 {
    (secs.max(0.0) * 1000.0).round() as u32
}

impl HubInstance {
    /// Once per tick in the combat step (after the fog).
    pub(super) fn env_step(&mut self, out: &mut Outgoing, hits: &mut Vec<HitEvent>) {
        if self.env.mission.is_none() || self.outcome != Outcome::Running {
            return;
        }
        let Some(horde) = self.horde.as_ref() else { return };
        let (map, now) = (horde.map(), horde.director_elapsed());
        if !self.env.started {
            self.env_start(map, now, out);
        }
        self.env_events(map, now, out);
        if self.tick.is_multiple_of(OBJECT_SCAN) {
            self.env_objects(map, now, out);
        }
        if self.env.mission.is_some_and(|m| m.id == super::payon::MISSION) {
            self.payon_step(map, now, out);
        }
        self.env_light(map, now, out);
        self.env_hazards(map, now, hits);
    }

    /// The first step: the base light, the objects, the always-on hazards.
    fn env_start(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        self.env.started = true;
        let Some(m) = self.env.mission else { return };
        let Some(horde) = self.horde.as_mut() else { return };
        self.env.base_light = horde.light_radius();
        for o in m.env.objects {
            let Some(kind) = horde.catalog().kind_of(o.monster) else {
                warn!("mission {}: object {} is not in the roster, skipped", m.id, o.monster);
                continue;
            };
            let hatch = o.hatch.and_then(|h| match horde.catalog().kind_of(h.monster) {
                Some(k) => Some((now + h.secs, k, h.count)),
                None => {
                    warn!("mission {}: {} hatches {}, which is not in the roster", m.id, o.monster, h.monster);
                    None
                }
            });
            for &(x, y) in o.spots {
                match horde.place_object(kind, x, y) {
                    Some(slot) => {
                        let generation = horde.store().generation[slot as usize];
                        self.env.objects.push(LiveObject { slot, generation, effect: o.on_felled, hatch });
                    }
                    None => warn!("mission {}: object {} at ({x}, {y}) not placed (not a plant, a wall, or the pool is full)", m.id, o.monster),
                }
            }
        }
        let limit = m.time_limit_secs;
        let always: Vec<usize> = (0..self.env.hazards.len()).filter(|&i| self.env.hazards[i].def.during.is_none()).collect();
        self.show_hazards(map, &always, 0.0, limit - now, out);
        for i in always {
            self.env.hazards[i].on = true;
        }
    }

    /// Shows hazards `idx` (lasting ground resolving in `delay` s, lasting
    /// `span` s after) to the map: a few messages of up to [`ZONES_PER_MSG`]
    /// zones each (a location may bake hundreds of boxes; one message per
    /// box would overrun a player's outbox).
    fn show_hazards(&mut self, map: MapIdx, idx: &[usize], delay: f32, span: f32, out: &mut Outgoing) {
        // One message per family of boxes (`bank_0`, `bank_1`, ... of a
        // baked band), split every ZONES_PER_MSG.
        let mut groups: Vec<Vec<usize>> = Vec::new();
        for &i in idx {
            let fam = family(self.env.hazards[i].def.id);
            match groups.last_mut() {
                Some(g) if family(self.env.hazards[g[0]].def.id) == fam && g.len() < ZONES_PER_MSG => g.push(i),
                _ => groups.push(vec![i]),
            }
        }
        for chunk in &groups {
            let mut zones = Vec::with_capacity(chunk.len());
            for &i in chunk {
                let id = self.next_zone;
                self.next_zone = self.next_zone.wrapping_add(1).max(1);
                let h = &mut self.env.hazards[i];
                h.zone = id;
                zones.push(BossZone { area: telegraph(&h.def.shape, id, h.def.look_id(), ms(span)), delay_ms: ms(delay), flags: 0 });
            }
            let msg = ServerMsg::BossZones { caster: 0, ability: hazard_ability(family(self.env.hazards[chunk[0]].def.id)), zones };
            self.broadcast(map, &msg, out);
        }
    }

    fn hide_hazard(&mut self, map: MapIdx, i: usize, out: &mut Outgoing) {
        let h = &mut self.env.hazards[i];
        h.on = false;
        let id = std::mem::take(&mut h.zone);
        if id != 0 {
            self.broadcast(map, &ServerMsg::ZoneEnd { id }, out);
        }
    }

    /// Warnings, beginnings and ends of the map events, and their hazards.
    fn env_events(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        for k in 0..self.env.events.len() {
            let e = self.env.events[k];
            let d = e.def;
            let event_msg = |phase: u8, start: f32| ServerMsg::EnvEvent {
                phase,
                id: d.id.into(),
                name: d.name.into(),
                text: if phase == env_phase::WARNING { d.warning.into() } else { String::new() },
                start_ms: ms(start),
                duration_ms: ms(d.duration_secs),
            };
            if let Some(start) = e.running {
                if now >= start + d.duration_secs {
                    self.env.events[k].running = None;
                    self.broadcast(map, &event_msg(env_phase::ENDS, start), out);
                    // Their ground was sent lasting just as long: it goes on
                    // clients by itself (no ZONE_END per box).
                    for h in self.env.hazards.iter_mut().filter(|h| h.def.during == Some(d.id)) {
                        h.on = false;
                        h.zone = 0;
                    }
                }
                continue;
            }
            if let Some(start) = d.running_at(now) {
                self.env.events[k].running = Some(start);
                let left = start + d.duration_secs - now;
                if e.warned != Some(start) {
                    // No warning (none due, or a late start): show it now.
                    self.env.events[k].warned = Some(start);
                    self.event_hazards(map, d.id, 0.0, left, out);
                }
                info!("{}: {} begins at {now:.0} s", self.name, d.name);
                self.broadcast(map, &event_msg(env_phase::BEGINS, start), out);
                if d.shake > 0 {
                    self.env_shake(map, d.shake, 1000, out);
                }
                for h in self.env.hazards.iter_mut().filter(|h| h.def.during == Some(d.id) && !h.gone) {
                    h.on = true;
                }
                continue;
            }
            if let Some(next) = d.next_start(now)
                && d.warn_secs > 0.0
                && next - now <= d.warn_secs
                && e.warned != Some(next)
            {
                self.env.events[k].warned = Some(next);
                self.broadcast(map, &event_msg(env_phase::WARNING, next), out);
                self.event_hazards(map, d.id, next - now, d.duration_secs, out);
            }
        }
    }

    /// Shows the hazards of event `id` (they bite once it begins).
    fn event_hazards(&mut self, map: MapIdx, id: &str, delay: f32, span: f32, out: &mut Outgoing) {
        let idx: Vec<usize> = (0..self.env.hazards.len()).filter(|&i| self.env.hazards[i].def.during == Some(id) && !self.env.hazards[i].gone).collect();
        self.show_hazards(map, &idx, delay, span, out);
    }

    /// Shakes the camera of everyone on the map (`intensity` 1..=255; a
    /// boss slam, a cave-in). Clients with reduced motion skip it.
    pub(super) fn env_shake(&self, map: MapIdx, intensity: u8, ms: u32, out: &mut Outgoing) {
        self.broadcast(map, &ServerMsg::EnvShake { intensity, duration_ms: ms }, out);
    }

    /// Felled objects fire their effect once; standing ones whose time is
    /// up hatch.
    fn env_objects(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_mut() else { return };
        let mut k = 0;
        while k < self.env.objects.len() {
            let o = self.env.objects[k];
            let s = o.slot as usize;
            let store = horde.store();
            let standing = store.is_alive(o.slot) && store.generation[s] == o.generation && store.hp[s] > 0;
            if standing && let Some((at, kind, count)) = o.hatch.filter(|h| now >= h.0) {
                // Breaks open: the object goes, its brood lands around it.
                let (x, y) = (store.x[s], store.y[s]);
                horde.despawn(o.slot);
                for i in 0..count {
                    let a = std::f32::consts::TAU * i as f32 / count.max(1) as f32;
                    let _ = horde.spawn(kind, x + a.cos() * 0.8, y + a.sin() * 0.8).or_else(|| horde.spawn(kind, x, y));
                }
                info!("{}: an object hatched {count} at {at:.0} s", self.name);
                self.env.objects.swap_remove(k);
                continue;
            }
            if standing {
                k += 1;
                continue;
            }
            let store = horde.store();
            let (x, y) = (store.x[s], store.y[s]);
            self.env.objects.swap_remove(k);
            self.object_felled(map, o.effect, x, y, now, out);
            // The store may have changed under a hook: look again next scan.
            return;
        }
    }

    fn object_felled(&mut self, map: MapIdx, effect: ObjectEffect, x: f32, y: f32, now: f32, out: &mut Outgoing) {
        match effect {
            ObjectEffect::None => {}
            ObjectEffect::Light { scale, secs } => self.env.boosts.push((scale, now + secs)),
            ObjectEffect::EndHazard(id) => {
                for i in 0..self.env.hazards.len() {
                    if self.env.hazards[i].def.id == id {
                        self.env.hazards[i].gone = true;
                        self.hide_hazard(map, i, out);
                    }
                }
            }
            ObjectEffect::Hook(id) => self.object_hook(map, id, x, y, out),
        }
    }

    /// Calms the hazards inside a disc for `secs` (frozen over, dried up):
    /// whoever stands in it is spared until then.
    pub(super) fn env_calm(&mut self, x: f32, y: f32, r: f32, secs: f32) {
        let now = self.horde.as_ref().map_or(0.0, |h| h.director_elapsed());
        self.env.calm.push((x, y, r, now + secs));
    }

    /// An object planted mid-run (a boss laying eggs): it joins the
    /// stage's objects, felled with no effect, hatching at run second
    /// `hatch.0` into `hatch.2` of kind `hatch.1` unless broken first.
    pub(super) fn env_adopt(&mut self, slot: u16, generation: u8, hatch: Option<(f32, u8, u8)>) {
        self.env.objects.push(LiveObject { slot, generation, effect: ObjectEffect::None, hatch });
    }

    /// Hatches every standing hatchable object within `r` of `(x, y)` on
    /// the next scan (a brood call); returns how many.
    pub(super) fn env_hatch_near(&mut self, x: f32, y: f32, r: f32) -> u32 {
        let Some(horde) = self.horde.as_ref() else { return 0 };
        let now = horde.director_elapsed();
        let store = horde.store();
        let mut n = 0;
        for o in self.env.objects.iter_mut() {
            let s = o.slot as usize;
            let standing = store.is_alive(o.slot) && store.generation[s] == o.generation && store.hp[s] > 0;
            if let Some(h) = o.hatch.as_mut()
                && standing
                && (store.x[s] - x).powi(2) + (store.y[s] - y).powi(2) <= r * r
            {
                h.0 = h.0.min(now);
                n += 1;
            }
        }
        n
    }

    /// A location's own object effects ([`ObjectEffect::Hook`]): add an arm
    /// per hook id (keep the work in the location's own module).
    fn object_hook(&mut self, map: MapIdx, id: &str, x: f32, y: f32, out: &mut Outgoing) {
        match id {
            super::payon::BRAZIER_HOOK => self.payon_kindle(map, x, y, out),
            super::payon::BELL_HOOK => self.payon_ring(map, x, y, out),
            _ => warn!("{}: object hook {id} at ({x:.1}, {y:.1}) has no handler", self.name),
        }
    }

    /// Plants one more object of `monster` (a roster plant) at `(x, y)`
    /// whose felling fires `effect` (a brazier lit again after it burnt
    /// out): its slot and generation, `None` when it could not be placed.
    pub(super) fn env_plant(&mut self, monster: &str, x: f32, y: f32, effect: ObjectEffect) -> Option<(u16, u8)> {
        let horde = self.horde.as_mut()?;
        let kind = horde.catalog().kind_of(monster)?;
        let slot = horde.place_object(kind, x, y)?;
        let generation = horde.store().generation[slot as usize];
        self.env.objects.push(LiveObject { slot, generation, effect, hatch: None });
        Some((slot, generation))
    }

    /// Applies the light multiplier when it changed.
    fn env_light(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        self.env.boosts.retain(|b| b.1 > now);
        let scale = self.env.scale_at(now);
        if (scale - self.env.light_scale).abs() < 1e-3 {
            return;
        }
        self.env.light_scale = scale;
        let Some(horde) = self.horde.as_mut() else { return };
        horde.set_light_radius(self.env.base_light * scale);
        let radius = horde.light_radius();
        self.broadcast(map, &ServerMsg::EnvLight { radius, fade_ms: LIGHT_FADE_MS }, out);
    }

    /// Hazards bite: pull every tick, damage and conditions on the pulse.
    fn env_hazards(&mut self, map: MapIdx, now: f32, hits: &mut Vec<HitEvent>) {
        if !self.env.hazards.iter().any(|h| h.on) {
            return;
        }
        let hz = self.config.tick_hz.max(1) as f32;
        let level = self.env.mission.map_or(1, |m| m.levels.min);
        let walk = &self.world.map(map).walk;
        self.env.calm.retain(|c| c.3 > now);
        let calm = &self.env.calm;
        for (i, h) in self.env.hazards.iter().enumerate() {
            if !h.on {
                continue;
            }
            let def = &h.def;
            let pulse = now >= self.env.pulse_at[i];
            if pulse {
                self.env.pulse_at[i] = now + def.every_secs;
            }
            let (cx, cy) = def.shape.centre();
            for p in self.players.iter_mut().filter(|p| p.map == map) {
                let Some(f) = p.fighter.as_mut() else { continue };
                if !f.is_up() || !def.shape.contains(p.x, p.y, BODY) {
                    continue;
                }
                if calm.iter().any(|&(x, y, r, _)| (p.x - x).powi(2) + (p.y - y).powi(2) <= r * r) {
                    continue;
                }
                if def.pull != 0.0 {
                    let (dx, dy) = (cx - p.x, cy - p.y);
                    let d = (dx * dx + dy * dy).sqrt();
                    if d > 0.05 {
                        // A small step a tick: it stops at walls (no corner cutting).
                        let step = (def.pull / hz).clamp(-d, d);
                        let (nx, ny) = (p.x + dx / d * step, p.y + dy / d * step);
                        if walk.is_walkable(nx, ny) {
                            (p.x, p.y) = (nx, ny);
                        }
                    }
                }
                if !pulse {
                    continue;
                }
                let cond = def.condition();
                let saved = def.dc > 0 && !f.status.saves_fail() && saving_throw(&mut self.dice, f.save_against(cond), def.dc, 0);
                let [n, sides] = def.damage;
                if n > 0 && sides > 0 {
                    let mut amount = Dice::new(n as u32, sides as u32, (level / 4) as i32).roll(&mut self.dice, false);
                    if saved {
                        amount /= 2;
                    }
                    let kind = def.damage_type();
                    f.quiet = 0;
                    let taken = f.take_from(amount, kind == DamageType::Physical, false);
                    hits.push(HitEvent { target: p.profile.character_id, amount: taken as u32, flags: damage_type_bits(kind) });
                }
                if let Some(c) = cond
                    && !saved
                {
                    // At least until the next pulse: it holds while inside.
                    let ticks = ticks_of(def.condition_secs.max(def.every_secs), hz) + 1;
                    f.afflict(c, ticks.min(u16::MAX as u32) as u16);
                }
                f.fall_if_spent(hz);
            }
        }
    }

    /// A player joining the stage: running events and shown hazards
    /// (the light is in `STAGE_JOINED`).
    pub(super) fn announce_env(&self, idx: usize, out: &mut Outgoing) {
        let (Some(m), Some(horde)) = (self.env.mission, self.horde.as_ref()) else { return };
        let p = &self.players[idx];
        if p.map != horde.map() {
            return;
        }
        let (uid, now) = (p.profile.user_id, horde.director_elapsed());
        for e in &self.env.events {
            if let Some(start) = e.running {
                let msg = ServerMsg::EnvEvent {
                    phase: env_phase::BEGINS,
                    id: e.def.id.into(),
                    name: e.def.name.into(),
                    text: String::new(),
                    start_ms: ms(start),
                    duration_ms: ms(e.def.duration_secs),
                };
                out.push((uid, HubEvent::Control(msg)));
            }
        }
        // Batched like `show_hazards`.
        let mut zones: Vec<BossZone> = Vec::new();
        let mut first = "";
        for h in self.env.hazards.iter().filter(|h| h.zone != 0) {
            let end = match h.def.during {
                None => m.time_limit_secs,
                Some(ev) => self.env.events.iter().find(|e| e.def.id == ev).and_then(|e| e.running.or(e.warned).map(|s| s + e.def.duration_secs)).unwrap_or(now),
            };
            let delay = match h.def.during {
                Some(ev) if !self.env.running(ev) => self.env.events.iter().find(|e| e.def.id == ev).and_then(|e| e.warned).map_or(0.0, |s| s - now),
                _ => 0.0,
            };
            let span = (end - now - delay.max(0.0)).max(0.0);
            if !zones.is_empty() && family(first) != family(h.def.id) {
                let msg = ServerMsg::BossZones { caster: 0, ability: hazard_ability(family(first)), zones: std::mem::take(&mut zones) };
                out.push((uid, HubEvent::Control(msg)));
            }
            if zones.is_empty() {
                first = h.def.id;
            }
            zones.push(BossZone { area: telegraph(&h.def.shape, h.zone, h.def.look_id(), ms(span)), delay_ms: ms(delay), flags: 0 });
            if zones.len() == ZONES_PER_MSG {
                let msg = ServerMsg::BossZones { caster: 0, ability: hazard_ability(family(first)), zones: std::mem::take(&mut zones) };
                out.push((uid, HubEvent::Control(msg)));
            }
        }
        if !zones.is_empty() {
            out.push((uid, HubEvent::Control(ServerMsg::BossZones { caster: 0, ability: hazard_ability(family(first)), zones })));
        }
    }

    /// When the running occurrence of map event `id` began (run seconds),
    /// if one runs (scheduled or forced).
    pub(super) fn env_event_start(&self, id: &str) -> Option<f32> {
        self.env.events.iter().find(|e| e.def.id == id).and_then(|e| e.running)
    }

    /// A location's script starts map event `id` now, off its schedule (a
    /// boss calling a sandstorm): announced, its hazards on, its light and
    /// shake as a scheduled one, ending after its duration. Returns false
    /// when it already runs or the mission has no such event.
    pub(super) fn env_force_event(&mut self, map: MapIdx, id: &str, now: f32, out: &mut Outgoing) -> bool {
        let Some(k) = self.env.events.iter().position(|e| e.def.id == id) else { return false };
        if self.env.events[k].running.is_some() {
            return false;
        }
        let d = self.env.events[k].def;
        self.env.events[k].running = Some(now);
        self.env.events[k].warned = Some(now);
        self.event_hazards(map, d.id, 0.0, d.duration_secs, out);
        for h in self.env.hazards.iter_mut().filter(|h| h.def.during == Some(d.id) && !h.gone) {
            h.on = true;
        }
        info!("{}: {} forced at {now:.0} s", self.name, d.name);
        let msg = ServerMsg::EnvEvent {
            phase: env_phase::BEGINS,
            id: d.id.into(),
            name: d.name.into(),
            text: String::new(),
            start_ms: ms(now),
            duration_ms: ms(d.duration_secs),
        };
        self.broadcast(map, &msg, out);
        if d.shake > 0 {
            self.env_shake(map, d.shake, 1000, out);
        }
        true
    }

    /// Test hook: the run's light multiplier now.
    #[cfg(test)]
    pub(super) fn env_light_scale(&self) -> f32 {
        self.env.light_scale
    }
}

/// The environment a stage of `mission` (by id) starts with.
pub(super) fn for_stage(mission: &str, map_hazards: &[Hazard]) -> Env {
    Env::new(missions::find(mission), map_hazards)
}

#[cfg(test)]
#[path = "env_tests.rs"]
mod tests;
