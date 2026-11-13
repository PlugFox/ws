//! Server-side horde simulation for stage instances.
//!
//! Layout (see `docs/ROADMAP.md`, Phase 5, for the measurements behind it):
//!
//! * [`storage`]: struct-of-arrays pool, FIFO free queue, generations.
//! * [`field`]: one lazy, time-sliced multi-source flow field per stage
//!   (distance to the nearest player) that every monster descends.
//! * [`grid`]: counting-sort spatial hash for monster-vs-monster push-out.
//! * [`director`]: the wave game master: buys each wave with the points
//!   the mission's difficulty grants and releases it around the players.
//!
//!
//! A player sees monsters only within their *light radius*: a fixed circle
//! around them, independent of the camera. Spawns land just outside it (in
//! random directions), snapshots carry what is inside it, and the
//! simulation LOD follows it: anything inside some player's light radius
//! (plus a margin) is simulated every tick, because a client interpolates movement
//! segments and a mob stepping at a third of the snapshot rate shows as
//! stop-go. Monsters nobody can see step every `far_period`-th tick
//! (staggered by slot so the work is spread evenly), sub-stepped so a long
//! step cannot tunnel through a wall, with no push-out. Ones left far
//! behind for a few seconds are recycled to just outside the light around the
//! party, which keeps the pressure where the players are and removes
//! long-distance pathing altogether.

pub mod catalog;
pub mod director;
pub mod field;
pub mod grid;
pub mod storage;
pub mod view;

use super::hub::{HubWorld, MapIdx};
use catalog::MonsterCatalog;
use director::{Director, Formation, Pack, WaveRules};
use field::{FlowField, UNREACHABLE};
use grid::SpatialGrid;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use crate::world::combat::MonsterBlock;
use std::sync::Arc;
use storage::{HordeStorage, FLAG_BOSS, FLAG_FULL, FLAG_LOS};
/// A player as the horde sees it: position and light radius (cells).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerView {
    pub x: f32,
    pub y: f32,
    /// Monsters are visible to this player within this radius.
    pub light: f32,
}

pub use catalog::{CatalogError, MonsterDef};

/// Longest distance a monster moves per sub-step (cells). Below one cell,
/// so axis-separated wall checks cannot skip a one-cell wall.
const MAX_SUBSTEP: f32 = 0.45;

#[derive(Clone, Debug)]
pub struct HordeConfig {
    /// Slots in the pool (pre-allocated).
    pub pool: usize,
    /// Director stops spawning at this many live monsters.
    pub alive_cap: usize,
    /// Default player light radius (cells): monsters are visible within it.
    /// Per player in [`PlayerView`] (upgrades may change it).
    pub light_radius: f32,
    /// Snapshots carry the monsters inside the light radius plus this many
    /// cells, so a monster is known a moment before it steps into the light.
    pub aoi_margin: f32,
    /// Full-rate tier: light radius + AoI margin + this margin (cells).
    pub full_margin: f32,
    /// Extra distance before a full-rate monster is demoted (hysteresis).
    pub demote_margin: f32,
    /// Far monsters step every this many ticks.
    pub far_period: u32,
    /// Recycle radius as a multiple of (light radius + AoI margin).
    pub recycle_factor: f32,
    /// Seconds outside the recycle radius before a monster is recycled.
    pub recycle_after_secs: f32,
    /// Flow-field queue pops per tick.
    pub field_budget: usize,
    /// Within this distance a monster with line of sight steers straight
    /// at its nearest player instead of descending the field.
    pub los_range: f32,
    /// Fraction of an overlap resolved per tick (with `damping`, swept on a
    /// 40-monster pile: 0.5 / 0.7 gives the widest spacing without wobble).
    pub push_stiffness: f32,
    /// Largest push-out per tick (cells).
    pub max_push: f32,
    /// Share of last tick's displacement kept (velocity damping, 0..1).
    pub damping: f32,
    /// Overlap pairs considered per monster per tick (bounds the worst case).
    pub max_contacts: usize,
    /// Player body radius: monsters stop at `radius + this` (contact).
    pub player_radius: f32,
    /// What each player's snapshots carry (`view`).
    pub view: view::ViewConfig,
    /// Wave rules of the mission (difficulty, spawn margins).
    pub waves: WaveRules,
    /// Game master on/off (benchmarks place monsters by hand).
    pub director_enabled: bool,
    /// Fixed RNG seed (tests); random when `None`.
    pub seed: Option<u64>,
}

impl Default for HordeConfig {
    fn default() -> Self {
        Self {
            pool: 4096,
            alive_cap: 1500,
            light_radius: 40.0,
            aoi_margin: 4.0,
            full_margin: 6.0,
            demote_margin: 4.0,
            far_period: 4,
            recycle_factor: 1.5,
            recycle_after_secs: 4.0,
            field_budget: 20_000,
            los_range: 64.0,
            push_stiffness: 0.5,
            max_push: 0.25,
            damping: 0.7,
            max_contacts: 16,
            player_radius: 0.4,
            view: view::ViewConfig::default(),
            waves: WaveRules::default(),
            director_enabled: true,
            seed: None,
        }
    }
}

/// Double-box radial filter (zero-sqrt): reject outside the bounding
/// square, accept inside the inscribed square, and only the remaining
/// boundary band pays two multiplications.
#[inline]
pub fn in_radius(dx: f32, dy: f32, r: f32) -> bool {
    let (ax, ay) = (dx.abs(), dy.abs());
    if ax > r || ay > r {
        return false;
    }
    let inner = r * std::f32::consts::FRAC_1_SQRT_2;
    if ax <= inner && ay <= inner {
        return true;
    }
    dx * dx + dy * dy <= r * r
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HordeStats {
    pub alive: usize,
    /// Monsters simulated at full rate this tick.
    pub full: usize,
    /// Monsters that moved this tick (full + this tick's far share).
    pub stepped: usize,
    pub field_builds: u64,
    pub recycled: u64,
    pub spawned: u64,
}

pub struct Horde {
    config: HordeConfig,
    catalog: Arc<MonsterCatalog>,
    world: Arc<HubWorld>,
    map: MapIdx,
    store: HordeStorage,
    field: FlowField,
    grid: SpatialGrid,
    director: Director,
    rng: StdRng,
    max_radius: f32,
    // Scratch, sized once.
    players: Vec<PlayerView>,
    player_cells: Vec<(i32, i32)>,
    requested: Vec<(i32, i32)>,
    push: Vec<(f32, f32)>,
    /// Planned step: unit direction and distance.
    step: Vec<(f32, f32, f32)>,
    recycle: Vec<u16>,
    packs: Vec<Pack>,
    stats: HordeStats,
    /// Players in the last tick (boss HP scales with the party).
    party: u32,
}

impl Horde {
    /// A horde on the entry map of `world`.
    pub fn new(world: Arc<HubWorld>, catalog: Arc<MonsterCatalog>, config: HordeConfig) -> Self {
        let map = world.entry();
        let walk = &world.map(map).walk;
        let field = FlowField::new(walk);
        let grid = SpatialGrid::new(walk.width(), walk.height(), 1, config.pool);
        let director = Director::new(config.waves.clone());
        let rng = match config.seed {
            Some(s) => StdRng::seed_from_u64(s),
            None => StdRng::from_entropy(),
        };
        let max_radius = catalog.defs().iter().map(|d| d.radius).fold(0.0, f32::max);
        Self {
            store: HordeStorage::new(config.pool),
            push: vec![(0.0, 0.0); config.pool],
            step: vec![(0.0, 0.0, 0.0); config.pool],
            recycle: Vec::with_capacity(config.pool),
            packs: Vec::with_capacity(64),
            players: Vec::with_capacity(8),
            player_cells: Vec::with_capacity(8),
            requested: Vec::with_capacity(8),
            config,
            catalog,
            world,
            map,
            field,
            grid,
            director,
            rng,
            max_radius,
            stats: HordeStats::default(),
            party: 1,
        }
    }

    pub fn catalog(&self) -> &MonsterCatalog {
        &self.catalog
    }

    pub fn map(&self) -> MapIdx {
        self.map
    }

    pub fn stats(&self) -> HordeStats {
        self.stats
    }

    pub fn len(&self) -> usize {
        self.store.len()
    }

    /// Byte budget of each player's monster section (tools and benches).
    pub fn set_view_budget(&mut self, budget: usize) {
        self.config.view.budget = budget;
    }

    /// Pool size (slots).
    pub fn capacity(&self) -> usize {
        self.store.capacity()
    }

    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    pub fn field(&self) -> &FlowField {
        &self.field
    }

    pub fn director_elapsed(&self) -> f32 {
        self.director.elapsed()
    }

    /// Waves the game master has bought so far.
    pub fn waves(&self) -> u32 {
        self.director.waves()
    }

    /// `(slot, generation, kind, x, y)` of every live monster (tests and tools).
    pub fn monsters(&self) -> impl Iterator<Item = (u16, u8, u8, f32, f32)> + '_ {
        self.store.alive().iter().map(|&s| {
            let i = s as usize;
            (s, self.store.generation[i], self.store.kind[i], self.store.x[i], self.store.y[i])
        })
    }

    /// Places a monster by hand. `None` when the pool is full or the spot is
    /// not walkable.
    pub fn spawn(&mut self, kind: u8, x: f32, y: f32) -> Option<u16> {
        if !self.world.map(self.map).walk.is_walkable(x, y) {
            return None;
        }
        let def = self.catalog.get(kind);
        let flags = if def.boss { FLAG_BOSS | FLAG_FULL } else { 0 };
        let level = self.spawn_level(kind);
        let block = MonsterBlock::new(level, def.archetype, self.party);
        let slot = self.store.spawn(kind, x, y, block.hp, flags)?;
        let s = slot as usize;
        self.store.level[s] = level as u8;
        // Stagger first swings so a surround does not land in one tick.
        self.store.cooldown[s] = slot % 8;
        self.stats.spawned += 1;
        Some(slot)
    }

    /// Level a kind spawns at now: its RO level clamped into the mission's
    /// band, plus one per `level_every_secs` of the run; bosses at the top
    /// of the band + 5.
    pub fn spawn_level(&self, kind: u8) -> u32 {
        let def = self.catalog.get(kind);
        let rules = &self.config.waves;
        let (lo, hi) = (rules.levels.0.max(1), rules.levels.1.max(rules.levels.0.max(1)));
        if def.boss {
            return (hi + 5).min(u8::MAX as u32);
        }
        let grow = if rules.level_every_secs > 0.0 { (self.director.elapsed() / rules.level_every_secs) as u32 } else { 0 };
        (def.level.clamp(lo, hi) + grow).min(hi)
    }

    /// Combat numbers of a live monster.
    pub fn block(&self, slot: u16) -> MonsterBlock {
        let s = slot as usize;
        let def = self.catalog.get(self.store.kind[s]);
        MonsterBlock::new(self.store.level[s] as u32, def.archetype, self.party)
    }

    /// The pool (combat reads positions and HP, and spends cooldowns).
    pub fn store(&self) -> &HordeStorage {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut HordeStorage {
        &mut self.store
    }

    pub fn despawn(&mut self, slot: u16) {
        self.store.despawn(slot);
    }

    /// Default light radius (cells).
    pub fn light_radius(&self) -> f32 {
        self.config.light_radius
    }

    /// [`Self::tick_views`] with the default light radius for everyone.
    pub fn tick(&mut self, tick: u32, tick_hz: u32, players: &[(f32, f32)]) {
        let light = self.config.light_radius;
        let mut views = std::mem::take(&mut self.players);
        views.clear();
        views.extend(players.iter().map(|&(x, y)| PlayerView { x, y, light }));
        self.tick_views(tick, tick_hz, &views);
        self.players = views;
    }

    /// One simulation step. `players` are the players on the horde's map;
    /// with nobody there the horde is frozen.
    pub fn tick_views(&mut self, tick: u32, tick_hz: u32, players: &[PlayerView]) {
        self.players.clear();
        self.players.extend_from_slice(players);
        if !players.is_empty() {
            self.party = players.len() as u32;
        }
        self.stats.stepped = 0;
        self.stats.full = 0;
        if self.players.is_empty() {
            return;
        }
        let dt = 1.0 / tick_hz as f32;
        self.update_field();
        self.plan_steps(tick, dt);
        self.compute_push();
        self.integrate();
        self.recycle_far(tick_hz);
        if self.config.director_enabled {
            self.direct(dt);
        }
        self.stats.alive = self.store.len();
        self.stats.field_builds = self.field.builds();
    }

    fn update_field(&mut self) {
        self.player_cells.clear();
        self.player_cells.extend(self.players.iter().map(|p| (p.x.floor() as i32, p.y.floor() as i32)));
        if !self.field.is_ready() {
            self.field.build_now(&self.player_cells);
            self.requested.clone_from(&self.player_cells);
        } else if self.player_cells != self.requested {
            self.field.request(&self.player_cells);
            self.requested.clone_from(&self.player_cells);
        }
        self.field.step(self.config.field_budget);
    }

    /// Nearest player: `(dx, dy, d²)` from the monster to the player.
    #[inline]
    fn nearest(players: &[PlayerView], x: f32, y: f32) -> (f32, f32, f32) {
        let mut best = (0.0, 0.0, f32::INFINITY);
        for p in players {
            let (dx, dy) = (p.x - x, p.y - y);
            let d2 = dx * dx + dy * dy;
            if d2 < best.2 {
                best = (dx, dy, d2);
            }
        }
        best
    }

    /// Tier classification and the desired step of every monster that moves
    /// this tick (into `self.step`; zero for the rest).
    fn plan_steps(&mut self, tick: u32, dt: f32) {
        let c = &self.config;
        let los_r2 = c.los_range * c.los_range;
        let walk = &self.world.map(self.map).walk;
        let s = &mut self.store;
        for i in 0..s.len() {
            let slot = s.alive()[i];
            let k = slot as usize;
            self.step[k] = (0.0, 0.0, 0.0);
            let (x, y) = (s.x[k], s.y[k]);
            let was_full = s.flags[k] & FLAG_FULL != 0;
            let boss = s.flags[k] & FLAG_BOSS != 0;

            // Tier: promote at once, demote with hysteresis.
            let extra = c.full_margin + if was_full { c.demote_margin } else { 0.0 };
            let full = boss
                || self.players.iter().any(|p| in_radius(p.x - x, p.y - y, p.light + c.aoi_margin + extra));
            let out_of_reach = !full
                && !self.players.iter().any(|p| {
                    in_radius(p.x - x, p.y - y, (p.light + c.aoi_margin) * c.recycle_factor)
                });
            s.flags[k] = (s.flags[k] & !FLAG_FULL) | if full { FLAG_FULL } else { 0 };
            s.far_ticks[k] = if out_of_reach { s.far_ticks[k].saturating_add(1) } else { 0 };

            let period = if full { 1 } else { c.far_period };
            if !tick.wrapping_add(slot as u32).is_multiple_of(period) {
                continue;
            }
            self.stats.stepped += 1;
            self.stats.full += usize::from(full);

            let def = self.catalog.get(s.kind[k]);
            let (ndx, ndy, nd2) = Self::nearest(&self.players, x, y);
            let contact = def.radius + c.player_radius;
            if nd2 <= contact * contact {
                continue; // touching: Phase 6 turns this into attacks
            }
            if full && nd2 < los_r2 && tick.wrapping_add(slot as u32).is_multiple_of(4) {
                let clear = line_clear(walk, x, y, x + ndx, y + ndy);
                s.flags[k] = (s.flags[k] & !FLAG_LOS) | if clear { FLAG_LOS } else { 0 };
            }
            let nd = nd2.sqrt();
            let dir = if full && nd2 < los_r2 && s.flags[k] & FLAG_LOS != 0 {
                Some((ndx / nd, ndy / nd))
            } else {
                self.field.descent(x, y)
            };
            let Some((ux, uy)) = dir else { continue };
            // Never step past the contact distance.
            let dist = (def.speed * dt * period as f32).min(nd - contact);
            self.step[k] = (ux, uy, dist.max(0.0));
        }
    }

    /// Overlap push-out for full-rate monsters, from the same positions the
    /// steps were planned on (Jacobi style). Positional, not a force.
    fn compute_push(&mut self) {
        let s = &self.store;
        let entries = s
            .alive()
            .iter()
            .filter(|&&slot| s.flags[slot as usize] & FLAG_FULL != 0)
            .map(|&slot| (slot, s.x[slot as usize], s.y[slot as usize]));
        self.grid.rebuild(entries);

        let bucket = self.grid.bucket_size();
        let c = &self.config;
        for &a in s.alive() {
            let ai = a as usize;
            self.push[ai] = (0.0, 0.0);
            if s.flags[ai] & FLAG_FULL == 0 {
                continue;
            }
            let ra = self.catalog.get(s.kind[ai]).radius;
            let reach = ((ra + self.max_radius) / bucket).ceil() as i32;
            let (bx, by) = self.grid.bucket_of(s.x[ai], s.y[ai]);
            let (mut px, mut py) = (0.0f32, 0.0f32);
            let mut contacts = 0;
            'scan: for gy in by - reach..=by + reach {
                for gx in bx - reach..=bx + reach {
                    for &b in self.grid.bucket(gx, gy) {
                        if b == a {
                            continue;
                        }
                        let bi = b as usize;
                        let rb = self.catalog.get(s.kind[bi]).radius;
                        let rr = ra + rb;
                        let (dx, dy) = (s.x[ai] - s.x[bi], s.y[ai] - s.y[bi]);
                        if dx.abs() >= rr || dy.abs() >= rr {
                            continue;
                        }
                        let d2 = dx * dx + dy * dy;
                        if d2 >= rr * rr {
                            continue;
                        }
                        // Lighter monsters yield more (area ratio).
                        let share = rb * rb / (ra * ra + rb * rb);
                        if d2 < 1e-8 {
                            // Exactly stacked: split along a slot-ordered axis.
                            let sign = if a < b { 1.0 } else { -1.0 };
                            px += sign * rr * share;
                        } else {
                            let d = d2.sqrt();
                            let k = (rr - d) * share / d;
                            px += dx * k;
                            py += dy * k;
                        }
                        contacts += 1;
                        if contacts >= c.max_contacts {
                            break 'scan;
                        }
                    }
                }
            }
            // Players are immovable disks: the monster takes the whole overlap.
            for &PlayerView { x: qx, y: qy, .. } in &self.players {
                let rr = ra + c.player_radius;
                let (dx, dy) = (s.x[ai] - qx, s.y[ai] - qy);
                let d2 = dx * dx + dy * dy;
                if d2 < rr * rr && d2 > 1e-8 {
                    let d = d2.sqrt();
                    px += dx * (rr - d) / d;
                    py += dy * (rr - d) / d;
                }
            }
            self.push[ai] = (px, py);
        }
    }

    /// Applies step and push together. A monster does not walk into the
    /// overlap that pushes it back (the opposing overlap is taken off its
    /// step): a jam stops advancing instead of compressing and wobbling, and
    /// the flow goes around it. Moves are sub-stepped against walls, so the
    /// long steps of far monsters cannot tunnel.
    fn integrate(&mut self) {
        let c = &self.config;
        let walk = &self.world.map(self.map).walk;
        let s = &mut self.store;
        for i in 0..s.len() {
            let k = s.alive()[i] as usize;
            let (ux, uy, mut dist) = self.step[k];
            let (ox, oy) = self.push[k];
            let against = -(ox * ux + oy * uy);
            if against > 0.0 {
                dist = (dist - against).max(0.0);
            }
            let (mut px, mut py) = (ox * c.push_stiffness, oy * c.push_stiffness);
            let len2 = px * px + py * py;
            if len2 > c.max_push * c.max_push {
                let f = c.max_push / len2.sqrt();
                px *= f;
                py *= f;
            }
            // Damped velocity (exponential average of the displacement) for
            // full-rate monsters: kills the step-in / pushed-out limit cycle
            // of a settled crowd (measured: 0.03-0.15 → ~0.0005 cells/tick).
            // Far monsters move in multi-tick steps without push-out and keep
            // no velocity, so a promoted one does not carry a long step over.
            let (rx, ry) = (ux * dist + px, uy * dist + py);
            let (mut mx, mut my) = (rx, ry);
            if s.flags[k] & FLAG_FULL != 0 {
                mx = c.damping * s.vel_x[k] + (1.0 - c.damping) * rx;
                my = c.damping * s.vel_y[k] + (1.0 - c.damping) * ry;
                s.vel_x[k] = mx;
                s.vel_y[k] = my;
            } else {
                s.vel_x[k] = 0.0;
                s.vel_y[k] = 0.0;
            }
            if mx == 0.0 && my == 0.0 {
                continue;
            }
            let len = (mx * mx + my * my).sqrt();
            let n = (len / MAX_SUBSTEP).ceil().max(1.0);
            mx /= n;
            my /= n;
            let (mut x, mut y) = (s.x[k], s.y[k]);
            for _ in 0..n as u32 {
                if walk.is_walkable(x + mx, y) {
                    x += mx;
                }
                if walk.is_walkable(x, y + my) {
                    y += my;
                }
            }
            s.x[k] = x;
            s.y[k] = y;
        }
    }

    /// Monsters left far behind (or stuck where no player is reachable)
    /// come back just outside the light, as a new entity for clients.
    fn recycle_far(&mut self, tick_hz: u32) {
        let limit = (self.config.recycle_after_secs * tick_hz as f32) as u16;
        self.recycle.clear();
        for &slot in self.store.alive() {
            let k = slot as usize;
            let stuck = self.field.dist(self.store.x[k] as i32, self.store.y[k] as i32) == UNREACHABLE
                && self.store.flags[k] & FLAG_FULL == 0;
            if self.store.flags[k] & FLAG_BOSS == 0 && (self.store.far_ticks[k] > limit || stuck) {
                self.recycle.push(slot);
            }
        }
        for i in 0..self.recycle.len() {
            let slot = self.recycle[i];
            let kind = self.store.kind[slot as usize];
            self.store.despawn(slot);
            self.stats.recycled += 1;
            if let Some((x, y)) = self.ring_spot() {
                self.spawn(kind, x, y);
            }
        }
    }

    fn direct(&mut self, dt: f32) {
        self.packs.clear();
        let store = &self.store;
        let boss_alive = |kind: u8| store.alive().iter().any(|&s| store.kind[s as usize] == kind);
        self.director.tick(dt, self.players.len(), &self.catalog, &mut self.rng, boss_alive, &mut self.packs);
        for i in 0..self.packs.len() {
            let pack = self.packs[i];
            let room = self.config.alive_cap.saturating_sub(self.store.len()).min(u16::MAX as usize) as u16;
            let placed = if room == 0 { 0 } else { self.place_pack(pack.kind, pack.count.min(room), pack.formation) };
            if placed < pack.count {
                self.director.refund(pack.kind, pack.count - placed, &self.catalog);
            }
        }
    }

    /// Whether a spot can take a fresh monster: walkable, a player is
    /// reachable from it, and it is outside every player's light radius by
    /// at least the inner spawn margin.
    fn spawnable(&self, x: f32, y: f32) -> bool {
        let margin = self.config.waves.spawn_margin.0;
        self.world.map(self.map).walk.is_walkable(x, y)
            && self.field.dist(x.floor() as i32, y.floor() as i32) != UNREACHABLE
            && !self.players.iter().any(|p| in_radius(x - p.x, y - p.y, p.light + margin))
    }

    /// A spot just past `player`'s light radius in direction `angle`, at a
    /// random spawn margin.
    fn off_screen(&mut self, player: usize, angle: f32) -> (f32, f32) {
        let p = self.players[player];
        let (lo, hi) = self.config.waves.spawn_margin;
        let d = p.light + self.rng.gen_range(lo..hi.max(lo + 0.01));
        (p.x + angle.cos() * d, p.y + angle.sin() * d)
    }

    /// A spawnable spot just outside the light of a random player, in a
    /// random direction.
    fn ring_spot(&mut self) -> Option<(f32, f32)> {
        if self.players.is_empty() || !self.field.is_ready() {
            return None;
        }
        for _ in 0..16 {
            let who = self.rng.gen_range(0..self.players.len());
            let a = self.rng.gen_range(0.0..std::f32::consts::TAU);
            let (x, y) = self.off_screen(who, a);
            if self.spawnable(x, y) {
                return Some((x, y));
            }
        }
        None
    }

    /// Places up to `count` monsters of `kind` just off-screen. Returns how
    /// many landed.
    fn place_pack(&mut self, kind: u8, count: u16, formation: Formation) -> u16 {
        if self.players.is_empty() || !self.field.is_ready() {
            return 0;
        }
        let mut placed = 0;
        let mut put = |h: &mut Self, x: f32, y: f32| {
            if h.spawnable(x, y) && h.spawn(kind, x, y).is_some() {
                placed += 1;
                true
            } else {
                false
            }
        };
        match formation {
            // Every member on its own: random player, random direction.
            Formation::Scatter => {
                for _ in 0..count {
                    if let Some((x, y)) = self.ring_spot() {
                        put(self, x, y);
                    }
                }
            }
            // A loose group arriving from one direction.
            Formation::Cluster => {
                let Some((cx, cy)) = self.ring_spot() else { return 0 };
                let spread = (count as f32).sqrt() + 1.0;
                for _ in 0..count {
                    for _ in 0..6 {
                        let (x, y) = (cx + self.rng.gen_range(-spread..spread), cy + self.rng.gen_range(-spread..spread));
                        if put(self, x, y) {
                            break;
                        }
                    }
                }
            }
            // Closing in on one player from all sides, unevenly: jittered
            // angles and distances, so it reads as a crowd, not a circle.
            Formation::Surround => {
                let who = self.rng.gen_range(0..self.players.len());
                let base = self.rng.gen_range(0.0..std::f32::consts::TAU);
                let step = std::f32::consts::TAU / count as f32;
                for i in 0..count {
                    let a = base + step * (i as f32 + self.rng.gen_range(-0.45..0.45));
                    let (x, y) = self.off_screen(who, a);
                    put(self, x, y);
                }
            }
        }
        placed
    }

}

/// Whether the straight segment stays on walkable cells (sampled every half
/// cell; corner grazes may pass, wall sliding handles those).
fn line_clear(walk: &crate::world::map::WalkGrid, x0: f32, y0: f32, x1: f32, y1: f32) -> bool {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let n = (dx.abs().max(dy.abs()) * 2.0).ceil() as i32;
    (1..n).all(|i| {
        let t = i as f32 / n as f32;
        walk.is_walkable(x0 + dx * t, y0 + dy * t)
    })
}

#[cfg(test)]
mod tests;
