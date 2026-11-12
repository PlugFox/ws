//! Walkable-world simulation: one hub channel of the social town (city +
//! interiors), or one stage instance of a mission.
//!
//! [`HubInstance`] is a pure, deterministic state machine: commands go in,
//! [`Outgoing`] events come out, and [`HubInstance::tick`] advances time by one
//! fixed step. The async actor in [`super::directory`] owns it and routes
//! events to sockets; tests drive it directly.

use super::map::{MapData, MapError};
use super::missions;
use super::protocol::{MapInfo, PlayerInfo, ServerMsg, SnapshotBody};
use super::horde::{Horde, PlayerView};
use bytes::{Bytes, BytesMut};
use std::path::Path;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

pub type MapIdx = u8;

/// Where a portal leads.
#[derive(Clone, Debug, PartialEq)]
pub enum PortalLink {
    /// Another map of the same hub instance.
    Internal { map: MapIdx, arrival: (f32, f32) },
    /// A map outside the hub (combat fields are instanced per mission and
    /// entered through the mission board, not by walking).
    External { target_map: String },
}

/// The static maps one hub channel simulates, with portals resolved.
#[derive(Debug)]
pub struct HubWorld {
    maps: Vec<Arc<MapData>>,
    entry: MapIdx,
    /// `links[map][portal]`, parallel to each manifest's `portals`.
    links: Vec<Vec<PortalLink>>,
}

impl HubWorld {
    /// `maps[0]` need not be the entry; `entry` names it.
    pub fn new(maps: Vec<MapData>, entry: &str) -> Result<Self, MapError> {
        if maps.is_empty() || maps.len() > MapIdx::MAX as usize {
            return Err(MapError::Inconsistent {
                map: entry.into(),
                message: format!("hub needs 1..={} maps, got {}", MapIdx::MAX, maps.len()),
            });
        }
        let maps: Vec<Arc<MapData>> = maps.into_iter().map(Arc::new).collect();
        let index_of = |id: &str| maps.iter().position(|m| m.id() == id).map(|i| i as MapIdx);
        let entry_idx = index_of(entry).ok_or_else(|| MapError::Inconsistent {
            map: entry.into(),
            message: "entry map is not part of the hub".into(),
        })?;

        let mut links = Vec::with_capacity(maps.len());
        for map in &maps {
            let mut map_links = Vec::with_capacity(map.manifest.portals.len());
            for p in &map.manifest.portals {
                let link = match index_of(&p.target_map) {
                    Some(t) => {
                        let target = &maps[t as usize];
                        let [ax, ay] = p.target_pos;
                        if !target.walk.is_walkable_cell(ax as i32, ay as i32) {
                            return Err(MapError::Inconsistent {
                                map: map.id().into(),
                                message: format!(
                                    "portal {} arrives on blocked cell ({ax},{ay}) of {}",
                                    p.name, p.target_map
                                ),
                            });
                        }
                        let arrival = p.arrival();
                        if let Some(back) =
                            target.manifest.portals.iter().find(|q| q.contains(arrival.0, arrival.1))
                        {
                            // Harmless thanks to the re-arm guard, but worth
                            // knowing when tuning the recipe.
                            info!(
                                "portal {}:{} arrives inside trigger {}:{} (guarded)",
                                map.id(),
                                p.name,
                                target.id(),
                                back.name
                            );
                        }
                        PortalLink::Internal { map: t, arrival }
                    }
                    None => PortalLink::External { target_map: p.target_map.clone() },
                };
                map_links.push(link);
            }
            links.push(map_links);
        }
        Ok(Self { maps, entry: entry_idx, links })
    }

    pub fn load(dist_root: &Path, map_ids: &[String], entry: &str) -> Result<Self, MapError> {
        let maps = map_ids
            .iter()
            .map(|id| MapData::load(dist_root, id))
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(maps, entry)
    }

    pub fn map(&self, idx: MapIdx) -> &MapData {
        &self.maps[idx as usize]
    }

    pub fn map_index(&self, id: &str) -> Option<MapIdx> {
        self.maps.iter().position(|m| m.id() == id).map(|i| i as MapIdx)
    }

    pub fn entry(&self) -> MapIdx {
        self.entry
    }

    pub fn link(&self, map: MapIdx, portal: usize) -> &PortalLink {
        &self.links[map as usize][portal]
    }

    pub fn map_count(&self) -> usize {
        self.maps.len()
    }
}

#[derive(Clone, Debug)]
pub struct HubSimConfig {
    pub tick_hz: u32,
    /// Walk speed in cells per second.
    pub walk_speed: f32,
    /// A held direction is dropped after this many ticks without input, so a
    /// frozen client does not walk forever.
    pub input_timeout_ticks: u32,
}

impl Default for HubSimConfig {
    fn default() -> Self {
        Self { tick_hz: 20, walk_speed: 6.0, input_timeout_ticks: 10 }
    }
}

#[derive(Clone, Debug)]
pub struct PlayerProfile {
    pub user_id: u32,
    pub character_id: u32,
    pub name: String,
    pub archetype: String,
}

#[derive(Debug)]
struct HubPlayer {
    profile: PlayerProfile,
    session_id: Uuid,
    map: MapIdx,
    map_epoch: u16,
    x: f32,
    y: f32,
    dir: (f32, f32),
    facing: u8,
    last_seq: u32,
    has_seq: bool,
    last_input_tick: u32,
    /// Set on arrival (and on a denied portal); portals stay inert until the
    /// player has stepped outside every trigger. Without it, arrival points
    /// placed inside the return trigger (as in the baked recipe) would bounce
    /// the player straight back.
    portal_guard: bool,
}

impl HubPlayer {
    fn info(&self) -> PlayerInfo {
        PlayerInfo {
            entity_id: self.profile.character_id,
            name: self.profile.name.clone(),
            archetype: self.profile.archetype.clone(),
            x: self.x,
            y: self.y,
            facing: self.facing,
        }
    }
}

/// Event addressed to one player (by `user_id`).
#[derive(Debug, Clone)]
pub enum HubEvent {
    Control(ServerMsg),
    Snapshot(Bytes),
    /// Internal (not sent to the client): the player picked a mission at a
    /// board; the socket task hands it to the lobby directory.
    MissionChosen { mission: String, public: bool },
    /// Internal: the player left this instance through a portal into a map
    /// owned by someone else (stage → hub). The instance has already removed
    /// the player; the socket task joins the target.
    Transfer { target_map: String, arrival: (f32, f32) },
}

/// What an instance simulates.
#[derive(Clone, Debug, PartialEq)]
pub enum InstanceKind {
    /// Hub channel `Hub-<id>`: external portals are denied (stages are
    /// entered through the mission board).
    Hub { channel: u16 },
    /// Stage instance of a mission: external portals lead back to the hub.
    Stage { stage_id: u32, mission: String },
}

/// Where a joining player appears instead of the entry spawn.
#[derive(Clone, Debug, PartialEq)]
pub struct Arrival {
    pub map: String,
    pub x: f32,
    pub y: f32,
}

pub type Outgoing = Vec<(u32, HubEvent)>;

#[derive(Debug, PartialEq, Eq)]
pub enum JoinOutcome {
    Joined,
    /// Same user was present with another session; it was replaced.
    Replaced,
}

pub struct HubInstance {
    kind: InstanceKind,
    name: String,
    world: Arc<HubWorld>,
    config: HubSimConfig,
    players: Vec<HubPlayer>,
    tick: u32,
    /// Stage monsters (on the world's entry map); `None` in hubs.
    horde: Option<Horde>,
    /// Scratch: player positions on the horde map, and one recipient's
    /// encoded monster entries.
    horde_players: Vec<PlayerView>,
    monster_buf: BytesMut,
}

pub const MISSION_BOARD: &str = "mission_board";

fn hub_error(code: &str, message: impl Into<String>) -> ServerMsg {
    ServerMsg::HubError { code: code.into(), message: message.into() }
}

/// 8-way facing from a direction, 0 = north (+y), clockwise.
pub fn facing_of(dx: f32, dy: f32) -> u8 {
    let angle = dx.atan2(dy); // 0 at +y, positive towards +x
    let step = (angle / std::f32::consts::FRAC_PI_4).round() as i32;
    step.rem_euclid(8) as u8
}

impl HubInstance {
    /// A hub channel.
    pub fn new(channel: u16, world: Arc<HubWorld>, config: HubSimConfig) -> Self {
        Self::with_kind(InstanceKind::Hub { channel }, world, config)
    }

    /// A stage instance of `mission`.
    pub fn stage(stage_id: u32, mission: &str, world: Arc<HubWorld>, config: HubSimConfig) -> Self {
        Self::with_kind(InstanceKind::Stage { stage_id, mission: mission.into() }, world, config)
    }

    fn with_kind(kind: InstanceKind, world: Arc<HubWorld>, config: HubSimConfig) -> Self {
        let name = match &kind {
            InstanceKind::Hub { channel } => format!("Hub-{channel}"),
            InstanceKind::Stage { stage_id, .. } => format!("Stage-{stage_id}"),
        };
        Self {
            kind,
            name,
            world,
            config,
            players: Vec::new(),
            tick: 0,
            horde: None,
            horde_players: Vec::new(),
            monster_buf: BytesMut::new(),
        }
    }

    /// Adds a horde to a stage. Its map must be this instance's world entry.
    pub fn with_horde(mut self, horde: Horde) -> Self {
        debug_assert_eq!(horde.map(), self.world.entry());
        self.horde = Some(horde);
        self
    }

    pub fn horde(&self) -> Option<&Horde> {
        self.horde.as_ref()
    }

    pub fn horde_mut(&mut self) -> Option<&mut Horde> {
        self.horde.as_mut()
    }

    pub fn kind(&self) -> &InstanceKind {
        &self.kind
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn population(&self) -> usize {
        self.players.len()
    }

    pub fn tick_hz(&self) -> u32 {
        self.config.tick_hz
    }

    pub fn current_tick(&self) -> u32 {
        self.tick
    }

    fn find(&self, user_id: u32) -> Option<usize> {
        self.players.iter().position(|p| p.profile.user_id == user_id)
    }

    /// Position of a player: `(map id, x, y)`.
    pub fn position_of(&self, user_id: u32) -> Option<(&str, f32, f32)> {
        self.find(user_id).map(|i| {
            let p = &self.players[i];
            (self.world.map(p.map).id(), p.x, p.y)
        })
    }

    /// Adds a player. `arrival` places them on a given walkable spot (e.g.
    /// the hub side of the stage exit) instead of the entry spawn; an unknown
    /// map or blocked cell falls back to the spawn. `epoch` seeds the
    /// player's map epoch so it keeps increasing across instances.
    pub fn join(
        &mut self,
        profile: PlayerProfile,
        session_id: Uuid,
        arrival: Option<Arrival>,
        epoch: u16,
        out: &mut Outgoing,
    ) -> JoinOutcome {
        let user_id = profile.user_id;
        let outcome = if self.find(user_id).is_some() {
            self.remove(user_id, out);
            JoinOutcome::Replaced
        } else {
            JoinOutcome::Joined
        };
        let placed = arrival.and_then(|a| {
            let idx = self.world.map_index(&a.map)?;
            self.world.map(idx).walk.is_walkable(a.x, a.y).then_some((idx, a.x, a.y))
        });
        let (map, x, y) = placed.unwrap_or_else(|| {
            let entry = self.world.entry();
            let (x, y) = self.world.map(entry).spawn;
            (entry, x, y)
        });
        let player = HubPlayer {
            profile,
            session_id,
            map,
            map_epoch: epoch,
            x,
            y,
            dir: (0.0, 0.0),
            facing: 0,
            last_seq: 0,
            has_seq: false,
            last_input_tick: self.tick,
            portal_guard: true,
        };
        let joined = match &self.kind {
            InstanceKind::Hub { channel } => ServerMsg::HubJoined {
                channel: *channel,
                channel_name: self.name.clone(),
                tick_rate: self.config.tick_hz,
                entity_id: player.profile.character_id,
            },
            InstanceKind::Stage { stage_id, mission } => ServerMsg::StageJoined {
                stage_id: *stage_id,
                mission: mission.clone(),
                tick_rate: self.config.tick_hz,
                entity_id: player.profile.character_id,
                monsters: self.horde.as_ref().map(|h| h.catalog().ids()).unwrap_or_default(),
                light_radius: self.horde.as_ref().map_or(0.0, |h| h.light_radius()),
            },
        };
        out.push((user_id, HubEvent::Control(joined)));
        self.players.push(player);
        let idx = self.players.len() - 1;
        self.announce_arrival(idx, None, out);
        outcome
    }

    /// Removes a player if `session_id` still owns the slot. Returns whether
    /// anything was removed.
    pub fn leave(&mut self, user_id: u32, session_id: Uuid, out: &mut Outgoing) -> bool {
        match self.find(user_id) {
            Some(i) if self.players[i].session_id == session_id => {
                self.remove(user_id, out);
                true
            }
            _ => false,
        }
    }

    /// Unconditional removal (slow consumer eviction).
    pub fn evict(&mut self, user_id: u32, out: &mut Outgoing) -> bool {
        self.remove(user_id, out)
    }

    fn remove(&mut self, user_id: u32, out: &mut Outgoing) -> bool {
        let Some(i) = self.find(user_id) else { return false };
        let p = self.players.swap_remove(i);
        let id = p.profile.character_id;
        for other in self.players.iter().filter(|o| o.map == p.map) {
            out.push((other.profile.user_id, HubEvent::Control(ServerMsg::PlayerVanished { entity_id: id })));
        }
        true
    }

    /// Applies a movement intent. Non-finite vectors and stale `seq` values are
    /// ignored; vectors longer than 1 are normalised.
    pub fn input(&mut self, user_id: u32, seq: u32, dx: f32, dy: f32) {
        let tick = self.tick;
        let Some(i) = self.find(user_id) else { return };
        let p = &mut self.players[i];
        if p.has_seq && seq <= p.last_seq {
            return;
        }
        if !dx.is_finite() || !dy.is_finite() {
            return;
        }
        let len_sq = dx * dx + dy * dy;
        let dir = if len_sq > 1.0 {
            let len = len_sq.sqrt();
            (dx / len, dy / len)
        } else {
            (dx, dy)
        };
        p.dir = dir;
        p.last_seq = seq;
        p.has_seq = true;
        p.last_input_tick = tick;
    }

    /// Opens an NPC's dialog if the player stands within its radius.
    pub fn interact(&mut self, user_id: u32, npc_id: &str, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let p = &self.players[i];
        let map = self.world.map(p.map);
        let reply = match map.manifest.npcs.iter().find(|n| n.id == npc_id) {
            None => hub_error("no_such_npc", format!("no NPC {npc_id} on {}", map.id())),
            Some(n) if !n.in_reach(p.x, p.y) => hub_error("npc_too_far", format!("walk closer to {}", n.name)),
            Some(n) => ServerMsg::NpcDialog {
                npc: n.id.clone(),
                name: n.name.clone(),
                kind: n.kind.clone(),
                missions: if n.kind == MISSION_BOARD { missions::MISSIONS.to_vec() } else { Vec::new() },
            },
        };
        out.push((user_id, HubEvent::Control(reply)));
    }

    /// Validates a mission choice made at a board (position and mission id);
    /// a valid choice goes to the socket task as [`HubEvent::MissionChosen`],
    /// which forms or joins a lobby.
    pub fn select_mission(&mut self, user_id: u32, mission: &str, public: bool, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let p = &self.players[i];
        let at_board = self
            .world
            .map(p.map)
            .manifest
            .npcs
            .iter()
            .any(|n| n.kind == MISSION_BOARD && n.in_reach(p.x, p.y));
        let event = if !at_board {
            HubEvent::Control(hub_error("npc_too_far", "stand at a mission board to pick a mission"))
        } else {
            match missions::find(mission) {
                None => HubEvent::Control(hub_error("unknown_mission", format!("no mission {mission}"))),
                Some(m) => HubEvent::MissionChosen { mission: m.id.into(), public },
            }
        };
        out.push((user_id, event));
    }

    /// Advances the simulation by one fixed step and emits snapshots.
    pub fn tick(&mut self, out: &mut Outgoing) {
        self.tick = self.tick.wrapping_add(1);
        let step = self.config.walk_speed / self.config.tick_hz as f32;
        let timeout = self.config.input_timeout_ticks;
        let world = Arc::clone(&self.world);

        let mut transitions: Vec<(usize, usize)> = Vec::new();
        for (idx, p) in self.players.iter_mut().enumerate() {
            if self.tick.wrapping_sub(p.last_input_tick) > timeout {
                p.dir = (0.0, 0.0);
            }
            let map = world.map(p.map);
            let (dx, dy) = p.dir;
            if dx != 0.0 || dy != 0.0 {
                // Axis-separated moves let the player slide along walls. The
                // step (0.3 cells at defaults) is below one cell, so a move
                // can never tunnel through a one-cell wall.
                let nx = p.x + dx * step;
                if map.walk.is_walkable(nx, p.y) {
                    p.x = nx;
                }
                let ny = p.y + dy * step;
                if map.walk.is_walkable(p.x, ny) {
                    p.y = ny;
                }
                p.facing = facing_of(dx, dy);
            }

            let hit = map.manifest.portals.iter().position(|portal| portal.contains(p.x, p.y));
            match (hit, p.portal_guard) {
                (None, true) => p.portal_guard = false,
                (Some(portal), false) => transitions.push((idx, portal)),
                _ => {}
            }
        }

        // Descending: a stage exit swap-removes the player, which only moves
        // an already-processed (higher) index into its slot.
        for (idx, portal) in transitions.into_iter().rev() {
            self.use_portal(idx, portal, out);
        }
        if let Some(horde) = &mut self.horde {
            // Everyone has the default light radius until upgrades exist.
            let light = horde.light_radius();
            self.horde_players.clear();
            self.horde_players.extend(
                self.players.iter().filter(|p| p.map == horde.map()).map(|p| PlayerView { x: p.x, y: p.y, light }),
            );
            horde.tick_views(self.tick, self.config.tick_hz, &self.horde_players);
        }
        self.emit_snapshots(out);
    }

    fn use_portal(&mut self, idx: usize, portal: usize, out: &mut Outgoing) {
        let world = Arc::clone(&self.world);
        let from = self.players[idx].map;
        let def = &world.map(from).manifest.portals[portal];
        match world.link(from, portal) {
            PortalLink::External { target_map } if matches!(self.kind, InstanceKind::Stage { .. }) => {
                // Leaving the stage: the hub owns the target map.
                let user_id = self.players[idx].profile.user_id;
                let [ax, ay] = def.target_pos;
                self.remove(user_id, out);
                out.push((
                    user_id,
                    HubEvent::Transfer { target_map: target_map.clone(), arrival: (ax as f32 + 0.5, ay as f32 + 0.5) },
                ));
            }
            PortalLink::External { target_map } => {
                let p = &mut self.players[idx];
                p.portal_guard = true;
                out.push((
                    p.profile.user_id,
                    HubEvent::Control(ServerMsg::PortalDenied {
                        portal: def.name.clone(),
                        target_map: target_map.clone(),
                        reason: "stage_requires_mission".into(),
                    }),
                ));
            }
            PortalLink::Internal { map, arrival } => {
                let id = self.players[idx].profile.character_id;
                for other in self.players.iter().filter(|o| o.map == from && o.profile.character_id != id) {
                    out.push((other.profile.user_id, HubEvent::Control(ServerMsg::PlayerVanished { entity_id: id })));
                }
                let p = &mut self.players[idx];
                p.map = *map;
                p.map_epoch = p.map_epoch.wrapping_add(1);
                (p.x, p.y) = *arrival;
                p.dir = (0.0, 0.0);
                p.portal_guard = true;
                self.announce_arrival(idx, Some(def.name.clone()), out);
            }
        }
    }

    /// Sends `map_enter` to the player and `player_appeared` to the others on
    /// that map.
    fn announce_arrival(&self, idx: usize, via_portal: Option<String>, out: &mut Outgoing) {
        let p = &self.players[idx];
        let others: Vec<&HubPlayer> = self
            .players
            .iter()
            .filter(|o| o.map == p.map && o.profile.user_id != p.profile.user_id)
            .collect();
        out.push((
            p.profile.user_id,
            HubEvent::Control(ServerMsg::MapEnter {
                map: MapInfo::of(self.world.map(p.map)),
                map_epoch: p.map_epoch,
                x: p.x,
                y: p.y,
                via_portal,
                players: others.iter().map(|o| o.info()).collect(),
            }),
        ));
        let info = p.info();
        for o in others {
            out.push((o.profile.user_id, HubEvent::Control(ServerMsg::PlayerAppeared { player: info.clone() })));
        }
    }

    fn emit_snapshots(&mut self, out: &mut Outgoing) {
        for map in 0..self.world.map_count() as MapIdx {
            let on_map = || self.players.iter().filter(move |p| p.map == map);
            let count = on_map().count();
            if count == 0 {
                continue;
            }
            let mut body = SnapshotBody::with_capacity(count);
            for p in on_map() {
                body.push(p.profile.character_id, p.x, p.y, p.facing);
            }
            let horde = self.horde.as_ref().filter(|h| h.map() == map);
            for p in on_map() {
                let frame = match horde {
                    Some(h) => {
                        self.monster_buf.clear();
                        let viewer = PlayerView { x: p.x, y: p.y, light: h.light_radius() };
                        let n = h.write_visible(&viewer, &mut self.monster_buf);
                        body.stage_frame(p.map_epoch, self.tick, p.last_seq, &self.monster_buf, n)
                    }
                    None => body.frame(p.map_epoch, self.tick, p.last_seq),
                };
                out.push((p.profile.user_id, HubEvent::Snapshot(frame)));
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use crate::world::map::{
        CameraConstraint, Dimensions, HeightBounds, Heightmap, LocationKind, LocationManifest, PortalDef, WalkGrid,
    };

    pub fn portal(name: &str, trigger: [u32; 3], target_map: &str, target_pos: [u32; 2]) -> PortalDef {
        PortalDef { name: name.into(), trigger, target_map: target_map.into(), target_pos }
    }

    pub fn map(id: &str, kind: LocationKind, camera: CameraConstraint, rows: &[&str], portals: Vec<PortalDef>) -> MapData {
        let walk = WalkGrid::from_ascii(rows);
        let (w, h) = (walk.width(), walk.height());
        let manifest = LocationManifest {
            id: id.into(),
            name: id.into(),
            kind,
            dimensions: Dimensions { width: w, height: h, cell_size: 0.5 },
            height_bounds: HeightBounds { min: 0.0, max: 0.0 },
            camera,
            portals,
            bgm: None,
            npcs: Vec::new(),
        };
        MapData::new(manifest, walk, Heightmap::flat(w, h)).unwrap()
    }

    /// 12×7 town with a door at (10,3) into a 6×5 house, and a field exit at
    /// (1,3). Arrival points are deliberately inside the return triggers.
    pub fn test_world() -> HubWorld {
        let mut town = map(
            "town",
            LocationKind::Hub,
            CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
            &[
                "############",
                "#..........#",
                "#..........#",
                "#..........#",
                "#..........#",
                "#..........#",
                "############",
            ],
            vec![
                portal("to_house", [10, 3, 1], "house", [2, 2]),
                portal("to_field", [1, 3, 1], "field", [5, 5]),
            ],
        );
        let house = map(
            "house",
            LocationKind::Indoor,
            CameraConstraint::Fixed { yaw: 0.0, pitch: -45.0, zoom: 18.0 },
            &["######", "#....#", "#....#", "#....#", "######"],
            vec![portal("exit", [1, 2, 1], "town", [9, 3])],
        );
        town.manifest.npcs.push(crate::world::map::NpcDef {
            id: "board".into(),
            sprite: "mission_npc".into(),
            name: "Board Keeper".into(),
            kind: MISSION_BOARD.into(),
            x: 2,
            y: 5,
            facing: 4,
            interact_radius: 1.5,
        });
        HubWorld::new(vec![town, house], "town").unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::test_support::*;
    use crate::world::map::{CameraConstraint, LocationKind};
    use crate::world::protocol::Snapshot;

    fn profile(user_id: u32) -> PlayerProfile {
        PlayerProfile { user_id, character_id: 100 + user_id, name: format!("p{user_id}"), archetype: "novice".into() }
    }

    fn controls_for(out: &Outgoing, user: u32) -> Vec<ServerMsg> {
        out.iter()
            .filter(|(u, _)| *u == user)
            .filter_map(|(_, e)| match e {
                HubEvent::Control(m) => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    fn last_snapshot(out: &Outgoing, user: u32) -> Option<Snapshot> {
        out.iter().rev().find_map(|(u, e)| match e {
            HubEvent::Snapshot(b) if *u == user => Snapshot::decode(b),
            _ => None,
        })
    }

    /// Holds a direction until `pred` holds or `max_ticks` pass.
    fn walk_until(
        hub: &mut HubInstance,
        user: u32,
        seq: &mut u32,
        dir: (f32, f32),
        max_ticks: u32,
        out: &mut Outgoing,
        pred: impl Fn(&HubInstance) -> bool,
    ) -> bool {
        for _ in 0..max_ticks {
            *seq += 1;
            hub.input(user, *seq, dir.0, dir.1);
            hub.tick(out);
            if pred(hub) {
                return true;
            }
        }
        false
    }

    #[test]
    fn mission_board_requires_standing_next_to_it() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), HubSimConfig::default());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        out.clear();
        let code = |out: &Outgoing| match controls_for(out, 1).last() {
            Some(ServerMsg::HubError { code, .. }) => code.clone(),
            other => format!("{other:?}"),
        };

        hub.interact(1, "board", &mut out);
        assert_eq!(code(&out), "npc_too_far");
        hub.select_mission(1, "poring_island_survival", true, &mut out);
        assert_eq!(code(&out), "npc_too_far");
        hub.interact(1, "nobody", &mut out);
        assert_eq!(code(&out), "no_such_npc");

        // Walk north-west to the board at (2,5).
        for seq in 1..=20 {
            hub.input(1, seq, -1.0, 1.0);
            hub.tick(&mut out);
        }
        out.clear();
        hub.interact(1, "board", &mut out);
        match controls_for(&out, 1).last() {
            Some(ServerMsg::NpcDialog { npc, kind, missions, .. }) => {
                assert_eq!(npc, "board");
                assert_eq!(kind, MISSION_BOARD);
                assert_eq!(missions[0].map, "prt_fild08");
            }
            other => panic!("expected npc_dialog, got {other:?}"),
        }
        hub.select_mission(1, "nope", true, &mut out);
        assert_eq!(code(&out), "unknown_mission");
        out.clear();
        hub.select_mission(1, "poring_island_survival", false, &mut out);
        assert!(
            matches!(&out[..], [(1, HubEvent::MissionChosen { mission, public: false })] if mission == "poring_island_survival"),
            "{out:?}"
        );
    }

    #[test]
    fn join_sends_joined_then_map_enter_with_camera() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), HubSimConfig::default());
        let mut out = Vec::new();
        assert_eq!(hub.join(profile(1), Uuid::nil(), None, 0, &mut out), JoinOutcome::Joined);
        let msgs = controls_for(&out, 1);
        assert!(matches!(&msgs[0], ServerMsg::HubJoined { channel: 1, channel_name, entity_id: 101, .. } if channel_name == "Hub-1"));
        match &msgs[1] {
            ServerMsg::MapEnter { map, map_epoch: 0, via_portal: None, players, .. } => {
                assert_eq!(map.id, "town");
                assert!(matches!(map.camera, CameraConstraint::Free { .. }));
                assert!(players.is_empty());
            }
            other => panic!("expected map_enter, got {other:?}"),
        }
    }

    #[test]
    fn walls_block_and_player_slides_along_them() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), HubSimConfig::default());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        let mut seq = 0;
        // Walk north-west into the corner for 3 s: must stop at the walls.
        walk_until(&mut hub, 1, &mut seq, (-1.0, 1.0), 60, &mut out, |_| false);
        let (_, x, y) = hub.position_of(1).unwrap();
        assert!((1.0..1.5).contains(&x), "x={x}");
        assert!(y < 6.0 && y > 5.5, "y={y}");
        // Now slide east along the north wall.
        let before_y = y;
        walk_until(&mut hub, 1, &mut seq, (0.7, 0.7), 10, &mut out, |_| false);
        let (_, x2, y2) = hub.position_of(1).unwrap();
        assert!(x2 > x + 1.0, "slid east: {x} -> {x2}");
        assert!((y2 - before_y).abs() < 0.5 && y2 < 6.0);
    }

    #[test]
    fn input_validation_and_timeout() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), HubSimConfig::default());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        let (_, x0, y0) = hub.position_of(1).unwrap();

        hub.input(1, 1, f32::NAN, 0.0);
        hub.tick(&mut out);
        assert_eq!(hub.position_of(1).unwrap().1, x0, "NaN ignored");

        hub.input(1, 5, 100.0, 0.0); // normalised to speed 1
        hub.input(1, 4, -1.0, 0.0); // stale seq ignored
        hub.tick(&mut out);
        let (_, x1, _) = hub.position_of(1).unwrap();
        assert!((x1 - x0 - 0.3).abs() < 1e-4, "one step of 6 cells/s at 20 Hz, got {}", x1 - x0);
        assert_eq!(last_snapshot(&out, 1).unwrap().ack_seq, 5);

        // No further input: direction expires after the timeout.
        for _ in 0..30 {
            hub.tick(&mut out);
        }
        let (_, xa, _) = hub.position_of(1).unwrap();
        for _ in 0..5 {
            hub.tick(&mut out);
        }
        assert_eq!(hub.position_of(1).unwrap().1, xa, "stopped after timeout");
        let _ = y0;
    }

    #[test]
    fn portal_moves_player_and_guard_prevents_bounce() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), HubSimConfig::default());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        let mut seq = 0;
        out.clear();
        let entered = walk_until(&mut hub, 1, &mut seq, (1.0, 0.0), 100, &mut out, |h| {
            h.position_of(1).unwrap().0 == "house"
        });
        assert!(entered, "walked east into the door");
        let enter = controls_for(&out, 1)
            .into_iter()
            .find(|m| matches!(m, ServerMsg::MapEnter { .. }))
            .unwrap();
        match enter {
            ServerMsg::MapEnter { map, map_epoch, x, y, via_portal, .. } => {
                assert_eq!(map.id, "house");
                assert_eq!(map.camera, CameraConstraint::Fixed { yaw: 0.0, pitch: -45.0, zoom: 18.0 });
                assert_eq!(map_epoch, 1);
                assert_eq!((x, y), (2.5, 2.5));
                assert_eq!(via_portal.as_deref(), Some("to_house"));
            }
            _ => unreachable!(),
        }
        assert_eq!(last_snapshot(&out, 1).unwrap().map_epoch, 1, "snapshots follow the new epoch");

        // Arrived inside the exit trigger: idling there must not bounce back.
        for _ in 0..40 {
            hub.tick(&mut out);
        }
        assert_eq!(hub.position_of(1).unwrap().0, "house");

        // Step out of the trigger (east), then walk back west into it.
        assert!(walk_until(&mut hub, 1, &mut seq, (1.0, 0.0), 20, &mut out, |h| h.position_of(1).unwrap().1 > 3.6));
        assert!(walk_until(&mut hub, 1, &mut seq, (-1.0, 0.0), 40, &mut out, |h| {
            h.position_of(1).unwrap().0 == "town"
        }));
        let (_, x, y) = hub.position_of(1).unwrap();
        assert_eq!((x, y), (9.5, 3.5));
    }

    #[test]
    fn external_portal_is_denied_once() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), HubSimConfig::default());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        let mut seq = 0;
        out.clear();
        walk_until(&mut hub, 1, &mut seq, (-1.0, 0.0), 80, &mut out, |_| false);
        let denials: Vec<_> = controls_for(&out, 1)
            .into_iter()
            .filter(|m| matches!(m, ServerMsg::PortalDenied { .. }))
            .collect();
        assert_eq!(denials.len(), 1, "standing in the trigger must not spam: {denials:?}");
        assert!(matches!(&denials[0], ServerMsg::PortalDenied { target_map, .. } if target_map == "field"));
        assert_eq!(hub.position_of(1).unwrap().0, "town");
    }

    #[test]
    fn visibility_is_per_map() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), HubSimConfig::default());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        out.clear();
        hub.join(profile(2), Uuid::nil(), None, 0, &mut out);
        // 1 sees 2 appear; 2's map_enter lists 1.
        assert!(controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::PlayerAppeared { player } if player.entity_id == 102)));
        assert!(controls_for(&out, 2).iter().any(|m| matches!(m, ServerMsg::MapEnter { players, .. } if players.len() == 1)));

        // 2 walks into the house: 1 sees it vanish; snapshots split by map.
        let mut seq = 0;
        out.clear();
        assert!(walk_until(&mut hub, 2, &mut seq, (1.0, 0.0), 100, &mut out, |h| h.position_of(2).unwrap().0 == "house"));
        assert!(controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::PlayerVanished { entity_id: 102 })));
        out.clear();
        hub.tick(&mut out);
        let s1 = last_snapshot(&out, 1).unwrap();
        let s2 = last_snapshot(&out, 2).unwrap();
        assert_eq!(s1.entities.iter().map(|e| e.0).collect::<Vec<_>>(), vec![101]);
        assert_eq!(s2.entities.iter().map(|e| e.0).collect::<Vec<_>>(), vec![102]);

        // Leaving notifies only players on the same map.
        out.clear();
        assert!(hub.leave(2, Uuid::nil(), &mut out));
        assert!(controls_for(&out, 1).is_empty());
        assert_eq!(hub.population(), 1);
    }

    #[test]
    fn leave_with_stale_session_is_ignored_and_rejoin_replaces() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), HubSimConfig::default());
        let (old, new) = (Uuid::from_u128(1), Uuid::from_u128(2));
        let mut out = Vec::new();
        hub.join(profile(1), old, None, 0, &mut out);
        assert_eq!(hub.join(profile(1), new, None, 0, &mut out), JoinOutcome::Replaced);
        assert_eq!(hub.population(), 1);
        assert!(!hub.leave(1, old, &mut out), "old socket's late leave must not kick the new one");
        assert!(hub.leave(1, new, &mut out));
        assert_eq!(hub.population(), 0);
    }

    /// One-map stage whose west exit leads to the hub's `town` at (3,3).
    fn stage_world() -> HubWorld {
        let field = map(
            "field",
            LocationKind::Field,
            CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
            &["#########", "#.......#", "#.......#", "#.......#", "#########"],
            vec![portal("to_town", [1, 2, 1], "town", [3, 3])],
        );
        HubWorld::new(vec![field], "field").unwrap()
    }

    #[test]
    fn stage_join_announces_stage_and_keeps_epoch_rising() {
        let mut stage = HubInstance::stage(7, "m1", Arc::new(stage_world()), HubSimConfig::default());
        let mut out = Vec::new();
        stage.join(profile(1), Uuid::nil(), None, 5, &mut out);
        let msgs = controls_for(&out, 1);
        assert!(matches!(&msgs[0], ServerMsg::StageJoined { stage_id: 7, mission, entity_id: 101, .. } if mission == "m1"));
        assert!(matches!(&msgs[1], ServerMsg::MapEnter { map, map_epoch: 5, .. } if map.id == "field"));
        assert_eq!(stage.name(), "Stage-7");
    }

    #[test]
    fn stage_exit_portal_transfers_player_out() {
        let mut stage = HubInstance::stage(1, "m1", Arc::new(stage_world()), HubSimConfig::default());
        let mut out = Vec::new();
        stage.join(profile(1), Uuid::nil(), None, 0, &mut out);
        stage.join(profile(2), Uuid::nil(), None, 0, &mut out);
        let mut seq = 0;
        out.clear();
        assert!(walk_until(&mut stage, 1, &mut seq, (-1.0, 0.0), 80, &mut out, |s| s.position_of(1).is_none()));
        assert!(
            out.iter().any(|(u, e)| *u == 1
                && matches!(e, HubEvent::Transfer { target_map, arrival } if target_map == "town" && *arrival == (3.5, 3.5))),
            "{out:?}"
        );
        assert!(controls_for(&out, 2).iter().any(|m| matches!(m, ServerMsg::PlayerVanished { entity_id: 101 })));
        assert_eq!(stage.population(), 1);
    }

    #[test]
    fn stage_with_horde_sends_monsters_in_view() {
        use crate::world::horde::catalog::{test_def, MonsterCatalog};
        use crate::world::horde::{Horde, HordeConfig};
        let world = Arc::new(stage_world());
        let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 2.0, 1, 0.0, false)]).unwrap());
        let cfg = HordeConfig { light_radius: 3.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
        let horde = Horde::new(Arc::clone(&world), catalog, cfg);
        let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde);
        let mut out = Vec::new();
        stage.join(profile(1), Uuid::nil(), None, 0, &mut out);
        assert!(controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::StageJoined { monsters, .. } if monsters == &["blob"])));
        let (_, px, py) = stage.position_of(1).unwrap();
        let near = stage.horde_mut().unwrap().spawn(0, px + 2.0, py).unwrap();
        out.clear();
        stage.tick(&mut out);
        let snap = last_snapshot(&out, 1).unwrap();
        assert_eq!(snap.monsters.len(), 1);
        assert_eq!(snap.monsters[0].slot, near);
        assert!(snap.monsters[0].x < px + 2.0, "it moved towards the player");
        // Monsters stand still while nobody is on the map.
        stage.leave(1, Uuid::nil(), &mut out);
        let before: Vec<_> = stage.horde().unwrap().monsters().collect();
        stage.tick(&mut out);
        assert_eq!(stage.horde().unwrap().monsters().collect::<Vec<_>>(), before);
    }

    #[test]
    fn hub_join_with_arrival_places_player() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), HubSimConfig::default());
        let mut out = Vec::new();
        let arrival = Arrival { map: "town".into(), x: 3.5, y: 3.5 };
        hub.join(profile(1), Uuid::nil(), Some(arrival), 2, &mut out);
        assert_eq!(hub.position_of(1).unwrap(), ("town", 3.5, 3.5));
        // A blocked or unknown arrival falls back to the entry spawn.
        let wall = Arrival { map: "town".into(), x: 0.5, y: 0.5 };
        hub.join(profile(2), Uuid::nil(), Some(wall), 0, &mut out);
        let spawn = hub.world.map(hub.world.entry()).spawn;
        let (_, x, y) = hub.position_of(2).unwrap();
        assert_eq!((x, y), spawn);
    }

    #[test]
    fn facing_directions() {
        assert_eq!(facing_of(0.0, 1.0), 0);
        assert_eq!(facing_of(1.0, 1.0), 1);
        assert_eq!(facing_of(1.0, 0.0), 2);
        assert_eq!(facing_of(0.0, -1.0), 4);
        assert_eq!(facing_of(-1.0, 0.0), 6);
        assert_eq!(facing_of(-1.0, 1.0), 7);
    }

    #[test]
    fn world_rejects_portal_into_wall_and_unknown_entry() {
        let town = || {
            map(
                "town",
                LocationKind::Hub,
                CameraConstraint::Fixed { yaw: 0.0, pitch: -45.0, zoom: 18.0 },
                &["#####", "#...#", "#####"],
                vec![portal("p", [1, 1, 1], "house", [0, 0])],
            )
        };
        let house = || {
            map(
                "house",
                LocationKind::Indoor,
                CameraConstraint::Fixed { yaw: 0.0, pitch: -45.0, zoom: 18.0 },
                &["###", "#.#", "###"],
                vec![],
            )
        };
        assert!(HubWorld::new(vec![town(), house()], "town").is_err(), "arrival (0,0) is a wall");
        assert!(HubWorld::new(vec![house()], "town").is_err(), "entry missing");
    }
}
