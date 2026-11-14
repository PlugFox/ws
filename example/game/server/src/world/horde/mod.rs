//! Server-side horde simulation for stage instances.
//!
//! Layout (see `docs/ROADMAP.md`, Phase 5, for the measurements behind it):
//!
//! * [`storage`]: struct-of-arrays pool, FIFO free queue, generations.
//! * [`field`]: one lazy, time-sliced multi-source flow field per stage
//!   (distance to the nearest player) that every monster descends.
//! * [`grid`]: counting-sort spatial hash for monster-vs-monster push-out,
//!   and a second, coarser one over every live monster that skills and
//!   attacks query ([`Horde::near_box`]).
//! * [`director`]: the wave game master: buys each wave with the points
//!   the mission's difficulty grants and releases it around the players.
//! * [`plants`]: the field's flora, planted once at creation (rooted,
//!   harmless, outside the wave budget).
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
pub mod plants;
pub mod storage;
pub mod view;

use super::hub::{HubWorld, MapIdx};
use catalog::MonsterCatalog;
use director::{Director, Formation, Pack, WaveRules};
use field::{FlowField, UNREACHABLE};
use grid::SpatialGrid;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use crate::world::combat::{Archetype, Condition, MonsterBlock};
use crate::world::modifiers::{Modifiers, MonsterScale};
use std::sync::Arc;
use storage::{HordeStorage, FLAG_BOSS, FLAG_FULL, FLAG_LOS, FLAG_PLANT};
/// A player as the horde sees it: position and light radius (cells).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerView {
    pub x: f32,
    pub y: f32,
    /// Monsters are visible to this player within this radius.
    pub light: f32,
    /// Who it is (user id): taunted monsters hunt their taunter.
    pub id: u32,
    /// Monsters come for it. A downed, dead or hidden player is still seen
    /// by the horde (simulation tiers, light, its body blocks) but is no
    /// target: with no target at all, monsters lose interest and roam.
    pub hunted: bool,
}

/// A flow field toward one taunting player.
struct TauntField {
    id: u32,
    field: FlowField,
    requested: (i32, i32),
}

/// Taunt fields kept at most (a party is four).
const MAX_TAUNT_FIELDS: usize = 4;

/// The query index over every live monster: a counting-sort grid built
/// once per tick, right after the horde moves (everything moves, so it
/// cannot be kept up to date incrementally; the build is O(n), ~22 µs for
/// 3000 monsters). Changes later in the tick need no rebuild:
/// * a monster moved by a skill widens every query by its drift from where
///   it was indexed (`slack`); the exact test reads current positions;
/// * a monster spawned since the build sits in `fresh`, scanned linearly;
/// * a grid entry whose slot was freed and reused (another generation) is
///   skipped; dead ones (HP 0) and despawned ones are skipped too.
///
/// It is rebuilt early, on the spot, only when the slack outgrows a bucket
/// or `fresh` outgrows [`MAX_FRESH`]. Queries are plain reads (`&self`).
struct QueryIndex {
    grid: SpatialGrid,
    /// Generation and position of each slot at the build.
    built_gen: Vec<u8>,
    built_x: Vec<f32>,
    built_y: Vec<f32>,
    fresh: Vec<u16>,
    slack: f32,
    /// Inside the horde step: it builds at its end, spawns wait for that.
    hold: bool,
    builds: u64,
}

/// Spawns since the build kept in the linear list.
const MAX_FRESH: usize = 64;

/// Bucket shift of the query index: 4×4 cells (2 m), about the size of a
/// small skill area, so a query reads a handful of buckets.
const INDEX_SHIFT: u32 = 2;

pub use catalog::{CatalogError, MonsterDef};

/// Idle roaming (nobody to hunt): each leg holds one heading this long.
const ROAM_LEG_SECS: f32 = 2.5;
/// ... at this share of the monster's speed ...
const ROAM_PACE: f32 = 0.35;
/// ... and one leg in this many is spent standing.
const ROAM_REST: u32 = 3;

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
    /// Mission modifiers of the run (`HordeConfig::with_modifiers`):
    /// tougher monsters, scaled loot, the curses the stage reads.
    pub modifiers: Modifiers,
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
            modifiers: Modifiers::NONE,
        }
    }
}

impl HordeConfig {
    /// This configuration under mission modifiers: the wave budget and the
    /// elites' weight grow with the count factors, the light shrinks under
    /// Darkness; block toughness, speed and the loot share are read from
    /// `modifiers` as the horde runs.
    pub fn with_modifiers(mut self, mods: Modifiers) -> Self {
        let s = mods.monsters();
        self.waves.budget *= s.count;
        self.waves.elite_weight *= s.elite_weight;
        self.light_radius *= mods.light();
        self.modifiers = mods;
        self
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
    /// Every live monster, for area and nearest queries ([`QueryIndex`]).
    index: QueryIndex,
    /// One flow field per taunting player, built while a monster is
    /// taunted by them (COMBAT.md §4.4: at most one per party member).
    taunt_fields: Vec<TauntField>,
    director: Director,
    rng: StdRng,
    max_radius: f32,
    // Scratch, sized once.
    /// Every player on the map (tiers, light, bodies).
    players: Vec<PlayerView>,
    /// The hunted ones among them (flow field, chase, spawns).
    targets: Vec<PlayerView>,
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
    /// Simulation rate of the last tick (flinch lengths are in seconds).
    hz: u32,
    /// What the mission modifiers do to every monster (once, at creation).
    scale: MonsterScale,
    /// Live plants (`plants.rs`): not monsters to the alive cap.
    plants: u16,
}

impl Horde {
    /// A horde on the entry map of `world`.
    pub fn new(world: Arc<HubWorld>, catalog: Arc<MonsterCatalog>, config: HordeConfig) -> Self {
        let map = world.entry();
        let walk = &world.map(map).walk;
        let field = FlowField::new(walk);
        let grid = SpatialGrid::new(walk.width(), walk.height(), 1, config.pool);
        let index = QueryIndex {
            grid: SpatialGrid::new(walk.width(), walk.height(), INDEX_SHIFT, config.pool),
            built_gen: vec![0; config.pool],
            built_x: vec![0.0; config.pool],
            built_y: vec![0.0; config.pool],
            fresh: Vec::with_capacity(MAX_FRESH),
            slack: 0.0,
            hold: false,
            builds: 0,
        };
        let director = Director::new(config.waves.clone());
        let rng = match config.seed {
            Some(s) => StdRng::seed_from_u64(s),
            None => StdRng::from_entropy(),
        };
        let max_radius = catalog.defs().iter().map(|d| d.radius).fold(0.0, f32::max);
        let scale = config.modifiers.monsters();
        let mut horde = Self {
            store: HordeStorage::new(config.pool),
            push: vec![(0.0, 0.0); config.pool],
            step: vec![(0.0, 0.0, 0.0); config.pool],
            recycle: Vec::with_capacity(config.pool),
            packs: Vec::with_capacity(64),
            players: Vec::with_capacity(8),
            targets: Vec::with_capacity(8),
            player_cells: Vec::with_capacity(8),
            requested: Vec::with_capacity(8),
            config,
            catalog,
            world,
            map,
            field,
            grid,
            index,
            taunt_fields: Vec::new(),
            director,
            rng,
            max_radius,
            stats: HordeStats::default(),
            party: 1,
            hz: 20,
            scale,
            plants: 0,
        };
        horde.plant_flora();
        horde
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

    /// The mission is over: no more waves, and every monster leaves.
    pub fn halt(&mut self) {
        self.config.director_enabled = false;
        while let Some(&slot) = self.store.alive().last() {
            self.despawn(slot);
        }
    }

    pub fn director_elapsed(&self) -> f32 {
        self.director.elapsed()
    }

    /// Test hook: jumps the run clock (the director must be off, or it
    /// will buy every wave it missed).
    #[cfg(test)]
    pub fn set_director_elapsed(&mut self, secs: f32) {
        self.director.set_elapsed(secs);
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
        if def.is_plant() {
            return self.spawn_plant(kind, x, y, def.hits.1);
        }
        let flags = if def.boss { FLAG_BOSS | FLAG_FULL } else { 0 };
        let level = self.spawn_level(kind);
        let block = self.scale.apply(MonsterBlock::new(level, def.archetype, self.party));
        let slot = self.store.spawn(kind, x, y, block.hp, flags)?;
        let s = slot as usize;
        self.store.level[s] = level as u8;
        self.store.legendary[s] = block.legendary_resistance;
        // Stagger first swings so a surround does not land in one tick.
        self.store.cooldown[s] = slot % 8;
        self.stats.spawned += 1;
        if !self.index.hold {
            if self.index.fresh.len() >= MAX_FRESH {
                self.rebuild_index();
            } else if !self.index.fresh.contains(&slot) {
                self.index.fresh.push(slot);
            }
        }
        Some(slot)
    }

    /// Level a kind spawns at now: its RO level clamped into the mission's
    /// band, plus one per `level_every_secs` of the run; bosses at the top
    /// of the band + 5.
    pub fn spawn_level(&self, kind: u8) -> u32 {
        let def = self.catalog.get(kind);
        self.config.waves.level_at(def.level, def.boss, self.director.elapsed())
    }

    /// The mission's wave rules this horde runs under.
    pub fn rules(&self) -> &WaveRules {
        &self.config.waves
    }

    /// Level of the location: the middle of the mission's monster band
    /// (Poring Island, 1–20: 11). Map-wide rewards scale with it (pickup
    /// bonuses), so early maps never hand out much.
    pub fn location_level(&self) -> u32 {
        let (lo, hi) = self.config.waves.levels;
        let lo = lo.max(1);
        (lo + hi.max(lo)).div_ceil(2)
    }

    /// Combat numbers of a live monster (made tougher by the mission's
    /// modifiers).
    pub fn block(&self, slot: u16) -> MonsterBlock {
        let s = slot as usize;
        let def = self.catalog.get(self.store.kind[s]);
        if def.is_plant() {
            // Its hit count, untouched by the modifiers.
            return MonsterBlock { hp: def.hits.1.max(1) as i32, ..MonsterBlock::new(1, Archetype::Plant, 1) };
        }
        self.scale.apply(MonsterBlock::new(self.store.level[s] as u32, def.archetype, self.party))
    }

    /// The mission modifiers this horde runs under.
    pub fn modifiers(&self) -> Modifiers {
        self.config.modifiers
    }

    /// A kill's share of drop chances, zeny and experience, per mille
    /// (more monsters, each worth less: `MonsterScale::loot_permille`).
    pub fn loot_permille(&self, archetype: Archetype) -> u32 {
        self.scale.loot_permille(archetype)
    }

    /// The pool (combat reads positions and HP, and spends cooldowns).
    /// A hit landed on `slot`: it flinches for 0.3 s, then cannot flinch
    /// again for 0.35 s (bosses never flinch).
    pub fn flinch(&mut self, slot: u16) {
        let t = |secs: f32| (secs * self.hz as f32).round().clamp(1.0, 255.0) as u8;
        let (ticks, guard) = (t(0.3), t(0.35));
        self.store.flinch(slot, ticks, guard);
    }

    pub fn store(&self) -> &HordeStorage {
        &self.store
    }

    pub fn store_mut(&mut self) -> &mut HordeStorage {
        &mut self.store
    }

    pub fn despawn(&mut self, slot: u16) {
        if self.store.is_alive(slot) && self.store.is_plant(slot) {
            self.plants -= 1;
        }
        self.store.despawn(slot);
    }

    /// Largest monster body radius of the catalog (cells).
    pub fn max_radius(&self) -> f32 {
        self.max_radius
    }

    /// Moves a monster (knockback, pull); the query index follows. Plants
    /// are rooted: nothing moves them.
    pub fn place(&mut self, slot: u16, x: f32, y: f32) {
        let s = slot as usize;
        if self.store.flags[s] & FLAG_PLANT != 0 {
            return;
        }
        (self.store.x[s], self.store.y[s]) = (x, y);
        let idx = &mut self.index;
        if idx.hold || idx.built_gen[s] != self.store.generation[s] {
            return; // built at the end of the step, or in `fresh`
        }
        let drift = ((x - idx.built_x[s]).powi(2) + (y - idx.built_y[s]).powi(2)).sqrt();
        idx.slack = idx.slack.max(drift);
        if idx.slack > idx.grid.bucket_size() {
            self.rebuild_index();
        }
    }

    /// Builds the query index from the current positions (the horde step
    /// does it; public for benches).
    pub fn rebuild_index(&mut self) {
        let s = &self.store;
        let idx = &mut self.index;
        let entries = s
            .alive()
            .iter()
            .filter(|&&slot| s.hp[slot as usize] > 0)
            .map(|&slot| (slot, s.x[slot as usize], s.y[slot as usize]));
        idx.grid.rebuild(entries);
        for &slot in s.alive() {
            let i = slot as usize;
            (idx.built_gen[i], idx.built_x[i], idx.built_y[i]) = (s.generation[i], s.x[i], s.y[i]);
        }
        idx.fresh.clear();
        idx.slack = 0.0;
        idx.builds += 1;
    }

    /// Full index builds so far (tests and stats).
    pub fn index_builds(&self) -> u64 {
        self.index.builds
    }

    /// Live monsters (HP above 0) whose body may reach the box
    /// `[x0, x1] × [y0, y1]`: the box is grown by the largest body and the
    /// drift since the build. Candidates only; the caller runs the exact
    /// shape test. Cost: the buckets the box covers, never the whole horde.
    pub fn near_box(&self, x0: f32, y0: f32, x1: f32, y1: f32, out: &mut Vec<u16>) {
        let s = &self.store;
        let idx = &self.index;
        let m = self.max_radius + idx.slack;
        let (bx0, by0) = idx.grid.bucket_of(x0 - m, y0 - m);
        let (bx1, by1) = idx.grid.bucket_of(x1 + m, y1 + m);
        let live = |slot: u16| s.is_alive(slot) && s.hp[slot as usize] > 0;
        for by in by0..=by1 {
            for bx in bx0..=bx1 {
                for &slot in idx.grid.bucket(bx, by) {
                    // Killed, or freed and reused since the build.
                    if live(slot) && idx.built_gen[slot as usize] == s.generation[slot as usize] {
                        out.push(slot);
                    }
                }
            }
        }
        let mf = self.max_radius;
        for &slot in &idx.fresh {
            let i = slot as usize;
            if live(slot) && s.x[i] >= x0 - mf && s.x[i] <= x1 + mf && s.y[i] >= y0 - mf && s.y[i] <= y1 + mf {
                out.push(slot);
            }
        }
    }

    /// Nearest live monster to `(x, y)` whose body is within `range`:
    /// `(slot, gap)`, the gap measured to its body. Searches outward ring by
    /// ring of index buckets and stops once a ring cannot beat the best.
    pub fn nearest_to(&self, x: f32, y: f32, range: f32, skip: impl Fn(u16) -> bool) -> Option<(u16, f32)> {
        let bucket = self.index.grid.bucket_size();
        let mut best: Option<(u16, f32)> = None;
        let mut buf = Vec::new();
        let mut r = 0.0f32;
        loop {
            let reach = (r + bucket).min(range);
            buf.clear();
            // Whole box each ring: simple, and rings stay small (a few buckets).
            self.near_box(x - reach, y - reach, x + reach, y + reach, &mut buf);
            for &s in &buf {
                if skip(s) {
                    continue;
                }
                let i = s as usize;
                let body = self.catalog.get(self.store.kind[i]).radius;
                let (dx, dy) = (self.store.x[i] - x, self.store.y[i] - y);
                let gap = (dx * dx + dy * dy).sqrt() - body;
                if gap <= range && best.is_none_or(|(b, g)| gap < g || (gap == g && s < b)) {
                    best = Some((s, gap));
                }
            }
            // Everything within `reach` of the point has been seen.
            if best.is_some_and(|(_, g)| g <= reach) || reach >= range {
                return best;
            }
            r = reach * 2.0;
        }
    }

    /// [`Self::nearest_to`] among monsters only: what automatic targeting
    /// (the basic attack, auto-cast) goes for, so auto-play never spends
    /// itself on the scenery.
    pub fn nearest_foe(&self, x: f32, y: f32, range: f32) -> Option<(u16, f32)> {
        self.nearest_to(x, y, range, |s| self.store.is_plant(s))
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
        views.extend(players.iter().map(|&(x, y)| PlayerView { x, y, light, id: 0, hunted: true }));
        self.tick_views(tick, tick_hz, &views);
        self.players = views;
    }

    /// One simulation step. `players` are the players on the horde's map;
    /// with nobody there the horde is frozen. With nobody [`PlayerView::hunted`]
    /// the horde still moves (monsters roam), but the flow field, recycling
    /// and the director wait for a target.
    pub fn tick_views(&mut self, tick: u32, tick_hz: u32, players: &[PlayerView]) {
        self.players.clear();
        self.players.extend_from_slice(players);
        self.targets.clear();
        self.targets.extend(players.iter().filter(|p| p.hunted));
        if !players.is_empty() {
            self.party = players.len() as u32;
        }
        self.stats.stepped = 0;
        self.stats.full = 0;
        if self.players.is_empty() {
            return;
        }
        let dt = 1.0 / tick_hz as f32;
        self.hz = tick_hz.max(1);
        self.index.hold = true;
        self.store.tick_conditions();
        let hunting = !self.targets.is_empty();
        if hunting {
            // Kept as it is otherwise: a target back on the same cell
            // finds it ready.
            self.update_field();
        }
        self.plan_steps(tick, dt);
        self.compute_push();
        self.integrate();
        if hunting {
            // Respawn spots need a target to circle.
            self.recycle_far(tick_hz);
            if self.config.director_enabled {
                self.direct(dt);
            }
        }
        self.stats.alive = self.store.len();
        self.stats.field_builds = self.field.builds();
        // Everything moved: one build for the rest of the tick.
        self.index.hold = false;
        self.rebuild_index();
    }

    fn update_field(&mut self) {
        self.player_cells.clear();
        self.player_cells.extend(self.targets.iter().map(|p| (p.x.floor() as i32, p.y.floor() as i32)));
        if !self.field.is_ready() {
            self.field.build_now(&self.player_cells);
            self.requested.clone_from(&self.player_cells);
        } else if self.player_cells != self.requested {
            self.field.request(&self.player_cells);
            self.requested.clone_from(&self.player_cells);
        }
        self.field.step(self.config.field_budget);
        self.update_taunt_fields();
    }

    /// Steps the field of every player some monster is taunted by.
    fn update_taunt_fields(&mut self) {
        let mut ids: [u32; MAX_TAUNT_FIELDS] = [0; MAX_TAUNT_FIELDS];
        let mut n = 0;
        let s = &self.store;
        for &slot in s.alive() {
            let i = slot as usize;
            if s.status[i] & Condition::Taunted.bit() == 0 {
                continue;
            }
            let id = s.taunter[i];
            if n < MAX_TAUNT_FIELDS && id != 0 && !ids[..n].contains(&id) {
                ids[n] = id;
                n += 1;
            }
        }
        for &id in &ids[..n] {
            let Some(p) = self.targets.iter().find(|p| p.id == id) else { continue };
            let cell = (p.x.floor() as i32, p.y.floor() as i32);
            let at = match self.taunt_fields.iter().position(|f| f.id == id) {
                Some(at) => at,
                None if self.taunt_fields.len() < MAX_TAUNT_FIELDS => {
                    let field = FlowField::new(&self.world.map(self.map).walk);
                    self.taunt_fields.push(TauntField { id, field, requested: (i32::MIN, 0) });
                    self.taunt_fields.len() - 1
                }
                // Reuse the field of a player nobody is taunted by any more.
                None => match self.taunt_fields.iter().position(|f| !ids[..n].contains(&f.id)) {
                    Some(at) => {
                        self.taunt_fields[at].id = id;
                        self.taunt_fields[at].requested = (i32::MIN, 0);
                        at
                    }
                    None => continue,
                },
            };
            let f = &mut self.taunt_fields[at];
            if !f.field.is_ready() && f.requested != cell {
                f.field.build_now(&[cell]);
                f.requested = cell;
            } else if f.requested != cell {
                f.field.request(&[cell]);
                f.requested = cell;
            }
            f.field.step(self.config.field_budget / 2);
        }
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
        // Mission modifiers: tougher tiers and Frenzy move faster.
        let pace = self.scale.speed;
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
            if s.flags[k] & FLAG_PLANT != 0 {
                continue; // rooted: tiered for the snapshots only
            }

            let period = if full { 1 } else { c.far_period };
            if !tick.wrapping_add(slot as u32).is_multiple_of(period) {
                continue;
            }
            self.stats.stepped += 1;
            self.stats.full += usize::from(full);

            let status = s.status_of(slot);
            if !status.can_move() || s.stagger[k] > 0 {
                continue; // frozen, stunned, petrified, rooted or flinching
            }
            let def = self.catalog.get(s.kind[k]);
            if self.targets.is_empty() {
                // Nobody to hunt: lose interest and roam.
                if let Some((ux, uy)) = roam_heading(slot, s.generation[k], tick, self.hz) {
                    let dist = def.speed * pace * status.speed() * ROAM_PACE * dt * period as f32;
                    self.step[k] = (ux, uy, dist);
                }
                continue;
            }
            // A taunted monster hunts its taunter while they are here.
            let hunted = if status.has(Condition::Taunted) {
                self.targets.iter().find(|p| p.id != 0 && p.id == s.taunter[k])
            } else {
                None
            };
            let (ndx, ndy, nd2) = match hunted {
                Some(p) => {
                    let (dx, dy) = (p.x - x, p.y - y);
                    (dx, dy, dx * dx + dy * dy)
                }
                None => Self::nearest(&self.targets, x, y),
            };
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
            } else if let Some(p) = hunted {
                self.taunt_fields
                    .iter()
                    .find(|f| f.id == p.id && f.field.is_ready())
                    .and_then(|f| f.field.descent(x, y))
                    .or_else(|| self.field.descent(x, y))
            } else {
                self.field.descent(x, y)
            };
            let Some((ux, uy)) = dir else { continue };
            // Never step past the contact distance.
            let dist = (def.speed * pace * status.speed() * dt * period as f32).min(nd - contact);
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
            // Plants are no obstacle: the horde walks over them.
            .filter(|&&slot| s.flags[slot as usize] & (FLAG_FULL | FLAG_PLANT) == FLAG_FULL)
            .map(|&slot| (slot, s.x[slot as usize], s.y[slot as usize]));
        self.grid.rebuild(entries);

        let bucket = self.grid.bucket_size();
        let c = &self.config;
        for &a in s.alive() {
            let ai = a as usize;
            self.push[ai] = (0.0, 0.0);
            if s.flags[ai] & (FLAG_FULL | FLAG_PLANT) != FLAG_FULL {
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
            if !s.status_of(k as u16).can_move() || s.flags[k] & FLAG_PLANT != 0 {
                // Held in place (or rooted): not even the crowd pushes it.
                s.vel_x[k] = 0.0;
                s.vel_y[k] = 0.0;
                continue;
            }
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
            if self.store.flags[k] & (FLAG_BOSS | FLAG_PLANT) == 0 && (self.store.far_ticks[k] > limit || stuck) {
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
        self.director.tick(dt, self.targets.len(), &self.catalog, &mut self.rng, boss_alive, &mut self.packs);
        for i in 0..self.packs.len() {
            let pack = self.packs[i];
            let monsters = self.store.len() - self.plants as usize;
            let room = self.config.alive_cap.saturating_sub(monsters).min(u16::MAX as usize) as u16;
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
        let p = self.targets[player];
        let (lo, hi) = self.config.waves.spawn_margin;
        let d = p.light + self.rng.gen_range(lo..hi.max(lo + 0.01));
        (p.x + angle.cos() * d, p.y + angle.sin() * d)
    }

    /// A spawnable spot just outside the light of a random player, in a
    /// random direction.
    fn ring_spot(&mut self) -> Option<(f32, f32)> {
        if self.targets.is_empty() || !self.field.is_ready() {
            return None;
        }
        for _ in 0..16 {
            let who = self.rng.gen_range(0..self.targets.len());
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
        if self.targets.is_empty() || !self.field.is_ready() {
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
                let who = self.rng.gen_range(0..self.targets.len());
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

/// The heading of an idle (roaming) monster: one random heading per leg of
/// [`ROAM_LEG_SECS`], legs staggered by slot, one in [`ROAM_REST`] spent
/// standing (`None`). Stateless (a hash of slot, generation and leg), so
/// it costs no storage and stays deterministic.
fn roam_heading(slot: u16, generation: u8, tick: u32, hz: u32) -> Option<(f32, f32)> {
    let leg_ticks = ((ROAM_LEG_SECS * hz as f32) as u32).max(1);
    let leg = tick.wrapping_add((slot as u32).wrapping_mul(97)) / leg_ticks;
    let mut h = (slot as u64) << 40 | (generation as u64) << 32 | leg as u64;
    // splitmix64 finaliser.
    h = (h ^ (h >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    h ^= h >> 31;
    if (h as u32).is_multiple_of(ROAM_REST) {
        return None;
    }
    let a = (h >> 40) as f32 / (1u64 << 24) as f32 * std::f32::consts::TAU;
    Some((a.cos(), a.sin()))
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
