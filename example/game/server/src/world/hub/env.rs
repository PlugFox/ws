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
//!   gone from its slot was felled and fires its effect once. A hatching
//!   one breaks open into its brood, or (a spawner, `Hatch::every_secs`)
//!   lets one out on every beat until felled, after a warning ring
//!   (`BOSS_ZONES` caster 0, ability `hatch:<monster>`).
//! * Drifting ground (`Hazard::drift`, conveyor belts): every tick it
//!   carries the players and the monsters standing on it (centre inside)
//!   along its direction, a step per axis that walls stop. Clients get it
//!   as a box along the drift with `zone_flags::DRIFT` and the speed (they
//!   predict the local player's ride); a location may scale a belt's drift
//!   at run time (`env_set_drift`: reversed, stopped), which re-sends it.
//! * Event strikes (`environment::Strikes`): while their event runs,
//!   volleys of telegraphed circles fall on the players (`BOSS_ZONES`
//!   caster 0, ability = the strikes' id) and land as a boss beat does
//!   (`boss::land`: damage, save, rider, ward, push), maybe leaving a
//!   short-lived hazard patch and calling the location's `strike_hook`.
//!
//! The state is a few small vectors made once; a quiet tick allocates
//! nothing.

use super::aim::BODY;
use super::area::Area;
use super::boss::{land, Blow, Hurt, Zone};
use super::cast::ticks_of;
use super::fight::damage_type_bits;
use super::mission::Outcome;
use super::{HubEvent, HubInstance, MapIdx, Outgoing};
use crate::world::combat::rules::saving_throw;
use crate::world::combat::{Condition, DamageType, Dice};
use crate::world::environment::{Hatch, Hazard, HazardShape, MapEvent, ObjectEffect, Strikes, HATCH_WARN_SECS};
use crate::world::missions::{self, MissionInfo};
use crate::world::protocol::{env_phase, hazard_ability, look, BossZone, HitEvent, ServerMsg, Telegraph};
use tracing::{info, warn};

/// Light changes ease in over this long on clients.
const LIGHT_FADE_MS: u32 = 1500;
/// Hazard zones per `BOSS_ZONES` message.
const ZONES_PER_MSG: usize = 64;
/// Look for felled objects this often (ticks).
const OBJECT_SCAN: u32 = 5;
/// Tries per strike circle to find open, unmasked ground before it is
/// skipped.
const STRIKE_TRIES: u32 = 8;
/// A spawner's brood lands this far round it (cells).
const BROOD_RING: f32 = 1.2;
/// The radius of a spawner's warning ring (cells).
const HATCH_RING: f32 = 1.6;

#[derive(Debug)]
struct LiveHazard {
    def: Hazard,
    /// Zone id on clients while shown (0 = not shown).
    zone: u32,
    /// Biting now.
    on: bool,
    /// Ended for good (an object turned it off).
    gone: bool,
    /// A strike's patch: it goes at this run second.
    until: Option<f32>,
    /// Its drift times this (a belt reversed: < 0, stopped: 0).
    scale: f32,
}

impl LiveHazard {
    fn new(def: Hazard) -> Self {
        LiveHazard { def, zone: 0, on: false, gone: false, until: None, scale: 1.0 }
    }

    /// Cells per second it carries bodies along now.
    fn drift(&self) -> (f32, f32) {
        (self.def.drift[0] * self.scale, self.def.drift[1] * self.scale)
    }

    /// Its zone on clients: drifting ground is a box along its drift with
    /// the speed in the flags; other ground as its shape.
    fn zone(&self, id: u32, delay_ms: u32, span_ms: u32) -> BossZone {
        let look = self.def.look_id();
        if !self.def.drifts() {
            return BossZone { area: telegraph(&self.def.shape, id, look, span_ms), delay_ms, flags: 0 };
        }
        let (dx, dy) = self.drift();
        // The belt's own direction when stopped (the scroll stands still).
        let (ax, ay) = if dx == 0.0 && dy == 0.0 { (self.def.drift[0], self.def.drift[1]) } else { (dx, dy) };
        let turn = ay.atan2(ax).rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU;
        let dir = ((turn * 256.0).round() as u32 % 256) as u8;
        let len = (ax * ax + ay * ay).sqrt().max(1e-6);
        let (ux, uy) = (ax / len, ay / len);
        let area = match self.def.shape {
            HazardShape::Circle { x, y, r } => Telegraph { id, shape: 0, look, x, y, dir, a: r, b: 0.0, span_ms, follow: 0 },
            // Along and across the drift (exact for a belt along an axis).
            HazardShape::Rect { x, y, w, h } => {
                Telegraph { id, shape: 1, look, x, y, dir, a: (w * ux).abs() + (h * uy).abs(), b: (w * uy).abs() + (h * ux).abs(), span_ms, follow: 0 }
            }
        };
        let flags = crate::world::protocol::zone_flags::drift_flags((dx * dx + dy * dy).sqrt());
        BossZone { area, delay_ms, flags }
    }
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
    /// A spawner: hatches again this long after each brood (0 = once).
    every: f32,
    /// The hatch second its warning ring was shown for.
    warned: f32,
}

#[derive(Clone, Copy, Debug)]
struct LiveStrikes {
    def: Strikes,
    /// Start of the occurrence of its event it is falling in.
    occurrence: Option<f32>,
    /// Next volley (run seconds).
    next: f32,
}

/// A strike circle in the air.
#[derive(Clone, Copy, Debug)]
struct Falling {
    /// Index into `Env::strikes`.
    strikes: usize,
    x: f32,
    y: f32,
    /// Lands at this run second.
    at: f32,
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
    strikes: Vec<LiveStrikes>,
    falling: Vec<Falling>,
    /// Scratch: monsters near a drifting hazard (reused every tick).
    carried: Vec<u16>,
    started: bool,
    /// Payon Caves' lit braziers (`payon.rs`).
    pub(super) payon: super::payon::Braziers,
    /// Orc Village's banners and horn columns (`orc.rs`).
    pub(super) orc: super::orc::Camp,
    /// Thor Volcano's state (`thor.rs`).
    pub(super) thor: super::thor::Volcano,
    /// Toy Factory's belts and switches (`toy.rs`).
    pub(super) toy: super::toy::Factory,
    /// Glast Heim's graves, fonts and risings (`glast.rs`).
    pub(super) glast: super::glast::Churchyard,
    /// Thanatos Tower's state (`thanatos.rs`).
    pub(super) tower: super::thanatos::Tower,
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
            .map(|h| LiveHazard::new(*h))
            .collect();
        let strikes = m
            .env
            .strikes
            .iter()
            .filter(|s| match s.problem(m.env.events) {
                Some(p) => {
                    warn!("mission {}: strikes skipped: {p}", m.id);
                    false
                }
                None => true,
            })
            .map(|&def| LiveStrikes { def, occurrence: None, next: 0.0 })
            .collect();
        Self {
            mission,
            pulse_at: vec![0.0; hazards.len()],
            calm: Vec::new(),
            strikes,
            falling: Vec::new(),
            carried: Vec::new(),
            hazards,
            events: m.env.events.iter().map(|&def| LiveEvent { def, warned: None, running: None }).collect(),
            objects: Vec::new(),
            boosts: Vec::new(),
            base_light: 0.0,
            light_scale: 1.0,
            started: false,
            payon: Default::default(),
            orc: Default::default(),
            thor: Default::default(),
            toy: Default::default(),
            glast: Default::default(),
            tower: Default::default(),
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

    /// What a spell of `kind` landing at `(x, y)` does to the hazard
    /// ground there ([`GROUND_MEETS`]), if anything.
    pub(super) fn ground_meet(&self, x: f32, y: f32, kind: DamageType) -> Option<&'static GroundMeet> {
        GROUND_MEETS.iter().find(|m| m.kind == kind && self.ground_is(x, y, m.looks))
    }

    /// Centres of the calm discs (frozen sewage, obsidian) standing now:
    /// they shelter from a [`super::boss::scripts::Pattern::LitSanctuary`]
    /// nova as a lit light does (Ifrit's Eruption).
    pub(super) fn calm_spots(&self, now: f32) -> impl Iterator<Item = (f32, f32)> + '_ {
        self.calm.iter().filter(move |c| c.3 > now).map(|c| (c.0, c.1))
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

/// A damage type meeting hazard ground of some looks (S9, generalised from
/// the Culvert's sewage): the spell's area calms the hazards for
/// `calm_secs` (whoever stands there is spared), shown as safe ground of
/// `show` under the effects of `ability`; `douse` puts out the spell's own
/// lasting ground at its first bite (fire on sewage).
#[derive(Debug)]
pub(super) struct GroundMeet {
    pub looks: &'static [u8],
    pub kind: DamageType,
    pub calm_secs: f32,
    /// Smallest and largest radius of the calm disc (cells): the spell's
    /// own size, clamped.
    pub r: (f32, f32),
    pub ability: &'static str,
    pub show: u8,
    pub douse: bool,
}

/// Every pairing, first match wins.
pub(super) const GROUND_MEETS: &[GroundMeet] = &[
    // Prontera Culvert (`culvert.rs`): cold freezes sewage over into safe
    // ice; fire fizzles out in a puff of steam.
    GroundMeet { looks: super::culvert::SEWAGE, kind: DamageType::Cold, calm_secs: 10.0, r: (2.5, 6.0), ability: "culvert_ice", show: look::COLD, douse: false },
    GroundMeet { looks: super::culvert::SEWAGE, kind: DamageType::Fire, calm_secs: 0.0, r: (1.5, 1.5), ability: "culvert_steam", show: look::SWAMP, douse: true },
    // Thor Volcano (`thor.rs`): cold quenches lava into obsidian, calm
    // ground for a while; fire on fire does nothing.
    GroundMeet { looks: super::thor::LAVA, kind: DamageType::Cold, calm_secs: 8.0, r: (2.5, 5.0), ability: "thor_obsidian", show: look::COLD, douse: false },
];

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

/// One tick of drift from `(x, y)` by `(dx, dy)` cells: x first, then y,
/// each only onto walkable ground (the walking step's rule, so clients
/// predict the same).
fn drift_step(walk: &crate::world::map::WalkGrid, x: f32, y: f32, dx: f32, dy: f32) -> (f32, f32) {
    let (mut x, mut y) = (x, y);
    if dx != 0.0 && walk.is_walkable(x + dx, y) {
        x += dx;
    }
    if dy != 0.0 && walk.is_walkable(x, y + dy) {
        y += dy;
    }
    (x, y)
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
        // A location's own step (one fence per location; keep the work in
        // its module: `if self.env.mission.is_some_and(|m| m.id == super::orc::MISSION) { self.orc_step(map, now, out); }`).
        // >>> orc_village: step (agent ORC) >>>
        if self.env.mission.is_some_and(|m| m.id == super::orc::MISSION) {
            self.orc_step(map, now, out);
        }
        // <<< orc_village: step <<<
        // >>> toy_factory: step (agent TOY) >>>
        if self.env.mission.is_some_and(|m| m.id == super::toy::MISSION) {
            self.toy_step(map, now, out);
        }
        // <<< toy_factory: step <<<
        // >>> glast_heim: step (agent GLAST) >>>
        if self.env.mission.is_some_and(|m| m.id == super::glast::MISSION) {
            self.glast_step(map, now, out, hits);
        }
        // <<< glast_heim: step <<<
        // >>> thor_volcano: step (agent THOR) >>>
        if self.env.mission.is_some_and(|m| m.id == super::thor::MISSION) {
            self.thor_step(map, now, out);
        }
        // <<< thor_volcano: step <<<
        // >>> thanatos_tower: step (agent TOWER) >>>
        if self.env.mission.is_some_and(|m| m.id == super::thanatos::MISSION) {
            self.tower_step(map, now, out);
        }
        // <<< thanatos_tower: step <<<
        self.env_light(map, now, out);
        self.env_strikes(map, now, out, hits);
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
            let every = o.hatch.map_or(0.0, |h| h.every_secs.max(0.0));
            for &(x, y) in o.spots {
                match horde.place_object(kind, x, y) {
                    Some(slot) => {
                        let generation = horde.store().generation[slot as usize];
                        self.env.objects.push(LiveObject { slot, generation, effect: o.on_felled, hatch, every, warned: -1.0 });
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
                zones.push(h.zone(id, ms(delay), ms(span)));
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
        let closed = self.horde.as_ref().is_some_and(|h| h.spawning_closed());
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
            // The fog closed the waves: an event heralding monsters stops
            // (one running plays out; one only warned of is called off, its
            // banner and ground taken back).
            if closed && d.calls_waves && e.running.is_none() {
                if let Some(next) = e.warned.filter(|&w| w > now) {
                    self.env.events[k].warned = None;
                    info!("{}: {} called off at {now:.0} s (the fog closed the waves)", self.name, d.name);
                    self.broadcast(map, &event_msg(env_phase::ENDS, next), out);
                    for i in 0..self.env.hazards.len() {
                        if self.env.hazards[i].def.during == Some(d.id) && self.env.hazards[i].zone != 0 {
                            self.hide_hazard(map, i, out);
                        }
                    }
                }
                continue;
            }
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
    /// up hatch (a spawner lets its brood out and stays, after a warning
    /// ring).
    fn env_objects(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_mut() else { return };
        // Spawners about to open: (x, y, seconds left, object kind).
        let mut rings: Vec<(f32, f32, f32, u8)> = Vec::new();
        let mut k = 0;
        while k < self.env.objects.len() {
            let o = self.env.objects[k];
            let s = o.slot as usize;
            let store = horde.store();
            let standing = store.is_alive(o.slot) && store.generation[s] == o.generation && store.hp[s] > 0;
            // Once the fog is up nothing hatches any more (it stays an egg).
            let closed = horde.spawning_closed();
            if standing && !closed && o.every > 0.0 && let Some((at, kind, count)) = o.hatch {
                let (x, y) = (store.x[s], store.y[s]);
                if now >= at {
                    // A spawner: the brood climbs out round it, it stays.
                    let base = self.dice.die(360) as f32;
                    for i in 0..count {
                        let a = (base + 360.0 * i as f32 / count.max(1) as f32).to_radians();
                        let _ = horde.spawn(kind, x + a.cos() * BROOD_RING, y + a.sin() * BROOD_RING).or_else(|| horde.spawn(kind, x, y));
                    }
                    self.env.objects[k].hatch = Some((at + o.every, kind, count));
                } else if at - now <= HATCH_WARN_SECS && o.warned != at {
                    self.env.objects[k].warned = at;
                    rings.push((x, y, at - now, store.kind[s]));
                }
                k += 1;
                continue;
            }
            if standing && !closed && let Some((at, kind, count)) = o.hatch.filter(|h| now >= h.0) {
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
            self.hatch_rings(map, &rings, out);
            self.object_felled(map, o.effect, x, y, now, out);
            // The store may have changed under a hook: look again next scan.
            return;
        }
        self.hatch_rings(map, &rings, out);
    }

    /// Spawners' warning rings: a circle filling until the brood comes.
    fn hatch_rings(&mut self, map: MapIdx, rings: &[(f32, f32, f32, u8)], out: &mut Outgoing) {
        for &(x, y, left, kind) in rings {
            let Some(horde) = self.horde.as_ref() else { return };
            let ability = format!("hatch:{}", horde.catalog().get(kind).id);
            let zone = Zone::Area(Area::Circle { x, y, r: HATCH_RING });
            let msg = ServerMsg::BossZones {
                caster: 0,
                ability,
                zones: vec![BossZone { area: zone.telegraph(0, crate::world::protocol::look::DARK, 0), delay_ms: ms(left), flags: 0 }],
            };
            self.broadcast(map, &msg, out);
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

    /// Spells that met hazard ground this step (`cast.rs`, [`GROUND_MEETS`]):
    /// the calm for the hazards, the safe ground (or the puff) for clients.
    pub(super) fn ground_meets(&mut self, met: &[(DamageType, Area)], msgs: &mut Vec<ServerMsg>) {
        for &(kind, area) in met {
            let (x, y) = area.centre();
            let Some(m) = self.env.ground_meet(x, y, kind) else { continue };
            let (x0, y0, x1, y1) = area.bounds();
            let r = ((x1 - x0).max(y1 - y0) / 2.0).clamp(m.r.0, m.r.1);
            if m.calm_secs > 0.0 {
                self.env_calm(x, y, r, m.calm_secs);
            }
            let span = ms(m.calm_secs);
            let id = if span > 0 {
                let id = self.next_zone;
                self.next_zone = self.next_zone.wrapping_add(1).max(1);
                id
            } else {
                0
            };
            let area = Area::Circle { x, y, r }.telegraph(id, m.show, span, 0);
            let flags = crate::world::protocol::zone_flags::SAFE;
            msgs.push(ServerMsg::BossZones { caster: 0, ability: m.ability.into(), zones: vec![BossZone { area, delay_ms: 0, flags }] });
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
        self.env.objects.push(LiveObject { slot, generation, effect: ObjectEffect::None, hatch, every: 0.0, warned: -1.0 });
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
            // >>> orc_village (agent ORC) >>>
            super::orc::BANNER_HOOK => self.orc_banner_felled(map, x, y, out),
            // <<< orc_village <<<
            // >>> toy_factory (agent TOY) >>>
            super::toy::SWITCH_HOOK => self.toy_switch(map, x, y, out),
            // <<< toy_factory <<<
            // >>> glast_heim (agent GLAST) >>>
            super::glast::GRAVE_HOOK => self.glast_consecrate(map, x, y, out),
            super::glast::FONT_HOOK => self.glast_font(map, x, y, out),
            // <<< glast_heim <<<
            // >>> thor_volcano (agent THOR) >>>
            // <<< thor_volcano <<<
            // >>> thanatos_tower (agent TOWER) >>>
            // <<< thanatos_tower <<<
            _ => warn!("{}: object hook {id} at ({x:.1}, {y:.1}) has no handler", self.name),
        }
    }

    /// A location's own landing effects of event strikes
    /// ([`Strikes::hook`]): add an arm per hook id (a crate leaving a Myst
    /// Case, a beam healing angels); `(x, y)` is where the circle landed.
    fn strike_hook(&mut self, map: MapIdx, id: &str, x: f32, y: f32, out: &mut Outgoing) {
        match id {
            // >>> orc_village: strike_hook (agent ORC) >>>
            // <<< orc_village: strike_hook <<<
            // >>> toy_factory: strike_hook (agent TOY) >>>
            super::toy::CRATE_HOOK => self.toy_crate(x, y),
            // <<< toy_factory: strike_hook <<<
            // >>> glast_heim: strike_hook (agent GLAST) >>>
            // <<< glast_heim: strike_hook <<<
            // >>> thor_volcano: strike_hook (agent THOR) >>>
            // <<< thor_volcano: strike_hook <<<
            // >>> thanatos_tower: strike_hook (agent TOWER) >>>
            crate::world::missions::tower::JUDGMENT => self.tower_judged(map, x, y, out),
            // <<< thanatos_tower: strike_hook <<<
            _ => warn!("{}: strike hook {id} at ({x:.1}, {y:.1}) has no handler", self.name),
        }
    }

    /// Plants one more object of `monster` (a roster plant) at `(x, y)`
    /// whose felling fires `effect` (a brazier lit again after it burnt
    /// out): its slot and generation, `None` when it could not be placed.
    pub(super) fn env_plant(&mut self, monster: &str, x: f32, y: f32, effect: ObjectEffect) -> Option<(u16, u8)> {
        self.env_plant_hatching(monster, x, y, effect, None)
    }

    /// [`Self::env_plant`] of an object that hatches (`hatch.secs` from
    /// now; a spawner again every `hatch.every_secs`): a grave risen again
    /// in the Witching Hour. `None` when it could not be placed, or the
    /// hatch's monster is not in the roster.
    pub(super) fn env_plant_hatching(&mut self, monster: &str, x: f32, y: f32, effect: ObjectEffect, hatch: Option<Hatch>) -> Option<(u16, u8)> {
        let horde = self.horde.as_mut()?;
        let now = horde.director_elapsed();
        let kind = horde.catalog().kind_of(monster)?;
        let brood = match hatch {
            Some(h) => Some((now + h.secs, horde.catalog().kind_of(h.monster)?, h.count)),
            None => None,
        };
        let slot = horde.place_object(kind, x, y)?;
        let generation = horde.store().generation[slot as usize];
        let every = hatch.map_or(0.0, |h| h.every_secs.max(0.0));
        self.env.objects.push(LiveObject { slot, generation, effect, hatch: brood, every, warned: -1.0 });
        Some((slot, generation))
    }

    /// Event strikes: volleys while their event runs, circles landing.
    fn env_strikes(&mut self, map: MapIdx, now: f32, out: &mut Outgoing, hits: &mut Vec<HitEvent>) {
        if self.env.strikes.is_empty() {
            return;
        }
        for k in 0..self.env.strikes.len() {
            let st = self.env.strikes[k];
            let Some(start) = self.env_event_start(st.def.during) else { continue };
            let end = start + self.env.events.iter().find(|e| e.def.id == st.def.during).map_or(0.0, |e| e.def.duration_secs);
            if st.occurrence != Some(start) {
                self.env.strikes[k].occurrence = Some(start);
                self.env.strikes[k].next = start;
            }
            // A stalled stage skips the volleys it missed instead of
            // dropping them all at once.
            if self.env.strikes[k].next + st.def.every_secs <= now {
                self.env.strikes[k].next = now;
            }
            while self.env.strikes[k].next <= now && self.env.strikes[k].next < end {
                self.env.strikes[k].next += st.def.every_secs;
                self.strike_volley(map, k, now, out);
            }
        }
        // Circles landing.
        let mut i = 0;
        while i < self.env.falling.len() {
            if self.env.falling[i].at > now {
                i += 1;
                continue;
            }
            let fall = self.env.falling.swap_remove(i);
            self.strike_lands(map, fall, now, out, hits);
        }
        // Patches whose time is up (clients drop them by their span).
        let mut h = self.env.hazards.len();
        while h > 0 {
            h -= 1;
            if self.env.hazards[h].until.is_some_and(|u| u <= now) {
                self.env.hazards.remove(h);
                self.env.pulse_at.remove(h);
            }
        }
    }

    /// One volley of strikes `k`: circles on or near every standing player.
    fn strike_volley(&mut self, map: MapIdx, k: usize, now: f32, out: &mut Outgoing) {
        let def = self.env.strikes[k].def;
        let walk = &self.world.map(map).walk;
        let mut zones = Vec::new();
        let ups: Vec<(f32, f32)> = self.players.iter().filter(|p| p.map == map && p.fighter.as_ref().is_some_and(|f| f.hunted())).map(|p| (p.x, p.y)).collect();
        for (px, py) in ups {
            for n in 0..=def.extra as u32 {
                let spread = if n == 0 { def.scatter } else { def.extra_spread };
                let spot = (0..STRIKE_TRIES).find_map(|t| {
                    let (x, y) = if t == 0 && spread <= 0.0 {
                        (px, py)
                    } else {
                        let a = self.dice.die(3600) as f32 * std::f32::consts::TAU / 3600.0;
                        let d = spread.max(0.0) * (self.dice.die(1000) as f32 / 1000.0).sqrt();
                        (px + a.cos() * d, py + a.sin() * d)
                    };
                    (walk.is_walkable(x, y) && def.allowed(x, y)).then_some((x, y))
                });
                let Some((x, y)) = spot else { continue };
                self.env.falling.push(Falling { strikes: k, x, y, at: now + def.fall_secs });
                let zone = Zone::Area(Area::Circle { x, y, r: def.r });
                zones.push(BossZone { area: zone.telegraph(0, def.look_id(), 0), delay_ms: ms(def.fall_secs), flags: 0 });
            }
        }
        if !zones.is_empty() {
            self.broadcast(map, &ServerMsg::BossZones { caster: 0, ability: def.id.into(), zones }, out);
        }
    }

    /// A strike circle lands: the blow, its patch, the location's hook.
    fn strike_lands(&mut self, map: MapIdx, fall: Falling, now: f32, out: &mut Outgoing, hits: &mut Vec<HitEvent>) {
        let def = self.env.strikes[fall.strikes].def;
        let (x, y) = (fall.x, fall.y);
        let hz = self.config.tick_hz.max(1) as f32;
        let level = self.env.mission.map_or(1, |m| m.levels.min);
        let kind = DamageType::from_id(def.damage_type).unwrap_or(DamageType::Physical);
        let rider = def.condition.and_then(Condition::from_id).map(|c| (c, def.condition_secs));
        let hurt = Hurt { dice: (def.damage[0], def.damage[1]), kind, rider, save: def.dc > 0, ranged: def.ranged, push: def.push, split: 0 };
        let blow = Blow { zone: Zone::Area(Area::Circle { x, y, r: def.r }), hurt, map, safe: &[], level, dc: def.dc, boost: 1.0, evil: false, from: (x, y) };
        let walk = &self.world.map(map).walk;
        land(&blow, &mut self.players, &self.lives, walk, &mut self.dice, hz, hits);
        if let Some(patch) = def.patch
            && def.patch_secs > 0.0
        {
            let shape = match patch.shape {
                HazardShape::Circle { r, .. } => HazardShape::Circle { x, y, r },
                HazardShape::Rect { w, h, .. } => HazardShape::Rect { x, y, w, h },
            };
            let i = self.env.hazards.len();
            self.env.hazards.push(LiveHazard { on: true, until: Some(now + def.patch_secs), ..LiveHazard::new(Hazard { shape, during: None, ..patch }) });
            self.env.pulse_at.push(now);
            self.show_hazards(map, &[i], 0.0, def.patch_secs, out);
        }
        if let Some(id) = def.hook {
            self.strike_hook(map, id, x, y, out);
        }
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
        self.env_carry_monsters(map);
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
                let (ddx, ddy) = h.drift();
                if (ddx != 0.0 || ddy != 0.0) && def.shape.contains(p.x, p.y, 0.0) {
                    // As a walking step (clients mirror it: prediction.dart
                    // `MoveRules.drift`): one axis at a time, walls stop it.
                    (p.x, p.y) = drift_step(walk, p.x, p.y, ddx / hz, ddy / hz);
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

    /// Drifting ground carries the monsters on it (centre inside; plants
    /// stay rooted, burrowed ones are under it), a step per axis each tick.
    fn env_carry_monsters(&mut self, map: MapIdx) {
        if !self.env.hazards.iter().any(|h| h.on && h.def.drifts() && h.scale != 0.0) {
            return;
        }
        let hz = self.config.tick_hz.max(1) as f32;
        let walk = &self.world.map(map).walk;
        let Some(horde) = self.horde.as_mut() else { return };
        let mut near = std::mem::take(&mut self.env.carried);
        for h in self.env.hazards.iter().filter(|h| h.on && h.def.drifts()) {
            let (dx, dy) = h.drift();
            if dx == 0.0 && dy == 0.0 {
                continue;
            }
            let (x0, y0, x1, y1) = match h.def.shape {
                HazardShape::Circle { x, y, r } => (x - r, y - r, x + r, y + r),
                HazardShape::Rect { x, y, w, h } => (x - w / 2.0, y - h / 2.0, x + w / 2.0, y + h / 2.0),
            };
            near.clear();
            horde.near_box(x0, y0, x1, y1, &mut near);
            for &slot in &near {
                let s = slot as usize;
                let (mx, my) = (horde.store().x[s], horde.store().y[s]);
                if horde.store().is_plant(slot) || !h.def.shape.contains(mx, my, 0.0) {
                    continue;
                }
                let (nx, ny) = drift_step(walk, mx, my, dx / hz, dy / hz);
                if (nx, ny) != (mx, my) {
                    horde.place(slot, nx, ny);
                }
            }
        }
        self.env.carried = near;
    }

    /// Scales the drift of every drifting hazard `pick` chooses (a belt
    /// reversed: −1.5, stopped: 0, back to normal: 1); those shown now are
    /// sent again with their new direction and speed. Returns how many
    /// changed.
    pub(super) fn env_set_drift(&mut self, map: MapIdx, pick: impl Fn(&Hazard) -> bool, scale: f32, out: &mut Outgoing) -> usize {
        let Some(m) = self.env.mission else { return 0 };
        let now = self.horde.as_ref().map_or(0.0, |h| h.director_elapsed());
        let mut shown = Vec::new();
        let mut n = 0;
        for i in 0..self.env.hazards.len() {
            let h = &self.env.hazards[i];
            if !h.def.drifts() || h.scale == scale || !pick(&h.def) {
                continue;
            }
            n += 1;
            self.env.hazards[i].scale = scale;
            if self.env.hazards[i].zone != 0 && self.env.hazards[i].def.during.is_none() {
                self.hide_hazard(map, i, out);
                self.env.hazards[i].on = true;
                shown.push(i);
            }
        }
        if !shown.is_empty() {
            self.show_hazards(map, &shown, 0.0, (m.time_limit_secs - now).max(0.0), out);
        }
        n
    }

    /// The drifting hazards of the stage (id, shape): a location's belts.
    pub(super) fn env_drifting(&self) -> Vec<(&'static str, HazardShape)> {
        self.env.hazards.iter().filter(|h| h.def.drifts()).map(|h| (h.def.id, h.def.shape)).collect()
    }

    /// The drift scale of hazard `id` (tests).
    #[cfg(test)]
    pub(super) fn env_drift_scale(&self, id: &str) -> Option<f32> {
        self.env.hazards.iter().find(|h| h.def.id == id).map(|h| h.scale)
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
            zones.push(h.zone(h.zone, ms(delay), ms(span)));
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
    /// when it already runs, the mission has no such event, or it calls
    /// waves ([`MapEvent::calls_waves`]) and the fog closed them.
    pub(super) fn env_force_event(&mut self, map: MapIdx, id: &str, now: f32, out: &mut Outgoing) -> bool {
        let Some(k) = self.env.events.iter().position(|e| e.def.id == id) else { return false };
        if self.env.events[k].running.is_some() {
            return false;
        }
        // No herald of monsters once the fog closed the waves (the Orc
        // Lord's Call the Clans blows no horn then).
        if self.env.events[k].def.calls_waves && self.horde.as_ref().is_some_and(|h| h.spawning_closed()) {
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
