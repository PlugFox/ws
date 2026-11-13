//! Walkable-world simulation: one hub channel of the social town (city +
//! interiors), or one stage instance of a mission.
//!
//! [`HubInstance`] is a pure, deterministic state machine: commands go in,
//! [`Outgoing`] events come out, and [`HubInstance::tick`] advances time by one
//! fixed step. The async actor in [`super::directory`] owns it and routes
//! events to sockets; tests drive it directly.

use super::map::{MapData, MapError};
use super::missions;
use super::protocol::{pose, Look, MapInfo, PlayerInfo, ServerMsg, SnapshotBody, MAX_EMOTE, MAX_FLAG};
use super::horde::view::MonsterView;
use super::horde::{Horde, PlayerView};
use super::combat::{self, Class, Loadout, SkillBook};
use bytes::{Bytes, BytesMut};
use std::path::Path;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

pub type MapIdx = u8;

mod aim;
mod area;
mod brain;
mod cast;
mod fight;
mod mission;
mod residents;
pub use fight::{Fighter, Vital};

/// Where a portal leads.
#[derive(Clone, Debug, PartialEq)]
pub enum PortalLink {
    /// Another map of the same hub instance.
    Internal { map: MapIdx, arrival: (f32, f32) },
    /// A map outside the hub (combat fields are instanced per mission and
    /// entered through a Kafra employee, not by walking).
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
    /// Test hook (`GAME_DEV=1`): a mission can be picked anywhere in town.
    pub missions_anywhere: bool,
    /// Simulated townsfolk walking the entry map of every hub channel
    /// (none on stages).
    pub residents: u32,
}

impl Default for HubSimConfig {
    fn default() -> Self {
        Self { tick_hz: 20, walk_speed: 6.0, input_timeout_ticks: 10, missions_anywhere: false, residents: 14 }
    }
}

#[derive(Clone, Debug)]
pub struct PlayerProfile {
    pub user_id: u32,
    pub character_id: u32,
    pub name: String,
    pub archetype: String,
    pub look: Look,
    pub level: u32,
    /// Experience into the current level.
    pub exp: u64,
    /// Held weapon id (`combat::weapon`); empty = the class's starting one.
    pub weapon: String,
    /// Learned skills and the hotbar.
    pub loadout: Loadout,
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
    /// What this player's client knows about the horde (stages only).
    monsters: Option<Box<MonsterView>>,
    /// Combat state on stages with monsters (`None` in hubs).
    fighter: Option<Fighter>,
    /// Latest aim (1/8 m from the player) and the skill slots pressed since
    /// the last tick.
    aim: [i8; 2],
    pressed: u16,
    /// Sitting (hubs only); any movement stands the player up.
    sitting: bool,
    /// Earliest ticks of the next emote and flag (rate limits).
    emote_ready: u32,
    flag_ready: u32,
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
            look: self.profile.look,
            level: self.fighter.as_ref().map_or(self.profile.level, |f| f.level),
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
    /// Internal: the player restyled; the socket task persists the look
    /// (`player_look` goes out separately).
    LookChanged(Look),
    /// Internal: level or experience changed; the socket task updates the
    /// session's character (persisted by the flusher).
    Progress { level: u32, exp: u64 },
    /// Internal: the player stands at a skill master; the socket task
    /// resets the loadout.
    SkillResetApproved,
}

/// What an instance simulates.
#[derive(Clone, Debug, PartialEq)]
pub enum InstanceKind {
    /// Hub channel `Hub-<id>`: external portals are denied (stages are
    /// entered through a Kafra employee).
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
    /// encoded monster section.
    horde_players: Vec<PlayerView>,
    monster_buf: BytesMut,
    /// Dice of this instance's combat.
    dice: combat::Rng,
    /// Wave count at the last combat step (the dead return on a new wave).
    last_wave: u32,
    /// The skill table (empty without assets).
    skills: Arc<SkillBook>,
    /// Skill areas in flight and on the ground.
    lives: Vec<cast::Live>,
    /// Next telegraph id of lasting ground.
    next_zone: u32,
    /// Monster skills: per-slot memory, wind-ups, effects on the ground,
    /// skill indices per monster kind.
    minds: Vec<brain::Mind>,
    mob_casts: Vec<brain::MobCast>,
    hazards: Vec<brain::Hazard>,
    kind_skills: Vec<Vec<u16>>,
    /// Monsters the party killed this stage, the boss bar, the outcome.
    kills: u32,
    boss: Option<mission::BossBar>,
    outcome: mission::Outcome,
    /// Simulated townsfolk on the entry map (hub channels only).
    residents: residents::Residents,
    /// Scratch: the player block of the snapshot being built.
    snapshot_body: SnapshotBody,
}

pub const MISSION_BOARD: &str = "mission_board";
/// NPC kind of the Kafra employee: missions, skill resets and restyling.
pub const KAFRA: &str = "kafra";
/// NPC kind of the changelog board (talk only).
pub const CHANGELOG: &str = "changelog";
/// NPC kind that restyles hair and dyes (`style_set`).
pub const STYLIST: &str = "stylist";
/// NPC kind that resets learned skills (`skill_reset`).
pub const SKILL_MASTER: &str = "skill_master";

/// Whether talking to an NPC of this kind lists the missions.
pub fn gives_missions(kind: &str) -> bool {
    kind == MISSION_BOARD || kind == KAFRA
}

/// Minimum time between two emotes and between two flags of a player (s).
const EMOTE_GAP_SECS: f32 = 0.5;
const FLAG_GAP_SECS: f32 = 1.0;
/// How far from the player a flag may be planted (cells).
const FLAG_RANGE: f32 = 40.0;

/// Hub snapshots reach this far past the map's light radius (cells), so
/// entities fade in before they enter the light.
const SIGHT_MARGIN: f32 = 4.0;

/// Whether `(dx, dy)` lies within `r`: box reject, inscribed-square accept,
/// squared distance only for the band in between (no sqrt).
fn in_sight(dx: f32, dy: f32, r: f32) -> bool {
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
        let residents = match kind {
            InstanceKind::Hub { .. } => residents::Residents::spawn(&world, &config, rand::random()),
            InstanceKind::Stage { .. } => residents::Residents::none(world.entry()),
        };
        Self {
            residents,
            snapshot_body: SnapshotBody::default(),
            kind,
            name,
            world,
            config,
            players: Vec::new(),
            tick: 0,
            horde: None,
            dice: combat::Rng::new(rand::random()),
            last_wave: 0,
            skills: Arc::new(SkillBook::default()),
            lives: Vec::new(),
            next_zone: 1,
            minds: Vec::new(),
            mob_casts: Vec::new(),
            hazards: Vec::new(),
            kind_skills: Vec::new(),
            kills: 0,
            boss: None,
            outcome: mission::Outcome::Running,
            horde_players: Vec::new(),
            monster_buf: BytesMut::new(),
        }
    }

    /// Adds a horde to a stage. Its map must be this instance's world entry.
    /// Skills cast from this table; the default is empty.
    pub fn with_skills(mut self, skills: Arc<SkillBook>) -> Self {
        self.skills = skills;
        self
    }

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
            monsters: None,
            fighter: None,
            aim: [0, 0],
            pressed: 0,
            sitting: false,
            emote_ready: 0,
            flag_ready: 0,
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
        let mut player = player;
        if self.horde.is_some() {
            let class = Class::from_id(&player.profile.archetype).unwrap_or(Class::Swordsman);
            let mut f = Fighter::new(class, player.profile.level, player.profile.exp, &player.profile.weapon);
            cast::equip(&mut f, &self.skills, &player.profile.loadout);
            player.fighter = Some(f);
        }
        self.players.push(player);
        let idx = self.players.len() - 1;
        self.announce_arrival(idx, None, out);
        self.announce_fighter(idx, out);
        out.push((user_id, HubEvent::Control(self.skills_msg(idx))));
        outcome
    }

    /// The player's skills, points and hotbar.
    fn skills_msg(&self, idx: usize) -> ServerMsg {
        let p = &self.players[idx];
        let level = p.fighter.as_ref().map_or(p.profile.level, |f| f.level);
        let l = &p.profile.loadout;
        ServerMsg::Skills {
            level,
            points: self.skills.points_left(level, l),
            skills: l.skills.iter().map(|(k, v)| (k.clone(), *v)).collect(),
            hotbar: (0..combat::skills::SLOTS).map(|i| l.slot(i).to_string()).collect(),
        }
    }

    /// New learned skills or hotbar (validated by the caller against the
    /// book); the fighter picks them up at once, the client gets `skills`.
    pub fn set_loadout(&mut self, user_id: u32, loadout: Loadout, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let p = &mut self.players[i];
        p.profile.loadout = loadout;
        if let Some(f) = p.fighter.as_mut() {
            cast::equip(f, &self.skills, &p.profile.loadout);
        }
        out.push((user_id, HubEvent::Control(self.skills_msg(i))));
    }

    /// Aim and skill slots of an accepted input (see [`HubInstance::input`]).
    pub fn act(&mut self, user_id: u32, aim: [i8; 2], slots: u16) {
        let Some(i) = self.find(user_id) else { return };
        let p = &mut self.players[i];
        p.aim = aim;
        p.pressed |= slots;
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
    /// Returns whether the input was accepted.
    pub fn input(&mut self, user_id: u32, seq: u32, dx: f32, dy: f32) -> bool {
        let tick = self.tick;
        let Some(i) = self.find(user_id) else { return false };
        let p = &mut self.players[i];
        if p.has_seq && seq <= p.last_seq {
            return false;
        }
        if !dx.is_finite() || !dy.is_finite() {
            return false;
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
        true
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
                missions: if gives_missions(&n.kind) { missions::MISSIONS.to_vec() } else { Vec::new() },
            },
        };
        out.push((user_id, HubEvent::Control(reply)));
    }

    /// A skill reset is allowed only next to a skill master (or a Kafra).
    pub fn skill_reset(&mut self, user_id: u32, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let p = &self.players[i];
        let near = self.world.map(p.map).manifest.npcs.iter().any(|n| (n.kind == SKILL_MASTER || n.kind == KAFRA) && n.in_reach(p.x, p.y));
        let event = if near {
            HubEvent::SkillResetApproved
        } else {
            HubEvent::Control(hub_error("npc_too_far", "talk to a skill master to reset your skills"))
        };
        out.push((user_id, event));
    }

    /// Restyles a player standing at a stylist (or a Kafra): hair style and dyes change,
    /// the gender is kept. Everyone on the map (the player too) gets
    /// `player_look`; the socket task persists it via [`HubEvent::LookChanged`].
    pub fn restyle(&mut self, user_id: u32, look: Look, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let p = &self.players[i];
        let look = Look { gender: p.profile.look.gender, ..look };
        if !look.is_valid() {
            out.push((user_id, HubEvent::Control(hub_error("bad_message", "no such style"))));
            return;
        }
        let at_stylist = self.world.map(p.map).manifest.npcs.iter().any(|n| (n.kind == STYLIST || n.kind == KAFRA) && n.in_reach(p.x, p.y));
        if !at_stylist {
            out.push((user_id, HubEvent::Control(hub_error("npc_too_far", "stand next to a stylist to restyle"))));
            return;
        }
        let (map, entity_id) = (p.map, p.profile.character_id);
        self.players[i].profile.look = look;
        for o in self.players.iter().filter(|o| o.map == map) {
            out.push((o.profile.user_id, HubEvent::Control(ServerMsg::PlayerLook { entity_id, look })));
        }
        out.push((user_id, HubEvent::LookChanged(look)));
    }

    /// Validates a mission choice made at a board or a Kafra (position and
    /// mission id);
    /// a valid choice goes to the socket task as [`HubEvent::MissionChosen`],
    /// which forms or joins a lobby.
    pub fn select_mission(&mut self, user_id: u32, mission: &str, public: bool, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let p = &self.players[i];
        let at_board = self.config.missions_anywhere
            || self.world.map(p.map).manifest.npcs.iter().any(|n| gives_missions(&n.kind) && n.in_reach(p.x, p.y));
        let event = if !at_board {
            HubEvent::Control(hub_error("npc_too_far", "talk to a Kafra employee to pick a mission"))
        } else {
            match missions::find(mission) {
                None => HubEvent::Control(hub_error("unknown_mission", format!("no mission {mission}"))),
                Some(m) => HubEvent::MissionChosen { mission: m.id.into(), public },
            }
        };
        out.push((user_id, event));
    }

    /// Sends `msg` to every player on `map`.
    fn broadcast(&self, map: MapIdx, msg: &ServerMsg, out: &mut Outgoing) {
        for o in self.players.iter().filter(|o| o.map == map) {
            out.push((o.profile.user_id, HubEvent::Control(msg.clone())));
        }
    }

    /// Plays an emote for everyone on the player's map (hubs and stages).
    /// Unknown ids and emotes faster than one per 0.5 s are ignored.
    pub fn emote(&mut self, user_id: u32, emote: u8, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let (tick, gap) = (self.tick, self.ticks_of(EMOTE_GAP_SECS));
        let p = &mut self.players[i];
        if emote > MAX_EMOTE || tick < p.emote_ready {
            return;
        }
        p.emote_ready = tick + gap;
        let (map, entity_id) = (p.map, p.profile.character_id);
        self.broadcast(map, &ServerMsg::Emote { entity_id, emote }, out);
    }

    /// Sits down (hubs only) or stands up; everyone on the map learns it.
    pub fn set_pose(&mut self, user_id: u32, pose: u8, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let sit = match pose {
            pose::STAND => false,
            pose::SIT if matches!(self.kind, InstanceKind::Hub { .. }) => true,
            _ => return,
        };
        let p = &mut self.players[i];
        if p.sitting == sit {
            return;
        }
        p.sitting = sit;
        if sit {
            p.dir = (0.0, 0.0);
        }
        let (map, entity_id) = (p.map, p.profile.character_id);
        self.broadcast(map, &ServerMsg::Pose { entity_id, pose }, out);
    }

    /// Plants a ping marker near the player for everyone on the map. Spots
    /// further than 40 cells, unknown flags and more than one flag a second
    /// are ignored.
    pub fn flag(&mut self, user_id: u32, flag: u8, x: f32, y: f32, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let (tick, gap) = (self.tick, self.ticks_of(FLAG_GAP_SECS));
        let p = &mut self.players[i];
        if flag > MAX_FLAG || !x.is_finite() || !y.is_finite() || tick < p.flag_ready {
            return;
        }
        let (dx, dy) = (x - p.x, y - p.y);
        if dx * dx + dy * dy > FLAG_RANGE * FLAG_RANGE {
            return;
        }
        p.flag_ready = tick + gap;
        let (map, entity_id) = (p.map, p.profile.character_id);
        self.broadcast(map, &ServerMsg::Flag { entity_id, flag, x, y }, out);
    }

    /// `secs` in whole ticks (at least one).
    fn ticks_of(&self, secs: f32) -> u32 {
        ((secs * self.config.tick_hz as f32).ceil() as u32).max(1)
    }

    /// Advances the simulation by one fixed step and emits snapshots.
    pub fn tick(&mut self, out: &mut Outgoing) {
        self.tick = self.tick.wrapping_add(1);
        let step = self.config.walk_speed / self.config.tick_hz as f32;
        let timeout = self.config.input_timeout_ticks;
        let world = Arc::clone(&self.world);

        let mut transitions: Vec<(usize, usize)> = Vec::new();
        let mut stood_up: Vec<(MapIdx, u32)> = Vec::new();
        for (idx, p) in self.players.iter_mut().enumerate() {
            if self.tick.wrapping_sub(p.last_input_tick) > timeout || p.fighter.as_ref().is_some_and(|f| !f.is_up()) {
                p.dir = (0.0, 0.0);
            }
            if p.sitting && p.dir != (0.0, 0.0) {
                p.sitting = false;
                stood_up.push((p.map, p.profile.character_id));
            }
            let map = world.map(p.map);
            let (mut dx, mut dy) = p.dir;
            // Buffs and conditions change the pace; moving ends playing dead.
            let step = match p.fighter.as_mut() {
                Some(f) => {
                    if !f.status.can_move() {
                        (dx, dy) = (0.0, 0.0);
                    }
                    if dx != 0.0 || dy != 0.0 {
                        f.unhide();
                    }
                    step * f.mods.pace() * f.status.speed()
                }
                None => step,
            };
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

        for (map, entity_id) in stood_up {
            self.broadcast(map, &ServerMsg::Pose { entity_id, pose: pose::STAND }, out);
        }
        // Descending: a stage exit swap-removes the player, which only moves
        // an already-processed (higher) index into its slot.
        for (idx, portal) in transitions.into_iter().rev() {
            self.use_portal(idx, portal, out);
        }
        if let Some(horde) = &mut self.horde {
            let light = horde.light_radius();
            self.horde_players.clear();
            // Downed, dead and hidden players are no longer hunted.
            self.horde_players.extend(
                self.players
                    .iter()
                    .filter(|p| p.map == horde.map() && p.fighter.as_ref().is_none_or(Fighter::hunted))
                    .map(|p| PlayerView { x: p.x, y: p.y, light: light + p.fighter.as_ref().map_or(0.0, |f| f.mods.light), id: p.profile.user_id }),
            );
            horde.tick_views(self.tick, self.config.tick_hz, &self.horde_players);
        }
        self.fight(out);
        self.residents_step(out);
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
                p.monsters = None;
                (p.x, p.y) = *arrival;
                p.dir = (0.0, 0.0);
                p.portal_guard = true;
                p.sitting = false;
                self.announce_arrival(idx, Some(def.name.clone()), out);
            }
        }
    }

    /// Sends `map_enter` to the player (then `pose` for everyone sitting
    /// there) and `player_appeared` to the others on that map.
    fn announce_arrival(&self, idx: usize, via_portal: Option<String>, out: &mut Outgoing) {
        let p = &self.players[idx];
        let others: Vec<&HubPlayer> = self
            .players
            .iter()
            .filter(|o| o.map == p.map && o.profile.user_id != p.profile.user_id)
            .collect();
        let mut players: Vec<PlayerInfo> = others.iter().map(|o| o.info()).collect();
        if self.residents.map() == p.map {
            players.extend(self.residents.iter().map(residents::Resident::info));
        }
        out.push((
            p.profile.user_id,
            HubEvent::Control(ServerMsg::MapEnter {
                map: MapInfo::of(self.world.map(p.map)),
                map_epoch: p.map_epoch,
                x: p.x,
                y: p.y,
                via_portal,
                players,
            }),
        ));
        let sitting = others.iter().filter(|o| o.sitting).map(|o| o.profile.character_id);
        let here = self.residents.map() == p.map;
        let resting = self.residents.iter().filter(|r| here && r.is_sitting()).map(|r| r.id);
        for entity_id in sitting.chain(resting) {
            out.push((p.profile.user_id, HubEvent::Control(ServerMsg::Pose { entity_id, pose: pose::SIT })));
        }
        let info = p.info();
        for o in others {
            out.push((o.profile.user_id, HubEvent::Control(ServerMsg::PlayerAppeared { player: info.clone() })));
        }
    }

    fn emit_snapshots(&mut self, out: &mut Outgoing) {
        let mut body = std::mem::take(&mut self.snapshot_body);
        for map in 0..self.world.map_count() as MapIdx {
            if !self.players.iter().any(|p| p.map == map) {
                continue;
            }
            let town = self.residents.map() == map && !self.residents.is_empty();
            let sight = match self.kind {
                InstanceKind::Hub { .. } => self.world.map(map).manifest.light_radius,
                InstanceKind::Stage { .. } => 0.0,
            };
            if sight > 0.0 {
                self.emit_in_sight(map, sight + SIGHT_MARGIN, &mut body, out);
                continue;
            }
            body.clear();
            for p in self.players.iter().filter(|p| p.map == map) {
                body.push(p.profile.character_id, p.x, p.y, p.facing);
            }
            if town {
                for r in self.residents.iter() {
                    body.push(r.id, r.x, r.y, r.facing);
                }
            }
            let horde = self.horde.as_ref().filter(|h| h.map() == map);
            for p in self.players.iter_mut().filter(|p| p.map == map) {
                let frame = match horde {
                    Some(h) => {
                        self.monster_buf.clear();
                        let light = h.light_radius() + p.fighter.as_ref().map_or(0.0, |f| f.mods.light);
                        let viewer = PlayerView { x: p.x, y: p.y, light, id: p.profile.user_id };
                        let view = p.monsters.get_or_insert_with(|| Box::new(MonsterView::new(h.capacity())));
                        h.write_delta(view, &viewer, self.tick, &mut self.monster_buf);
                        body.stage_frame(p.map_epoch, self.tick, p.last_seq, &self.monster_buf)
                    }
                    None => body.frame(p.map_epoch, self.tick, p.last_seq),
                };
                out.push((p.profile.user_id, HubEvent::Snapshot(frame)));
            }
        }
        self.snapshot_body = body;
    }

    /// Hub maps with a light radius: each recipient's snapshot holds itself
    /// and the players and residents within `radius` cells of it.
    fn emit_in_sight(&self, map: MapIdx, radius: f32, body: &mut SnapshotBody, out: &mut Outgoing) {
        let town = self.residents.map() == map;
        for me in self.players.iter().filter(|p| p.map == map) {
            body.clear();
            body.push(me.profile.character_id, me.x, me.y, me.facing);
            for o in self.players.iter() {
                if o.map == map && o.profile.user_id != me.profile.user_id && in_sight(o.x - me.x, o.y - me.y, radius) {
                    body.push(o.profile.character_id, o.x, o.y, o.facing);
                }
            }
            if town {
                for r in self.residents.iter().filter(|r| in_sight(r.x - me.x, r.y - me.y, radius)) {
                    body.push(r.id, r.x, r.y, r.facing);
                }
            }
            out.push((me.profile.user_id, HubEvent::Snapshot(body.frame(me.map_epoch, self.tick, me.last_seq))));
        }
    }

    /// The player's last snapshot never reached them (outbox full) or their
    /// client could not apply a monster delta: the next frame is a keyframe.
    pub fn resync(&mut self, user_id: u32) {
        if let Some(view) = self.players.iter_mut().find(|p| p.profile.user_id == user_id).and_then(|p| p.monsters.as_mut()) {
            view.reset();
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
            light_radius: 0.0,
            residents: Default::default(),
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
        town.manifest.npcs.push(crate::world::map::NpcDef {
            id: "stylist".into(),
            sprite: "4w_m_01".into(),
            name: "Stylist".into(),
            kind: STYLIST.into(),
            x: 8,
            y: 5,
            facing: 4,
            interact_radius: 1.5,
        });
        town.manifest.npcs.push(crate::world::map::NpcDef {
            id: "kafra".into(),
            sprite: "kafra".into(),
            name: "Kafra Employee".into(),
            kind: KAFRA.into(),
            x: 5,
            y: 1,
            facing: 0,
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
    use crate::world::combat::DamageType;

    pub(super) fn profile(user_id: u32) -> PlayerProfile {
        PlayerProfile { user_id, character_id: 100 + user_id, name: format!("p{user_id}"), archetype: "novice".into(), look: Look::default(), level: 1, exp: 0, weapon: String::new(), loadout: Default::default() }
    }

    /// Default simulation without town residents.
    pub(super) fn quiet() -> HubSimConfig {
        HubSimConfig { residents: 0, ..Default::default() }
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
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
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
    fn kafra_gives_missions_skill_resets_and_restyles() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        out.clear();
        hub.skill_reset(1, &mut out);
        assert!(matches!(controls_for(&out, 1).last(), Some(ServerMsg::HubError { code, .. }) if code == "npc_too_far"));
        // The Kafra stands at (5,1), two cells south of the spawn.
        let mut seq = 0;
        assert!(walk_until(&mut hub, 1, &mut seq, (0.0, -1.0), 40, &mut out, |h| {
            h.position_of(1).is_some_and(|(_, x, y)| (x - 5.5).hypot(y - 1.5) < 1.0)
        }));
        out.clear();
        hub.interact(1, "kafra", &mut out);
        match controls_for(&out, 1).last() {
            Some(ServerMsg::NpcDialog { kind, missions, .. }) => {
                assert_eq!(kind, KAFRA);
                assert_eq!(missions.len(), missions::MISSIONS.len());
            }
            other => panic!("expected npc_dialog, got {other:?}"),
        }
        out.clear();
        hub.select_mission(1, "poring_island_survival", true, &mut out);
        assert!(matches!(&out[..], [(1, HubEvent::MissionChosen { public: true, .. })]), "{out:?}");
        out.clear();
        hub.skill_reset(1, &mut out);
        assert!(matches!(&out[..], [(1, HubEvent::SkillResetApproved)]), "{out:?}");
        out.clear();
        let look = Look { gender: 0, hair: 3, hair_color: 4, body_color: 5 };
        hub.restyle(1, look, &mut out);
        assert!(out.iter().any(|(_, e)| matches!(e, HubEvent::LookChanged(l) if *l == look)), "{out:?}");
        assert!(gives_missions(MISSION_BOARD) && gives_missions(KAFRA) && !gives_missions(CHANGELOG));
    }

    fn poses(out: &Outgoing, user: u32) -> Vec<(u32, u8)> {
        controls_for(out, user)
            .into_iter()
            .filter_map(|m| match m {
                ServerMsg::Pose { entity_id, pose } => Some((entity_id, pose)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn sitting_is_broadcast_and_movement_stands_up() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        hub.join(profile(2), Uuid::nil(), None, 0, &mut out);
        out.clear();
        hub.set_pose(1, pose::SIT, &mut out);
        assert_eq!(poses(&out, 1), vec![(101, pose::SIT)]);
        assert_eq!(poses(&out, 2), vec![(101, pose::SIT)]);
        hub.set_pose(1, pose::SIT, &mut out);
        hub.set_pose(1, 7, &mut out);
        assert_eq!(poses(&out, 2).len(), 1, "repeats and unknown poses are ignored");

        // A newcomer learns who sits, right after map_enter.
        out.clear();
        hub.join(profile(3), Uuid::nil(), None, 0, &mut out);
        let msgs = controls_for(&out, 3);
        let enter = msgs.iter().position(|m| matches!(m, ServerMsg::MapEnter { .. })).unwrap();
        assert_eq!(msgs[enter + 1], ServerMsg::Pose { entity_id: 101, pose: pose::SIT });
        assert_eq!(poses(&out, 3), vec![(101, pose::SIT)]);

        // Seated players do not move; moving stands them up first.
        let (_, x0, _) = hub.position_of(1).unwrap();
        out.clear();
        hub.tick(&mut out);
        assert!(poses(&out, 2).is_empty());
        assert_eq!(hub.position_of(1).unwrap().1, x0);
        hub.input(1, 1, 1.0, 0.0);
        hub.tick(&mut out);
        assert_eq!(poses(&out, 2), vec![(101, pose::STAND)]);
        assert_eq!(poses(&out, 1), vec![(101, pose::STAND)]);
        assert!(hub.position_of(1).unwrap().1 > x0);

        // Standing up explicitly.
        hub.set_pose(2, pose::SIT, &mut out);
        out.clear();
        hub.set_pose(2, pose::STAND, &mut out);
        assert_eq!(poses(&out, 1), vec![(102, pose::STAND)]);
    }

    #[test]
    fn nobody_sits_on_a_stage_and_portals_reset_sitting() {
        let mut stage = HubInstance::stage(1, "m1", Arc::new(stage_world()), HubSimConfig::default());
        let mut out = Vec::new();
        stage.join(profile(1), Uuid::nil(), None, 0, &mut out);
        out.clear();
        stage.set_pose(1, pose::SIT, &mut out);
        assert!(out.is_empty());

        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        hub.set_pose(1, pose::SIT, &mut out);
        let i = hub.find(1).unwrap();
        hub.players[i].map = 1;
        hub.players[i].x = 2.5;
        hub.players[i].y = 2.5;
        // Simulated walk through the house exit: arriving in town resets it.
        hub.use_portal(i, 0, &mut out);
        assert!(!hub.players[i].sitting);
    }

    #[test]
    fn emotes_are_broadcast_validated_and_rate_limited() {
        let mut stage = HubInstance::stage(1, "m1", Arc::new(stage_world()), HubSimConfig::default());
        let mut out = Vec::new();
        stage.join(profile(1), Uuid::nil(), None, 0, &mut out);
        stage.join(profile(2), Uuid::nil(), None, 0, &mut out);
        out.clear();
        let emotes = |out: &Outgoing, user: u32| -> Vec<ServerMsg> {
            controls_for(out, user).into_iter().filter(|m| matches!(m, ServerMsg::Emote { .. })).collect()
        };
        stage.emote(1, 128, &mut out);
        assert!(out.is_empty(), "out of range");
        stage.emote(1, 27, &mut out);
        for user in [1, 2] {
            assert_eq!(emotes(&out, user), vec![ServerMsg::Emote { entity_id: 101, emote: 27 }]);
        }
        out.clear();
        stage.emote(1, 3, &mut out);
        assert!(out.is_empty(), "too soon");
        for _ in 0..10 {
            stage.tick(&mut out);
        }
        out.clear();
        stage.emote(1, 3, &mut out);
        assert_eq!(emotes(&out, 2), vec![ServerMsg::Emote { entity_id: 101, emote: 3 }], "after 0.5 s");
    }

    #[test]
    fn flags_are_broadcast_within_range_and_rate_limited() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        hub.join(profile(2), Uuid::nil(), None, 0, &mut out);
        out.clear();
        let (_, x, y) = hub.position_of(1).unwrap();
        hub.flag(1, 10, x, y, &mut out);
        hub.flag(1, 1, f32::NAN, y, &mut out);
        hub.flag(1, 1, x + 41.0, y, &mut out);
        assert!(out.is_empty(), "bad flag, NaN, too far");
        hub.flag(1, 9, x + 30.0, y + 20.0, &mut out);
        for user in [1, 2] {
            assert_eq!(controls_for(&out, user), vec![ServerMsg::Flag { entity_id: 101, flag: 9, x: x + 30.0, y: y + 20.0 }]);
        }
        out.clear();
        hub.flag(1, 2, x, y, &mut out);
        for _ in 0..19 {
            hub.tick(&mut out);
        }
        hub.flag(1, 2, x, y, &mut out);
        assert!(controls_for(&out, 2).iter().all(|m| !matches!(m, ServerMsg::Flag { .. })), "under a second");
        hub.tick(&mut out);
        hub.flag(1, 2, x, y, &mut out);
        assert!(controls_for(&out, 2).iter().any(|m| matches!(m, ServerMsg::Flag { flag: 2, .. })));
        // Players on another map do not see it.
        hub.join(profile(3), Uuid::nil(), Some(Arrival { map: "house".into(), x: 3.5, y: 2.5 }), 0, &mut out);
        out.clear();
        hub.emote(1, 1, &mut out);
        assert!(controls_for(&out, 3).is_empty());
    }

    #[test]
    fn stylist_restyles_hair_and_dyes_but_keeps_gender() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        let mut out = Vec::new();
        let her = PlayerProfile { look: Look { gender: 1, hair: 2, hair_color: 0, body_color: 0 }, ..profile(1) };
        hub.join(her, Uuid::nil(), None, 0, &mut out);
        hub.join(profile(2), Uuid::nil(), None, 0, &mut out);
        out.clear();
        let wanted = Look { gender: 0, hair: 9, hair_color: 14, body_color: 3 };
        let code = |out: &Outgoing| match controls_for(out, 1).last() {
            Some(ServerMsg::HubError { code, .. }) => code.clone(),
            other => format!("{other:?}"),
        };

        hub.restyle(1, wanted, &mut out);
        assert_eq!(code(&out), "npc_too_far", "away from the stylist");

        // Walk north-east to the stylist at (8,5).
        let mut seq = 0;
        assert!(walk_until(&mut hub, 1, &mut seq, (1.0, 1.0), 40, &mut out, |h| {
            h.position_of(1).is_some_and(|(_, x, y)| (x - 8.5).hypot(y - 5.5) < 1.0)
        }));
        out.clear();
        hub.restyle(1, Look { hair: 13, ..wanted }, &mut out);
        assert_eq!(code(&out), "bad_message", "no such style");
        out.clear();

        hub.restyle(1, wanted, &mut out);
        let kept = Look { gender: 1, ..wanted };
        for user in [1, 2] {
            assert!(
                controls_for(&out, user).iter().any(|m| matches!(m, ServerMsg::PlayerLook { entity_id: 101, look } if *look == kept)),
                "user {user} sees the new look: {out:?}"
            );
        }
        assert!(out.iter().any(|(u, e)| *u == 1 && matches!(e, HubEvent::LookChanged(l) if *l == kept)));
        // Newcomers get it through player_appeared / map_enter.
        out.clear();
        hub.join(profile(3), Uuid::nil(), None, 0, &mut out);
        let seen = controls_for(&out, 3).iter().find_map(|m| match m {
            ServerMsg::MapEnter { players, .. } => players.iter().find(|p| p.entity_id == 101).map(|p| p.look),
            _ => None,
        });
        assert_eq!(seen, Some(kept));
    }

    #[test]
    fn join_sends_joined_then_map_enter_with_camera() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
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
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
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
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
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
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
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
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
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
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
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
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
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
        use crate::world::horde::view::DeltaEntry;
        use crate::world::horde::{Horde, HordeConfig};
        use crate::world::protocol::decode_pos;
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
        let delta = snap.monsters.expect("stage snapshot");
        assert_eq!(delta.baseline, 0, "the first frame is a keyframe");
        match delta.entries[..] {
            [DeltaEntry::Spawn { slot, x, vx, .. }] => {
                assert_eq!(slot, near);
                assert!(decode_pos(x) < px + 2.0 && vx < 0, "it moves towards the player");
            }
            ref other => panic!("{other:?}"),
        }
        // A dropped frame (or a client resync) makes the next one a keyframe.
        stage.resync(1);
        out.clear();
        stage.tick(&mut out);
        assert_eq!(last_snapshot(&out, 1).unwrap().monsters.unwrap().baseline, 0);
        out.clear();
        stage.tick(&mut out);
        assert_eq!(last_snapshot(&out, 1).unwrap().monsters.unwrap().baseline, 1);
        // Monsters stand still while nobody is on the map.
        stage.leave(1, Uuid::nil(), &mut out);
        let before: Vec<_> = stage.horde().unwrap().monsters().collect();
        stage.tick(&mut out);
        assert_eq!(stage.horde().unwrap().monsters().collect::<Vec<_>>(), before);
    }

    #[test]
    fn hub_join_with_arrival_places_player() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
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

    fn fighting_stage(archetype: &str) -> HubInstance {
        use crate::world::horde::catalog::{test_def, MonsterCatalog};
        use crate::world::horde::{Horde, HordeConfig};
        let world = Arc::new(stage_world());
        let catalog = Arc::new(MonsterCatalog::new(vec![test_def("blob", 2.0, 1, 0.0, false)]).unwrap());
        let cfg = HordeConfig { light_radius: 6.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
        let horde = Horde::new(Arc::clone(&world), catalog, cfg);
        let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde);
        stage.dice = combat::Rng::new(3);
        let mut out = Vec::new();
        stage.join(PlayerProfile { archetype: archetype.into(), ..profile(1) }, Uuid::nil(), None, 0, &mut out);
        assert!(
            controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::PlayerVitals { entity_id: 101, state: 0, .. })),
            "joining a stage announces the fighter's vitals"
        );
        stage
    }

    /// A roomy field (20 × 9 cells) with a stage of the test skill table;
    /// player 1 joins at `level` with `skills` learned and slotted in order.
    fn skill_stage(archetype: &str, level: u32, skills: &[(&str, u8)]) -> HubInstance {
        skill_stage_with(archetype, level, skills, &[], &[])
    }

    /// [`skill_stage`] whose blob (kind 0) and king (kind 1, the boss)
    /// carry these monster skills.
    fn skill_stage_with(
        archetype: &str,
        level: u32,
        skills: &[(&str, u8)],
        blob_skills: &[&str],
        king_skills: &[&str],
    ) -> HubInstance {
        use crate::world::combat::skills::tests::book;
        use crate::world::horde::catalog::{test_def, MonsterCatalog, MonsterDef};
        use crate::world::horde::{Horde, HordeConfig};
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let row = format!("#{}#", ".".repeat(20));
        let wall = "#".repeat(22);
        let mut rows = vec![wall.as_str()];
        rows.extend(std::iter::repeat_n(row.as_str(), 9));
        rows.push(wall.as_str());
        let field = map(
            "field",
            LocationKind::Field,
            CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
            &rows,
            vec![portal("to_town", [1, 1, 1], "town", [3, 3])],
        );
        let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
        let catalog = Arc::new(
            MonsterCatalog::new(vec![
                MonsterDef { skills: ids(blob_skills), ..test_def("blob", 2.0, 1, 0.0, false) },
                MonsterDef { skills: ids(king_skills), ..test_def("king", 2.0, 50, 0.0, true) },
            ])
            .unwrap(),
        );
        let cfg = HordeConfig { light_radius: 12.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
        let horde = Horde::new(Arc::clone(&world), catalog, cfg);
        let mut stage =
            HubInstance::stage(1, "m1", world, HubSimConfig::default()).with_horde(horde).with_skills(Arc::new(book()));
        stage.dice = combat::Rng::new(5);
        let mut loadout = Loadout::default();
        for (i, (id, lvl)) in skills.iter().enumerate() {
            loadout.skills.insert(id.to_string(), *lvl);
            loadout.hotbar.resize(combat::skills::SLOTS, String::new());
            loadout.hotbar[i] = id.to_string();
        }
        let mut out = Vec::new();
        stage.join(PlayerProfile { archetype: archetype.into(), level, loadout, ..profile(1) }, Uuid::nil(), None, 0, &mut out);
        assert!(
            controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::Skills { hotbar, .. } if hotbar.len() == combat::skills::SLOTS)),
            "joining sends the skills"
        );
        stage
    }

    fn fighter(stage: &HubInstance, user: u32) -> &Fighter {
        stage.players[stage.find(user).unwrap()].fighter.as_ref().unwrap()
    }

    #[test]
    fn a_standing_mage_casts_bolts_at_the_crowd() {
        use crate::world::protocol::MONSTER_ENTITY_BASE;
        let mut stage = skill_stage("mage", 10, &[("bolt", 3)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slots: Vec<u16> = (0..3).filter_map(|i| stage.horde_mut().unwrap().spawn(0, px + 6.0, py - 1.0 + i as f32)).collect();
        assert_eq!(slots.len(), 3);
        let sp = fighter(&stage, 1).sp;
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        let msgs = controls_for(&out, 1);
        let cast = msgs.iter().find_map(|m| match m {
            ServerMsg::Cast { caster: 101, skill, x, cast_ms, .. } if skill == "bolt" => Some((*x, *cast_ms)),
            _ => None,
        });
        let (x, cast_ms) = cast.expect("the cast is announced");
        assert!((x - (px + 6.0)).abs() < 1.5, "aimed at the group, x = {x}");
        assert_eq!(cast_ms, 500, "0.3 + 0.06·3 s rounded to ticks");
        assert_eq!(fighter(&stage, 1).sp, sp, "SP is paid when the cast lands");
        out.clear();
        for _ in 0..20 {
            stage.tick(&mut out);
        }
        let fire_hits = controls_for(&out, 1)
            .iter()
            .flat_map(|m| match m {
                ServerMsg::Combat { hits, .. } => hits.clone(),
                _ => Vec::new(),
            })
            .filter(|h| h.target >= MONSTER_ENTITY_BASE && h.flags >> 4 == DamageType::Fire as u8)
            .count();
        assert!(fire_hits >= 3, "three bolts land, each splashing ({fire_hits} fire hits)");
        assert_eq!(fighter(&stage, 1).sp, sp - 3, "1 + 0.5·3 rounds to 3 SP (regen is below a point)");
    }

    #[test]
    fn moving_cancels_a_wind_up_for_free() {
        let mut stage = skill_stage("mage", 10, &[("bolt", 3)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        stage.horde_mut().unwrap().spawn(0, px + 6.0, py).unwrap();
        let sp = fighter(&stage, 1).sp;
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        assert!(fighter(&stage, 1).casting.is_some());
        out.clear();
        stage.input(1, 1, 0.0, 1.0);
        stage.tick(&mut out);
        assert!(fighter(&stage, 1).casting.is_none());
        assert!(controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::Cast { caster: 101, skill, .. } if skill.is_empty())));
        assert_eq!(fighter(&stage, 1).sp, sp);
        assert_eq!(fighter(&stage, 1).cooldowns.iter().sum::<u32>(), 0, "no cooldown after a cancel");
    }

    #[test]
    fn no_target_means_no_cast() {
        let mut stage = skill_stage("mage", 10, &[("bolt", 3)]);
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        assert!(fighter(&stage, 1).casting.is_none());
        assert!(!controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::Cast { .. })));
    }

    #[test]
    fn back_slide_goes_away_from_the_aim_even_while_moving() {
        let mut stage = skill_stage("swordsman", 5, &[("slide", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let mut out = Vec::new();
        stage.input(1, 1, 0.0, 0.0);
        stage.act(1, [8, 0], 1); // aim 1 m east
        stage.tick(&mut out);
        let (_, x, y) = stage.position_of(1).unwrap();
        assert!(x < px - 2.0, "slid west from {px} to {x}");
        assert!((y - py).abs() < 0.01);
        assert!(stage.world.map(0).walk.is_walkable(x, y));
        assert!(fighter(&stage, 1).cooldowns.iter().any(|&c| c > 0));
    }

    #[test]
    fn mending_finds_the_most_wounded_ally() {
        let mut stage = skill_stage("acolyte", 10, &[("mending", 2)]);
        let mut out = Vec::new();
        stage.join(PlayerProfile { archetype: "acolyte".into(), level: 10, ..profile(2) }, Uuid::nil(), None, 0, &mut out);
        let i2 = stage.find(2).unwrap();
        let max = stage.players[i2].fighter.as_ref().unwrap().max_hp;
        stage.players[i2].fighter.as_mut().unwrap().hp = 3;
        out.clear();
        stage.act(1, [0, 0], 1);
        for _ in 0..10 {
            stage.tick(&mut out);
        }
        let hp = fighter(&stage, 2).hp;
        assert!(hp > 3 && hp <= max, "ally healed to {hp}");
        assert!(controls_for(&out, 2).iter().any(|m| matches!(m, ServerMsg::Combat { hits, .. }
            if hits.iter().any(|h| h.target == 102 && h.flags & crate::world::protocol::hit::HEAL != 0))));
    }

    #[test]
    fn loadout_changes_reach_the_fighter() {
        let mut stage = skill_stage("mage", 10, &[]);
        let mut loadout = Loadout::default();
        loadout.skills.insert("bolt".into(), 2);
        loadout.skills.insert("focus".into(), 5);
        loadout.hotbar = vec![String::new(), "bolt".into()];
        let mut out = Vec::new();
        stage.set_loadout(1, loadout, &mut out);
        let f = fighter(&stage, 1);
        assert_eq!(f.hotbar[1], stage.skills.find("bolt").map(|(i, _)| i));
        assert!((f.mods.sp_regen - 0.01).abs() < 1e-6, "Focus 5 = +1% SP/s");
        assert!(controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::Skills { points, .. } if *points == 9 - 7)));
    }

    #[test]
    fn standing_player_kills_a_monster_and_gains_experience() {
        use crate::world::protocol::{hit, MONSTER_ENTITY_BASE};
        let mut stage = fighting_stage("mage");
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = stage.horde_mut().unwrap().spawn(0, px + 2.0, py).unwrap();
        let mut out = Vec::new();
        let mut killed = false;
        for _ in 0..200 {
            stage.tick(&mut out);
            killed |= controls_for(&out, 1).iter().any(|m| {
                matches!(m, ServerMsg::Combat { hits, .. }
                    if hits.iter().any(|h| h.target == MONSTER_ENTITY_BASE + slot as u32 && h.flags & hit::KILL != 0))
            });
            if killed {
                break;
            }
        }
        assert!(killed, "the mage's basic attack kills the blob");
        assert!(!stage.horde().unwrap().store().is_alive(slot));
        for _ in 0..10 {
            stage.tick(&mut out);
        }
        assert!(controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::Progress { gained, .. } if *gained > 0)));
        assert!(out.iter().any(|(u, e)| *u == 1 && matches!(e, HubEvent::Progress { exp, .. } if *exp > 0)));
    }

    #[test]
    fn moving_player_does_not_attack() {
        let mut stage = fighting_stage("archer");
        let (_, px, py) = stage.position_of(1).unwrap();
        stage.horde_mut().unwrap().spawn(0, px + 3.0, py).unwrap();
        let mut out = Vec::new();
        for seq in 1..20 {
            stage.input(1, seq, 0.0, if seq % 2 == 0 { 1.0 } else { -1.0 });
            stage.tick(&mut out);
        }
        assert!(
            !controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::Combat { swings, .. } if swings.iter().any(|s| s.entity_id == 101))),
            "no swing while the stick is held"
        );
    }

    #[test]
    fn solo_player_downed_means_defeat_and_exit() {
        let mut stage = fighting_stage("swordsman");
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = stage.horde_mut().unwrap().spawn(0, px + 1.0, py).unwrap();
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 100_000;
        stage.players[0].fighter.as_mut().unwrap().hp = 1;
        let mut out = Vec::new();
        for _ in 0..600 {
            stage.tick(&mut out);
            if stage.position_of(1).is_none() {
                break;
            }
        }
        assert!(stage.position_of(1).is_none(), "the defeated party leaves the stage");
        assert!(controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::StageEnd { victory: false, leave_ms: 5000, .. })));
        assert!(out.iter().any(|(u, e)| *u == 1 && matches!(e, HubEvent::Transfer { target_map, .. } if target_map == "town")));
    }

    /// Kind index of the boss in [`skill_stage`].
    fn king(stage: &mut HubInstance) -> u8 {
        stage.horde_mut().unwrap().catalog().defs().iter().position(|d| d.boss).unwrap() as u8
    }

    #[test]
    fn a_failed_save_freezes_a_monster_until_a_blow_shatters_it() {
        use crate::world::combat::Condition;
        let mut stage = skill_stage("mage", 10, &[("chill", 10)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = stage.horde_mut().unwrap().spawn(0, px + 8.0, py).unwrap();
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 100_000;
        let status = |stage: &mut HubInstance| stage.horde_mut().unwrap().store().status_of(slot);
        let mut out = Vec::new();
        let mut casts = 0;
        while !status(&mut stage).has(Condition::Frozen) {
            assert!(casts < 50, "some save fails (DC 13 against +1)");
            stage.act(1, [0, 0], 1);
            stage.tick(&mut out);
            casts += 1;
        }
        let at = |stage: &mut HubInstance| {
            let s = stage.horde_mut().unwrap().store();
            (s.x[slot as usize], s.y[slot as usize])
        };
        let before = at(&mut stage);
        for _ in 0..40 {
            stage.tick(&mut out);
        }
        assert_eq!(at(&mut stage), before, "frozen 1 + 0.5·10 s: it does not move");
        assert!(status(&mut stage).has(Condition::Frozen));
        // A physical blow shatters the ice.
        let horde = stage.horde_mut().unwrap();
        let mut rng = combat::Rng::new(3);
        let blow = combat::Damage::new(combat::Dice::new(1, 4, 0), DamageType::Physical);
        let dealt = super::cast::wound(horde, &mut rng, slot, blow, None, false, false, 2);
        assert!(dealt >= 3, "1d4 + 2d6 of shattering ice ({dealt})");
        assert!(!horde.store().status_of(slot).has(Condition::Frozen), "the ice is gone");
    }

    #[test]
    fn held_monsters_do_not_strike() {
        use crate::world::combat::Condition;
        use crate::world::protocol::MONSTER_ENTITY_BASE;
        let mut stage = skill_stage("mage", 10, &[]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = stage.horde_mut().unwrap().spawn(0, px + 0.8, py).unwrap();
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 100_000;
        stage.horde_mut().unwrap().store_mut().afflict(slot, Condition::Stunned, 1000);
        let mut out = Vec::new();
        for _ in 0..80 {
            stage.tick(&mut out);
        }
        let monster_swings = controls_for(&out, 1)
            .iter()
            .flat_map(|m| match m {
                ServerMsg::Combat { swings, .. } => swings.clone(),
                _ => Vec::new(),
            })
            .filter(|s| s.entity_id >= MONSTER_ENTITY_BASE)
            .count();
        assert_eq!(monster_swings, 0);
        assert_eq!(fighter(&stage, 1).hp, fighter(&stage, 1).max_hp);
    }

    #[test]
    fn legendary_resistance_absorbs_three_failed_saves() {
        use crate::world::combat::Condition;
        let mut stage = skill_stage("mage", 10, &[("curse", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let kind = king(&mut stage);
        let boss = stage.horde_mut().unwrap().spawn(kind, px + 8.0, py).unwrap();
        let store = stage.horde_mut().unwrap().store_mut();
        assert_eq!(store.legendary[boss as usize], 3);
        // Stunned: every save fails, so only legendary resistance helps.
        store.afflict(boss, Condition::Stunned, 1000);
        let mut out = Vec::new();
        let petrified =
            |stage: &mut HubInstance| stage.horde_mut().unwrap().store().status_of(boss).has(Condition::Petrified);
        for left in [2, 1, 0] {
            stage.act(1, [0, 0], 1);
            stage.tick(&mut out);
            assert!(!petrified(&mut stage));
            assert_eq!(stage.horde_mut().unwrap().store().legendary[boss as usize], left);
        }
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        assert!(petrified(&mut stage), "the fourth failure sticks");
    }

    /// Hits on monsters in the messages of `out`, as `(target, amount, flags)`.
    fn monster_hits(out: &Outgoing) -> Vec<(u32, u32, u8)> {
        use crate::world::protocol::MONSTER_ENTITY_BASE;
        controls_for(out, 1)
            .iter()
            .flat_map(|m| match m {
                ServerMsg::Combat { hits, .. } => hits.iter().map(|h| (h.target, h.amount, h.flags)).collect(),
                _ => Vec::new(),
            })
            .filter(|h| h.0 >= MONSTER_ENTITY_BASE)
            .collect()
    }

    #[test]
    fn a_swamp_holds_what_stands_in_it_and_lets_go_after() {
        use crate::world::combat::Condition;
        let mut stage = skill_stage("mage", 10, &[("swamp", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = stage.horde_mut().unwrap().spawn(0, px + 8.0, py).unwrap();
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 100_000;
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        assert_eq!(stage.lives.len(), 1);
        let status = |stage: &mut HubInstance| stage.horde_mut().unwrap().store().status_of(slot);
        for _ in 0..10 {
            stage.tick(&mut out);
            let st = status(&mut stage);
            assert!(st.has(Condition::Slowed) && st.has(Condition::Mired), "held while inside: {st:?}");
            assert_eq!(st.ac(), -2);
        }
        for _ in 0..80 {
            stage.tick(&mut out);
        }
        assert!(stage.lives.is_empty(), "4 s are over");
        assert_eq!(status(&mut stage), combat::Status(0), "the swamp's hold runs out");
    }

    #[test]
    fn a_fire_wall_burns_pushes_back_and_sets_alight() {
        use crate::world::combat::Condition;
        let mut stage = skill_stage("mage", 10, &[("flames", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = stage.horde_mut().unwrap().spawn(0, px + 6.0, py).unwrap();
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 100_000;
        let mut out = Vec::new();
        stage.act(1, [24, 0], 1); // aim 3 m east, onto the monster
        stage.tick(&mut out);
        assert_eq!(stage.lives.len(), 1);
        let x = stage.horde_mut().unwrap().store().x[slot as usize];
        assert!(x >= px + 7.0, "pushed 1 m away from the caster: {x}");
        let fire = DamageType::Fire as u8;
        let mut burned = false;
        for _ in 0..60 {
            stage.tick(&mut out);
            let st = stage.horde_mut().unwrap().store().status_of(slot);
            burned |= st.has(Condition::Burning);
        }
        let fire_hits = monster_hits(&out).iter().filter(|h| h.2 >> 4 == fire).count();
        assert!(fire_hits >= 2, "the wall hits on contact and the burn bites ({fire_hits})");
        assert!(burned, "a failed save sets it alight (DC 13 against +1)");
        assert!(stage.horde_mut().unwrap().store().burn_k[slot as usize] >= 1);
    }

    #[test]
    fn poison_bites_once_a_second() {
        use crate::world::combat::Condition;
        let mut stage = skill_stage("mage", 10, &[]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = stage.horde_mut().unwrap().spawn(0, px + 8.0, py).unwrap();
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 100_000;
        stage.horde_mut().unwrap().store_mut().afflict(slot, Condition::Poisoned, 61);
        let mut out = Vec::new();
        for _ in 0..80 {
            stage.tick(&mut out);
        }
        let poison = monster_hits(&out).iter().filter(|h| h.2 >> 4 == DamageType::Poison as u8 && h.1 > 0).count();
        assert_eq!(poison, 3, "3 s of poison at 20 Hz");
    }

    #[test]
    fn a_ward_takes_the_next_melee_hits() {
        let mut stage = skill_stage("mage", 10, &[("shield", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = stage.horde_mut().unwrap().spawn(0, px + 0.8, py).unwrap();
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 100_000;
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        let held = |stage: &HubInstance| match &stage.lives[..] {
            [cast::Live { load: cast::Load::Ward { hits, .. }, .. }] => *hits,
            _ => 0,
        };
        let max = fighter(&stage, 1).max_hp;
        let swings = |out: &Outgoing| {
            controls_for(out, 1)
                .iter()
                .flat_map(|m| match m {
                    ServerMsg::Combat { swings, .. } => swings.clone(),
                    _ => Vec::new(),
                })
                .filter(|s| s.entity_id >= crate::world::protocol::MONSTER_ENTITY_BASE)
                .count()
        };
        // 1 + 1·slv hits; the first may land on the tick of the cast.
        assert_eq!(held(&stage) as usize + swings(&out), 2);
        while swings(&out) < 2 {
            stage.tick(&mut out);
        }
        assert_eq!(fighter(&stage, 1).hp, max, "two blows land on the ward");
        assert!(stage.lives.is_empty() || matches!(stage.lives[0].load, cast::Load::Ward { hits: 0, .. }));
        stage.tick(&mut out);
        assert!(stage.lives.is_empty(), "a spent ward is gone");
    }

    fn tough(stage: &mut HubInstance, x: f32, y: f32) -> u16 {
        let slot = stage.horde_mut().unwrap().spawn(0, x, y).unwrap();
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 100_000;
        slot
    }

    fn hit_slots(out: &Outgoing) -> Vec<u16> {
        use crate::world::protocol::MONSTER_ENTITY_BASE;
        let mut v: Vec<u16> = monster_hits(out).iter().map(|h| (h.0 - MONSTER_ENTITY_BASE) as u16).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    #[test]
    fn taunted_monsters_hunt_the_taunter_past_a_nearer_player() {
        use crate::world::combat::Condition;
        let mut stage = skill_stage("swordsman", 10, &[("taunt", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let mut out = Vec::new();
        stage.join(profile(2), Uuid::nil(), None, 0, &mut out);
        let i2 = stage.find(2).unwrap();
        stage.players[i2].x = px + 12.0;
        let slot = tough(&mut stage, px + 3.0, py);
        stage.horde_mut().unwrap().store_mut().afflict(slot, Condition::Stunned, 5); // its save fails
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        let st = stage.horde_mut().unwrap().store();
        assert!(st.status_of(slot).has(Condition::Taunted), "a failed Mind save taunts it");
        assert_eq!(st.taunter[slot as usize], 1);
        // Put player 2 right next to it: it still walks to player 1.
        stage.players[i2].x = px + 4.0;
        stage.players[i2].y = py;
        let before = (stage.horde_mut().unwrap().store().x[slot as usize] - px).abs();
        for _ in 0..30 {
            stage.tick(&mut out);
        }
        let after = (stage.horde_mut().unwrap().store().x[slot as usize] - px).abs();
        assert!(after < before - 1.0, "closes in on its taunter: {before} -> {after}");
    }

    #[test]
    fn a_chain_hops_to_the_nearest_new_monster_within_its_jump() {
        let mut stage = skill_stage("mage", 10, &[("chain", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let a = tough(&mut stage, px + 3.0, py);
        let b = tough(&mut stage, px + 5.5, py);
        let c = tough(&mut stage, px + 8.0, py);
        let far = tough(&mut stage, px + 8.0, py + 3.5);
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        let hit = hit_slots(&out);
        assert!(hit.contains(&a) && hit.contains(&b) && hit.contains(&c), "{hit:?}");
        assert!(!hit.contains(&far), "3.5 cells is beyond a 3-cell jump");
    }

    #[test]
    fn a_line_with_a_limit_stops_at_the_first_monster_and_knocks_it_back() {
        let mut stage = skill_stage("archer", 10, &[("arrow", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let near = tough(&mut stage, px + 4.0, py);
        let back = tough(&mut stage, px + 8.0, py);
        let mut out = Vec::new();
        stage.act(1, [32, 0], 1); // aim 4 m east
        stage.tick(&mut out);
        assert_eq!(hit_slots(&out), vec![near], "only the first along the line");
        let x = stage.horde_mut().unwrap().store().x[near as usize];
        assert!(x >= px + 5.5, "pushed along the shot: {x}");
        let _ = back;
    }

    #[test]
    fn a_trap_waits_for_a_monster_then_fires_once() {
        use crate::world::combat::Condition;
        let mut stage = skill_stage("archer", 10, &[("snare", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let mut out = Vec::new();
        stage.act(1, [16, 0], 1); // 2 m east = 4 cells
        stage.tick(&mut out);
        let id = controls_for(&out, 1)
            .iter()
            .find_map(|m| match m {
                ServerMsg::Cast { areas, .. } if !areas.is_empty() => Some(areas[0].id),
                _ => None,
            })
            .unwrap();
        assert!(id > 0, "a trap is lasting ground with an id");
        for _ in 0..10 {
            stage.tick(&mut out);
        }
        assert_eq!(stage.lives.len(), 1, "nothing stepped on it");
        let slot = tough(&mut stage, px + 4.0, py);
        stage.horde_mut().unwrap().store_mut().afflict(slot, Condition::Stunned, 10);
        out.clear();
        for _ in 0..3 {
            stage.tick(&mut out);
        }
        assert!(stage.horde_mut().unwrap().store().status_of(slot).has(Condition::Rooted));
        assert!(stage.lives.is_empty(), "sprung once");
        assert!(controls_for(&out, 1).contains(&ServerMsg::ZoneEnd { id }), "clients drop its telegraph");
    }

    #[test]
    fn a_barrier_keeps_monsters_out() {
        let mut stage = skill_stage("mage", 10, &[("icewall", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = tough(&mut stage, px + 6.1, py);
        stage.horde_mut().unwrap().store_mut().afflict(slot, crate::world::combat::Condition::Rooted, 1000);
        let mut out = Vec::new();
        stage.act(1, [24, 0], 1); // 3 m east: the wall across x = px + 6
        stage.tick(&mut out);
        let x = stage.horde_mut().unwrap().store().x[slot as usize];
        assert!(x >= px + 6.6, "ejected out the far side: {x}");
    }

    #[test]
    fn an_aura_moves_with_its_caster_and_keeps_its_buff_up() {
        let mut stage = skill_stage("acolyte", 10, &[("chant", 1)]);
        let base = fighter(&stage, 1).mods.ac;
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        assert_eq!(fighter(&stage, 1).mods.ac, base + 2);
        let i = stage.find(1).unwrap();
        stage.players[i].x += 3.0;
        for _ in 0..12 {
            stage.tick(&mut out);
        }
        let (x, _) = stage.lives[0].area.centre();
        assert!((x - stage.players[i].x).abs() < 1e-3, "the aura follows");
        assert_eq!(fighter(&stage, 1).mods.ac, base + 2, "held while inside");
        for _ in 0..100 {
            stage.tick(&mut out);
        }
        assert!(stage.lives.is_empty());
        assert_eq!(fighter(&stage, 1).mods.ac, base, "and gone with it");
    }

    #[test]
    fn buffs_quicken_the_party_and_run_out() {
        let mut stage = skill_stage("acolyte", 10, &[("haste", 2)]);
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        let m = fighter(&stage, 1).mods;
        assert_eq!((m.ac, m.haste, m.speed), (2, 0.5, 0.25));
        let (_, x0, _) = stage.position_of(1).unwrap();
        stage.input(1, 1, 1.0, 0.0);
        stage.tick(&mut out);
        let (_, x1, _) = stage.position_of(1).unwrap();
        let step = HubSimConfig::default().walk_speed / HubSimConfig::default().tick_hz as f32;
        assert!((x1 - x0 - step * 1.25).abs() < 1e-3, "25% faster: {}", x1 - x0);
        stage.input(1, 2, 0.0, 0.0);
        for _ in 0..200 {
            stage.tick(&mut out);
        }
        assert_eq!(fighter(&stage, 1).mods.haste, 0.0, "10 s are over");
    }

    #[test]
    fn temporary_hp_soaks_blows_first() {
        let mut stage = skill_stage("swordsman", 10, &[("endure", 3)]);
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        let f = fighter(&stage, 1);
        assert!(f.temp_hp >= 3 && f.temp_hp <= 30, "3d10: {}", f.temp_hp);
        let (hp, temp) = (f.hp, f.temp_hp);
        let i = stage.find(1).unwrap();
        let f = stage.players[i].fighter.as_mut().unwrap();
        f.take(temp + 2, true);
        assert_eq!((f.temp_hp, f.hp), (0, hp - 2));
    }

    #[test]
    fn a_homing_hit_flies_then_lands_on_its_moving_target() {
        let mut stage = skill_stage("mage", 10, &[("homing", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = tough(&mut stage, px + 10.0, py);
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        assert!(hit_slots(&out).is_empty(), "10 cells at 10 cells/s: still flying");
        let telegraph = controls_for(&out, 1).iter().find_map(|m| match m {
            ServerMsg::Cast { areas, .. } => areas.first().copied(),
            _ => None,
        });
        assert_eq!(telegraph.map(|t| t.follow), Some(crate::world::protocol::MONSTER_ENTITY_BASE + slot as u32));
        stage.horde_mut().unwrap().place(slot, px + 6.0, py + 3.0);
        for _ in 0..25 {
            stage.tick(&mut out);
        }
        assert_eq!(hit_slots(&out), vec![slot], "it cannot miss");
    }

    #[test]
    fn passives_add_up_and_pull_drags_monsters_in() {
        let stage = skill_stage("swordsman", 10, &[("mastery", 4)]);
        assert_eq!((fighter(&stage, 1).mods.to_hit, fighter(&stage, 1).mods.damage_flat), (2, 4));
        let mut stage = skill_stage("mage", 10, &[("pull", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = tough(&mut stage, px + 6.0, py);
        stage.horde_mut().unwrap().store_mut().afflict(slot, crate::world::combat::Condition::Rooted, 1000);
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        let x = stage.horde_mut().unwrap().store().x[slot as usize];
        assert!((x - (px + 2.0)).abs() < 0.3, "pulled 4 cells in: {x}");
    }

    #[test]
    fn a_downed_ally_is_raised() {
        let mut stage = skill_stage("acolyte", 10, &[("raise", 1)]);
        let mut out = Vec::new();
        stage.join(PlayerProfile { archetype: "mage".into(), level: 10, ..profile(2) }, Uuid::nil(), None, 0, &mut out);
        let i2 = stage.find(2).unwrap();
        let f = stage.players[i2].fighter.as_mut().unwrap();
        f.hp = 0;
        f.state = fight::Vital::Downed { ticks: 400 };
        let max = f.max_hp;
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        let f = fighter(&stage, 2);
        assert!(f.is_up());
        assert!(f.hp >= max / 2 - 1 && f.hp <= max / 2 + 1);
    }

    #[test]
    fn a_nova_waits_for_monsters_but_a_self_buff_does_not() {
        let mut stage = skill_stage("swordsman", 10, &[("taunt", 1), ("endure", 1)]);
        let sp = fighter(&stage, 1).sp;
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        assert!(!controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::Cast { .. })), "nothing to taunt");
        assert_eq!(fighter(&stage, 1).sp, sp);
        stage.act(1, [0, 0], 2);
        stage.tick(&mut out);
        assert!(fighter(&stage, 1).temp_hp > 0, "a buff on yourself needs no monster");
    }

    use crate::world::combat::Mods;
    use crate::world::protocol::{hit, HitEvent};

    fn hits_on(out: &Outgoing, user: u32, target: u32) -> Vec<HitEvent> {
        controls_for(out, user)
            .iter()
            .flat_map(|m| match m {
                ServerMsg::Combat { hits, .. } => hits.clone(),
                _ => Vec::new(),
            })
            .filter(|h| h.target == target)
            .collect()
    }

    #[test]
    fn monsters_telegraph_and_land_their_skills_on_players() {
        use crate::world::protocol::MONSTER_ENTITY_BASE;
        let mut stage = skill_stage_with("swordsman", 40, &[], &["splash"], &[]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = stage.horde_mut().unwrap().spawn(0, px + 1.5, py).unwrap();
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 100_000;
        let mut out = Vec::new();
        for _ in 0..140 {
            stage.tick(&mut out);
        }
        let cast = controls_for(&out, 1).into_iter().find_map(|m| match m {
            ServerMsg::Cast { caster, skill, cast_ms, areas, .. } if skill == "splash" => Some((caster, cast_ms, areas)),
            _ => None,
        });
        let (caster, cast_ms, areas) = cast.expect("the monster telegraphs its skill");
        assert_eq!((caster, cast_ms), (MONSTER_ENTITY_BASE + slot as u32, 500));
        assert_eq!((areas.len(), areas[0].a), (1, 3.0), "a 1.5 m splash around the monster");
        let cold = hits_on(&out, 1, 101).iter().filter(|h| h.flags >> 4 == DamageType::Cold as u8).count();
        assert!(cold >= 1, "the splash lands on the player ({cold} cold hits)");
    }

    #[test]
    fn pneuma_turns_ranged_monster_skills_aside() {
        let mut stage = skill_stage_with("mage", 40, &[("pneuma", 1)], &["cloud"], &[]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = stage.horde_mut().unwrap().spawn(0, px + 6.0, py).unwrap();
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 100_000;
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        for _ in 0..140 {
            stage.tick(&mut out);
        }
        assert!(controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::Cast { skill, .. } if skill == "cloud")));
        let hits = hits_on(&out, 1, 101);
        assert!(hits.iter().any(|h| h.flags == hit::GUARD), "the ward guards against the cloud");
        assert!(!hits.iter().any(|h| h.flags >> 4 == DamageType::Poison as u8 && h.amount > 0), "no poison gets through");
        assert!(!fighter(&stage, 1).status.has(combat::Condition::Poisoned));
    }

    #[test]
    fn a_stunned_player_neither_moves_nor_casts_and_poison_bites() {
        use crate::world::combat::Condition;
        let mut stage = skill_stage("mage", 10, &[("bolt", 3)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        stage.horde_mut().unwrap().spawn(0, px + 6.0, py).unwrap();
        let f = stage.players[0].fighter.as_mut().unwrap();
        f.afflict(Condition::Stunned, 20);
        f.afflict(Condition::Poisoned, 60);
        let hp = f.hp;
        let mut out = Vec::new();
        stage.input(1, 1, 1.0, 0.0);
        stage.tick(&mut out);
        assert_eq!(stage.position_of(1).unwrap().1, px, "stunned: no step");
        stage.input(1, 2, 0.0, 0.0);
        stage.act(1, [0, 0], 1);
        stage.tick(&mut out);
        assert!(fighter(&stage, 1).casting.is_none(), "stunned: no cast");
        for _ in 0..40 {
            stage.tick(&mut out);
        }
        let f = fighter(&stage, 1);
        assert!(!f.status.has(Condition::Stunned) && f.status.has(Condition::Poisoned));
        assert!(f.hp < hp, "poison bites once a second");
        assert!(
            controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::PlayerVitals { entity_id: 101, status, .. } if status & Condition::Poisoned.bit() as u32 != 0)),
            "the condition reaches the party"
        );
    }

    #[test]
    fn killing_the_boss_wins_and_everyone_leaves() {
        let mut stage = skill_stage("swordsman", 40, &[]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let k = king(&mut stage);
        let slot = stage.horde_mut().unwrap().spawn(k, px + 9.0, py).unwrap();
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 1_000_000;
        let mut out = Vec::new();
        for _ in 0..25 {
            stage.tick(&mut out);
        }
        assert!(controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::Boss { hp: 1_000_000, .. })), "the boss bar shows up");
        stage.horde_mut().unwrap().place(slot, px + 1.5, py);
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 1;
        stage.horde_mut().unwrap().spawn(0, px - 6.0, py).unwrap();
        out.clear();
        for _ in 0..30 {
            stage.tick(&mut out);
        }
        let msgs = controls_for(&out, 1);
        assert!(msgs.iter().any(|m| matches!(m, ServerMsg::Boss { hp: 0, .. })), "the bar goes away");
        assert!(msgs.iter().any(|m| matches!(m, ServerMsg::StageEnd { victory: true, kills: 1, .. })), "victory");
        assert_eq!(stage.horde_mut().unwrap().store().len(), 0, "the field empties");
        assert!(stage.position_of(1).is_some(), "the result screen stays up for a while");
        for _ in 0..260 {
            stage.tick(&mut out);
        }
        assert!(stage.position_of(1).is_none(), "then everyone leaves");
    }

    #[test]
    fn buffs_reach_the_hud_and_energy_coat_spends_sp_first() {
        let mut stage = skill_stage("swordsman", 10, &[("endure", 1)]);
        let mut out = Vec::new();
        stage.act(1, [0, 0], 1);
        for _ in 0..20 {
            stage.tick(&mut out);
        }
        assert!(controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::Buffs { buffs } if buffs.iter().any(|(id, ms)| id == "endure" && *ms > 0))));
        let f = stage.players[0].fighter.as_mut().unwrap();
        f.temp_hp = 0;
        f.toggle(999, Mods { mana_shield: true, ..Mods::default() });
        let (hp, sp) = (f.hp, f.sp);
        f.take(10, false);
        assert_eq!((f.hp, f.sp), (hp, sp - 5), "1 SP per 2 HP");
        f.toggle(999, Mods::default());
        f.take(4, false);
        assert_eq!(f.hp, hp - 4, "toggled off");
    }

    #[test]
    fn a_channel_lasts_while_held_and_ends_when_let_go() {
        let mut stage = skill_stage("mage", 30, &[("gravity", 1)]);
        let (_, px, py) = stage.position_of(1).unwrap();
        let slot = stage.horde_mut().unwrap().spawn(0, px + 4.0, py).unwrap();
        stage.horde_mut().unwrap().store_mut().hp[slot as usize] = 100_000;
        let mut out = Vec::new();
        for _ in 0..12 {
            stage.act(1, [0, 0], 1);
            stage.tick(&mut out);
        }
        assert!(fighter(&stage, 1).channel.is_some(), "channelling while held");
        assert!(!stage.lives.is_empty());
        out.clear();
        for _ in 0..8 {
            stage.tick(&mut out);
        }
        assert!(fighter(&stage, 1).channel.is_none() && stage.lives.is_empty(), "let go: the field ends");
        assert!(controls_for(&out, 1).iter().any(|m| matches!(m, ServerMsg::ZoneEnd { .. })));
    }
}
