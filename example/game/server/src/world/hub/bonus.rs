//! Pickup bonuses (COMBAT.md §2.7): bubbles that float over the field of a
//! running mission; the first standing player to walk into one sets it
//! off. The server owns them: it spawns them (a few at a time, near the
//! party), bursts them when their time runs out, detects the pickup and
//! applies the effect.
//!
//! Most bonuses are skills of the book cast as the player who took them,
//! for free (no SP, no cooldown, no wind-up) and at the *location's*
//! level ([`Horde::location_level`](crate::world::horde::Horde::location_level)):
//! the skill level and the dice multiplier come from the map, not from
//! the character, so an early field never hands out much. Their damage,
//! traps, walls and buffs are ordinary [`super::cast::Live`] areas and
//! timed buffs: kills are credited to the player, nothing is persisted
//! (a relog never keeps a bonus buff). Zeny, restoration and the berserk
//! potion are applied here.

use super::aim::{aim_skill, Aimed, Field};
use super::cast::{land, telegraphs, zone_ids, Landing};
use super::fight::Fighter;
use super::mission::Outcome;
use super::{HubEvent, HubInstance, MapIdx, Outgoing};
use crate::world::combat::{Dice, Mods, Rng, SkillBook};
use crate::world::protocol::{hit, HitEvent, ServerMsg};
use std::f32::consts::TAU;
use std::sync::Arc;

/// The first bonus of a stage appears this long after it starts (s).
const FIRST_SECS: f32 = 15.0;
/// Then one more every this many seconds (uniform in the range).
const EVERY_SECS: (f32, f32) = (18.0, 30.0);
/// A bonus nobody takes bursts after this long (s).
pub(super) const LIFE_SECS: f32 = 30.0;
/// At most this many at once: the base, one more per extra party member on
/// the map, never above the party cap.
const MAX_LIVE: usize = 3;
const MAX_LIVE_PARTY: usize = 5;
/// Spawn distance from a random standing player (cells), within the light.
const SPAWN_RING: (f32, f32) = (6.0, 22.0);
/// Bonuses keep this far apart (cells).
const SPACING: f32 = 5.0;
/// A player whose centre comes this close takes it (cells: body + bubble).
pub(super) const PICK_RADIUS: f32 = 1.2;
/// Arrow Spiral: an arrow this often, each turned this much further.
const ARROW_EVERY_SECS: f32 = 0.08;
const ARROW_TURN: f32 = TAU / 10.0;
/// Fire ring: the walls stand at least this far out (cells).
const RING_MIN: f32 = 4.0;
/// Traps land this far from the player (cells).
const TRAP_RING: (f32, f32) = (3.0, 9.0);
/// Buff ids (`Fighter::buff`) of bonuses that are not skills: `BUFF_BASE +
/// kind`, far above any book index.
pub(super) const BUFF_BASE: u16 = 0xFF00;

/// What a bonus does (its wire id is [`Kind::id`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A fire nova around the player (Magnum Break).
    Nova,
    /// A ring of fire walls around the player.
    FireRing,
    /// Hunter traps scattered around (skid, ankle snare, land mine,
    /// shockwave, claymore), each with the trap's own timer.
    Traps,
    /// Arrows released one by one along a spiral (Charge Arrow).
    ArrowSpiral,
    /// Storm Gust on the densest crowd nearby (else on the player).
    StormGust,
    /// Party buffs of the Acolyte tree, on the player and allies near.
    Blessing,
    IncreaseAgi,
    Gloria,
    /// Temporary HP on the player.
    Kyrie,
    /// HP and SP back at once.
    Restore,
    /// A little zeny.
    Zeny,
    /// Attacks faster and runs a little faster for a while.
    Berserk,
}

impl Kind {
    pub const ALL: [Kind; 12] = [
        Kind::Nova,
        Kind::FireRing,
        Kind::Traps,
        Kind::ArrowSpiral,
        Kind::StormGust,
        Kind::Blessing,
        Kind::IncreaseAgi,
        Kind::Gloria,
        Kind::Kyrie,
        Kind::Restore,
        Kind::Zeny,
        Kind::Berserk,
    ];

    /// Wire id (the client's icon, name and effects are keyed by it).
    pub fn id(self) -> &'static str {
        match self {
            Kind::Nova => "nova",
            Kind::FireRing => "fire_ring",
            Kind::Traps => "traps",
            Kind::ArrowSpiral => "arrow_spiral",
            Kind::StormGust => "storm_gust",
            Kind::Blessing => "blessing",
            Kind::IncreaseAgi => "increase_agi",
            Kind::Gloria => "gloria",
            Kind::Kyrie => "kyrie",
            Kind::Restore => "restore",
            Kind::Zeny => "zeny",
            Kind::Berserk => "berserk",
        }
    }

    /// Relative chance of being the next one spawned.
    fn weight(self) -> u32 {
        match self {
            Kind::Nova | Kind::Restore => 10,
            Kind::FireRing | Kind::Traps | Kind::ArrowSpiral => 8,
            Kind::StormGust | Kind::Zeny => 7,
            Kind::Berserk => 6,
            Kind::Blessing | Kind::IncreaseAgi => 5,
            Kind::Gloria | Kind::Kyrie => 4,
        }
    }

    /// Book skills it casts (any one is enough; empty = not a skill).
    fn skills(self) -> &'static [&'static str] {
        match self {
            Kind::Nova => &["magnum_break"],
            Kind::FireRing => &["fire_wall"],
            Kind::Traps => &["skid_trap", "ankle_snare", "land_mine", "shockwave_trap", "claymore_trap"],
            Kind::ArrowSpiral => &["charge_arrow"],
            Kind::StormGust => &["storm_gust"],
            Kind::Blessing => &["blessing"],
            Kind::IncreaseAgi => &["increase_agi"],
            Kind::Gloria => &["gloria"],
            Kind::Kyrie => &["kyrie_eleison"],
            Kind::Restore | Kind::Zeny | Kind::Berserk => &[],
        }
    }

    /// Castable skills of this kind in `book` (book indices).
    fn castable(self, book: &SkillBook) -> Vec<u16> {
        self.skills().iter().filter_map(|id| book.find(id).filter(|(_, d)| d.ready()).map(|(i, _)| i)).collect()
    }

    /// It can spawn with this book.
    pub fn available(self, book: &SkillBook) -> bool {
        self.skills().is_empty() || !self.castable(book).is_empty()
    }
}

/// HUD id of a running buff that came from a bonus (`bonus:<id>`), if it is one.
pub(super) fn buff_id(skill: u16) -> Option<&'static str> {
    skill.checked_sub(BUFF_BASE).and_then(|k| Kind::ALL.get(k as usize)).map(|k| k.id())
}

// ---------------------------------------------------------------------------
// Strength by location level
// ---------------------------------------------------------------------------

/// Skill level a bonus casts at on a location of level `location`.
pub fn skill_level(location: u32, max: u8) -> u8 {
    (1 + location / 8).clamp(1, max.max(1) as u32) as u8
}

/// Zeny of a zeny bonus: `(level + 1) × (1 + 1d3)` (Poring Island, level
/// 11: 24–48, a few dozen swarm kills' worth).
pub fn zeny(location: u32, rng: &mut Rng) -> u64 {
    (location as u64 + 1) * (1 + rng.die(3) as u64)
}

/// HP and SP dice of a restoration.
pub fn restore(location: u32) -> (Dice, Dice) {
    (Dice::new(2 + location / 5, 8, location as i32), Dice::new(1 + location / 8, 6, (location / 2) as i32))
}

/// The berserk potion's modifiers and duration (s).
pub fn berserk(location: u32) -> (Mods, f32) {
    let mods = Mods { haste: (0.2 + location as f32 * 0.003).min(0.4), speed: 0.1, ..Mods::default() };
    (mods, (15.0 + location as f32 * 0.25).min(40.0))
}

/// Fire walls in the ring.
pub fn walls(location: u32) -> u32 {
    (6 + location / 20).min(10)
}

/// Traps scattered.
pub fn traps(location: u32) -> u32 {
    (3 + location / 15).min(8)
}

/// Arrows in the spiral.
pub fn arrows(location: u32) -> u32 {
    (12 + location / 3).min(36)
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// A bonus on the ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Bonus {
    pub id: u32,
    pub kind: Kind,
    pub x: f32,
    pub y: f32,
    /// It bursts at this tick.
    pub until: u32,
}

/// Arrows of a spiral still to be released.
#[derive(Clone, Copy, Debug)]
struct Volley {
    uid: u32,
    skill: u16,
    slv: u8,
    power: u32,
    left: u32,
    next: u32,
    angle: f32,
}

/// The bonuses of a stage.
#[derive(Debug, Default)]
pub(super) struct Bonuses {
    pub live: Vec<Bonus>,
    next_id: u32,
    /// Tick of the next spawn (set when the first player stands on the map).
    next_spawn: Option<u32>,
    volleys: Vec<Volley>,
}

impl HubInstance {
    /// One step after the skills: arrows due, bursts, pickups, spawns.
    /// Healing lands in `hits` (shown with the tick's combat).
    pub(super) fn bonus_step(&mut self, out: &mut Outgoing, hits: &mut Vec<HitEvent>) {
        let Some(map) = self.horde.as_ref().map(|h| h.map()) else { return };
        let now = self.tick;
        let mut msgs: Vec<ServerMsg> = Vec::new();
        self.volley_step(map, &mut msgs);

        // Bursts.
        self.bonuses.live.retain(|b| {
            let keep = now < b.until;
            if !keep {
                msgs.push(ServerMsg::BonusGone { id: b.id, bonus: b.kind.id().into(), entity_id: 0 });
            }
            keep
        });

        // Pickups: the first standing player inside (lowest index); a
        // link-dead one takes nothing.
        let mut taken: Vec<(usize, Bonus)> = Vec::new();
        let players = &self.players;
        self.bonuses.live.retain(|b| {
            let who = players.iter().position(|p| {
                p.map == map
                    && !p.link_dead
                    && p.fighter.as_ref().is_some_and(Fighter::is_up)
                    && (p.x - b.x).powi(2) + (p.y - b.y).powi(2) <= PICK_RADIUS * PICK_RADIUS
            });
            match who {
                Some(i) => {
                    taken.push((i, *b));
                    false
                }
                None => true,
            }
        });
        for (idx, b) in taken {
            let entity_id = self.players[idx].profile.character_id;
            msgs.push(ServerMsg::BonusGone { id: b.id, bonus: b.kind.id().into(), entity_id });
            self.bonus_apply(idx, b.kind, hits, &mut msgs, out);
        }

        self.bonus_spawn(map, &mut msgs);

        if msgs.is_empty() {
            return;
        }
        for p in self.players.iter().filter(|p| p.map == map) {
            for m in &msgs {
                out.push((p.profile.user_id, HubEvent::Control(m.clone())));
            }
        }
    }

    /// The live bonuses to a player who just joined the stage.
    pub(super) fn announce_bonuses(&self, idx: usize, out: &mut Outgoing) {
        let p = &self.players[idx];
        if self.horde.as_ref().is_none_or(|h| h.map() != p.map) {
            return;
        }
        let ms_per_tick = 1000 / self.config.tick_hz.max(1);
        for b in &self.bonuses.live {
            let life_ms = b.until.saturating_sub(self.tick) * ms_per_tick;
            let msg = ServerMsg::Bonus { id: b.id, bonus: b.kind.id().into(), x: b.x, y: b.y, life_ms };
            out.push((p.profile.user_id, HubEvent::Control(msg)));
        }
    }

    /// Spawns the next bonus when it is due and there is room.
    fn bonus_spawn(&mut self, map: MapIdx, msgs: &mut Vec<ServerMsg>) {
        if self.outcome != Outcome::Running {
            return;
        }
        let up: Vec<(f32, f32)> = self
            .players
            .iter()
            .filter(|p| p.map == map && p.fighter.as_ref().is_some_and(Fighter::is_up))
            .map(|p| (p.x, p.y))
            .collect();
        if up.is_empty() {
            return;
        }
        let now = self.tick;
        let first = self.ticks_of(FIRST_SECS);
        let due = *self.bonuses.next_spawn.get_or_insert(now + first);
        if now < due {
            return;
        }
        let gap = EVERY_SECS.0 + (EVERY_SECS.1 - EVERY_SECS.0) * (self.dice.die(1000) - 1) as f32 / 999.0;
        self.bonuses.next_spawn = Some(now + self.ticks_of(gap));
        let fighters = self.players.iter().filter(|p| p.map == map && p.fighter.is_some()).count();
        let cap = (MAX_LIVE + fighters.saturating_sub(1)).min(MAX_LIVE_PARTY);
        if self.bonuses.live.len() >= cap {
            return;
        }
        // The kind: weighted among what this book can cast.
        let kinds: Vec<Kind> = Kind::ALL.into_iter().filter(|k| k.available(&self.skills)).collect();
        let total: u32 = kinds.iter().map(|k| k.weight()).sum();
        if total == 0 {
            return;
        }
        let mut roll = self.dice.die(total);
        let Some(kind) = kinds.iter().copied().find(|k| {
            if roll <= k.weight() {
                return true;
            }
            roll -= k.weight();
            false
        }) else {
            return;
        };
        // Where: around a random standing player, inside the light, on open
        // ground, apart from the others and from everyone's feet.
        let light = self.horde.as_ref().map_or(0.0, |h| h.light_radius());
        let far = if light > 0.0 { SPAWN_RING.1.min(light * 0.9).max(SPAWN_RING.0) } else { SPAWN_RING.1 };
        let walk = &self.world.map(map).walk;
        // Never out in the poison fog.
        let fog = self.fog_now();
        let mut spot = None;
        for _ in 0..16 {
            let (px, py) = up[(self.dice.die(up.len() as u32) - 1) as usize];
            let a = self.dice.die(360) as f32 * TAU / 360.0;
            let d = SPAWN_RING.0 + (far - SPAWN_RING.0) * (self.dice.die(1000) - 1) as f32 / 999.0;
            let (x, y) = (px + a.cos() * d, py + a.sin() * d);
            let clear = |&(ox, oy): &(f32, f32), r: f32| (ox - x).powi(2) + (oy - y).powi(2) >= r * r;
            if walk.is_walkable(x, y)
                && fog.is_none_or(|(c, t)| !c.outside(x, y, t))
                && up.iter().all(|o| clear(o, PICK_RADIUS * 2.0))
                && self.bonuses.live.iter().all(|b| clear(&(b.x, b.y), SPACING))
            {
                spot = Some((x, y));
                break;
            }
        }
        let Some((x, y)) = spot else { return };
        self.bonuses.next_id = self.bonuses.next_id.wrapping_add(1).max(1);
        let life = self.ticks_of(LIFE_SECS);
        let b = Bonus { id: self.bonuses.next_id, kind, x, y, until: now + life };
        self.bonuses.live.push(b);
        let life_ms = life * 1000 / self.config.tick_hz.max(1);
        msgs.push(ServerMsg::Bonus { id: b.id, bonus: kind.id().into(), x, y, life_ms });
    }

    /// What taking a bonus does to player `idx` (and around them).
    fn bonus_apply(&mut self, idx: usize, kind: Kind, hits: &mut Vec<HitEvent>, msgs: &mut Vec<ServerMsg>, out: &mut Outgoing) {
        let level = self.horde.as_ref().map_or(1, |h| h.location_level());
        let book = Arc::clone(&self.skills);
        let skills = kind.castable(&book);
        let (px, py) = (self.players[idx].x, self.players[idx].y);
        let uid = self.players[idx].profile.user_id;
        let entity = self.players[idx].profile.character_id;
        let around = |x: f32, y: f32, dir: (f32, f32)| Aimed { x, y, ox: px, oy: py, dir, ally: Some(uid), monster: None };
        let here = around(px, py, (1.0, 0.0));
        match kind {
            Kind::Nova | Kind::Blessing | Kind::IncreaseAgi | Kind::Gloria | Kind::Kyrie => {
                if let Some(&s) = skills.first() {
                    let slv = skill_level(level, book.get(s).max_level);
                    self.bonus_cast(idx, s, slv, level, here, msgs);
                }
            }
            Kind::StormGust => {
                let Some(&s) = skills.first() else { return };
                let def = book.get(s);
                let slv = skill_level(level, def.max_level);
                let aimed = self
                    .horde
                    .as_ref()
                    .and_then(|h| {
                        let field = Field { horde: h, players: &self.players, map: self.players[idx].map };
                        aim_skill(&field, &self.players[idx], def, slv)
                    })
                    .unwrap_or(here);
                self.bonus_cast(idx, s, slv, level, aimed, msgs);
            }
            Kind::FireRing => {
                let Some(&s) = skills.first() else { return };
                let def = book.get(s);
                let slv = skill_level(level, def.max_level);
                let n = walls(level);
                // Wide enough that the walls close the ring.
                let width = def.effects.iter().find_map(|e| e.act()).map_or(4.0, |a| a.area.width.at(slv).max(1.0));
                let r = (n as f32 * width / TAU).max(RING_MIN);
                let walk = &self.world.map(self.players[idx].map).walk;
                let spots: Vec<Aimed> = (0..n)
                    .map(|i| i as f32 * TAU / n as f32)
                    .map(|a| (a.cos(), a.sin()))
                    .filter(|d| walk.is_walkable(px + d.0 * r, py + d.1 * r))
                    .map(|d| around(px + d.0 * r, py + d.1 * r, d))
                    .collect();
                for aimed in spots {
                    self.bonus_cast(idx, s, slv, level, aimed, msgs);
                }
            }
            Kind::Traps => {
                if skills.is_empty() {
                    return;
                }
                let walk = &self.world.map(self.players[idx].map).walk;
                let mut spots = Vec::new();
                for _ in 0..traps(level) * 4 {
                    if spots.len() as u32 >= traps(level) {
                        break;
                    }
                    let a = self.dice.die(360) as f32 * TAU / 360.0;
                    let d = TRAP_RING.0 + (TRAP_RING.1 - TRAP_RING.0) * (self.dice.die(1000) - 1) as f32 / 999.0;
                    let (x, y) = (px + a.cos() * d, py + a.sin() * d);
                    if walk.is_walkable(x, y) {
                        let s = skills[(self.dice.die(skills.len() as u32) - 1) as usize];
                        spots.push((s, around(x, y, (a.cos(), a.sin()))));
                    }
                }
                for (s, aimed) in spots {
                    let slv = skill_level(level, book.get(s).max_level);
                    self.bonus_cast(idx, s, slv, level, aimed, msgs);
                }
            }
            Kind::ArrowSpiral => {
                let Some(&s) = skills.first() else { return };
                let slv = skill_level(level, book.get(s).max_level);
                let angle = self.dice.die(360) as f32 * TAU / 360.0;
                // The first arrow leaves at once, the rest follow (volley_step).
                self.bonuses.volleys.push(Volley { uid, skill: s, slv, power: level, left: arrows(level), next: self.tick, angle });
                let map = self.players[idx].map;
                self.volley_step(map, msgs);
            }
            Kind::Restore => {
                let (hp, sp) = restore(level);
                let rng = &mut self.dice;
                let Some(f) = self.players[idx].fighter.as_mut() else { return };
                let amount = hp.roll(rng, false).max(0).min(f.max_hp - f.hp).max(0);
                f.hp += amount;
                f.sp = (f.sp + sp.roll(rng, false).max(0)).min(f.max_sp);
                hits.push(HitEvent { target: entity, amount: amount as u32, flags: hit::HEAL });
            }
            Kind::Zeny => {
                let n = zeny(level, &mut self.dice);
                let Some(f) = self.players[idx].fighter.as_mut() else { return };
                f.add_zeny(n);
                let msg = ServerMsg::Loot { entity_id: entity, x: px, y: py, item: "zeny".into(), count: n as u32, rare: false };
                out.push((uid, HubEvent::Control(msg)));
            }
            Kind::Berserk => {
                let (mods, secs) = berserk(level);
                let until = self.tick + self.ticks_of(secs);
                if let Some(f) = self.players[idx].fighter.as_mut() {
                    f.buff(BUFF_BASE + Kind::Berserk as u16, until, mods, 0);
                }
            }
        }
    }

    /// Casts book skill `skill` as player `idx` at `aimed`: free, at once,
    /// at skill level `slv` with dice of character level `power`. The
    /// cast goes out like the player's own (`msgs`), with no wind-up and
    /// no cooldown, so the client only plays it.
    fn bonus_cast(&mut self, idx: usize, skill: u16, slv: u8, power: u32, aimed: Aimed, msgs: &mut Vec<ServerMsg>) {
        let book = Arc::clone(&self.skills);
        let def = book.get(skill);
        let hz = self.config.tick_hz as f32;
        let zone = self.next_zone;
        self.next_zone = self.next_zone.wrapping_add(zone_ids(def, slv)).max(1);
        let areas = telegraphs(def, slv, &self.players[idx], &aimed, zone, &self.players);
        msgs.push(ServerMsg::Cast {
            caster: self.players[idx].profile.character_id,
            skill: def.id.clone(),
            x: aimed.x,
            y: aimed.y,
            cast_ms: 0,
            cooldown_ms: 0,
            areas,
            level: slv,
        });
        let Some(horde) = self.horde.as_ref() else { return };
        let walk = &self.world.map(self.players[idx].map).walk;
        // The skills of this tick already ran: it lands on the next one.
        let cx = Landing { def, skill, slv, now: self.tick + 1, hz, zone, power: Some(power) };
        land(&cx, &mut self.players[idx], aimed, horde, walk, &mut self.dice, &mut self.lives);
    }

    /// Releases the spiral arrows that are due. A player who went down or
    /// left loses the rest.
    fn volley_step(&mut self, map: MapIdx, msgs: &mut Vec<ServerMsg>) {
        if self.bonuses.volleys.is_empty() {
            return;
        }
        let now = self.tick;
        let every = self.ticks_of(ARROW_EVERY_SECS);
        let mut volleys = std::mem::take(&mut self.bonuses.volleys);
        let mut i = 0;
        while i < volleys.len() {
            let v = &mut volleys[i];
            if v.next > now {
                i += 1;
                continue;
            }
            let shooter = self
                .players
                .iter()
                .position(|p| p.profile.user_id == v.uid && p.map == map && p.fighter.as_ref().is_some_and(Fighter::is_up));
            let Some(idx) = shooter else {
                volleys.swap_remove(i);
                continue;
            };
            let (px, py) = (self.players[idx].x, self.players[idx].y);
            let dir = (v.angle.cos(), v.angle.sin());
            let range = self.skills.get(v.skill).range.max(1.0);
            let aimed = Aimed { x: px + dir.0 * range, y: py + dir.1 * range, ox: px, oy: py, dir, ally: None, monster: None };
            let (skill, slv, power) = (v.skill, v.slv, v.power);
            v.angle = (v.angle + ARROW_TURN) % TAU;
            v.left = v.left.saturating_sub(1);
            v.next = now + every;
            let done = v.left == 0;
            self.bonus_cast(idx, skill, slv, power, aimed, msgs);
            if done {
                volleys.swap_remove(i);
            } else {
                i += 1;
            }
        }
        volleys.append(&mut self.bonuses.volleys);
        self.bonuses.volleys = volleys;
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::map;
    use super::super::{HubSimConfig, HubWorld, PlayerProfile, Vital};
    use super::*;
    use crate::world::combat::Loadout;
    use crate::world::horde::catalog::{test_def, MonsterCatalog};
    use crate::world::horde::{Horde, HordeConfig};
    use crate::world::map::{CameraConstraint, LocationKind};
    use crate::world::protocol::{Look, MONSTER_ENTITY_BASE};
    use uuid::Uuid;

    /// The real skills the bonuses cast (copied from skills.toml, in the
    /// book's JSON: metres, as the pipeline writes them).
    const BOOK: &str = r#"{"skills":[
      {"id":"magnum_break","name":"Magnum Break","class":"swordsman","max_level":10,"target":"self",
       "effects":[{"type":"damage","dice":"2d6","scale":"k","damage":"fire","radius":2.5,"knockback":1.0},
                  {"type":"buff","extra":"1d4","secs":10.0}]},
      {"id":"charge_arrow","name":"Charge Arrow","class":"archer","max_level":1,"target":"aim","range":10.0,
       "effects":[{"type":"damage","shape":"line","width":0.6,"limit":1,"dice":"2d8","scale":"k","knockback":3.0}]},
      {"id":"blessing","name":"Blessing","class":"acolyte","max_level":10,"target":"allies","range":5.0,
       "effects":[{"type":"buff","radius":5.0,"secs":[30.0,6.0],"bless":"1d4"}]},
      {"id":"ankle_snare","name":"Ankle Snare","class":"archer","max_level":5,"target":"aim","range":4.0,
       "effects":[{"type":"condition","radius":1.5,"trigger":0.5,"secs":30.0,"save":{"ability":"reflex","condition":"rooted","secs":[2.0,1.0]}}]}
    ]}"#;

    fn profile(user_id: u32) -> PlayerProfile {
        PlayerProfile {
            user_id,
            character_id: 100 + user_id,
            name: format!("p{user_id}"),
            archetype: "swordsman".into(),
            look: Look::default(),
            level: 30,
            exp: 0,
            weapon: String::new(),
            loadout: Loadout::default(),
            bag: Default::default(),
        }
    }

    /// An open 40 × 40 field, a level 1–20 horde (location level 11), no
    /// game master, `players` joined at the spawn.
    fn stage(book: &str, players: &[u32]) -> HubInstance {
        let wall = "#".repeat(42);
        let row = format!("#{}#", ".".repeat(40));
        let mut rows = vec![wall.as_str()];
        rows.extend(std::iter::repeat_n(row.as_str(), 40));
        rows.push(wall.as_str());
        let field = map(
            "field",
            LocationKind::Field,
            CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
            &rows,
            Vec::new(),
        );
        let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
        let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 0.01, 1, 0.0, false)]).unwrap());
        let cfg = HordeConfig { light_radius: 20.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
        let horde = Horde::new(Arc::clone(&world), catalog, cfg);
        let book = Arc::new(SkillBook::from_json(book).unwrap());
        let mut s = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(book);
        s.dice = Rng::new(9);
        let mut out = Vec::new();
        for &u in players {
            s.join(profile(u), Uuid::nil(), None, 0, &mut out);
        }
        s
    }

    fn put(s: &mut HubInstance, kind: Kind, x: f32, y: f32) -> u32 {
        let id = 1000 + s.bonuses.live.len() as u32;
        s.bonuses.live.push(Bonus { id, kind, x, y, until: s.tick + 400 });
        id
    }

    fn msgs(out: &Outgoing, user: u32) -> Vec<ServerMsg> {
        out.iter()
            .filter(|(u, _)| *u == user)
            .filter_map(|(_, e)| match e {
                HubEvent::Control(m) => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    fn idx(s: &HubInstance, user: u32) -> usize {
        s.players.iter().position(|p| p.profile.user_id == user).unwrap()
    }

    #[test]
    fn strength_grows_with_the_location_level() {
        assert_eq!(skill_level(1, 10), 1);
        assert_eq!(skill_level(11, 10), 2);
        assert_eq!(skill_level(80, 10), 10);
        assert_eq!(skill_level(80, 5), 5, "capped at the skill's max level");
        assert_eq!(skill_level(80, 1), 1);
        let mut rng = Rng::new(1);
        let early: Vec<u64> = (0..200).map(|_| zeny(1, &mut rng)).collect();
        assert!(early.iter().all(|&z| (4..=8).contains(&z)), "an early field gives a few zeny");
        let late: Vec<u64> = (0..200).map(|_| zeny(70, &mut rng)).collect();
        assert!(late.iter().all(|&z| (142..=284).contains(&z)));
        assert!(restore(60).0.count > restore(1).0.count && restore(60).1.count > restore(1).1.count);
        let ((m1, s1), (m2, s2)) = (berserk(1), berserk(90));
        assert!(m2.haste > m1.haste && m2.haste <= 0.4 && s2 > s1 && s2 <= 40.0);
        assert!(walls(90) > walls(1) && traps(90) > traps(1) && arrows(90) > arrows(1));
        assert!(arrows(500) <= 36 && traps(500) <= 8 && walls(500) <= 10);
    }

    #[test]
    fn kinds_need_their_skills() {
        let empty = SkillBook::default();
        let free: Vec<Kind> = Kind::ALL.into_iter().filter(|k| k.available(&empty)).collect();
        assert_eq!(free, vec![Kind::Restore, Kind::Zeny, Kind::Berserk]);
        let book = SkillBook::from_json(BOOK).unwrap();
        assert!(Kind::Nova.available(&book) && Kind::Traps.available(&book) && !Kind::StormGust.available(&book));
        assert_eq!(buff_id(BUFF_BASE + Kind::Berserk as u16), Some("berserk"));
        assert_eq!(buff_id(3), None);
    }

    #[test]
    fn bonuses_spawn_near_the_party_up_to_a_cap_and_burst() {
        let mut s = stage(BOOK, &[1]);
        let mut out = Vec::new();
        let first = s.ticks_of(FIRST_SECS);
        for _ in 0..first - 2 {
            s.tick(&mut out);
        }
        assert!(s.bonuses.live.is_empty(), "nothing in the first seconds");
        let mut spawned = Vec::new();
        // Long enough for many spawns, too short for the first to burst.
        let wait = s.ticks_of(LIFE_SECS) - 5;
        for _ in 0..wait {
            out.clear();
            s.tick(&mut out);
            spawned.extend(msgs(&out, 1).into_iter().filter(|m| matches!(m, ServerMsg::Bonus { .. })));
        }
        let (px, py) = (s.players[0].x, s.players[0].y);
        assert!(!spawned.is_empty() && spawned.len() <= MAX_LIVE, "{spawned:?}");
        assert_eq!(s.bonuses.live.len(), spawned.len());
        let walk = &s.world.map(0).walk;
        for b in &s.bonuses.live {
            let d = ((b.x - px).powi(2) + (b.y - py).powi(2)).sqrt();
            assert!((SPAWN_RING.0 - 0.01..=18.01).contains(&d), "inside the light: {d}");
            assert!(walk.is_walkable(b.x, b.y));
            assert!(Kind::ALL.contains(&b.kind) && b.kind != Kind::StormGust, "only kinds the book can cast");
        }
        // A newcomer learns of them.
        let mut join = Vec::new();
        s.join(profile(2), Uuid::nil(), None, 0, &mut join);
        let told = msgs(&join, 2).into_iter().filter(|m| matches!(m, ServerMsg::Bonus { .. })).count();
        assert_eq!(told, s.bonuses.live.len());
        // Untouched, they burst when their time is up.
        let first_id = s.bonuses.live[0].id;
        let mut gone = false;
        for _ in 0..s.ticks_of(LIFE_SECS) {
            out.clear();
            s.tick(&mut out);
            gone |= msgs(&out, 1).iter().any(|m| matches!(m, ServerMsg::BonusGone { id, entity_id: 0, .. } if *id == first_id));
        }
        assert!(gone, "an untouched bonus bursts");
    }

    #[test]
    fn a_standing_player_takes_zeny_and_the_bag_gets_it() {
        let mut s = stage("{\"skills\":[]}", &[1]);
        let (x, y) = (s.players[0].x, s.players[0].y);
        let id = put(&mut s, Kind::Zeny, x + 0.5, y);
        let mut out = Vec::new();
        s.tick(&mut out);
        let mine = msgs(&out, 1);
        assert!(mine.iter().any(|m| matches!(m, ServerMsg::BonusGone { id: i, entity_id: 101, .. } if *i == id)));
        let Some(ServerMsg::Loot { item, count, entity_id, .. }) = mine.iter().find(|m| matches!(m, ServerMsg::Loot { .. })) else {
            panic!("a zeny floater");
        };
        assert_eq!((item.as_str(), *entity_id), ("zeny", 101));
        assert!((24..=48).contains(count), "level 11 location: {count}");
        let paid: u64 = out.iter().filter_map(|(u, e)| matches!(e, HubEvent::Zeny { .. }).then_some(*u)).count() as u64;
        assert_eq!(paid, 1, "reported with the progress");
        assert!(s.bonuses.live.is_empty());
    }

    #[test]
    fn the_downed_and_the_dead_cannot_take_one() {
        let mut s = stage("{\"skills\":[]}", &[1, 2]);
        // Player 2 stands far away so player 1 is not revived.
        let i2 = idx(&s, 2);
        s.players[i2].x += 15.0;
        let i1 = idx(&s, 1);
        let (x, y) = (s.players[i1].x, s.players[i1].y);
        put(&mut s, Kind::Zeny, x, y);
        for state in [Vital::Downed { ticks: 400 }, Vital::Dead] {
            s.players[i1].fighter.as_mut().unwrap().state = state;
            let mut out = Vec::new();
            s.tick(&mut out);
            assert_eq!(s.bonuses.live.len(), 1, "{state:?} leaves it");
            assert!(!msgs(&out, 1).iter().any(|m| matches!(m, ServerMsg::BonusGone { .. })));
        }
        s.players[i1].fighter.as_mut().unwrap().state = Vital::Up;
        let mut out = Vec::new();
        s.tick(&mut out);
        assert!(s.bonuses.live.is_empty(), "back up, it is taken");
    }

    #[test]
    fn restore_heals_and_berserk_is_a_timed_buff() {
        let mut s = stage("{\"skills\":[]}", &[1]);
        let i = idx(&s, 1);
        let (x, y) = (s.players[i].x, s.players[i].y);
        {
            let f = s.players[i].fighter.as_mut().unwrap();
            f.hp = 1;
            f.sp = 0;
        }
        put(&mut s, Kind::Restore, x, y);
        let mut out = Vec::new();
        s.tick(&mut out);
        let f = s.players[i].fighter.as_ref().unwrap();
        assert!(f.hp >= 14 && f.sp >= 6, "hp {} sp {}", f.hp, f.sp);
        let healed = msgs(&out, 1).iter().any(|m| {
            matches!(m, ServerMsg::Combat { hits, .. } if hits.iter().any(|h| h.target == 101 && h.flags & hit::HEAL != 0))
        });
        assert!(healed, "the heal shows as a number");

        put(&mut s, Kind::Berserk, x, y);
        let interval = s.players[i].fighter.as_ref().unwrap().mods.interval();
        let mut out = Vec::new();
        s.tick(&mut out);
        s.tick(&mut out);
        assert!(s.players[i].fighter.as_ref().unwrap().mods.interval() < interval, "attacks faster");
        let shown = msgs(&out, 1)
            .iter()
            .any(|m| matches!(m, ServerMsg::Buffs { buffs } if buffs.iter().any(|(id, ms)| id == "bonus:berserk" && *ms > 0)));
        assert!(shown, "the HUD shows it with its time left");
        let (_, secs) = berserk(11);
        for _ in 0..s.ticks_of(secs) + 2 {
            s.tick(&mut out);
        }
        assert_eq!(s.players[i].fighter.as_ref().unwrap().mods.interval(), interval, "and it runs out");
    }

    #[test]
    fn a_nova_bonus_casts_the_skill_for_free_as_the_player() {
        let mut s = stage(BOOK, &[1]);
        let i = idx(&s, 1);
        let (x, y) = (s.players[i].x, s.players[i].y);
        let slot = s.horde_mut().unwrap().spawn(0, x + 2.0, y).unwrap();
        s.players[i].fighter.as_mut().unwrap().sp = 0;
        put(&mut s, Kind::Nova, x, y);
        let mut out = Vec::new();
        s.tick(&mut out);
        let cast = msgs(&out, 1).into_iter().find_map(|m| match m {
            ServerMsg::Cast { caster, skill, cast_ms, cooldown_ms, level, .. } => Some((caster, skill, cast_ms, cooldown_ms, level)),
            _ => None,
        });
        assert_eq!(cast, Some((101, "magnum_break".to_string(), 0, 0, 2)), "free and instant, at the location's level");
        for _ in 0..3 {
            s.tick(&mut out);
        }
        let hit = msgs(&out, 1).iter().any(|m| {
            matches!(m, ServerMsg::Combat { hits, .. } if hits.iter().any(|h| h.target == MONSTER_ENTITY_BASE + slot as u32))
        });
        assert!(hit, "the blast reaches the monster");
        let f = s.players[i].fighter.as_ref().unwrap();
        assert!(f.cooldowns.iter().all(|&c| c == 0), "no cooldown started");
        let lives_by_player = s.lives.iter().all(|l| l.caster == 1);
        assert!(lives_by_player, "its areas are the player's (kills and drops credit them)");
    }

    #[test]
    fn the_arrow_spiral_is_released_over_time_and_traps_scatter() {
        let mut s = stage(BOOK, &[1]);
        let i = idx(&s, 1);
        let (x, y) = (s.players[i].x, s.players[i].y);
        put(&mut s, Kind::ArrowSpiral, x, y);
        let mut per_tick = Vec::new();
        let mut dirs = Vec::new();
        for _ in 0..s.ticks_of(ARROW_EVERY_SECS) * arrows(11) + 4 {
            let mut out = Vec::new();
            s.tick(&mut out);
            let casts: Vec<(f32, f32)> = msgs(&out, 1)
                .into_iter()
                .filter_map(|m| match m {
                    ServerMsg::Cast { skill, x: ax, y: ay, .. } if skill == "charge_arrow" => Some((ax, ay)),
                    _ => None,
                })
                .collect();
            per_tick.push(casts.len());
            dirs.extend(casts);
        }
        assert_eq!(dirs.len() as u32, arrows(11), "every arrow flies");
        assert!(per_tick.iter().all(|&n| n <= 1), "one at a time: {per_tick:?}");
        let angle = |&(ax, ay): &(f32, f32)| (ay - y).atan2(ax - x);
        let turns: Vec<f32> = dirs.windows(2).map(|w| (angle(&w[1]) - angle(&w[0])).rem_euclid(TAU)).collect();
        assert!(turns.iter().all(|t| (t - ARROW_TURN).abs() < 0.1), "along a spiral: {turns:?}");

        let mut s = stage(BOOK, &[1]);
        put(&mut s, Kind::Traps, x, y);
        let mut out = Vec::new();
        s.tick(&mut out);
        let traps_cast = msgs(&out, 1)
            .iter()
            .filter(|m| matches!(m, ServerMsg::Cast { skill, areas, .. } if skill == "ankle_snare" && areas[0].id > 0))
            .count();
        assert_eq!(traps_cast as u32, traps(11), "traps are lasting ground with ids");
        assert_eq!(s.lives.iter().filter(|l| l.trap.is_some()).count() as u32, traps(11));
    }
}
