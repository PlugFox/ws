//! Hub, lobby and stage wire protocol: binary WebSocket frames in both
//! directions, built from the primitives in [`super::wire`].
//!
//! The first byte is the message kind (`0xFF` escapes to a two-byte kind;
//! none are assigned yet). Unknown kinds are ignored by both sides so
//! either can grow first; a known kind with a malformed body is an error.
//!
//! Server → client:
//!
//! ```text
//! 0x01 HUB_SNAPSHOT    u16 map_epoch, u32 tick, u32 ack_seq, u16 count,
//!                      count × {u32 entity_id, u16 x, u16 y, u8 facing}
//! 0x03 STAGE_SNAPSHOT  as HUB_SNAPSHOT, then the monster section
//!                      (`horde::view`: dead-reckoning deltas)
//! 0x10 CONNECTED       var user_id, 16 bytes session_id
//! 0x11 PONG            zig ts (ms)
//! 0x12 DISPLACED       str reason
//! 0x13 NOTIFICATION    str message
//! 0x20 HUB_JOINED      var channel, str channel_name, var tick_rate, var entity_id
//! 0x21 MAP_ENTER       map, u16 map_epoch, pos x, pos y, opt str via_portal,
//!                      var n, n × player
//! 0x22 PLAYER_APPEARED player
//! 0x23 PLAYER_VANISHED var entity_id
//! 0x24 PORTAL_DENIED   str portal, str target_map, code reason
//! 0x25 HUB_CHANNELS    var n, n × {var id, str name, var population, var capacity}
//! 0x26 NPC_DIALOG      str npc, str name, str kind, var n, n × mission
//! 0x27 HUB_LEFT        code reason
//! 0x28 HUB_ERROR       code, str message
//! 0x29 PLAYER_LOOK     var entity_id, look (a player on the map restyled)
//! 0x2A PLAYER_VITALS   var entity_id, var hp, var max_hp, var sp, var max_sp,
//!                      u8 state (0 up, 1 downed, 2 dead)
//! 0x2B COMBAT          var n, n × {var entity_id, u8 facing}   (swings)
//!                      var m, m × {var target, var amount, u8 flags}  (hits)
//!                      flags: 1 critical, 2 miss, 4 kill, 8 heal,
//!                      high nibble = damage type; monsters are
//!                      `MONSTER_ENTITY_BASE + slot`
//! 0x2C PROGRESS        var level, var exp, var exp_to_next, var gained
//! 0x2D SKILLS          var level, var points, var n, n × {str skill, u8 level},
//!                      var m, m × str slot skill (empty = free)
//! 0x2E CAST            var caster, str skill (empty = cancel), pos x, pos y,
//!                      var cast_ms, var cooldown_ms
//! 0x30 STAGE_JOINED    var stage_id, str mission, var tick_rate, var entity_id,
//!                      var n, n × str monster kind, f32 light_radius
//! 0x31 LOBBY_STATE     var lobby_id, str code, mission, bool public,
//!                      var n, n × {var entity_id, str name, str archetype, u8 flags}
//!                      (flags: 1 ready, 2 leader)
//! 0x32 LOBBY_LEFT      code reason
//! 0x33 LOBBY_ERROR     code, str message
//!
//! map     = str id, str name, u8 kind (0 hub, 1 indoor, 2 field), var width,
//!           var height, f32 cell_size, camera, opt str bgm
//! camera  = u8 0 + f32 pitch, min_zoom, max_zoom, default_zoom (free)
//!         | u8 1 + f32 yaw, pitch, zoom (fixed)
//! player  = var entity_id, str name, str archetype, pos x, pos y, u8 facing,
//!           look
//! look    = u8 gender (0 male, 1 female), u8 hair style (1..=12),
//!           u8 hair dye (0..16), u8 clothes dye (0..8)
//! mission = str id, str name, str description, str map, u8 max_players
//! ```
//!
//! Client → server:
//!
//! ```text
//! 0x02 INPUT           u32 seq, i16 dx, i16 dy (direction × 32767),
//!                      i8 aim_x, i8 aim_y (from the player, 1/8 m; 0,0 = none),
//!                      u16 slots (skill slots pressed; bit i = slot i)
//!                      (the 9-byte form without aim and slots is accepted)
//! 0x40 PING
//! 0x41 HUB_JOIN        var channel + 1 (0 = server picks)
//! 0x42 HUB_LEAVE
//! 0x43 HUB_CHANNELS
//! 0x44 NPC_INTERACT    str npc
//! 0x45 MISSION_SELECT  str mission, bool public
//! 0x46 LOBBY_JOIN      str code
//! 0x47 LOBBY_READY     bool ready
//! 0x48 LOBBY_START
//! 0x49 LOBBY_LEAVE
//! 0x4A LOBBY_KICK      var entity_id
//! 0x4B STAGE_LEAVE
//! 0x4C RESYNC          the last monster delta did not apply: send a keyframe
//! 0x4D STYLE_SET       look (at a stylist; the gender is kept)
//! 0x4E SKILL_LEARN     str skill (one level up)
//! 0x4F SKILL_RESET     (in the hub only: every point back)
//! 0x50 SKILL_SLOT      u8 slot, str skill (empty = clear)
//! ```
//!
//! `map_epoch` bumps on every map change of the recipient; the client drops
//! snapshots whose epoch differs from the last `MAP_ENTER`.

use super::horde::view::MonsterDelta;
use super::map::{CameraConstraint, LocationKind, MapData};
use super::missions::{self, MissionInfo};
use super::wire::{PutWire, Reader};
use bytes::{BufMut, Bytes, BytesMut};
use uuid::Uuid;

pub use super::wire::{decode_pos, encode_pos};

pub const SNAPSHOT_KIND: u8 = 0x01;
pub const INPUT_KIND: u8 = 0x02;
pub const STAGE_SNAPSHOT_KIND: u8 = 0x03;
pub const SNAPSHOT_HEADER_LEN: usize = 13;
pub const SNAPSHOT_ENTRY_LEN: usize = 9;
/// `u8 kind, u32 seq, i16 dx, i16 dy`.
pub const INPUT_LEN: usize = 13;
/// The first INPUT form, without aim and slots.
pub const INPUT_LEN_V1: usize = 9;

/// Message kinds (first byte).
pub mod kind {
    pub const CONNECTED: u8 = 0x10;
    pub const PONG: u8 = 0x11;
    pub const DISPLACED: u8 = 0x12;
    pub const NOTIFICATION: u8 = 0x13;
    pub const HUB_JOINED: u8 = 0x20;
    pub const MAP_ENTER: u8 = 0x21;
    pub const PLAYER_APPEARED: u8 = 0x22;
    pub const PLAYER_VANISHED: u8 = 0x23;
    pub const PORTAL_DENIED: u8 = 0x24;
    pub const HUB_CHANNELS: u8 = 0x25;
    pub const NPC_DIALOG: u8 = 0x26;
    pub const HUB_LEFT: u8 = 0x27;
    pub const HUB_ERROR: u8 = 0x28;
    pub const PLAYER_LOOK: u8 = 0x29;
    pub const PLAYER_VITALS: u8 = 0x2A;
    pub const COMBAT: u8 = 0x2B;
    pub const PROGRESS: u8 = 0x2C;
    pub const SKILLS: u8 = 0x2D;
    pub const CAST: u8 = 0x2E;
    pub const STAGE_JOINED: u8 = 0x30;
    pub const LOBBY_STATE: u8 = 0x31;
    pub const LOBBY_LEFT: u8 = 0x32;
    pub const LOBBY_ERROR: u8 = 0x33;

    pub const PING: u8 = 0x40;
    pub const HUB_JOIN: u8 = 0x41;
    pub const HUB_LEAVE: u8 = 0x42;
    pub const HUB_CHANNELS_REQ: u8 = 0x43;
    pub const NPC_INTERACT: u8 = 0x44;
    pub const MISSION_SELECT: u8 = 0x45;
    pub const LOBBY_JOIN: u8 = 0x46;
    pub const LOBBY_READY: u8 = 0x47;
    pub const LOBBY_START: u8 = 0x48;
    pub const LOBBY_LEAVE: u8 = 0x49;
    pub const LOBBY_KICK: u8 = 0x4A;
    pub const STAGE_LEAVE: u8 = 0x4B;
    pub const RESYNC: u8 = 0x4C;
    pub const STYLE_SET: u8 = 0x4D;
    pub const SKILL_LEARN: u8 = 0x4E;
    pub const SKILL_RESET: u8 = 0x4F;
    pub const SKILL_SLOT: u8 = 0x50;
}

/// Character appearance. The dyes index the baked palette tables of the
/// customization pack (`client/customization/customization.json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    /// 0 male, 1 female.
    pub gender: u8,
    /// Hair style, `1..=HAIR_STYLES`.
    pub hair: u8,
    pub hair_color: u8,
    pub body_color: u8,
}

impl Default for Look {
    fn default() -> Self {
        Self { gender: 0, hair: 1, hair_color: 0, body_color: 0 }
    }
}

impl Look {
    pub const HAIR_STYLES: u8 = 12;
    pub const HAIR_COLORS: u8 = 16;
    pub const BODY_COLORS: u8 = 8;

    pub fn is_valid(&self) -> bool {
        self.gender < 2
            && (1..=Self::HAIR_STYLES).contains(&self.hair)
            && self.hair_color < Self::HAIR_COLORS
            && self.body_color < Self::BODY_COLORS
    }

    pub fn put(&self, b: &mut BytesMut) {
        b.put_slice(&[self.gender, self.hair, self.hair_color, self.body_color]);
    }

    pub fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(Self { gender: r.u8()?, hair: r.u8()?, hair_color: r.u8()?, body_color: r.u8()? })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapInfo {
    pub id: String,
    pub name: String,
    pub kind: LocationKind,
    pub width: u32,
    pub height: u32,
    pub cell_size: f32,
    pub camera: CameraConstraint,
    pub bgm: Option<String>,
}

impl MapInfo {
    pub fn of(map: &MapData) -> Self {
        let m = &map.manifest;
        Self {
            id: m.id.clone(),
            name: m.name.clone(),
            kind: m.kind,
            width: m.dimensions.width,
            height: m.dimensions.height,
            cell_size: m.dimensions.cell_size,
            camera: m.camera.clone(),
            bgm: m.bgm.clone(),
        }
    }

    fn put(&self, b: &mut BytesMut) {
        b.put_str(&self.id);
        b.put_str(&self.name);
        b.put_u8(match self.kind {
            LocationKind::Hub => 0,
            LocationKind::Indoor => 1,
            LocationKind::Field => 2,
        });
        b.put_var(self.width.into());
        b.put_var(self.height.into());
        b.put_f32_le(self.cell_size);
        match self.camera {
            CameraConstraint::Free { pitch, min_zoom, max_zoom, default_zoom } => {
                b.put_u8(0);
                for v in [pitch, min_zoom, max_zoom, default_zoom] {
                    b.put_f32_le(v);
                }
            }
            CameraConstraint::Fixed { yaw, pitch, zoom } => {
                b.put_u8(1);
                for v in [yaw, pitch, zoom] {
                    b.put_f32_le(v);
                }
            }
        }
        b.put_opt_str(self.bgm.as_deref());
    }

    fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(Self {
            id: r.str()?,
            name: r.str()?,
            kind: match r.u8()? {
                0 => LocationKind::Hub,
                1 => LocationKind::Indoor,
                2 => LocationKind::Field,
                _ => return None,
            },
            width: r.var32()?,
            height: r.var32()?,
            cell_size: r.f32()?,
            camera: match r.u8()? {
                0 => CameraConstraint::Free { pitch: r.f32()?, min_zoom: r.f32()?, max_zoom: r.f32()?, default_zoom: r.f32()? },
                1 => CameraConstraint::Fixed { yaw: r.f32()?, pitch: r.f32()?, zoom: r.f32()? },
                _ => return None,
            },
            bgm: r.opt_str()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PlayerInfo {
    pub entity_id: u32,
    pub name: String,
    pub archetype: String,
    pub x: f32,
    pub y: f32,
    pub facing: u8,
    pub look: Look,
}

impl PlayerInfo {
    fn put(&self, b: &mut BytesMut) {
        b.put_var(self.entity_id.into());
        b.put_str(&self.name);
        b.put_str(&self.archetype);
        b.put_pos(self.x);
        b.put_pos(self.y);
        b.put_u8(self.facing);
        self.look.put(b);
    }

    fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(Self {
            entity_id: r.var32()?,
            name: r.str()?,
            archetype: r.str()?,
            x: r.pos()?,
            y: r.pos()?,
            facing: r.u8()?,
            look: Look::read(r)?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LobbyMemberInfo {
    pub entity_id: u32,
    pub name: String,
    pub archetype: String,
    pub ready: bool,
    pub leader: bool,
}

/// Full lobby state, re-sent to every member on each change.
#[derive(Clone, Debug, PartialEq)]
pub struct LobbyInfo {
    pub lobby_id: u32,
    /// Join code (`lobby_join`); the only way into a private lobby.
    pub code: String,
    pub mission: MissionInfo,
    pub public: bool,
    pub members: Vec<LobbyMemberInfo>,
}

/// Entity id of a monster in combat events: above every character id and
/// exact in a float32 (the client's engine id for the slot).
pub const MONSTER_ENTITY_BASE: u32 = 0x80_0000;

/// An attack animation starts (player or monster) facing `facing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Swing {
    pub entity_id: u32,
    pub facing: u8,
}

/// Bits of [`HitEvent::flags`]; the high nibble is the damage type.
pub mod hit {
    pub const CRITICAL: u8 = 1;
    pub const MISS: u8 = 2;
    pub const KILL: u8 = 4;
    pub const HEAL: u8 = 8;
}

/// A number to show over `target` (damage, heal or "Miss").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HitEvent {
    pub target: u32,
    pub amount: u32,
    pub flags: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChannelInfo {
    pub id: u16,
    pub name: String,
    pub population: usize,
    pub capacity: usize,
}

fn put_mission(b: &mut BytesMut, m: &MissionInfo) {
    b.put_str(m.id);
    b.put_str(m.name);
    b.put_str(m.description);
    b.put_str(m.map);
    b.put_u8(m.max_players);
}

/// Missions decode against the local catalogue (decoding is for tests and
/// bots running next to this server build).
fn read_mission(r: &mut Reader<'_>) -> Option<MissionInfo> {
    let id = r.str()?;
    let (_name, _description, _map, _max) = (r.str()?, r.str()?, r.str()?, r.u8()?);
    missions::find(&id).cloned()
}

fn put_list<T>(b: &mut BytesMut, items: &[T], mut put: impl FnMut(&mut BytesMut, &T)) {
    b.put_var(items.len() as u64);
    for item in items {
        put(b, item);
    }
}

fn read_list<T>(r: &mut Reader<'_>, mut read: impl FnMut(&mut Reader<'_>) -> Option<T>) -> Option<Vec<T>> {
    let n = r.var32()? as usize;
    // Every item is at least one byte: bound the allocation by the frame.
    if n > r.remaining() {
        return None;
    }
    (0..n).map(|_| read(r)).collect()
}

/// Server → client control messages.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerMsg {
    /// First frame on a socket.
    Connected { user_id: u32, session_id: Uuid },
    Pong { ts: i64 },
    /// Signed in elsewhere; the socket closes after this.
    Displaced { reason: String },
    Notification { message: String },
    HubJoined {
        channel: u16,
        channel_name: String,
        tick_rate: u32,
        entity_id: u32,
    },
    /// The recipient is now on `map`. Everything about the previous map
    /// (entities, camera) must be discarded.
    MapEnter {
        map: MapInfo,
        map_epoch: u16,
        x: f32,
        y: f32,
        via_portal: Option<String>,
        /// Other players already on this map.
        players: Vec<PlayerInfo>,
    },
    PlayerAppeared { player: PlayerInfo },
    PlayerVanished { entity_id: u32 },
    /// The player stepped into a portal whose target is not part of the hub
    /// (e.g. a combat field, which is entered through the mission board).
    PortalDenied {
        portal: String,
        target_map: String,
        reason: String,
    },
    HubChannels { channels: Vec<ChannelInfo> },
    /// Answer to `npc_interact`. `missions` is filled for mission boards.
    NpcDialog {
        npc: String,
        name: String,
        kind: String,
        missions: Vec<MissionInfo>,
    },
    HubLeft { reason: String },
    HubError { code: String, message: String },
    /// A player on the recipient's map (possibly the recipient) restyled.
    PlayerLook { entity_id: u32, look: Look },
    /// Health and spell points of a player on the recipient's map (stages);
    /// sent on join and whenever they change.
    PlayerVitals { entity_id: u32, hp: u32, max_hp: u32, sp: u32, max_sp: u32, state: u8 },
    /// Attacks and numbers of one tick on the recipient's map.
    Combat { swings: Vec<Swing>, hits: Vec<HitEvent> },
    /// The recipient's level and experience into it; `gained` since the
    /// last progress message (kills, or negative-free penalties as 0).
    Progress { level: u32, exp: u64, exp_to_next: u64, gained: u64 },
    /// The recipient's learned skills, unspent points and hotbar (on join
    /// and after every change).
    Skills { level: u32, points: u32, skills: Vec<(String, u8)>, hotbar: Vec<String> },
    /// A player starts a skill: the telegraph at `x, y` for `cast_ms`, then
    /// `cooldown_ms`. An empty `skill` cancels the caster's wind-up.
    Cast { caster: u32, skill: String, x: f32, y: f32, cast_ms: u32, cooldown_ms: u32 },
    /// The recipient joined a stage instance; `map_enter` follows. Sent
    /// instead of `hub_joined` (the hub membership has ended).
    StageJoined {
        stage_id: u32,
        mission: String,
        tick_rate: u32,
        entity_id: u32,
        /// Monster kind ids; a snapshot's `kind` byte indexes this list
        /// (also the sprite names in the location atlas).
        monsters: Vec<String>,
        /// The recipient's light radius in cells: monsters are visible (and
        /// sent) only within it. 0 when the stage has no monsters.
        light_radius: f32,
    },
    /// The recipient is in a lobby; sent on every change.
    LobbyState { lobby: LobbyInfo },
    /// The recipient is no longer in a lobby: `left`, `kicked`, `launched`,
    /// `disbanded`, `replaced`.
    LobbyLeft { reason: String },
    /// Lobby errors: `already_in_lobby`, `not_in_lobby`, `no_such_lobby`,
    /// `lobby_full`, `not_leader`, `not_ready`, `stage_full`,
    /// `mission_unavailable`, `not_in_hub`.
    LobbyError { code: String, message: String },
}

impl ServerMsg {
    pub fn encode(&self) -> Bytes {
        let mut b = BytesMut::with_capacity(64);
        match self {
            ServerMsg::Connected { user_id, session_id } => {
                b.put_u8(kind::CONNECTED);
                b.put_var((*user_id).into());
                b.put_slice(session_id.as_bytes());
            }
            ServerMsg::Pong { ts } => {
                b.put_u8(kind::PONG);
                b.put_zig(*ts);
            }
            ServerMsg::Displaced { reason } => {
                b.put_u8(kind::DISPLACED);
                b.put_str(reason);
            }
            ServerMsg::Notification { message } => {
                b.put_u8(kind::NOTIFICATION);
                b.put_str(message);
            }
            ServerMsg::HubJoined { channel, channel_name, tick_rate, entity_id } => {
                b.put_u8(kind::HUB_JOINED);
                b.put_var((*channel).into());
                b.put_str(channel_name);
                b.put_var((*tick_rate).into());
                b.put_var((*entity_id).into());
            }
            ServerMsg::MapEnter { map, map_epoch, x, y, via_portal, players } => {
                b.put_u8(kind::MAP_ENTER);
                map.put(&mut b);
                b.put_u16_le(*map_epoch);
                b.put_pos(*x);
                b.put_pos(*y);
                b.put_opt_str(via_portal.as_deref());
                put_list(&mut b, players, |b, p| p.put(b));
            }
            ServerMsg::PlayerAppeared { player } => {
                b.put_u8(kind::PLAYER_APPEARED);
                player.put(&mut b);
            }
            ServerMsg::PlayerVanished { entity_id } => {
                b.put_u8(kind::PLAYER_VANISHED);
                b.put_var((*entity_id).into());
            }
            ServerMsg::PortalDenied { portal, target_map, reason } => {
                b.put_u8(kind::PORTAL_DENIED);
                b.put_str(portal);
                b.put_str(target_map);
                b.put_code(reason);
            }
            ServerMsg::HubChannels { channels } => {
                b.put_u8(kind::HUB_CHANNELS);
                put_list(&mut b, channels, |b, c| {
                    b.put_var(c.id.into());
                    b.put_str(&c.name);
                    b.put_var(c.population as u64);
                    b.put_var(c.capacity as u64);
                });
            }
            ServerMsg::NpcDialog { npc, name, kind: npc_kind, missions } => {
                b.put_u8(kind::NPC_DIALOG);
                b.put_str(npc);
                b.put_str(name);
                b.put_str(npc_kind);
                put_list(&mut b, missions, put_mission);
            }
            ServerMsg::HubLeft { reason } => {
                b.put_u8(kind::HUB_LEFT);
                b.put_code(reason);
            }
            ServerMsg::HubError { code, message } => {
                b.put_u8(kind::HUB_ERROR);
                b.put_code(code);
                b.put_str(message);
            }
            ServerMsg::PlayerLook { entity_id, look } => {
                b.put_u8(kind::PLAYER_LOOK);
                b.put_var((*entity_id).into());
                look.put(&mut b);
            }
            ServerMsg::PlayerVitals { entity_id, hp, max_hp, sp, max_sp, state } => {
                b.put_u8(kind::PLAYER_VITALS);
                for v in [*entity_id, *hp, *max_hp, *sp, *max_sp] {
                    b.put_var(v.into());
                }
                b.put_u8(*state);
            }
            ServerMsg::Combat { swings, hits } => {
                b.put_u8(kind::COMBAT);
                put_list(&mut b, swings, |b, s| {
                    b.put_var(s.entity_id.into());
                    b.put_u8(s.facing);
                });
                put_list(&mut b, hits, |b, h| {
                    b.put_var(h.target.into());
                    b.put_var(h.amount.into());
                    b.put_u8(h.flags);
                });
            }
            ServerMsg::Progress { level, exp, exp_to_next, gained } => {
                b.put_u8(kind::PROGRESS);
                b.put_var((*level).into());
                b.put_var(*exp);
                b.put_var(*exp_to_next);
                b.put_var(*gained);
            }
            ServerMsg::Skills { level, points, skills, hotbar } => {
                b.put_u8(kind::SKILLS);
                b.put_var((*level).into());
                b.put_var((*points).into());
                put_list(&mut b, skills, |b, (id, lvl)| {
                    b.put_str(id);
                    b.put_u8(*lvl);
                });
                put_list(&mut b, hotbar, |b, id| b.put_str(id));
            }
            ServerMsg::Cast { caster, skill, x, y, cast_ms, cooldown_ms } => {
                b.put_u8(kind::CAST);
                b.put_var((*caster).into());
                b.put_str(skill);
                b.put_pos(*x);
                b.put_pos(*y);
                b.put_var((*cast_ms).into());
                b.put_var((*cooldown_ms).into());
            }
            ServerMsg::StageJoined { stage_id, mission, tick_rate, entity_id, monsters, light_radius } => {
                b.put_u8(kind::STAGE_JOINED);
                b.put_var((*stage_id).into());
                b.put_str(mission);
                b.put_var((*tick_rate).into());
                b.put_var((*entity_id).into());
                put_list(&mut b, monsters, |b, m| b.put_str(m));
                b.put_f32_le(*light_radius);
            }
            ServerMsg::LobbyState { lobby } => {
                b.put_u8(kind::LOBBY_STATE);
                b.put_var(lobby.lobby_id.into());
                b.put_str(&lobby.code);
                put_mission(&mut b, &lobby.mission);
                b.put_bool(lobby.public);
                put_list(&mut b, &lobby.members, |b, m| {
                    b.put_var(m.entity_id.into());
                    b.put_str(&m.name);
                    b.put_str(&m.archetype);
                    b.put_u8(u8::from(m.ready) | u8::from(m.leader) << 1);
                });
            }
            ServerMsg::LobbyLeft { reason } => {
                b.put_u8(kind::LOBBY_LEFT);
                b.put_code(reason);
            }
            ServerMsg::LobbyError { code, message } => {
                b.put_u8(kind::LOBBY_ERROR);
                b.put_code(code);
                b.put_str(message);
            }
        }
        b.freeze()
    }

    /// Decodes a control message; `None` for snapshots, unknown kinds and
    /// malformed frames.
    pub fn decode(frame: &[u8]) -> Option<Self> {
        let mut r = Reader::new(frame);
        let k = r.kind()?;
        let k = u8::try_from(k).ok()?;
        let msg = match k {
            kind::CONNECTED => ServerMsg::Connected {
                user_id: r.var32()?,
                session_id: Uuid::from_slice(r.bytes(16)?).ok()?,
            },
            kind::PONG => ServerMsg::Pong { ts: r.zig()? },
            kind::DISPLACED => ServerMsg::Displaced { reason: r.str()? },
            kind::NOTIFICATION => ServerMsg::Notification { message: r.str()? },
            kind::HUB_JOINED => ServerMsg::HubJoined {
                channel: u16::try_from(r.var()?).ok()?,
                channel_name: r.str()?,
                tick_rate: r.var32()?,
                entity_id: r.var32()?,
            },
            kind::MAP_ENTER => ServerMsg::MapEnter {
                map: MapInfo::read(&mut r)?,
                map_epoch: r.u16()?,
                x: r.pos()?,
                y: r.pos()?,
                via_portal: r.opt_str()?,
                players: read_list(&mut r, PlayerInfo::read)?,
            },
            kind::PLAYER_APPEARED => ServerMsg::PlayerAppeared { player: PlayerInfo::read(&mut r)? },
            kind::PLAYER_VANISHED => ServerMsg::PlayerVanished { entity_id: r.var32()? },
            kind::PORTAL_DENIED => ServerMsg::PortalDenied { portal: r.str()?, target_map: r.str()?, reason: r.code()? },
            kind::HUB_CHANNELS => ServerMsg::HubChannels {
                channels: read_list(&mut r, |r| {
                    Some(ChannelInfo {
                        id: u16::try_from(r.var()?).ok()?,
                        name: r.str()?,
                        population: r.var()? as usize,
                        capacity: r.var()? as usize,
                    })
                })?,
            },
            kind::NPC_DIALOG => ServerMsg::NpcDialog {
                npc: r.str()?,
                name: r.str()?,
                kind: r.str()?,
                missions: read_list(&mut r, read_mission)?,
            },
            kind::HUB_LEFT => ServerMsg::HubLeft { reason: r.code()? },
            kind::HUB_ERROR => ServerMsg::HubError { code: r.code()?, message: r.str()? },
            kind::PLAYER_LOOK => ServerMsg::PlayerLook { entity_id: r.var32()?, look: Look::read(&mut r)? },
            kind::PLAYER_VITALS => ServerMsg::PlayerVitals {
                entity_id: r.var32()?,
                hp: r.var32()?,
                max_hp: r.var32()?,
                sp: r.var32()?,
                max_sp: r.var32()?,
                state: r.u8()?,
            },
            kind::COMBAT => ServerMsg::Combat {
                swings: read_list(&mut r, |r| Some(Swing { entity_id: r.var32()?, facing: r.u8()? }))?,
                hits: read_list(&mut r, |r| Some(HitEvent { target: r.var32()?, amount: r.var32()?, flags: r.u8()? }))?,
            },
            kind::PROGRESS => ServerMsg::Progress { level: r.var32()?, exp: r.var()?, exp_to_next: r.var()?, gained: r.var()? },
            kind::SKILLS => ServerMsg::Skills {
                level: r.var32()?,
                points: r.var32()?,
                skills: read_list(&mut r, |r| Some((r.str()?, r.u8()?)))?,
                hotbar: read_list(&mut r, |r| r.str())?,
            },
            kind::CAST => ServerMsg::Cast {
                caster: r.var32()?,
                skill: r.str()?,
                x: r.pos()?,
                y: r.pos()?,
                cast_ms: r.var32()?,
                cooldown_ms: r.var32()?,
            },
            kind::STAGE_JOINED => ServerMsg::StageJoined {
                stage_id: r.var32()?,
                mission: r.str()?,
                tick_rate: r.var32()?,
                entity_id: r.var32()?,
                monsters: read_list(&mut r, |r| r.str())?,
                light_radius: r.f32()?,
            },
            kind::LOBBY_STATE => ServerMsg::LobbyState {
                lobby: LobbyInfo {
                    lobby_id: r.var32()?,
                    code: r.str()?,
                    mission: read_mission(&mut r)?,
                    public: r.bool()?,
                    members: read_list(&mut r, |r| {
                        let (entity_id, name, archetype, flags) = (r.var32()?, r.str()?, r.str()?, r.u8()?);
                        Some(LobbyMemberInfo { entity_id, name, archetype, ready: flags & 1 != 0, leader: flags & 2 != 0 })
                    })?,
                },
            },
            kind::LOBBY_LEFT => ServerMsg::LobbyLeft { reason: r.code()? },
            kind::LOBBY_ERROR => ServerMsg::LobbyError { code: r.code()?, message: r.str()? },
            _ => return None,
        };
        r.is_empty().then_some(msg)
    }
}

/// Client → server messages.
#[derive(Clone, Debug, PartialEq)]
pub enum ClientMsg {
    Ping,
    /// Join the hub (or switch channel when already in it). `None` lets the
    /// server pick.
    HubJoin { channel: Option<u16> },
    HubLeave,
    HubChannels,
    /// Movement intent: desired direction, length ≤ 1 (longer is normalised).
    /// `seq` must increase; older or repeated values are ignored.
    /// `aim`: where the player points, relative to them, in 1/8 m (0,0 =
    /// no aim); `slots`: skill slots pressed this tick (bit i = slot i).
    Input { seq: u32, dx: f32, dy: f32, aim: [i8; 2], slots: u16 },
    /// Talk to an NPC on the current map; must be within its radius.
    NpcInteract { npc: String },
    /// Pick a mission at a mission board the player is standing at: joins
    /// the oldest public lobby of that mission with room (or opens one), or
    /// opens a private lobby when `public` is false.
    MissionSelect { mission: String, public: bool },
    /// Join a lobby by its code (public or private), from anywhere in the hub.
    LobbyJoin { code: String },
    LobbyReady { ready: bool },
    /// Leader only; every other member must be ready.
    LobbyStart,
    LobbyLeave,
    /// Leader only: remove a member (by entity id).
    LobbyKick { entity_id: u32 },
    /// Abandon the stage and return to the hub.
    StageLeave,
    /// A monster delta did not apply on the client: send a keyframe.
    Resync,
    /// Restyle at a stylist NPC (hair style and dyes; the gender is kept).
    StyleSet { look: Look },
    /// Spend a skill point on `skill`.
    SkillLearn { skill: String },
    /// Take every skill point back (hub only).
    SkillReset,
    /// Put a learned active skill in a hotbar slot (empty = clear).
    SkillSlot { slot: u8, skill: String },
}

/// Why a client frame was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientDecodeError {
    /// Unknown kind: ignored (newer client).
    Unknown,
    /// Known kind, bad body.
    Malformed,
}

impl ClientMsg {
    pub fn encode(&self) -> Bytes {
        let mut b = BytesMut::with_capacity(16);
        match self {
            ClientMsg::Ping => b.put_u8(kind::PING),
            ClientMsg::HubJoin { channel } => {
                b.put_u8(kind::HUB_JOIN);
                b.put_var(channel.map_or(0, |c| u64::from(c) + 1));
            }
            ClientMsg::HubLeave => b.put_u8(kind::HUB_LEAVE),
            ClientMsg::HubChannels => b.put_u8(kind::HUB_CHANNELS_REQ),
            ClientMsg::Input { seq, dx, dy, aim, slots } => {
                b.put_u8(INPUT_KIND);
                b.put_u32_le(*seq);
                b.put_i16_le((dx.clamp(-1.0, 1.0) * 32767.0).round() as i16);
                b.put_i16_le((dy.clamp(-1.0, 1.0) * 32767.0).round() as i16);
                b.put_i8(aim[0]);
                b.put_i8(aim[1]);
                b.put_u16_le(*slots);
            }
            ClientMsg::NpcInteract { npc } => {
                b.put_u8(kind::NPC_INTERACT);
                b.put_str(npc);
            }
            ClientMsg::MissionSelect { mission, public } => {
                b.put_u8(kind::MISSION_SELECT);
                b.put_str(mission);
                b.put_bool(*public);
            }
            ClientMsg::LobbyJoin { code } => {
                b.put_u8(kind::LOBBY_JOIN);
                b.put_str(code);
            }
            ClientMsg::LobbyReady { ready } => {
                b.put_u8(kind::LOBBY_READY);
                b.put_bool(*ready);
            }
            ClientMsg::LobbyStart => b.put_u8(kind::LOBBY_START),
            ClientMsg::LobbyLeave => b.put_u8(kind::LOBBY_LEAVE),
            ClientMsg::LobbyKick { entity_id } => {
                b.put_u8(kind::LOBBY_KICK);
                b.put_var((*entity_id).into());
            }
            ClientMsg::StageLeave => b.put_u8(kind::STAGE_LEAVE),
            ClientMsg::Resync => b.put_u8(kind::RESYNC),
            ClientMsg::StyleSet { look } => {
                b.put_u8(kind::STYLE_SET);
                look.put(&mut b);
            }
            ClientMsg::SkillLearn { skill } => {
                b.put_u8(kind::SKILL_LEARN);
                b.put_str(skill);
            }
            ClientMsg::SkillReset => b.put_u8(kind::SKILL_RESET),
            ClientMsg::SkillSlot { slot, skill } => {
                b.put_u8(kind::SKILL_SLOT);
                b.put_u8(*slot);
                b.put_str(skill);
            }
        }
        b.freeze()
    }

    pub fn decode(frame: &[u8]) -> Result<Self, ClientDecodeError> {
        use ClientDecodeError::*;
        let mut r = Reader::new(frame);
        let k = r.kind().ok_or(Malformed)?;
        let Ok(k) = u8::try_from(k) else { return Err(Unknown) };
        let body = |r: &mut Reader<'_>| -> Option<Self> {
            Some(match k {
                kind::PING => ClientMsg::Ping,
                kind::HUB_JOIN => ClientMsg::HubJoin {
                    channel: match r.var()? {
                        0 => None,
                        c => Some(u16::try_from(c - 1).ok()?),
                    },
                },
                kind::HUB_LEAVE => ClientMsg::HubLeave,
                kind::HUB_CHANNELS_REQ => ClientMsg::HubChannels,
                INPUT_KIND => {
                    let seq = r.u32()?;
                    let dx = r.u16()? as i16 as f32 / 32767.0;
                    let dy = r.u16()? as i16 as f32 / 32767.0;
                    let (aim, slots) = if r.is_empty() { ([0, 0], 0) } else { ([r.i8()?, r.i8()?], r.u16()?) };
                    ClientMsg::Input { seq, dx, dy, aim, slots }
                }
                kind::NPC_INTERACT => ClientMsg::NpcInteract { npc: r.str()? },
                kind::MISSION_SELECT => ClientMsg::MissionSelect { mission: r.str()?, public: r.bool()? },
                kind::LOBBY_JOIN => ClientMsg::LobbyJoin { code: r.str()? },
                kind::LOBBY_READY => ClientMsg::LobbyReady { ready: r.bool()? },
                kind::LOBBY_START => ClientMsg::LobbyStart,
                kind::LOBBY_LEAVE => ClientMsg::LobbyLeave,
                kind::LOBBY_KICK => ClientMsg::LobbyKick { entity_id: r.var32()? },
                kind::STAGE_LEAVE => ClientMsg::StageLeave,
                kind::RESYNC => ClientMsg::Resync,
                kind::STYLE_SET => ClientMsg::StyleSet { look: Look::read(r)? },
                kind::SKILL_LEARN => ClientMsg::SkillLearn { skill: r.str()? },
                kind::SKILL_RESET => ClientMsg::SkillReset,
                kind::SKILL_SLOT => ClientMsg::SkillSlot { slot: r.u8()?, skill: r.str()? },
                _ => return None,
            })
        };
        let known = matches!(k, INPUT_KIND | kind::PING..=kind::SKILL_SLOT);
        if !known {
            return Err(Unknown);
        }
        match body(&mut r) {
            Some(msg) if r.is_empty() => Ok(msg),
            _ => Err(Malformed),
        }
    }
}

/// Shared player block of a snapshot (identical for all recipients on a map).
pub struct SnapshotBody {
    buf: BytesMut,
    count: u16,
}

impl SnapshotBody {
    pub fn with_capacity(entities: usize) -> Self {
        Self { buf: BytesMut::with_capacity(entities * SNAPSHOT_ENTRY_LEN), count: 0 }
    }

    pub fn push(&mut self, entity_id: u32, x: f32, y: f32, facing: u8) {
        self.buf.put_u32_le(entity_id);
        self.buf.put_u16_le(encode_pos(x));
        self.buf.put_u16_le(encode_pos(y));
        self.buf.put_u8(facing);
        self.count += 1;
    }

    pub fn frame(&self, map_epoch: u16, tick: u32, ack_seq: u32) -> Bytes {
        let mut out = BytesMut::with_capacity(SNAPSHOT_HEADER_LEN + self.buf.len());
        self.put_head(&mut out, SNAPSHOT_KIND, map_epoch, tick, ack_seq);
        out.freeze()
    }

    /// Stage snapshot: this body plus the recipient's monster section
    /// (`horde::view`).
    pub fn stage_frame(&self, map_epoch: u16, tick: u32, ack_seq: u32, monsters: &[u8]) -> Bytes {
        let mut out = BytesMut::with_capacity(SNAPSHOT_HEADER_LEN + self.buf.len() + monsters.len());
        self.put_head(&mut out, STAGE_SNAPSHOT_KIND, map_epoch, tick, ack_seq);
        out.extend_from_slice(monsters);
        out.freeze()
    }

    fn put_head(&self, out: &mut BytesMut, kind: u8, map_epoch: u16, tick: u32, ack_seq: u32) {
        out.put_u8(kind);
        out.put_u16_le(map_epoch);
        out.put_u32_le(tick);
        out.put_u32_le(ack_seq);
        out.put_u16_le(self.count);
        out.extend_from_slice(&self.buf);
    }
}

/// Decoded snapshot (used by tests and headless bots).
#[derive(Debug, PartialEq)]
pub struct Snapshot {
    pub map_epoch: u16,
    pub tick: u32,
    pub ack_seq: u32,
    pub entities: Vec<(u32, f32, f32, u8)>,
    /// The monster section of a stage snapshot.
    pub monsters: Option<MonsterDelta>,
}

impl Snapshot {
    /// Decodes a hub or stage snapshot.
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < SNAPSHOT_HEADER_LEN || (b[0] != SNAPSHOT_KIND && b[0] != STAGE_SNAPSHOT_KIND) {
            return None;
        }
        let mut r = Reader::new(b);
        r.u8()?;
        let (map_epoch, tick, ack_seq, count) = (r.u16()?, r.u32()?, r.u32()?, r.u16()?);
        if r.remaining() < count as usize * SNAPSHOT_ENTRY_LEN {
            return None;
        }
        let mut entities = Vec::with_capacity(count as usize);
        for _ in 0..count {
            entities.push((r.u32()?, r.pos()?, r.pos()?, r.u8()?));
        }
        let monsters = if b[0] == STAGE_SNAPSHOT_KIND { Some(MonsterDelta::decode(&mut r)?) } else { None };
        r.is_empty().then_some(Self { map_epoch, tick, ack_seq, entities, monsters })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::horde::view::DeltaEntry;

    #[test]
    fn snapshot_round_trip() {
        let mut body = SnapshotBody::with_capacity(2);
        body.push(7, 12.5, 300.25, 3);
        body.push(u32::MAX, 0.0, 1023.0, 7);
        let frame = body.frame(9, 1234, 55);
        assert_eq!(frame.len(), SNAPSHOT_HEADER_LEN + 2 * SNAPSHOT_ENTRY_LEN);
        let s = Snapshot::decode(&frame).unwrap();
        assert_eq!(s.map_epoch, 9);
        assert_eq!(s.tick, 1234);
        assert_eq!(s.ack_seq, 55);
        assert_eq!(s.entities, vec![(7, 12.5, 300.25, 3), (u32::MAX, 0.0, 1023.0, 7)]);
        assert_eq!(s.monsters, None);
        assert!(Snapshot::decode(&frame[..frame.len() - 1]).is_none());
    }

    #[test]
    fn stage_snapshot_carries_the_monster_section() {
        let mut body = SnapshotBody::with_capacity(1);
        body.push(7, 12.5, 30.0, 3);
        // keyframe, one SPAWN at slot 513.
        let mut m = BytesMut::new();
        m.put_var(0);
        m.put_var(1);
        m.put_var(513 << 2 | 2);
        m.put_slice(&[4, 2]);
        m.put_u16_le(encode_pos(100.25));
        m.put_u16_le(encode_pos(7.5));
        m.put_slice(&[10, 0xF6]);
        let frame = body.stage_frame(1, 2, 3, &m);
        let s = Snapshot::decode(&frame).unwrap();
        assert_eq!(s.entities, vec![(7, 12.5, 30.0, 3)]);
        let d = s.monsters.unwrap();
        assert_eq!(d.baseline, 0);
        assert_eq!(
            d.entries,
            vec![DeltaEntry::Spawn { slot: 513, generation: 4, kind: 2, x: 6416, y: 480, vx: 10, vy: -10 }]
        );
        assert!(Snapshot::decode(&frame[..frame.len() - 1]).is_none());
        assert!(Snapshot::decode(&[frame.as_ref(), &[0]].concat()).is_none(), "trailing bytes");
    }

    #[test]
    fn fixed_point_precision_is_one_64th_cell() {
        for v in [0.0f32, 0.01, 1.0 / 64.0, 133.337, 1023.98] {
            assert!((decode_pos(encode_pos(v)) - v).abs() <= 1.0 / 128.0 + f32::EPSILON, "{v}");
        }
        assert_eq!(encode_pos(-3.0), 0, "negative clamps");
    }

    #[test]
    fn client_messages_round_trip() {
        let all = [
            ClientMsg::Ping,
            ClientMsg::HubJoin { channel: None },
            ClientMsg::HubJoin { channel: Some(0) },
            ClientMsg::HubJoin { channel: Some(2) },
            ClientMsg::HubLeave,
            ClientMsg::HubChannels,
            ClientMsg::Input { seq: 7, dx: 1.0, dy: -0.5, aim: [-12, 100], slots: 0b10_0000_0101 },
            ClientMsg::SkillLearn { skill: "bolt".into() },
            ClientMsg::SkillReset,
            ClientMsg::SkillSlot { slot: 4, skill: "slide".into() },
            ClientMsg::NpcInteract { npc: "board".into() },
            ClientMsg::MissionSelect { mission: "m".into(), public: false },
            ClientMsg::LobbyJoin { code: "AB2CD".into() },
            ClientMsg::LobbyReady { ready: true },
            ClientMsg::LobbyStart,
            ClientMsg::LobbyLeave,
            ClientMsg::LobbyKick { entity_id: 300 },
            ClientMsg::StageLeave,
            ClientMsg::Resync,
            ClientMsg::StyleSet { look: Look { gender: 1, hair: 12, hair_color: 15, body_color: 7 } },
        ];
        for m in all {
            let b = m.encode();
            match (ClientMsg::decode(&b).unwrap(), &m) {
                (ClientMsg::Input { seq, dx, dy, aim, slots }, ClientMsg::Input { seq: s, dx: x, dy: y, aim: a, slots: k }) => {
                    assert_eq!((seq, aim, slots), (*s, *a, *k));
                    assert!((dx - x).abs() < 1e-4 && (dy - y).abs() < 1e-4);
                }
                (got, want) => assert_eq!(&got, want),
            }
        }
        let input = ClientMsg::Input { seq: 1, dx: 0.0, dy: 0.0, aim: [0, 0], slots: 0 }.encode();
        assert_eq!(input.len(), INPUT_LEN);
        assert_eq!(ClientMsg::decode(&input[..INPUT_LEN_V1]), Ok(ClientMsg::Input { seq: 1, dx: 0.0, dy: 0.0, aim: [0, 0], slots: 0 }), "the first form still decodes");
        assert_eq!(ClientMsg::decode(&input[..INPUT_LEN - 1]), Err(ClientDecodeError::Malformed));
        assert_eq!(ClientMsg::Ping.encode().len(), 1);
    }

    #[test]
    fn client_decode_errors() {
        assert_eq!(ClientMsg::decode(&[]), Err(ClientDecodeError::Malformed));
        assert_eq!(ClientMsg::decode(&[0x7F]), Err(ClientDecodeError::Unknown));
        assert_eq!(ClientMsg::decode(&[0xFF, 1]), Err(ClientDecodeError::Unknown), "extended kinds");
        assert_eq!(ClientMsg::decode(&[kind::LOBBY_READY]), Err(ClientDecodeError::Malformed));
        assert_eq!(ClientMsg::decode(&[kind::LOBBY_READY, 1, 0]), Err(ClientDecodeError::Malformed), "trailing");
        assert_eq!(ClientMsg::decode(&[kind::HUB_JOIN, 0xFF, 0xFF, 0x7F]), Err(ClientDecodeError::Malformed));
        assert_eq!(ClientMsg::decode(b"{\"type\":\"ping\"}"), Err(ClientDecodeError::Unknown), "JSON is gone");
    }

    #[test]
    fn server_messages_round_trip() {
        let mission = missions::MISSIONS[0].clone();
        let player = PlayerInfo { entity_id: 5, name: "Ann".into(), archetype: "novice".into(), x: 10.5, y: 3.25, facing: 2, look: Look { gender: 1, hair: 7, hair_color: 3, body_color: 5 } };
        let all = vec![
            ServerMsg::Connected { user_id: 9, session_id: Uuid::from_u128(0x1234) },
            ServerMsg::Pong { ts: 1_700_000_000_000 },
            ServerMsg::Displaced { reason: "new login".into() },
            ServerMsg::Notification { message: "hi".into() },
            ServerMsg::HubJoined { channel: 1, channel_name: "Hub-1".into(), tick_rate: 20, entity_id: 5 },
            ServerMsg::MapEnter {
                map: MapInfo {
                    id: "izlude".into(),
                    name: "Izlude".into(),
                    kind: LocationKind::Hub,
                    width: 300,
                    height: 280,
                    cell_size: 0.5,
                    camera: CameraConstraint::Free { pitch: 45.0, min_zoom: 0.5, max_zoom: 2.0, default_zoom: 1.0 },
                    bgm: Some("town.ogg".into()),
                },
                map_epoch: 3,
                x: 150.0,
                y: 140.5,
                via_portal: None,
                players: vec![player.clone()],
            },
            ServerMsg::MapEnter {
                map: MapInfo {
                    id: "in".into(),
                    name: "Inn".into(),
                    kind: LocationKind::Indoor,
                    width: 40,
                    height: 40,
                    cell_size: 0.5,
                    camera: CameraConstraint::Fixed { yaw: 0.0, pitch: 50.0, zoom: 1.0 },
                    bgm: None,
                },
                map_epoch: 4,
                x: 1.0,
                y: 2.0,
                via_portal: Some("door".into()),
                players: vec![],
            },
            ServerMsg::PlayerAppeared { player },
            ServerMsg::PlayerVanished { entity_id: 5 },
            ServerMsg::PortalDenied { portal: "p".into(), target_map: "prt_fild08".into(), reason: "stage_requires_mission".into() },
            ServerMsg::HubChannels { channels: vec![ChannelInfo { id: 1, name: "Hub-1".into(), population: 3, capacity: 100 }] },
            ServerMsg::NpcDialog { npc: "board".into(), name: "Board".into(), kind: "mission_board".into(), missions: vec![mission.clone()] },
            ServerMsg::HubLeft { reason: "evicted".into() },
            ServerMsg::HubError { code: "bad_message".into(), message: "nope".into() },
            ServerMsg::PlayerLook { entity_id: 5, look: Look { gender: 0, hair: 3, hair_color: 9, body_color: 2 } },
            ServerMsg::PlayerVitals { entity_id: 5, hp: 40, max_hp: 122, sp: 0, max_sp: 131, state: 1 },
            ServerMsg::Combat {
                swings: vec![Swing { entity_id: 5, facing: 3 }],
                hits: vec![HitEvent { target: MONSTER_ENTITY_BASE + 7, amount: 13, flags: hit::CRITICAL | hit::KILL | 0x30 }],
            },
            ServerMsg::Progress { level: 31, exp: 87_000, exp_to_next: 97_000, gained: 353 },
            ServerMsg::Skills { level: 12, points: 3, skills: vec![("bolt".into(), 4), ("slide".into(), 1)], hotbar: vec!["bolt".into(), String::new()] },
            ServerMsg::Cast { caster: 300, skill: "bolt".into(), x: 10.5, y: 3.25, cast_ms: 540, cooldown_ms: 1000 },
            ServerMsg::Cast { caster: 300, skill: String::new(), x: 0.0, y: 0.0, cast_ms: 0, cooldown_ms: 0 },
            ServerMsg::StageJoined {
                stage_id: 4,
                mission: mission.id.into(),
                tick_rate: 20,
                entity_id: 5,
                monsters: vec!["poring".into(), "fabre".into()],
                light_radius: 40.0,
            },
            ServerMsg::LobbyState {
                lobby: LobbyInfo {
                    lobby_id: 2,
                    code: "AB2CD".into(),
                    mission,
                    public: true,
                    members: vec![
                        LobbyMemberInfo { entity_id: 5, name: "Ann".into(), archetype: "novice".into(), ready: false, leader: true },
                        LobbyMemberInfo { entity_id: 6, name: "Bo".into(), archetype: "novice".into(), ready: true, leader: false },
                    ],
                },
            },
            ServerMsg::LobbyLeft { reason: "some_new_reason".into() },
            ServerMsg::LobbyError { code: "lobby_full".into(), message: "full".into() },
        ];
        for m in all {
            let b = m.encode();
            assert_eq!(ServerMsg::decode(&b).as_ref(), Some(&m), "{m:?}");
            assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none(), "truncated {m:?}");
        }
        assert_eq!(ServerMsg::HubLeft { reason: "kicked".into() }.encode().len(), 2, "codes are one byte");
    }

    #[test]
    fn looks_are_validated() {
        assert!(Look::default().is_valid());
        assert!(Look { gender: 1, hair: 12, hair_color: 15, body_color: 7 }.is_valid());
        for bad in [
            Look { gender: 2, ..Look::default() },
            Look { hair: 0, ..Look::default() },
            Look { hair: 13, ..Look::default() },
            Look { hair_color: 16, ..Look::default() },
            Look { body_color: 8, ..Look::default() },
        ] {
            assert!(!bad.is_valid(), "{bad:?}");
        }
    }
}

/// Exact bytes shared with the Dart client's tests (`client_test.dart`):
/// both sides must agree on these frames.
#[cfg(test)]
pub(crate) mod golden {
    use super::*;

    pub fn server() -> Vec<(&'static str, Bytes)> {
        let mission = missions::MISSIONS[0].clone();
        vec![
            ("hub_error", ServerMsg::HubError { code: "npc_too_far".into(), message: "far".into() }.encode()),
            (
                "stage_joined",
                ServerMsg::StageJoined {
                    stage_id: 300,
                    mission: "m".into(),
                    tick_rate: 20,
                    entity_id: 7,
                    monsters: vec!["poring".into(), "fabre".into()],
                    light_radius: 40.0,
                }
                .encode(),
            ),
            (
                "map_enter",
                ServerMsg::MapEnter {
                    map: MapInfo {
                        id: "f".into(),
                        name: "Field".into(),
                        kind: LocationKind::Field,
                        width: 400,
                        height: 2,
                        cell_size: 0.5,
                        camera: CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
                        bgm: Some("a.ogg".into()),
                    },
                    map_epoch: 2,
                    x: 1.5,
                    y: 2.25,
                    via_portal: None,
                    players: vec![PlayerInfo {
                        entity_id: 9,
                        name: "Bo".into(),
                        archetype: "novice".into(),
                        x: 3.0,
                        y: 4.0,
                        facing: 6,
                        look: Look { gender: 1, hair: 4, hair_color: 2, body_color: 3 },
                    }],
                }
                .encode(),
            ),
            (
                "lobby_state",
                ServerMsg::LobbyState {
                    lobby: LobbyInfo {
                        lobby_id: 1,
                        code: "AB2CD".into(),
                        mission,
                        public: false,
                        members: vec![LobbyMemberInfo { entity_id: 9, name: "Bo".into(), archetype: "novice".into(), ready: true, leader: true }],
                    },
                }
                .encode(),
            ),
            ("lobby_left", ServerMsg::LobbyLeft { reason: "custom".into() }.encode()),
            ("pong", ServerMsg::Pong { ts: -3 }.encode()),
            (
                "player_look",
                ServerMsg::PlayerLook { entity_id: 300, look: Look { gender: 0, hair: 12, hair_color: 15, body_color: 7 } }
                    .encode(),
            ),
            ("player_vitals", ServerMsg::PlayerVitals { entity_id: 300, hp: 40, max_hp: 122, sp: 7, max_sp: 131, state: 1 }.encode()),
            (
                "combat",
                ServerMsg::Combat {
                    swings: vec![Swing { entity_id: 300, facing: 6 }],
                    hits: vec![
                        HitEvent { target: MONSTER_ENTITY_BASE + 7, amount: 200, flags: hit::CRITICAL | hit::KILL | 0x10 },
                        HitEvent { target: 300, amount: 0, flags: hit::MISS },
                    ],
                }
                .encode(),
            ),
            ("progress", ServerMsg::Progress { level: 31, exp: 87_000, exp_to_next: 97_000, gained: 353 }.encode()),
            (
                "skills",
                ServerMsg::Skills { level: 12, points: 3, skills: vec![("bolt".into(), 4)], hotbar: vec!["bolt".into(), String::new()] }.encode(),
            ),
            ("cast", ServerMsg::Cast { caster: 300, skill: "bolt".into(), x: 10.5, y: 3.25, cast_ms: 540, cooldown_ms: 1000 }.encode()),
        ]
    }

    pub fn client() -> Vec<(&'static str, Bytes)> {
        vec![
            ("hub_join_2", ClientMsg::HubJoin { channel: Some(2) }.encode()),
            ("hub_join_any", ClientMsg::HubJoin { channel: None }.encode()),
            ("mission_select", ClientMsg::MissionSelect { mission: "m1".into(), public: true }.encode()),
            ("lobby_kick", ClientMsg::LobbyKick { entity_id: 300 }.encode()),
            ("lobby_join", ClientMsg::LobbyJoin { code: "XY".into() }.encode()),
            ("resync", ClientMsg::Resync.encode()),
            ("style_set", ClientMsg::StyleSet { look: Look { gender: 1, hair: 5, hair_color: 10, body_color: 6 } }.encode()),
            ("input", ClientMsg::Input { seq: 9, dx: 1.0, dy: 0.0, aim: [-8, 16], slots: 0b10_0001 }.encode()),
            ("skill_learn", ClientMsg::SkillLearn { skill: "bolt".into() }.encode()),
            ("skill_slot", ClientMsg::SkillSlot { slot: 4, skill: "slide".into() }.encode()),
        ]
    }

    pub fn hex(b: &[u8]) -> String {
        b.iter().map(|v| format!("{v:02x}")).collect()
    }

    /// A stage snapshot: one player, then a delta against tick 99 with all
    /// four ops (slots 3 SPAWN, 4 NUDGE, 40 MOVE, 41 REMOVE).
    pub fn stage_snapshot() -> Bytes {
        let mut body = SnapshotBody::with_capacity(1);
        body.push(9, 1.5, 2.0, 2);
        let mut m = BytesMut::new();
        m.put_var(1);
        m.put_var(4);
        m.put_var(3 << 2 | 2);
        m.put_slice(&[5, 1]);
        m.put_u16_le(640);
        m.put_u16_le(1280);
        m.put_slice(&[16, 0xF0]);
        m.put_var(0 << 2);
        m.put_slice(&[0xFE, 3, 0, 0x80]);
        m.put_var(35 << 2 | 1);
        m.put_u16_le(65535);
        m.put_u16_le(0);
        m.put_slice(&[127, 1]);
        m.put_var(3);
        body.stage_frame(4, 100, 12, &m)
    }

    const EXPECTED: &[(&str, &str)] = &[
        ("hub_error", "280603666172"),
        ("stage_joined", "30ac02016d14070206706f72696e6705666162726500002042"),
        (
            "map_enter",
            "210166054669656c64029003020000003f00000034c200002041000020420000a04106612e6f676702006000900000010902426f066e6f76696365c00000010601040203",
        ),
        (
            "lobby_state",
            "310105414232434416706f72696e675f69736c616e645f737572766976616c16506f72696e672049736c616e6420537572766976616c48486f6c64206f757420616761696e73742067726f77696e67207761766573206f6e20506f72696e672049736c616e6420756e74696c2074686520626f73732073686f77732075702e0a7072745f66696c6430380400010902426f066e6f7669636503",
        ),
        ("lobby_left", "32ff06637573746f6d"),
        ("pong", "1105"),
        ("player_look", "29ac02000c0f07"),
        ("player_vitals", "2aac02287a07830101"),
        ("combat", "2b01ac02060287808004c80115ac020002"),
        ("progress", "2c1fd8a705e8f505e102"),
        ("skills", "2d0c030104626f6c74040204626f6c7400"),
        ("cast", "2eac0204626f6c74a002d0009c04e807"),
        ("hub_join_2", "4103"),
        ("hub_join_any", "4100"),
        ("mission_select", "45026d3101"),
        ("lobby_kick", "4aac02"),
        ("lobby_join", "46025859"),
        ("resync", "4c"),
        ("style_set", "4d01050a06"),
        ("input", "0209000000ff7f0000f8102100"),
        ("skill_learn", "4e04626f6c74"),
        ("skill_slot", "500405736c696465"),
        ("stage_snapshot", "030400640000000c000000010009000000600080000201040e05018002000510f000fe0300808d01ffff00007f0103"),
    ];

    /// Change these only together with `client_test.dart`.
    #[test]
    fn frames_match_the_dart_client_goldens() {
        let all: Vec<(&str, Bytes)> =
            server().into_iter().chain(client()).chain([("stage_snapshot", stage_snapshot())]).collect();
        assert_eq!(all.len(), EXPECTED.len());
        for ((name, b), (want_name, want)) in all.iter().zip(EXPECTED) {
            assert_eq!(name, want_name);
            assert_eq!(&hex(b), want, "{name}");
        }
        let s = Snapshot::decode(&stage_snapshot()).unwrap();
        use crate::world::horde::view::DeltaEntry::*;
        assert_eq!(
            s.monsters.unwrap().entries,
            vec![
                Spawn { slot: 3, generation: 5, kind: 1, x: 640, y: 1280, vx: 16, vy: -16 },
                Nudge { slot: 4, dx: -2, dy: 3, vx: 0, vy: -128 },
                Move { slot: 40, x: 65535, y: 0, vx: 127, vy: 1 },
                Remove { slot: 41 },
            ]
        );
    }
}
