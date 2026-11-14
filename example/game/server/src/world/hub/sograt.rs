//! Sograt Desert's own stage mechanics (example/docs/locations/sograt.md §3),
//! on top of the mission platform (`env.rs` runs the sandstorm's light and
//! warnings, the pits and the sinkhole):
//!
//! * **Sand-blast**: while a sandstorm runs, a player standing in the open
//!   takes a pulse every [`BLAST_EVERY`] s (a Reflex save, or damage and
//!   slowed). Standing in the lee of anything solid (a blocked cell within
//!   [`LEE_CELLS`] upwind: ruins, bones, cacti, cliffs) shelters you. Each
//!   occurrence blows from its own direction ([`wind_dir`]); clients derive
//!   the same direction from the event's start for their sand streaks.
//! * **Hodes travel buried** (the generic burrowed state, `Horde::set_burrowed`):
//!   under the sand until a player is within reach, then they surface to
//!   strike, and dive again once nobody is close.
//! * **Wind drift**: while a storm runs, what a player has sent flying
//!   (bolts, arrows, anything landing later than the cast) drifts
//!   [`DRIFT`] cells/s downwind; homing hits still find their target.
//! * **Phreeoni rises from the great sinkhole**: the first time the boss is
//!   on the field it is moved into the sinkhole's middle and the ground
//!   shakes. Under 40 % HP it roars a sandstorm up at once (`env_force_event`).
//! * **Muka thorns**: a blow on a Muka cactus pricks the nearest player in
//!   melee reach of it.
//! * **Lone Wolf**: once Sirocco has fought a while and no wolf of its pack
//!   is left around it, it enrages ahead of its timer.
//! * **Crumble**: a fallen Sand Colossus leaves a pool of quicksand (pull,
//!   mired) where it stood.
//!
//! The state is a few small vectors scanned every few ticks.

use super::aim::BODY;
use super::cast::{ticks_of, Follow};
use super::fight::damage_type_bits;
use super::{HubInstance, InstanceKind, MapIdx, Outgoing};
use crate::world::combat::rules::saving_throw;
use crate::world::combat::{Condition, DamageType, Dice};
use crate::world::map::WalkGrid;
use crate::world::missions;
use crate::world::protocol::{look, BossZone, HitEvent, ServerMsg, Telegraph, MONSTER_ENTITY_BASE};
use tracing::info;

/// The mission this module runs on.
pub(super) const MISSION: &str = "sograt_desert";
/// Map event id of the sandstorm (`missions.rs`).
pub(super) const SANDSTORM: &str = "sandstorm";
/// A sand-blast pulse this often while a storm runs (seconds).
pub(super) const BLAST_EVERY: f32 = 3.0;
/// Cells upwind a blocked cell shelters (3 m).
pub(super) const LEE_CELLS: i32 = 6;
/// Blast: dice + band min / 4, slowed this long on a failed save.
const BLAST_DICE: (u32, u32) = (1, 6);
const BLAST_SLOW_SECS: f32 = 2.0;
/// Hodes surface within this many cells of a standing player, stay up at
/// least `HODE_UP_SECS`, and dive again when nobody is within `HODE_DIVE`.
const HODE_SURFACE: f32 = 3.0;
const HODE_DIVE: f32 = 6.0;
const HODE_UP_SECS: f32 = 4.0;
/// Scan the field for hodes and the boss this often (ticks).
const SCAN_TICKS: u32 = 3;
/// The great sinkhole, Phreeoni's lair (cells, `recipe.toml` hazards).
pub(super) const SINKHOLE: (f32, f32) = (209.0, 235.0);
/// Cells per second a flying spell drifts downwind in a storm.
pub(super) const DRIFT: f32 = 1.5;
/// Muka thorns: dice + band min / 4 on whoever struck it, within this
/// many cells (melee reach).
const THORN_DICE: (u32, u32) = (1, 4);
const THORN_REACH: f32 = 2.5;
/// Sirocco turns Lone Wolf after fighting this long with no pack wolf
/// within `PACK_RANGE` cells.
const LONE_AFTER_SECS: f32 = 20.0;
const PACK_RANGE: f32 = 30.0;
const LONE_WOLF: &str = "My pack... gone. Then I hunt ALONE!";
/// Phreeoni calls the storm at this share of its HP (%).
const ROAR_BELOW: i64 = 40;
/// The Sand Colossus' crumbled quicksand: radius (cells), seconds, pull
/// (cells/s), the mired pulse.
const CRUMBLE_R: f32 = 8.0;
const CRUMBLE_SECS: f32 = 8.0;
const CRUMBLE_PULL: f32 = 1.0;
const CRUMBLE_PULSE: f32 = 0.5;

/// The 8 wind directions a sandstorm blows toward (unit vectors, cells).
const WINDS: [(f32, f32); 8] = [
    (1.0, 0.0),
    (0.7071, 0.7071),
    (0.0, 1.0),
    (-0.7071, 0.7071),
    (-1.0, 0.0),
    (-0.7071, -0.7071),
    (0.0, -1.0),
    (0.7071, -0.7071),
];

/// Where the storm that began at `start_secs` blows (index into the 8
/// directions, 0 = east, counter-clockwise). A hash of the start second, so
/// every occurrence differs and clients (engine `sograt.ts`) compute the
/// same one from the event's `start_ms` without another message.
pub(super) fn wind_dir(start_secs: f32) -> usize {
    let s = start_secs.round().max(0.0) as u32;
    (s.wrapping_mul(2_654_435_761) >> 29) as usize
}

/// Whether `(x, y)` is in the lee of something solid against a wind blowing
/// toward `wind`: a blocked cell within [`LEE_CELLS`] upwind.
pub(super) fn sheltered(walk: &WalkGrid, x: f32, y: f32, wind: (f32, f32)) -> bool {
    // Three rays upwind (the middle and half a cell to either side) in
    // half-cell steps: a diagonal wind does not slip between cells, and a
    // lone obstacle shelters a lane about a metre wide (the engine's
    // `sandstorm.ts` checks the same way for its calm tint).
    [0.0f32, -0.5, 0.5].iter().any(|&side| {
        let (ox, oy) = (x - wind.1 * side, y + wind.0 * side);
        (1..=LEE_CELLS * 2).any(|k| {
            let (ux, uy) = (ox - wind.0 * k as f32 * 0.5, oy - wind.1 * k as f32 * 0.5);
            !walk.is_walkable_cell(ux.floor() as i32, uy.floor() as i32)
        })
    })
}

/// Quicksand a fallen Sand Colossus left (cells, run seconds).
#[derive(Clone, Copy, Debug)]
struct Pool {
    x: f32,
    y: f32,
    until: f32,
    next: f32,
}

/// The desert's state on one stage.
#[derive(Debug, Default)]
pub(super) struct Desert {
    /// Next sand-blast pulse (run seconds).
    blast_at: f32,
    /// Hodes on the field: (slot, generation, run second it may dive again).
    hodes: Vec<(u16, u8, f32)>,
    /// The boss has risen (it is moved into the sinkhole once).
    risen: bool,
    /// Muka cacti: (slot, generation, HP last seen).
    mukas: Vec<(u16, u8, i32)>,
    /// Sirocco: (slot, generation, run second first seen, lone already).
    alpha: Option<(u16, u8, f32, bool)>,
    /// Sand Colossi: (slot, generation, last x, last y).
    colossi: Vec<(u16, u8, f32, f32)>,
    pools: Vec<Pool>,
    /// The Phreeoni that already roared its storm: (slot, generation).
    roared: Option<(u16, u8)>,
}

impl Desert {
    /// The desert's state for a stage of the Sograt mission, else `None`.
    pub(super) fn for_stage(kind: &InstanceKind) -> Option<Desert> {
        matches!(kind, InstanceKind::Stage { mission, .. } if mission == MISSION).then(Desert::default)
    }
}

impl HubInstance {
    /// Once per tick after the environment (`env.rs`).
    pub(super) fn desert_step(&mut self, out: &mut Outgoing, hits: &mut Vec<HitEvent>) {
        let Some(mut d) = self.desert.take() else { return };
        if self.outcome == super::mission::Outcome::Running && self.horde.is_some() {
            self.sand_blast(&mut d, hits);
            self.drift();
            self.thorns(&mut d, hits);
            self.pools(&mut d);
            if self.tick.is_multiple_of(SCAN_TICKS) {
                self.hodes(&mut d);
                self.boss_rises(&mut d, out);
                self.elites(&mut d, out);
            }
        }
        self.desert = Some(d);
    }

    /// The storm blasts whoever stands in the open.
    fn sand_blast(&mut self, d: &mut Desert, hits: &mut Vec<HitEvent>) {
        let Some(horde) = self.horde.as_ref() else { return };
        let (map, now) = (horde.map(), horde.director_elapsed());
        let Some(m) = missions::find(MISSION) else { return };
        // Scheduled or roared up by Phreeoni.
        let Some(start) = self.env_event_start(SANDSTORM) else {
            d.blast_at = 0.0;
            return;
        };
        if d.blast_at == 0.0 {
            // The first pulse a little after it begins: time to find cover.
            d.blast_at = now + BLAST_EVERY;
        }
        if now < d.blast_at {
            return;
        }
        d.blast_at = now + BLAST_EVERY;
        let wind = WINDS[wind_dir(start)];
        let hz = self.config.tick_hz.max(1) as f32;
        let level = m.levels.min;
        let dc = 12 + (level / 8) as i32;
        let walk = &self.world.map(map).walk;
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let Some(f) = p.fighter.as_mut() else { continue };
            if !f.is_up() || sheltered(walk, p.x, p.y, wind) {
                continue;
            }
            let saved = !f.status.saves_fail() && saving_throw(&mut self.dice, f.save_against(Some(Condition::Slowed)), dc, 0);
            if saved {
                continue;
            }
            let amount = Dice::new(BLAST_DICE.0, BLAST_DICE.1, (level / 4) as i32).roll(&mut self.dice, false);
            f.quiet = 0;
            let taken = f.take_from(amount, true, false);
            hits.push(HitEvent { target: p.profile.character_id, amount: taken as u32, flags: damage_type_bits(DamageType::Physical) });
            f.afflict(Condition::Slowed, ticks_of(BLAST_SLOW_SECS, hz).min(u16::MAX as u32) as u16);
            f.fall_if_spent(hz);
        }
    }

    /// Hodes: buried while travelling, up to strike.
    fn hodes(&mut self, d: &mut Desert) {
        let Some(horde) = self.horde.as_mut() else { return };
        let Some(kind) = horde.catalog().kind_of("hode") else { return };
        let (map, now) = (horde.map(), horde.director_elapsed());
        // Standing players only: the downed are not hunted.
        let ups: Vec<(f32, f32)> =
            self.players.iter().filter(|p| p.map == map && p.fighter.as_ref().is_some_and(|f| f.is_up())).map(|p| (p.x, p.y)).collect();
        // Forget the fallen, learn the new (they arrive buried).
        let store = horde.store();
        d.hodes.retain(|&(s, g, _)| store.is_alive(s) && store.generation[s as usize] == g);
        let fresh: Vec<u16> = store
            .alive()
            .iter()
            .copied()
            .filter(|&s| store.kind[s as usize] == kind && !d.hodes.iter().any(|h| h.0 == s))
            .collect();
        for s in fresh {
            let g = horde.store().generation[s as usize];
            horde.set_burrowed(s, true);
            d.hodes.push((s, g, 0.0));
        }
        for h in d.hodes.iter_mut() {
            let s = h.0 as usize;
            let (x, y) = (horde.store().x[s], horde.store().y[s]);
            let near = ups.iter().map(|&(px, py)| (px - x).powi(2) + (py - y).powi(2)).fold(f32::INFINITY, f32::min).sqrt();
            let under = horde.store().burrowed(h.0);
            if under && near <= HODE_SURFACE {
                horde.set_burrowed(h.0, false);
                h.2 = now + HODE_UP_SECS;
            } else if !under && now >= h.2 && near > HODE_DIVE {
                horde.set_burrowed(h.0, true);
            }
        }
    }

    /// The storm carries flying spells downwind: every player hit still on
    /// its way (one-off, not homing, not a trap) moves a step a tick.
    fn drift(&mut self) {
        let Some(start) = self.env_event_start(SANDSTORM) else { return };
        let (wx, wy) = WINDS[wind_dir(start)];
        let step = DRIFT / self.config.tick_hz.max(1) as f32;
        let now = self.tick;
        for l in self.lives.iter_mut().filter(|l| l.next > now && l.pulse == 0 && l.trap.is_none() && matches!(l.follow, Follow::None)) {
            let (x, y) = l.area.centre();
            l.area = l.area.moved_to(x + wx * step, y + wy * step);
        }
    }

    /// Muka thorns: a blow on a cactus pricks the nearest player in reach.
    fn thorns(&mut self, d: &mut Desert, hits: &mut Vec<HitEvent>) {
        let Some(horde) = self.horde.as_ref() else { return };
        let Some(kind) = horde.catalog().kind_of("muka") else { return };
        let (map, store) = (horde.map(), horde.store());
        if self.tick.is_multiple_of(SCAN_TICKS) {
            d.mukas.retain(|&(s, g, _)| store.is_alive(s) && store.generation[s as usize] == g);
            for &s in store.alive() {
                if store.kind[s as usize] == kind && !d.mukas.iter().any(|m| m.0 == s) {
                    d.mukas.push((s, store.generation[s as usize], store.hp[s as usize]));
                }
            }
        }
        let level = missions::find(MISSION).map_or(1, |m| m.levels.min);
        let hz = self.config.tick_hz.max(1) as f32;
        for m in d.mukas.iter_mut() {
            let s = m.0 as usize;
            // Felled or swapped: its last blow still pricks.
            let hp = if store.generation[s] == m.1 { store.hp[s].max(0) } else { 0 };
            if hp >= m.2 {
                continue;
            }
            m.2 = hp;
            let (x, y) = (store.x[s], store.y[s]);
            let striker = self
                .players
                .iter_mut()
                .filter(|p| p.map == map && p.fighter.as_ref().is_some_and(|f| f.is_up()))
                .map(|p| ((p.x - x).powi(2) + (p.y - y).powi(2), p))
                .filter(|(d2, _)| *d2 <= THORN_REACH * THORN_REACH)
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, p)| p);
            let Some(p) = striker else { continue };
            let Some(f) = p.fighter.as_mut() else { continue };
            let amount = Dice::new(THORN_DICE.0, THORN_DICE.1, (level / 4) as i32).roll(&mut self.dice, false);
            f.quiet = 0;
            let taken = f.take_from(amount, true, false);
            hits.push(HitEvent { target: p.profile.character_id, amount: taken as u32, flags: damage_type_bits(DamageType::Physical) });
            f.fall_if_spent(hz);
        }
        d.mukas.retain(|m| m.2 > 0);
    }

    /// Crumbled quicksand: pulls whoever stands in it to the middle and
    /// mires them.
    fn pools(&mut self, d: &mut Desert) {
        let Some(horde) = self.horde.as_ref() else { return };
        let (map, now) = (horde.map(), horde.director_elapsed());
        d.pools.retain(|p| p.until > now);
        if d.pools.is_empty() {
            return;
        }
        let hz = self.config.tick_hz.max(1) as f32;
        let walk = &self.world.map(map).walk;
        for pool in d.pools.iter_mut() {
            let pulse = now >= pool.next;
            if pulse {
                pool.next = now + CRUMBLE_PULSE;
            }
            for p in self.players.iter_mut().filter(|p| p.map == map) {
                let Some(f) = p.fighter.as_mut() else { continue };
                let (dx, dy) = (pool.x - p.x, pool.y - p.y);
                let dist = (dx * dx + dy * dy).sqrt();
                if !f.is_up() || dist > CRUMBLE_R + BODY {
                    continue;
                }
                if dist > 0.05 {
                    let step = (CRUMBLE_PULL / hz).min(dist);
                    let (nx, ny) = (p.x + dx / dist * step, p.y + dy / dist * step);
                    if walk.is_walkable(nx, ny) {
                        (p.x, p.y) = (nx, ny);
                    }
                }
                if pulse {
                    f.afflict(Condition::Mired, ticks_of(CRUMBLE_PULSE * 2.0, hz).min(u16::MAX as u32) as u16);
                }
            }
        }
    }

    /// The elites' and the boss's own beats: Lone Wolf, the Colossus'
    /// crumble, Phreeoni's storm.
    fn elites(&mut self, d: &mut Desert, out: &mut Outgoing) {
        let Some(horde) = self.horde.as_ref() else { return };
        let (map, now) = (horde.map(), horde.director_elapsed());
        let cat = horde.catalog();
        let kind = |id: &str| cat.kind_of(id);
        let (alpha_k, colossus_k, boss_k) = (kind("sirocco"), kind("sand_colossus"), kind("phreeoni"));
        let pack = [kind("desert_wolf"), kind("desert_wolf_b")];
        let store = horde.store();

        // Sirocco: first seen, then alone.
        if let Some(a) = d.alpha
            && !(store.is_alive(a.0) && store.generation[a.0 as usize] == a.1)
        {
            d.alpha = None;
        }
        if d.alpha.is_none()
            && let Some(s) = store.alive().iter().copied().find(|&s| Some(store.kind[s as usize]) == alpha_k)
        {
            d.alpha = Some((s, store.generation[s as usize], now, false));
        }
        let mut lone = None;
        if let Some((s, g, seen, false)) = d.alpha
            && now - seen >= LONE_AFTER_SECS
        {
            let (x, y) = (store.x[s as usize], store.y[s as usize]);
            let wolves = store.alive().iter().any(|&w| {
                let k = Some(store.kind[w as usize]);
                pack.contains(&k) && k.is_some() && (store.x[w as usize] - x).powi(2) + (store.y[w as usize] - y).powi(2) <= PACK_RANGE * PACK_RANGE
            });
            if !wolves {
                lone = Some((s, g, seen));
            }
        }

        // Sand Colossi: remember where they stand; a fallen one crumbles.
        let mut crumbled = Vec::new();
        d.colossi.retain_mut(|c| {
            let s = c.0 as usize;
            if store.is_alive(c.0) && store.generation[s] == c.1 {
                (c.2, c.3) = (store.x[s], store.y[s]);
                return true;
            }
            // Killed (its generation moved on once, its HP spent), not cleared away.
            if store.generation[s] == c.1.wrapping_add(1) && store.hp[s] <= 0 {
                crumbled.push((c.2, c.3));
            }
            false
        });
        for &s in store.alive() {
            if Some(store.kind[s as usize]) == colossus_k && !d.colossi.iter().any(|c| c.0 == s) {
                d.colossi.push((s, store.generation[s as usize], store.x[s as usize], store.y[s as usize]));
            }
        }

        // Phreeoni under 40 %: the storm comes now.
        let mut roar = false;
        if let Some(s) = store.alive().iter().copied().find(|&s| Some(store.kind[s as usize]) == boss_k) {
            let g = store.generation[s as usize];
            let max = horde.block(s).hp.max(1) as i64;
            if d.roared != Some((s, g)) && store.hp[s as usize] as i64 * 100 <= ROAR_BELOW * max {
                d.roared = Some((s, g));
                roar = true;
            }
        }

        if let Some((s, g, seen)) = lone
            && self.bosses.enrage(s)
        {
            d.alpha = Some((s, g, seen, true));
            let shout = ServerMsg::BossShout { entity_id: MONSTER_ENTITY_BASE + s as u32, name: "Sirocco".into(), text: LONE_WOLF.into(), style: 0 };
            self.broadcast(map, &shout, out);
            self.env_shake(map, 50, 600, out);
        }
        for (x, y) in crumbled {
            self.crumble(d, map, x, y, now, out);
        }
        if roar && self.env_force_event(map, SANDSTORM, now, out) {
            info!("{}: Phreeoni roars a sandstorm up", self.name);
        }
    }

    /// A fallen Sand Colossus collapses into quicksand.
    fn crumble(&mut self, d: &mut Desert, map: MapIdx, x: f32, y: f32, now: f32, out: &mut Outgoing) {
        d.pools.push(Pool { x, y, until: now + CRUMBLE_SECS, next: now });
        let id = self.next_zone;
        self.next_zone = self.next_zone.wrapping_add(1).max(1);
        let area = Telegraph { id, shape: 0, look: look::SAND, x, y, dir: 0, a: CRUMBLE_R, b: 0.0, span_ms: (CRUMBLE_SECS * 1000.0) as u32, follow: 0 };
        let msg = ServerMsg::BossZones { caster: 0, ability: "boss_sand_vortex".into(), zones: vec![BossZone { area, delay_ms: 0, flags: 0 }] };
        self.broadcast(map, &msg, out);
        self.env_shake(map, 80, 700, out);
    }

    /// Phreeoni's arrival: out of the great sinkhole, with a quake.
    fn boss_rises(&mut self, d: &mut Desert, out: &mut Outgoing) {
        if d.risen {
            return;
        }
        let Some(horde) = self.horde.as_mut() else { return };
        let Some(kind) = horde.catalog().kind_of("phreeoni") else {
            d.risen = true;
            return;
        };
        let store = horde.store();
        let Some(slot) = store.alive().iter().copied().find(|&s| store.kind[s as usize] == kind) else { return };
        d.risen = true;
        let map = horde.map();
        if self.world.map(map).walk.is_walkable(SINKHOLE.0, SINKHOLE.1) {
            horde.place(slot, SINKHOLE.0, SINKHOLE.1);
        }
        info!("{}: Phreeoni rises from the sinkhole", self.name);
        self.env_shake(map, 220, 1800, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::environment::MapEvent;

    #[test]
    fn every_storm_picks_one_of_the_eight_winds_and_they_vary() {
        let m = missions::find(MISSION).expect("the desert mission");
        let storm: &MapEvent = m.env.event(SANDSTORM).expect("a sandstorm event");
        let dirs: Vec<usize> = (0..6).filter_map(|n| storm.start(n)).map(wind_dir).collect();
        assert!(dirs.len() >= 5, "storms through the run: {dirs:?}");
        assert!(dirs.iter().all(|&d| d < 8));
        let distinct = dirs.iter().collect::<std::collections::HashSet<_>>().len();
        assert!(distinct >= 3, "the wind changes between storms: {dirs:?}");
        // The client hashes the same way (engine sograt.ts): pinned values.
        assert_eq!(wind_dir(150.0), (150u32.wrapping_mul(2_654_435_761) >> 29) as usize);
    }

    #[test]
    fn the_lee_of_a_wall_shelters_and_the_open_does_not() {
        // A 20×20 open field with one wall column at x = 10.
        let row = "..........#.........";
        let walk = WalkGrid::from_ascii(&[row; 20]);
        let east = (1.0, 0.0);
        // Downwind of the wall (east of it): sheltered within 3 m.
        assert!(sheltered(&walk, 12.5, 5.5, east));
        assert!(sheltered(&walk, 16.5, 5.5, east));
        // Too far downwind, or upwind of it: in the open.
        assert!(!sheltered(&walk, 18.5, 5.5, east));
        assert!(!sheltered(&walk, 7.5, 5.5, east));
        // The same spot shelters against the opposite wind on the other side.
        assert!(sheltered(&walk, 7.5, 5.5, (-1.0, 0.0)));
    }
}

#[cfg(test)]
#[path = "sograt_tests.rs"]
mod stage_tests;
