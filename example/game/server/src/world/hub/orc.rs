//! Orc Village (mission `orc_village`, docs/locations/orc_village.md §3): the
//! location's own mechanics, owned by agent ORC, hooked in through the
//! `orc_village` fences of `hub/env.rs` (the per-tick step and the banner's
//! `object_hook`). The war horn and its arrow volleys are the platform's
//! (`missions.rs` `ORC_ENV`: a map event with strikes); on top of them:
//!
//! * **War banners**: banner objects (a plant, five blows) on the map's
//!   banner spots (`missions.rs` `ORC_BANNERS`), and any the Orc Hero plants. Once a
//!   second every orc and goblin (not elites or bosses) within [`RALLY_R`]
//!   of a standing banner is spurred ([`Rally`]: +[`RALLY_HIT`] to hit,
//!   +[`RALLY_SPEED`] % speed) for a little longer than that second, so the
//!   spur ends soon after it walks out or the banner falls. Clients see each
//!   banner's reach as a thin red ring on the ground (`BOSS_ZONES` caster 0,
//!   ability [`RALLY_ABILITY`]) until it is felled.
//! * **The horn's column**: when a war horn begins (on schedule, or forced
//!   by the Orc Lord's Call the Clans), a formation marches in from one map
//!   side ([`horn_side`], hashed from the horn's start second; clients
//!   compute the same and show the side): two ranks of Orc Warriors with
//!   Archers behind, a High Orc in the rear rank from the third horn on,
//!   [`COLUMN_BASE`] + [`COLUMN_PER`] per extra fighter bodies,
//!   [`COLUMN_NEAR`]..[`COLUMN_FAR`] cells from the party toward that side.
//!   They are not bought from the wave budget. Every banner spot without a
//!   standing banner is replanted.
//! * **Call the Clans**: when the Orc Lord drops under [`CLANS_PCT`] % (his
//!   third phase opens with Call the Clans) a war horn is forced.
//! * When the Orc Lord falls, the banners fall with him.
//!
//! The state is a few small vectors; a quiet tick allocates nothing.

use super::area::Area;
use super::{HubInstance, MapIdx, Outgoing};
use crate::world::combat::Archetype;
use crate::world::environment::ObjectEffect;
use crate::world::horde::storage::Rally;
use crate::world::protocol::{look, BossZone, ServerMsg, Telegraph};
use tracing::info;

/// The mission this module runs on.
pub(super) const MISSION: &str = "orc_village";
/// Object hook id of a felled war banner (`missions.rs`).
pub(super) const BANNER_HOOK: &str = "orc_banner";
/// The banner monster (recipe `[monsters.orc_banner]`).
pub(super) const BANNER: &str = "orc_banner";
/// The war horn map event (`missions.rs` `WAR_HORN`).
pub(super) const HORN: &str = "war_horn";
/// Effects key of a banner's reach on the ground (effects.toml).
pub(super) const RALLY_ABILITY: &str = "orc_rally";
/// Effects key of the horn blown where the column forms (a red ground
/// mark there for a few seconds; its sound comes from that side).
pub(super) const HORN_ABILITY: &str = "orc_horn_call";
const HORN_MARK_R: f32 = 5.0;
const HORN_MARK_MS: u32 = 8000;
/// The Orc Lord's Call the Clans (effects key; it forces a horn).
pub(crate) const CALL_THE_CLANS: &str = "boss_orc_clans";
/// A banner's reach (cells; 7 m) and what it gives.
pub(super) const RALLY_R: f32 = 14.0;
pub(super) const RALLY_HIT: i8 = 2;
pub(super) const RALLY_SPEED: u8 = 20;
/// The spur lasts this long; banners renew it every second.
const RALLY_SECS: f32 = 1.6;
/// Width of the ring drawn at its reach (cells).
const RING_W: f32 = 0.9;
/// The kinds a banner rallies (the warband and its goblins).
pub(super) const RALLIED: [&str; 7] = ["orc_baby", "ork_warrior", "goblin_1", "orc_lady", "orc_archer", "zenorc", "high_orc"];
/// The horn's column: bodies for one fighter, more per extra one, at most.
pub(super) const COLUMN_BASE: u32 = 8;
pub(super) const COLUMN_PER: u32 = 4;
const COLUMN_MAX: u32 = 28;
/// How far from the party's centre it forms up (cells).
pub(super) const COLUMN_NEAR: f32 = 30.0;
pub(super) const COLUMN_FAR: f32 = 36.0;
/// Spacing inside a rank and between ranks (cells).
const FILE_GAP: f32 = 1.5;
const RANK_GAP: f32 = 1.8;
/// The column's kinds.
const WARRIOR: &str = "ork_warrior";
const ARCHER: &str = "orc_archer";
const HEAVY: &str = "high_orc";
/// A High Orc marches with the column from this horn on (1-based).
const HEAVY_FROM: u32 = 3;
/// The Orc Lord, and the HP % under which he calls the clans.
const LORD: &str = "orc_lord";
pub(super) const CLANS_PCT: i32 = 40;

/// The four sides a column comes from (unit vectors, cells: +y is north),
/// counter-clockwise from the east, and their names.
pub(super) const SIDES: [(f32, f32); 4] = [(1.0, 0.0), (0.0, 1.0), (-1.0, 0.0), (0.0, -1.0)];
pub(super) const SIDE_NAMES: [&str; 4] = ["east", "north", "west", "south"];

/// The side the horn that began at `start_secs` calls its column from
/// (index into [`SIDES`]): a hash of the start second, so every horn
/// differs and clients (engine `orc.ts`) compute the same from the event's
/// `start_ms` without another message.
pub(super) fn horn_side(start_secs: f32) -> usize {
    let s = start_secs.round().max(0.0) as u32;
    (s.wrapping_mul(2_654_435_761) >> 30) as usize
}

#[derive(Clone, Copy, Debug)]
struct Banner {
    slot: u16,
    generation: u8,
    /// Its reach ring on clients.
    zone: u32,
}

/// The war camp's state on one Orc Village stage.
#[derive(Debug, Default)]
pub(super) struct Camp {
    banners: Vec<Banner>,
    /// Where the map's banners stood at the start (the mission's banner
    /// spots, as planted): what a horn replants.
    homes: Vec<(f32, f32)>,
    homes_taken: bool,
    /// Next run second the banners rally.
    next_rally: f32,
    /// Start of the last horn whose column marched in, and how many have.
    horn_seen: Option<f32>,
    horns: u32,
    /// The Orc Lord on the field (slot, generation), and whether he has
    /// called the clans.
    lord: Option<(u16, u8)>,
    clans_called: bool,
    /// Scratch for monster queries.
    near: Vec<u16>,
    seen: Vec<(u16, u8, f32, f32)>,
}

impl Camp {
    /// Standing banners being tracked (tests).
    #[cfg(test)]
    pub fn banners(&self) -> usize {
        self.banners.len()
    }

    /// Columns that marched in so far (tests).
    #[cfg(test)]
    pub fn horns(&self) -> u32 {
        self.horns
    }
}

impl HubInstance {
    /// Once per tick on an Orc Village stage.
    pub(super) fn orc_step(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        if let Some(start) = self.env_event_start(HORN)
            && self.env.orc.horn_seen != Some(start)
        {
            self.env.orc.horn_seen = Some(start);
            self.env.orc.horns += 1;
            self.orc_replant();
            self.orc_column(start, out);
        }
        if now < self.env.orc.next_rally {
            return;
        }
        self.env.orc.next_rally = now + 1.0;
        self.orc_lord(map, now, out);
        self.orc_banners(map, out);
        self.orc_rally();
    }

    /// A war banner was felled at `(x, y)`: its ring goes on the next scan
    /// (the loot is its drop table's).
    pub(super) fn orc_banner_felled(&mut self, _map: MapIdx, x: f32, y: f32, _out: &mut Outgoing) {
        info!("{}: a war banner falls at ({x:.0}, {y:.0})", self.name);
        // Look again at once: the rally must not outlive the banner by a second.
        self.env.orc.next_rally = 0.0;
    }

    /// Tracks the standing banners (the map's and the Orc Hero's): a ring
    /// shows for each new one, and goes with each felled one.
    fn orc_banners(&mut self, map: MapIdx, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_ref() else { return };
        let Some(kind) = horde.catalog().kind_of(BANNER) else { return };
        let mut seen = std::mem::take(&mut self.env.orc.seen);
        seen.clear();
        seen.extend(horde.monsters().filter(|m| m.2 == kind && horde.store().hp[m.0 as usize] > 0).map(|m| (m.0, m.1, m.3, m.4)));
        if !self.env.orc.homes_taken {
            self.env.orc.homes_taken = true;
            self.env.orc.homes.extend(seen.iter().map(|s| (s.2, s.3)));
        }
        let span = crate::world::missions::find(MISSION).map_or(900.0, |m| m.time_limit_secs) * 1000.0;
        // Felled ones.
        let mut k = 0;
        while k < self.env.orc.banners.len() {
            let b = self.env.orc.banners[k];
            if seen.iter().any(|s| s.0 == b.slot && s.1 == b.generation) {
                k += 1;
                continue;
            }
            self.env.orc.banners.swap_remove(k);
            self.broadcast(map, &ServerMsg::ZoneEnd { id: b.zone }, out);
        }
        // New ones.
        let mut zones = Vec::new();
        for &(slot, generation, x, y) in &seen {
            if self.env.orc.banners.iter().any(|b| b.slot == slot && b.generation == generation) {
                continue;
            }
            let zone = self.next_zone;
            self.next_zone = self.next_zone.wrapping_add(1).max(1);
            self.env.orc.banners.push(Banner { slot, generation, zone });
            let area = Telegraph { id: zone, shape: 3, look: look::TAUNT, x, y, dir: 0, a: RALLY_R, b: RALLY_R - RING_W, span_ms: span as u32, follow: 0 };
            zones.push(BossZone { area, delay_ms: 0, flags: 0 });
        }
        self.env.orc.seen = seen;
        if !zones.is_empty() {
            self.broadcast(map, &ServerMsg::BossZones { caster: 0, ability: RALLY_ABILITY.into(), zones }, out);
        }
    }

    /// Every rallied kind within reach of a standing banner is spurred.
    fn orc_rally(&mut self) {
        if self.env.orc.banners.is_empty() {
            return;
        }
        let hz = self.config.tick_hz.max(1) as f32;
        let Some(horde) = self.horde.as_mut() else { return };
        let ticks = (RALLY_SECS * hz).round().clamp(1.0, u16::MAX as f32) as u16;
        let spur = Rally { to_hit: RALLY_HIT, speed: RALLY_SPEED, taken: 0, ticks };
        let near = &mut self.env.orc.near;
        for b in &self.env.orc.banners {
            let s = b.slot as usize;
            let (x, y) = (horde.store().x[s], horde.store().y[s]);
            near.clear();
            Area::Circle { x, y, r: RALLY_R }.monsters(horde, near);
            for &m in near.iter() {
                let def = horde.catalog().get(horde.store().kind[m as usize]);
                if horde.is_plant(m) || def.boss || def.archetype == Archetype::Elite || !RALLIED.contains(&def.id.as_str()) {
                    continue;
                }
                horde.store_mut().rally(m, spur);
            }
        }
    }

    /// Every banner spot without a standing banner gets one again.
    fn orc_replant(&mut self) {
        let Some(horde) = self.horde.as_ref() else { return };
        let Some(kind) = horde.catalog().kind_of(BANNER) else { return };
        let standing: Vec<(f32, f32)> = horde.monsters().filter(|m| m.2 == kind).map(|m| (m.3, m.4)).collect();
        let mut planted = 0;
        let homes = std::mem::take(&mut self.env.orc.homes);
        for &(x, y) in &homes {
            if standing.iter().any(|s| (s.0 - x).powi(2) + (s.1 - y).powi(2) < 4.0) {
                continue;
            }
            if self.env_plant(BANNER, x, y, ObjectEffect::Hook(BANNER_HOOK)).is_some() {
                planted += 1;
            }
        }
        self.env.orc.homes = homes;
        if planted > 0 {
            info!("{}: the horn replants {planted} war banner(s)", self.name);
            self.env.orc.next_rally = 0.0;
        }
    }

    /// The horn's column forms up on the side the horn named.
    fn orc_column(&mut self, start: f32, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_ref() else { return };
        if horde.spawning_closed() {
            return;
        }
        let map = horde.map();
        let ups: Vec<(f32, f32)> = self.players.iter().filter(|p| p.map == map && p.fighter.as_ref().is_some_and(|f| f.hunted())).map(|p| (p.x, p.y)).collect();
        if ups.is_empty() {
            return;
        }
        let n = ups.len() as f32;
        let (cx, cy) = (ups.iter().map(|p| p.0).sum::<f32>() / n, ups.iter().map(|p| p.1).sum::<f32>() / n);
        let side = horn_side(start);
        let horn = self.env.orc.horns;
        let bodies = (COLUMN_BASE + COLUMN_PER * (ups.len() as u32 - 1)).min(COLUMN_MAX);
        let walk = &self.world.map(map).walk;
        // Where it forms: toward the side, else turned a little, nearer or farther.
        let mut origin = None;
        'find: for turn in [0.0f32, 25.0, -25.0, 50.0, -50.0] {
            let (s, c) = turn.to_radians().sin_cos();
            let (dx, dy) = SIDES[side];
            let d = (dx * c - dy * s, dx * s + dy * c);
            for dist in [33.0, COLUMN_NEAR, COLUMN_FAR, 27.0, 40.0] {
                let (x, y) = (cx + d.0 * dist, cy + d.1 * dist);
                if walk.is_walkable(x, y) {
                    origin = Some((x, y, d));
                    break 'find;
                }
            }
        }
        let Some((ox, oy, d)) = origin else {
            info!("{}: no ground for the horn's column toward the {}", self.name, SIDE_NAMES[side]);
            return;
        };
        let catalog = horde.catalog();
        let (Some(warrior), archer, heavy) = (catalog.kind_of(WARRIOR), catalog.kind_of(ARCHER), catalog.kind_of(HEAVY)) else { return };
        let heavy = heavy.filter(|_| horn >= HEAVY_FROM);
        let archers = if archer.is_some() { (bodies / 4).max(2) } else { 0 };
        let warriors = bodies - archers - heavy.map_or(0, |_| 1);
        // Front rank (nearest the party), second rank, then the archers
        // (and the heavy in the middle of the rear rank).
        let front = warriors.div_ceil(2);
        let ranks: [(Option<u8>, u32); 3] = [(Some(warrior), front), (Some(warrior), warriors - front), (archer, archers)];
        let across = (-d.1, d.0);
        let Some(horde) = self.horde.as_mut() else { return };
        let mut placed = 0;
        for (r, &(kind, count)) in ranks.iter().enumerate() {
            let Some(kind) = kind else { continue };
            let (rx, ry) = (ox + d.0 * RANK_GAP * r as f32, oy + d.1 * RANK_GAP * r as f32);
            let mut files: Vec<u8> = vec![kind; count as usize];
            if r == 2
                && let Some(h) = heavy
            {
                files.insert(files.len() / 2, h);
            }
            let half = (files.len() as f32 - 1.0) / 2.0;
            for (i, &k) in files.iter().enumerate() {
                let off = (i as f32 - half) * FILE_GAP;
                let (x, y) = (rx + across.0 * off, ry + across.1 * off);
                if horde.spawn(k, x, y).or_else(|| horde.spawn(k, ox, oy)).is_some() {
                    placed += 1;
                }
            }
        }
        info!("{}: war horn #{horn}: a column of {placed} from the {} at ({ox:.0}, {oy:.0})", self.name, SIDE_NAMES[side]);
        let zone = self.next_zone;
        self.next_zone = self.next_zone.wrapping_add(1).max(1);
        let (mx, my) = (ox + d.0 * RANK_GAP, oy + d.1 * RANK_GAP);
        let area = Telegraph { id: zone, shape: 0, look: look::TAUNT, x: mx, y: my, dir: 0, a: HORN_MARK_R, b: 0.0, span_ms: HORN_MARK_MS, follow: 0 };
        self.broadcast(map, &ServerMsg::BossZones { caster: 0, ability: HORN_ABILITY.into(), zones: vec![BossZone { area, delay_ms: 0, flags: 0 }] }, out);
    }

    /// The Orc Lord calls the clans under [`CLANS_PCT`] %; when he falls,
    /// the banners fall with him.
    fn orc_lord(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_mut() else { return };
        let Some(kind) = horde.catalog().kind_of(LORD) else { return };
        match self.env.orc.lord {
            Some((slot, generation)) => {
                let store = horde.store();
                let alive = store.is_alive(slot) && store.generation[slot as usize] == generation && store.hp[slot as usize] > 0;
                if !alive {
                    self.env.orc.lord = None;
                    let banner = horde.catalog().kind_of(BANNER);
                    let fallen: Vec<u16> = horde.monsters().filter(|m| Some(m.2) == banner).map(|m| m.0).collect();
                    for b in &fallen {
                        horde.despawn(*b);
                    }
                    info!("{}: the Orc Lord falls, and {} banner(s) with him", self.name, fallen.len());
                    return;
                }
                let max = horde.block(slot).hp.max(1);
                if !self.env.orc.clans_called && store.hp[slot as usize] * 100 < max * CLANS_PCT {
                    self.env.orc.clans_called = true;
                    let forced = self.env_force_event(map, HORN, now, out);
                    info!("{}: the Orc Lord calls the clans (horn forced: {forced})", self.name);
                }
            }
            None => {
                if let Some(m) = horde.monsters().find(|m| m.2 == kind) {
                    self.env.orc.lord = Some((m.0, m.1));
                    self.env.orc.clans_called = false;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "orc_tests.rs"]
mod tests;
