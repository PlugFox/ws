//! Town residents: simulated townsfolk (and their pets) walking the entry
//! map of a hub channel so the town never looks empty.
//!
//! To clients they are players (`map_enter.players`, snapshots, `emote`,
//! `pose`) with entity ids from [`RESIDENT_ENTITY_BASE`]; they have no
//! session, never vanish and block nothing. Pets carry the archetype
//! `pet:<sprite>` and level 0.
//!
//! Behaviour follows the manifest's `residents.spots` when there are any:
//! a resident picks a free spot nearby, walks there on an A* path (8
//! neighbours, no corner cutting, capped search), then sits, stands or
//! strolls around depending on the spot's kind. Without spots they stroll
//! to random cells and sometimes sit where they are. Pets idle, wander
//! around a home spot or tag along behind somebody. Paths only cross
//! walkable cells outside portal triggers, so nobody gets stuck or warped.
//!
//! A tick allocates nothing: the planner, paths and events reuse buffers.

use super::{facing_of, HubInstance, HubSimConfig, HubWorld, MapIdx, Outgoing};
use crate::world::combat::Rng;
use crate::world::map::MapData;
use crate::world::protocol::{pose, Look, PlayerInfo, ServerMsg, RESIDENT_ENTITY_BASE};
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::Arc;

const NAMES: &[&str] = &[
    "Aldric", "Brenna", "Corwin", "Dalia", "Edric", "Fenna", "Garrick", "Hilde", "Isolde", "Jorund", "Kaelin",
    "Liora", "Merrick", "Nessa", "Osric", "Perrin", "Quilla", "Rowan", "Sabine", "Tobias", "Ulric", "Vesna",
    "Wendel", "Yara", "Elowen", "Bram", "Maren", "Thane", "Ilsa", "Cedric",
];
const ARCHETYPES: &[&str] = &["novice", "swordsman", "mage", "archer", "acolyte"];
/// Friendly emotes residents play; pets only `!` and the heart.
const EMOTES: &[u8] = &[0, 1, 2, 3, 15, 17, 18, 27, 33, 46];
const PET_EMOTES: &[u8] = &[0, 3];
const MAX_LEVEL: u32 = 80;

/// Residents without a spot start within this many cells of the map spawn.
const SPAWN_RANGE: f32 = 40.0;
/// A stroll goes at most this far (cells); pets wander this far from home.
const STROLL_RANGE: f32 = 10.0;
const WANDER_RANGE: f32 = 6.0;
/// Pets pick somebody to follow within this range (cells).
const FOLLOW_RANGE: f32 = 15.0;
/// Targets keep this far from NPCs (cells) and this far outside portal
/// triggers.
const NPC_CLEARANCE: f32 = 2.0;
const PORTAL_MARGIN: f32 = 1.0;
/// Attempts to find a free cell or spot before giving up for this turn.
const SPOT_TRIES: u32 = 32;
const SPOT_SAMPLES: u32 = 6;
/// A* gives up after expanding this many cells.
const MAX_EXPAND: u32 = 4000;

/// Seconds (min, max) of each activity, and between two emotes.
const SIT_SECS: (u32, u32) = (10, 40);
const SHOP_SECS: (u32, u32) = (6, 20);
const IDLE_SECS: (u32, u32) = (3, 10);
const PAUSE_SECS: (u32, u32) = (1, 3);
const FREE_SIT_SECS: (u32, u32) = (8, 25);
const PET_IDLE_SECS: (u32, u32) = (2, 8);
const EMOTE_SECS: (u32, u32) = (10, 30);
const SHOP_EMOTE_SECS: (u32, u32) = (3, 10);
const PET_EMOTE_SECS: (u32, u32) = (40, 90);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SpotKind {
    Sit,
    Stand,
    Wander,
}

/// An authored spot, on a cell centre.
#[derive(Clone, Copy, Debug)]
struct Spot {
    x: f32,
    y: f32,
    kind: SpotKind,
    facing: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// Following `path`.
    Walk,
    Stand,
    Sit,
}

/// Whom a pet tags along with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Follow {
    Nobody,
    Resident(u16),
    /// The nearest player at each leg.
    Player,
}

#[derive(Debug)]
pub(super) struct Resident {
    pub id: u32,
    name: String,
    archetype: String,
    level: u32,
    look: Look,
    pet: bool,
    pub x: f32,
    pub y: f32,
    pub facing: u8,
    /// Cells per tick.
    speed: f32,
    state: State,
    /// Ticks left in the current state.
    timer: u32,
    /// Ticks in a row a walk made no progress.
    stuck: u32,
    /// Ticks until the next emote.
    emote_in: u32,
    /// Cells still to walk, the next one last.
    path: Vec<u32>,
    /// The spot this resident walks to or uses (no one else picks it).
    spot: Option<u16>,
    /// Short strolls left before the next spot (or follow legs of a pet).
    legs: u8,
    /// A pet's home area.
    home: (f32, f32),
    follow: Follow,
}

impl Resident {
    pub fn info(&self) -> PlayerInfo {
        PlayerInfo {
            entity_id: self.id,
            name: self.name.clone(),
            archetype: self.archetype.clone(),
            x: self.x,
            y: self.y,
            facing: self.facing,
            look: self.look,
            level: self.level,
            weapon: String::new(),
        }
    }

    pub fn is_sitting(&self) -> bool {
        self.state == State::Sit
    }
}

/// Uniform in `lo..=hi`.
fn between(rng: &mut Rng, (lo, hi): (u32, u32)) -> u32 {
    lo + rng.die(hi - lo + 1) - 1
}

/// Uniform in `0.0..1.0`.
fn unit(rng: &mut Rng) -> f32 {
    (rng.next_u32() >> 8) as f32 / (1u32 << 24) as f32
}

/// Inside a portal trigger grown by `margin` cells.
fn in_portal(map: &MapData, x: f32, y: f32, margin: f32) -> bool {
    map.manifest.portals.iter().any(|p| {
        let (cx, cy) = p.center();
        let r = p.radius() + margin;
        let (dx, dy) = (x - cx, y - cy);
        dx * dx + dy * dy <= r * r
    })
}

/// A resident may stop here: walkable, clear of triggers and NPCs.
fn free_spot(map: &MapData, x: f32, y: f32) -> bool {
    map.walk.is_walkable(x, y)
        && !in_portal(map, x, y, PORTAL_MARGIN)
        && map.manifest.npcs.iter().all(|n| {
            let (cx, cy) = n.center();
            let (dx, dy) = (x - cx, y - cy);
            dx * dx + dy * dy >= NPC_CLEARANCE * NPC_CLEARANCE
        })
}

/// A random free cell centre within `range` cells of `(cx, cy)` (the box
/// is clipped to the map, so small maps still find room).
fn pick_cell(rng: &mut Rng, map: &MapData, cx: f32, cy: f32, range: f32) -> Option<(f32, f32)> {
    let (w, h) = (map.walk.width() as f32, map.walk.height() as f32);
    let (x0, x1) = ((cx - range).max(0.0), (cx + range).min(w));
    let (y0, y1) = ((cy - range).max(0.0), (cy + range).min(h));
    if x0 >= x1 || y0 >= y1 {
        return None;
    }
    (0..SPOT_TRIES).find_map(|_| {
        let x = (x0 + unit(rng) * (x1 - x0)).floor() + 0.5;
        let y = (y0 + unit(rng) * (y1 - y0)).floor() + 0.5;
        free_spot(map, x, y).then_some((x, y))
    })
}

/// A* on the walk grid with reused buffers. Cells inside portal triggers
/// count as walls.
#[derive(Debug, Default)]
struct Planner {
    w: i32,
    h: i32,
    pass: Vec<bool>,
    g: Vec<u32>,
    parent: Vec<u32>,
    /// Search generation that last touched each cell.
    seen: Vec<u32>,
    generation: u32,
    heap: BinaryHeap<Reverse<(u32, u32)>>,
}

const STRAIGHT: u32 = 10;
const DIAGONAL: u32 = 14;
const NEIGHBOURS: [(i32, i32, u32); 8] = [
    (1, 0, STRAIGHT),
    (-1, 0, STRAIGHT),
    (0, 1, STRAIGHT),
    (0, -1, STRAIGHT),
    (1, 1, DIAGONAL),
    (1, -1, DIAGONAL),
    (-1, 1, DIAGONAL),
    (-1, -1, DIAGONAL),
];

impl Planner {
    fn new(map: &MapData) -> Self {
        let (w, h) = (map.walk.width(), map.walk.height());
        let n = (w * h) as usize;
        let mut pass = vec![false; n];
        for y in 0..h {
            for x in 0..w {
                let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                pass[(y * w + x) as usize] =
                    map.walk.is_walkable_cell(x as i32, y as i32) && !in_portal(map, cx, cy, 0.0);
            }
        }
        Self {
            w: w as i32,
            h: h as i32,
            pass,
            g: vec![0; n],
            parent: vec![0; n],
            seen: vec![0; n],
            generation: 0,
            heap: BinaryHeap::new(),
        }
    }

    fn passable(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.w && y < self.h && self.pass[(y * self.w + x) as usize]
    }

    /// Octile distance (admissible and consistent).
    fn heuristic(&self, c: u32, goal: (i32, i32)) -> u32 {
        let (x, y) = ((c as i32) % self.w, (c as i32) / self.w);
        let (dx, dy) = ((x - goal.0).unsigned_abs(), (y - goal.1).unsigned_abs());
        STRAIGHT * dx.max(dy) + (DIAGONAL - STRAIGHT) * dx.min(dy)
    }

    /// Fills `path` with the cells from `from` (excluded) to `to`, the
    /// first step last. `false` when unreachable within the budget.
    fn plan(&mut self, from: (f32, f32), to: (f32, f32), path: &mut Vec<u32>) -> bool {
        path.clear();
        let (sx, sy) = (from.0 as i32, from.1 as i32);
        let goal = (to.0 as i32, to.1 as i32);
        if !self.passable(goal.0, goal.1) || sx < 0 || sy < 0 || sx >= self.w || sy >= self.h {
            return false;
        }
        if (sx, sy) == goal {
            return true;
        }
        self.generation = self.generation.wrapping_add(1);
        if self.generation == 0 {
            self.seen.fill(0);
            self.generation = 1;
        }
        let generation = self.generation;
        let start = (sy * self.w + sx) as u32;
        let target = (goal.1 * self.w + goal.0) as u32;
        self.heap.clear();
        self.seen[start as usize] = generation;
        self.g[start as usize] = 0;
        self.heap.push(Reverse((self.heuristic(start, goal), start)));
        let mut expanded = 0;
        while let Some(Reverse((f, c))) = self.heap.pop() {
            let gc = self.g[c as usize];
            if f > gc + self.heuristic(c, goal) {
                continue; // a better entry came later
            }
            if c == target {
                let mut at = c;
                while at != start {
                    path.push(at);
                    at = self.parent[at as usize];
                }
                return true;
            }
            expanded += 1;
            if expanded > MAX_EXPAND {
                return false;
            }
            let (cx, cy) = ((c as i32) % self.w, (c as i32) / self.w);
            for (dx, dy, cost) in NEIGHBOURS {
                let (nx, ny) = (cx + dx, cy + dy);
                if !self.passable(nx, ny) {
                    continue;
                }
                if dx != 0 && dy != 0 && !(self.passable(cx + dx, cy) && self.passable(cx, cy + dy)) {
                    continue;
                }
                let n = (ny * self.w + nx) as usize;
                let ng = gc + cost;
                if self.seen[n] == generation && self.g[n] <= ng {
                    continue;
                }
                self.seen[n] = generation;
                self.g[n] = ng;
                self.parent[n] = c;
                self.heap.push(Reverse((ng + self.heuristic(n as u32, goal), n as u32)));
            }
        }
        false
    }
}

/// The residents of one hub channel (empty on stages).
#[derive(Debug)]
pub(super) struct Residents {
    map: MapIdx,
    list: Vec<Resident>,
    spots: Vec<Spot>,
    rng: Rng,
    hz: u32,
    planner: Planner,
    /// Scratch: positions of the players on the map this tick.
    players: Vec<(f32, f32)>,
    /// This tick's emotes and poses (reused).
    events: Vec<ServerMsg>,
}

impl Residents {
    pub fn none(map: MapIdx) -> Self {
        Self {
            map,
            list: Vec::new(),
            spots: Vec::new(),
            rng: Rng::new(0),
            hz: 1,
            planner: Planner::default(),
            players: Vec::new(),
            events: Vec::new(),
        }
    }

    /// `config.residents` townsfolk (on the manifest's spots when it has
    /// some, else around the spawn) plus the manifest's pets. None when the
    /// knob is 0 or the map has no room.
    pub fn spawn(world: &HubWorld, config: &HubSimConfig, seed: u64) -> Self {
        let mut town = Self::none(world.entry());
        let map = world.map(town.map);
        if config.residents == 0 || map.walk.walkable_count() == 0 {
            return town;
        }
        town.rng = Rng::new(seed);
        town.hz = config.tick_hz.max(1);
        town.planner = Planner::new(map);
        let planner = &town.planner;
        town.spots = map
            .manifest
            .residents
            .spots
            .iter()
            .map(|s| Spot {
                x: s.x.floor() + 0.5,
                y: s.y.floor() + 0.5,
                kind: match s.kind.as_str() {
                    "sit" => SpotKind::Sit,
                    "wander" => SpotKind::Wander,
                    _ => SpotKind::Stand,
                },
                facing: s.facing.min(7),
            })
            .filter(|s| planner.passable(s.x as i32, s.y as i32))
            .take(u16::MAX as usize)
            .collect();
        let pets = &map.manifest.residents.pets;
        let total = config.residents as usize + pets.len();
        town.list.reserve_exact(total);
        town.events.reserve(total);
        let hz = town.hz;
        let (sx, sy) = map.spawn;
        for i in 0..total as u32 {
            let rng = &mut town.rng;
            let pet = pets.get((i as usize).wrapping_sub(config.residents as usize));
            // People take a free spot when there is one; pets live near one.
            let free = (0..SPOT_SAMPLES)
                .map(|_| rng.die(town.spots.len().max(1) as u32) as u16 - 1)
                .find(|&s| (s as usize) < town.spots.len() && (pet.is_some() || !town.list.iter().any(|r| r.spot == Some(s))));
            let (x, y, spot) = match free {
                Some(s) if pet.is_none() => (town.spots[s as usize].x, town.spots[s as usize].y, Some(s)),
                Some(s) => match pick_cell(rng, map, town.spots[s as usize].x, town.spots[s as usize].y, WANDER_RANGE) {
                    Some((x, y)) => (x, y, None),
                    None => continue,
                },
                None => match pick_cell(rng, map, sx, sy, SPAWN_RANGE) {
                    Some((x, y)) => (x, y, None),
                    None => continue,
                },
            };
            let (name, archetype, level, look) = match pet {
                Some(p) => (p.name.clone(), format!("pet:{}", p.sprite), 0, Look::default()),
                None => {
                    let look = Look {
                        gender: rng.die(2) as u8 - 1,
                        hair: rng.die(Look::HAIR_STYLES.into()) as u8,
                        hair_color: rng.die(Look::HAIR_COLORS.into()) as u8 - 1,
                        body_color: rng.die(Look::BODY_COLORS.into()) as u8 - 1,
                    };
                    debug_assert!(look.is_valid());
                    let name = NAMES[rng.die(NAMES.len() as u32) as usize - 1].to_string();
                    let archetype = ARCHETYPES[rng.die(ARCHETYPES.len() as u32) as usize - 1].to_string();
                    (name, archetype, rng.die(MAX_LEVEL), look)
                }
            };
            let pace = if pet.is_some() { 0.6 + 0.3 * unit(rng) } else { 0.5 + 0.2 * unit(rng) };
            let emote_in = rng.die(if pet.is_some() { PET_EMOTE_SECS.1 } else { EMOTE_SECS.1 } * hz);
            let mut r = Resident {
                id: RESIDENT_ENTITY_BASE + i,
                name,
                archetype,
                level,
                look,
                pet: pet.is_some(),
                x,
                y,
                facing: rng.die(8) as u8 - 1,
                speed: config.walk_speed * pace / hz as f32,
                state: State::Stand,
                timer: rng.die(IDLE_SECS.1 * hz),
                stuck: 0,
                emote_in,
                path: Vec::new(),
                spot,
                legs: 0,
                home: (x, y),
                follow: Follow::Nobody,
            };
            if spot.is_some() {
                // Already in place (nobody is watching yet: no pose event).
                arrive(&mut r, &town.spots, rng, hz, &mut Vec::new());
            } else if pet.is_none() && map.manifest.residents.spots.is_empty() && rng.chance(25) {
                r.state = State::Sit;
                r.timer = between(rng, FREE_SIT_SECS) * hz;
            }
            town.list.push(r);
        }
        town
    }

    pub fn map(&self) -> MapIdx {
        self.map
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Resident> {
        self.list.iter()
    }

    /// One tick of every resident; emotes and poses land in `events`.
    fn step(&mut self, map: &MapData) {
        self.events.clear();
        let hz = self.hz;
        for i in 0..self.list.len() {
            let r = &mut self.list[i];
            r.emote_in = r.emote_in.saturating_sub(1);
            if r.emote_in == 0 {
                let rng = &mut self.rng;
                let (list, secs) = if r.pet { (PET_EMOTES, PET_EMOTE_SECS) } else { (EMOTES, EMOTE_SECS) };
                let emote = list[rng.die(list.len() as u32) as usize - 1];
                self.events.push(ServerMsg::Emote { entity_id: r.id, emote });
                r.emote_in = between(rng, secs) * hz;
            }
            r.timer = r.timer.saturating_sub(1);
            match r.state {
                State::Walk => {
                    if walk(r, self.planner.w as u32) {
                        arrive(r, &self.spots, &mut self.rng, hz, &mut self.events);
                    } else if r.stuck >= hz {
                        // Should not happen on a planned path; give up the leg.
                        r.stuck = 0;
                        r.path.clear();
                        r.spot = None;
                        r.state = State::Stand;
                        r.timer = hz;
                    }
                }
                State::Stand => {
                    if r.timer > 0 {
                        if r.spot.is_none() && self.rng.die(4 * hz) == 1 {
                            r.facing = self.rng.die(8) as u8 - 1;
                        }
                        continue;
                    }
                    self.decide(i, map);
                }
                State::Sit => {
                    if r.timer > 0 {
                        continue;
                    }
                    self.events.push(ServerMsg::Pose { entity_id: r.id, pose: pose::STAND });
                    r.state = State::Stand;
                    r.timer = 0;
                    self.decide(i, map);
                }
            }
        }
    }

    /// What resident `i` does next, after an idle or a seat.
    fn decide(&mut self, i: usize, map: &MapData) {
        let hz = self.hz;
        if self.list[i].pet {
            return self.decide_pet(i, map);
        }
        let (x, y, legs, spot) = {
            let r = &self.list[i];
            (r.x, r.y, r.legs, r.spot)
        };
        let rng = &mut self.rng;
        // Strolls around a wander spot, or a stroll between spots.
        if legs > 0 || self.spots.is_empty() || rng.chance(25) {
            let r = &mut self.list[i];
            r.legs = r.legs.saturating_sub(1);
            if legs == 0 {
                r.spot = None;
            }
            if self.spots.is_empty() && legs == 0 && rng.chance(25) && free_spot(map, x, y) {
                r.state = State::Sit;
                r.timer = between(rng, FREE_SIT_SECS) * hz;
                self.events.push(ServerMsg::Pose { entity_id: r.id, pose: pose::SIT });
                return;
            }
            let (cx, cy) = match spot.filter(|_| legs > 0) {
                Some(s) => (self.spots[s as usize].x, self.spots[s as usize].y),
                None => (x, y),
            };
            let range = if legs > 0 { WANDER_RANGE } else { STROLL_RANGE };
            let target = pick_cell(rng, map, cx, cy, range);
            if target.is_some_and(|t| self.planner.plan((x, y), t, &mut r.path)) {
                r.state = State::Walk;
                r.stuck = 0;
                return;
            }
            r.state = State::Stand;
            r.timer = between(rng, PAUSE_SECS) * hz;
            return;
        }
        // The nearest of a few free spots.
        let mut best: Option<(u16, f32)> = None;
        for _ in 0..SPOT_SAMPLES {
            let s = rng.die(self.spots.len() as u32) as u16 - 1;
            if Some(s) == spot || self.list.iter().any(|o| o.spot == Some(s)) {
                continue;
            }
            let p = self.spots[s as usize];
            let d = (p.x - x) * (p.x - x) + (p.y - y) * (p.y - y);
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((s, d));
            }
        }
        let r = &mut self.list[i];
        r.spot = None;
        if let Some((s, _)) = best {
            let p = self.spots[s as usize];
            if self.planner.plan((x, y), (p.x, p.y), &mut r.path) {
                r.spot = Some(s);
                r.state = State::Walk;
                r.stuck = 0;
                return;
            }
        }
        // Unreachable or taken: try again soon.
        r.state = State::Stand;
        r.timer = between(rng, PAUSE_SECS) * hz;
    }

    fn decide_pet(&mut self, i: usize, map: &MapData) {
        let hz = self.hz;
        let rng = &mut self.rng;
        let (x, y) = (self.list[i].x, self.list[i].y);
        if self.list[i].legs == 0 {
            // A new outing: tag along with somebody close, or roam home.
            self.list[i].follow = Follow::Nobody;
            if rng.chance(40) {
                let near = |(ox, oy): (f32, f32)| (ox - x).abs() <= FOLLOW_RANGE && (oy - y).abs() <= FOLLOW_RANGE;
                let pick = rng.die(self.list.len() as u32) as usize - 1;
                let o = &self.list[pick];
                if pick != i && !o.pet && near((o.x, o.y)) {
                    self.list[i].follow = Follow::Resident(pick as u16);
                } else if self.players.iter().any(|&p| near(p)) {
                    self.list[i].follow = Follow::Player;
                }
            }
            self.list[i].legs = between(rng, (2, 4)) as u8;
        }
        let leader = match self.list[i].follow {
            Follow::Nobody => None,
            Follow::Resident(j) => Some((self.list[j as usize].x, self.list[j as usize].y)),
            Follow::Player => self
                .players
                .iter()
                .copied()
                .min_by(|a, b| ((a.0 - x).powi(2) + (a.1 - y).powi(2)).total_cmp(&((b.0 - x).powi(2) + (b.1 - y).powi(2)))),
        };
        let r = &mut self.list[i];
        r.legs -= 1;
        let target = match leader {
            // Loosely: somewhere within two cells of them.
            Some((lx, ly)) => pick_cell(rng, map, lx, ly, 2.0),
            None => pick_cell(rng, map, r.home.0, r.home.1, WANDER_RANGE),
        };
        if target.is_some_and(|t| self.planner.plan((x, y), t, &mut r.path)) {
            r.state = State::Walk;
            r.stuck = 0;
            return;
        }
        r.state = State::Stand;
        r.timer = between(rng, PAUSE_SECS) * hz;
    }
}

/// Moves `r` along its path; `true` once it has arrived.
fn walk(r: &mut Resident, w: u32) -> bool {
    let mut budget = r.speed;
    let (ox, oy) = (r.x, r.y);
    while let Some(&cell) = r.path.last() {
        let (tx, ty) = ((cell % w) as f32 + 0.5, (cell / w) as f32 + 0.5);
        let (dx, dy) = (tx - r.x, ty - r.y);
        let d = (dx * dx + dy * dy).sqrt();
        if d > 0.0 {
            r.facing = facing_of(dx, dy);
        }
        if d <= budget {
            (r.x, r.y) = (tx, ty);
            budget -= d;
            r.path.pop();
            continue;
        }
        r.x += dx / d * budget;
        r.y += dy / d * budget;
        break;
    }
    let moved = (r.x - ox).abs() + (r.y - oy).abs();
    r.stuck = if moved < r.speed * 0.1 { r.stuck + 1 } else { 0 };
    r.path.is_empty()
}

/// At the end of a walk: use the spot, or pause.
fn arrive(r: &mut Resident, spots: &[Spot], rng: &mut Rng, hz: u32, events: &mut Vec<ServerMsg>) {
    r.state = State::Stand;
    // Only when the walk ended on the spot itself: the last stroll around a
    // wander spot also ends with `legs == 0` and the spot still held, but
    // somewhere nearby. Snapping onto the spot from there was a warp of up
    // to WANDER_RANGE cells, seen on clients as a stutter and a jump back.
    let on = |s: &Spot| (r.x as i32, r.y as i32) == (s.x as i32, s.y as i32);
    let at = r.spot.filter(|_| r.legs == 0).map(|s| spots[s as usize]).filter(on);
    match at {
        Some(s) if !r.pet => {
            (r.x, r.y) = (s.x, s.y); // already there (cell centre)
            match s.kind {
                SpotKind::Sit => {
                    r.facing = s.facing;
                    r.state = State::Sit;
                    r.timer = between(rng, SIT_SECS) * hz;
                    events.push(ServerMsg::Pose { entity_id: r.id, pose: pose::SIT });
                }
                SpotKind::Stand => {
                    r.facing = s.facing;
                    r.timer = between(rng, SHOP_SECS) * hz;
                    r.emote_in = r.emote_in.min(between(rng, SHOP_EMOTE_SECS) * hz);
                }
                SpotKind::Wander => {
                    r.legs = between(rng, (2, 4)) as u8;
                    r.timer = between(rng, PAUSE_SECS) * hz;
                }
            }
        }
        _ if r.pet => r.timer = between(rng, PET_IDLE_SECS) * hz,
        _ if r.legs > 0 => r.timer = between(rng, PAUSE_SECS) * hz,
        _ => r.timer = between(rng, IDLE_SECS) * hz,
    }
}

impl HubInstance {
    /// Moves the residents while someone is on their map (nobody watching:
    /// they wait where they are) and sends their emotes and poses.
    pub(super) fn residents_step(&mut self, out: &mut Outgoing) {
        let map = self.residents.map();
        if self.residents.is_empty() || !self.players.iter().any(|p| p.map == map) {
            return;
        }
        self.residents.players.clear();
        self.residents.players.extend(self.players.iter().filter(|p| p.map == map).map(|p| (p.x, p.y)));
        let world = Arc::clone(&self.world);
        self.residents.step(world.map(map));
        for msg in &self.residents.events {
            self.broadcast(map, msg, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::test_world;
    use super::super::{HubEvent, HubInstance, HubSimConfig, InstanceKind};
    use super::*;
    use crate::world::map::{PetDef, SpotDef};
    use crate::world::protocol::Snapshot;
    use uuid::Uuid;

    fn profile(user_id: u32) -> super::super::PlayerProfile {
        super::super::PlayerProfile {
            user_id,
            character_id: 100 + user_id,
            name: format!("p{user_id}"),
            archetype: "novice".into(),
            look: Look::default(),
            level: 1,
            exp: 0,
            weapon: String::new(),
            loadout: Default::default(),
            bag: Default::default(),
        }
    }

    fn town(residents: u32) -> HubInstance {
        HubInstance::new(1, Arc::new(test_world()), HubSimConfig { residents, ..Default::default() })
    }

    /// A 30×20 town with an inner wall, spots on both sides and two pets.
    fn route_world() -> HubWorld {
        use super::super::test_support::{map, portal};
        use crate::world::map::{CameraConstraint, LocationKind};
        let mut rows = vec!["#".repeat(30)];
        for row_idx in 1..19 {
            // `from_ascii` puts the first row at the top (highest y).
            let y = 19 - row_idx;
            let mut row = String::from("#");
            for x in 1..29 {
                // A wall at x = 15 with a gap at y = 2.
                row.push(if x == 15 && y != 2 { '#' } else { '.' });
            }
            row.push('#');
            rows.push(row);
        }
        rows.push("#".repeat(30));
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
        let mut m = map("town", LocationKind::Hub, camera, &rows, vec![portal("gate", [2, 10, 1], "field", [1, 1])]);
        let spot = |x: f32, y: f32, kind: &str, facing: u8| SpotDef { x, y, kind: kind.into(), facing, tag: None };
        m.manifest.residents.spots = vec![
            spot(4.0, 15.0, "sit", 2),
            spot(6.0, 15.0, "sit", 2),
            spot(25.0, 15.0, "stand", 6),
            spot(25.0, 5.0, "wander", 0),
            spot(15.0, 10.0, "sit", 0), // inside the wall: dropped
        ];
        m.manifest.residents.pets = vec![PetDef { sprite: "4_cat".into(), name: "Mittens".into() }];
        HubWorld::new(vec![m], "town").unwrap()
    }

    fn snapshot(out: &Outgoing, user: u32) -> Option<Snapshot> {
        out.iter().rev().find_map(|(u, e)| match e {
            HubEvent::Snapshot(b) if *u == user => Snapshot::decode(b),
            _ => None,
        })
    }

    #[test]
    fn residents_appear_in_map_enter_and_snapshots_of_a_hub() {
        let mut hub = town(5);
        assert_eq!(hub.residents.list.len(), 5);
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        assert_eq!(hub.population(), 1, "residents are not counted");
        let listed = out.iter().find_map(|(_, e)| match e {
            HubEvent::Control(ServerMsg::MapEnter { players, .. }) => Some(players.clone()),
            _ => None,
        });
        let listed = listed.expect("map_enter");
        assert_eq!(listed.len(), 5);
        let world = test_world();
        let map = world.map(world.entry());
        for p in &listed {
            assert!(p.entity_id >= RESIDENT_ENTITY_BASE && p.entity_id < RESIDENT_ENTITY_BASE + 5);
            assert!(p.look.is_valid() && (1..=MAX_LEVEL).contains(&p.level));
            assert!(ARCHETYPES.contains(&p.archetype.as_str()));
            assert!(free_spot(map, p.x, p.y), "spawned on a free spot: {p:?}");
        }
        // Sitting residents are announced right after map_enter.
        let sitting = hub.residents.iter().filter(|r| r.is_sitting()).count();
        let poses = out
            .iter()
            .filter(|(u, e)| *u == 1 && matches!(e, HubEvent::Control(ServerMsg::Pose { pose: pose::SIT, .. })))
            .count();
        assert_eq!(poses, sitting);

        out.clear();
        hub.tick(&mut out);
        let s = snapshot(&out, 1).unwrap();
        assert_eq!(s.entities.len(), 6);
        assert!(s.entities.iter().any(|e| e.0 == 101));
        // Nothing is addressed to a resident.
        assert!(out.iter().all(|(u, _)| *u == 1));
    }

    #[test]
    fn stages_have_no_residents() {
        let world = Arc::new(test_world());
        let stage = HubInstance::stage(1, "m1", world, HubSimConfig { residents: 14, ..Default::default() });
        assert!(matches!(stage.kind(), InstanceKind::Stage { .. }));
        assert!(stage.residents.is_empty());
    }

    #[test]
    fn residents_walk_sit_emote_and_stay_off_portals_and_walls() {
        let mut hub = town(8);
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        let start: Vec<(f32, f32)> = hub.residents.iter().map(|r| (r.x, r.y)).collect();
        let world = test_world();
        let map = world.map(world.entry());
        let (mut emotes, mut sits, mut stands) = (0, 0, 0);
        // Two simulated minutes.
        for _ in 0..(120 * 20) {
            out.clear();
            hub.tick(&mut out);
            for (_, e) in &out {
                match e {
                    HubEvent::Control(ServerMsg::Emote { entity_id, emote }) => {
                        assert!(*entity_id >= RESIDENT_ENTITY_BASE && EMOTES.contains(emote));
                        emotes += 1;
                    }
                    HubEvent::Control(ServerMsg::Pose { pose: pose::SIT, .. }) => sits += 1,
                    HubEvent::Control(ServerMsg::Pose { pose: pose::STAND, .. }) => stands += 1,
                    _ => {}
                }
            }
            for r in hub.residents.iter() {
                assert!(map.walk.is_walkable(r.x, r.y), "{} on a wall", r.id);
                assert!(!in_portal(map, r.x, r.y, 0.0), "{} in a portal trigger", r.id);
            }
        }
        let moved = hub.residents.iter().zip(&start).filter(|(r, s)| (r.x, r.y) != **s).count();
        assert!(moved > 0, "somebody walked");
        assert!(emotes > 0 && sits > 0 && stands > 0, "emotes {emotes}, sits {sits}, stands {stands}");
        assert_eq!(hub.population(), 1);
    }

    #[test]
    fn nobody_watching_means_nobody_moves() {
        let mut hub = town(4);
        let start: Vec<(f32, f32)> = hub.residents.iter().map(|r| (r.x, r.y)).collect();
        let mut out = Vec::new();
        for _ in 0..400 {
            hub.tick(&mut out);
        }
        assert!(out.is_empty());
        assert!(hub.residents.iter().zip(&start).all(|(r, s)| (r.x, r.y) == *s));
    }

    #[test]
    fn a_star_goes_around_walls_without_cutting_corners() {
        let world = route_world();
        let map = world.map(world.entry());
        let mut planner = Planner::new(map);
        let mut path = Vec::new();
        assert!(planner.plan((10.5, 10.5), (20.5, 10.5), &mut path));
        let w = map.walk.width();
        let cells: Vec<(i32, i32)> = path.iter().rev().map(|&c| ((c % w) as i32, (c / w) as i32)).collect();
        assert_eq!(*cells.last().unwrap(), (20, 10));
        assert!(cells.contains(&(15, 2)), "through the gap");
        let mut prev = (10, 10);
        for &(x, y) in &cells {
            assert!(planner.passable(x, y));
            let (dx, dy) = (x - prev.0, y - prev.1);
            assert!(dx.abs() <= 1 && dy.abs() <= 1 && (dx, dy) != (0, 0));
            if dx != 0 && dy != 0 {
                assert!(planner.passable(prev.0 + dx, prev.1) && planner.passable(prev.0, prev.1 + dy), "corner cut");
            }
            prev = (x, y);
        }
        assert!(!planner.plan((10.5, 10.5), (15.5, 10.5), &mut path), "into a wall");
        assert!(!planner.passable(2, 10), "portal triggers are walls");
        // Buffers are reused: a second search works the same.
        assert!(planner.plan((20.5, 10.5), (10.5, 10.5), &mut path));
    }

    #[test]
    fn residents_use_authored_spots_and_pets_tag_along() {
        let world = Arc::new(route_world());
        let mut hub = HubInstance::new(1, Arc::clone(&world), HubSimConfig { residents: 3, ..Default::default() });
        assert_eq!(hub.residents.spots.len(), 4, "the spot inside the wall is dropped");
        assert_eq!(hub.residents.list.len(), 4);
        let pet = hub.residents.list.iter().find(|r| r.pet).unwrap();
        assert_eq!((pet.name.as_str(), pet.archetype.as_str(), pet.level), ("Mittens", "pet:4_cat", 0));
        assert_eq!(pet.id, RESIDENT_ENTITY_BASE + 3);
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        let map = world.map(world.entry());
        let mut visited = [false; 4];
        let mut pet_posed = false;
        for _ in 0..(300 * 20) {
            out.clear();
            hub.tick(&mut out);
            pet_posed |= out.iter().any(|(_, e)| matches!(e, HubEvent::Control(ServerMsg::Pose { entity_id, .. }) if *entity_id == RESIDENT_ENTITY_BASE + 3));
            let list = &hub.residents.list;
            for r in list {
                assert!(map.walk.is_walkable(r.x, r.y) && !in_portal(map, r.x, r.y, 0.0), "{} at {},{}", r.id, r.x, r.y);
                if let Some(s) = r.spot {
                    let sp = hub.residents.spots[s as usize];
                    if (r.x, r.y) == (sp.x, sp.y) && r.state != State::Walk {
                        visited[s as usize] = true;
                        if sp.kind == SpotKind::Sit {
                            assert!(r.is_sitting() && r.facing == sp.facing);
                        }
                    }
                }
            }
            // Never two people on one spot.
            for (i, a) in list.iter().enumerate() {
                assert!(a.spot.is_none() || list[i + 1..].iter().all(|b| b.spot != a.spot));
            }
        }
        assert!(visited.iter().filter(|v| **v).count() >= 3, "spots used: {visited:?}");
        assert!(!pet_posed, "pets never sit");
    }

    /// Residents only ever walk: between two ticks nobody moves further
    /// than their pace (a jump reads as a lag spike and a snap backward on
    /// clients). Covers the last stroll around a wander spot, which used to
    /// warp the resident back onto the spot.
    #[test]
    fn residents_never_jump_between_ticks() {
        let world = Arc::new(route_world());
        let config = HubSimConfig { residents: 3, ..Default::default() };
        let mut jumps = Vec::new();
        for seed in 0..12u64 {
            let mut hub = HubInstance::new(1, Arc::clone(&world), config.clone());
            hub.residents = Residents::spawn(&world, &config, seed);
            let mut out = Vec::new();
            hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
            let mut prev: Vec<(f32, f32)> = hub.residents.iter().map(|r| (r.x, r.y)).collect();
            for tick in 0..(300 * 20) {
                out.clear();
                hub.tick(&mut out);
                for (r, p) in hub.residents.iter().zip(&mut prev) {
                    let d = ((r.x - p.0).powi(2) + (r.y - p.1).powi(2)).sqrt();
                    if d > r.speed + 1e-4 {
                        jumps.push((seed, tick, r.id, *p, (r.x, r.y)));
                    }
                    *p = (r.x, r.y);
                }
            }
        }
        assert!(jumps.is_empty(), "{} jumps, first: {:?}", jumps.len(), &jumps[..jumps.len().min(5)]);
    }

    #[test]
    fn fog_limits_hub_snapshots_to_the_light_radius() {
        use super::super::test_support::map;
        use crate::world::map::{CameraConstraint, LocationKind};
        let rows: Vec<String> =
            (0..10).map(|y| if y == 0 || y == 9 { "#".repeat(60) } else { format!("#{}#", ".".repeat(58)) }).collect();
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let camera = CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 };
        let mut m = map("town", LocationKind::Hub, camera, &rows, Vec::new());
        m.manifest.light_radius = 10.0;
        let mut hub = HubInstance::new(1, Arc::new(HubWorld::new(vec![m], "town").unwrap()), HubSimConfig { residents: 0, ..Default::default() });
        let mut out = Vec::new();
        let at = |x: f32| Some(super::super::Arrival { map: "town".into(), x, y: 5.5 });
        hub.join(profile(1), Uuid::nil(), at(2.5), 0, &mut out);
        hub.join(profile(2), Uuid::nil(), at(15.5), 0, &mut out); // 13 cells: within 10 + 4
        hub.join(profile(3), Uuid::nil(), at(30.5), 0, &mut out); // 15 from 2, 28 from 1
        // Identity stays map-wide.
        let enter3 = out.iter().rev().find_map(|(u, e)| match e {
            HubEvent::Control(ServerMsg::MapEnter { players, .. }) if *u == 3 => Some(players.len()),
            _ => None,
        });
        assert_eq!(enter3, Some(2));
        out.clear();
        hub.tick(&mut out);
        let ids = |u: u32| snapshot(&out, u).unwrap().entities.iter().map(|e| e.0).collect::<Vec<_>>();
        assert_eq!(ids(1), vec![101, 102]);
        assert_eq!(ids(2), vec![102, 101]);
        assert_eq!(ids(3), vec![103]);
        assert!(in_sight_box_matches_circle());
    }

    fn in_sight_box_matches_circle() -> bool {
        let r = 14.0f32;
        (-20..=20).all(|i| {
            (-20..=20).all(|j| {
                let (dx, dy) = (i as f32 * 0.9, j as f32 * 0.9);
                super::super::in_sight(dx, dy, r) == (dx * dx + dy * dy <= r * r)
            })
        })
    }
}
