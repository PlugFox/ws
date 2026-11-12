//! Hub, lobby and stage wire protocol.
//!
//! Control messages are JSON text frames tagged by `type`. Snapshots are
//! binary frames (later WebTransport datagrams) with this little-endian layout:
//!
//! ```text
//! u8   kind       = 0x01 (HUB_SNAPSHOT)
//! u16  map_epoch  — bumps on every map change of the *recipient*; the client
//!                   drops snapshots whose epoch differs from the last
//!                   `map_enter` (needed once snapshots go over unordered
//!                   datagrams while `map_enter` stays on the reliable stream)
//! u32  tick
//! u32  ack_seq    — last input `seq` the server applied for the recipient
//! u16  count
//! count × {
//!   u32 entity_id  (character id)
//!   u16 x          (cells, unsigned fixed point, 6 fractional bits)
//!   u16 y
//!   u8  facing     (0 = north, clockwise in 45° steps)
//! }
//! ```
//!
//! 13 header bytes + 9 bytes per entity.
//!
//! Stage instances send `STAGE_SNAPSHOT` instead: the same header and player
//! block, then the monsters within the recipient's light radius (plus a
//! small margin):
//!
//! ```text
//! u8   kind       = 0x03 (STAGE_SNAPSHOT)
//! ...  header and player block exactly as HUB_SNAPSHOT
//! u16  monster_count
//! monster_count × {
//!   u16 slot       pool slot
//!   u8  generation bumps whenever the slot is freed: (slot, generation) is
//!                  the monster's identity, a new pair is a new monster
//!   u8  kind       index into `stage_joined.monsters`
//!   u16 x, u16 y   same fixed point as players
//! }
//! ```
//!
//! 8 bytes per monster. Facing and walk/idle are derived client-side from
//! the movement. A monster missing from snapshots for a while has left the
//! view (or died): the client times it out rather than trusting absence
//! from one frame, so the format survives lost or reordered datagrams.

use super::map::{CameraConstraint, LocationKind, MapData};
use super::missions::MissionInfo;
use bytes::{BufMut, Bytes, BytesMut};
use serde::{Deserialize, Serialize};

pub const SNAPSHOT_KIND: u8 = 0x01;
pub const STAGE_SNAPSHOT_KIND: u8 = 0x03;
pub const MONSTER_ENTRY_LEN: usize = 8;
pub const SNAPSHOT_HEADER_LEN: usize = 13;
pub const SNAPSHOT_ENTRY_LEN: usize = 9;
const FIXED_ONE: f32 = 64.0;

#[derive(Clone, Debug, Serialize, PartialEq)]
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
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct PlayerInfo {
    pub entity_id: u32,
    pub name: String,
    pub archetype: String,
    pub x: f32,
    pub y: f32,
    pub facing: u8,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct LobbyMemberInfo {
    pub entity_id: u32,
    pub name: String,
    pub archetype: String,
    pub ready: bool,
    pub leader: bool,
}

/// Full lobby state, re-sent to every member on each change.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct LobbyInfo {
    pub lobby_id: u32,
    /// Join code (`lobby_join`); the only way into a private lobby.
    pub code: String,
    pub mission: MissionInfo,
    pub public: bool,
    pub members: Vec<LobbyMemberInfo>,
}

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct ChannelInfo {
    pub id: u16,
    pub name: String,
    pub population: usize,
    pub capacity: usize,
}

/// Server → client control messages.
#[derive(Clone, Debug, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Pong { ts: i64 },
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

/// Client → server hub messages.
#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Ping,
    /// Join the hub (or switch channel when already in it). `channel: null`
    /// lets the server pick.
    HubJoin {
        #[serde(default)]
        channel: Option<u16>,
    },
    HubLeave,
    HubChannels,
    /// Movement intent: desired direction, length ≤ 1 (longer is normalised).
    /// `seq` must increase; older or repeated values are ignored.
    HubInput { seq: u32, dx: f32, dy: f32 },
    /// Talk to an NPC on the current map; must be within its radius.
    NpcInteract { npc: String },
    /// Pick a mission at a mission board the player is standing at: joins
    /// the oldest public lobby of that mission with room (or opens one), or
    /// opens a private lobby when `public` is false.
    MissionSelect {
        mission: String,
        #[serde(default = "yes")]
        public: bool,
    },
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
}

fn yes() -> bool {
    true
}

/// Binary client → server frame kinds (first byte). JSON control messages
/// may also arrive in binary frames: they start with `{`.
pub const INPUT_KIND: u8 = 0x02;
/// `u8 kind, u32 seq, i16 dx, i16 dy` (direction scaled by 32767).
pub const INPUT_LEN: usize = 9;

/// Decodes a binary movement input frame into `(seq, dx, dy)`.
pub fn decode_input(b: &[u8]) -> Option<(u32, f32, f32)> {
    if b.len() != INPUT_LEN || b[0] != INPUT_KIND {
        return None;
    }
    let seq = u32::from_le_bytes([b[1], b[2], b[3], b[4]]);
    let dx = i16::from_le_bytes([b[5], b[6]]) as f32 / 32767.0;
    let dy = i16::from_le_bytes([b[7], b[8]]) as f32 / 32767.0;
    Some((seq, dx, dy))
}

pub fn encode_pos(v: f32) -> u16 {
    (v * FIXED_ONE).round().clamp(0.0, u16::MAX as f32) as u16
}

pub fn decode_pos(v: u16) -> f32 {
    v as f32 / FIXED_ONE
}

/// Shared body of a snapshot (identical for all recipients on a map).
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

    /// Stage snapshot: this body plus `monster_count` pre-encoded monster
    /// entries (`MONSTER_ENTRY_LEN` bytes each).
    pub fn stage_frame(&self, map_epoch: u16, tick: u32, ack_seq: u32, monsters: &[u8], monster_count: u16) -> Bytes {
        debug_assert_eq!(monsters.len(), monster_count as usize * MONSTER_ENTRY_LEN);
        let mut out = BytesMut::with_capacity(SNAPSHOT_HEADER_LEN + self.buf.len() + 2 + monsters.len());
        self.put_head(&mut out, STAGE_SNAPSHOT_KIND, map_epoch, tick, ack_seq);
        out.put_u16_le(monster_count);
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

/// One monster of a stage snapshot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonsterEntry {
    pub slot: u16,
    pub generation: u8,
    pub kind: u8,
    pub x: f32,
    pub y: f32,
}

/// Decoded snapshot (used by tests and headless bots).
#[derive(Debug, PartialEq)]
pub struct Snapshot {
    pub map_epoch: u16,
    pub tick: u32,
    pub ack_seq: u32,
    pub entities: Vec<(u32, f32, f32, u8)>,
    /// Empty for hub snapshots.
    pub monsters: Vec<MonsterEntry>,
}

impl Snapshot {
    /// Decodes a hub or stage snapshot.
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < SNAPSHOT_HEADER_LEN || (b[0] != SNAPSHOT_KIND && b[0] != STAGE_SNAPSHOT_KIND) {
            return None;
        }
        let u16_at = |i: usize| u16::from_le_bytes([b[i], b[i + 1]]);
        let u32_at = |i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
        let count = u16_at(11) as usize;
        let players_end = SNAPSHOT_HEADER_LEN + count * SNAPSHOT_ENTRY_LEN;
        let mut monsters = Vec::new();
        if b[0] == STAGE_SNAPSHOT_KIND {
            if b.len() < players_end + 2 {
                return None;
            }
            let m = u16_at(players_end) as usize;
            if b.len() != players_end + 2 + m * MONSTER_ENTRY_LEN {
                return None;
            }
            monsters = (0..m)
                .map(|i| {
                    let o = players_end + 2 + i * MONSTER_ENTRY_LEN;
                    MonsterEntry {
                        slot: u16_at(o),
                        generation: b[o + 2],
                        kind: b[o + 3],
                        x: decode_pos(u16_at(o + 4)),
                        y: decode_pos(u16_at(o + 6)),
                    }
                })
                .collect();
        } else if b.len() != players_end {
            return None;
        }
        let entities = (0..count)
            .map(|i| {
                let o = SNAPSHOT_HEADER_LEN + i * SNAPSHOT_ENTRY_LEN;
                (u32_at(o), decode_pos(u16_at(o + 4)), decode_pos(u16_at(o + 6)), b[o + 8])
            })
            .collect();
        Some(Self { map_epoch: u16_at(1), tick: u32_at(3), ack_seq: u32_at(7), entities, monsters })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(Snapshot::decode(&frame[..frame.len() - 1]).is_none());
    }

    #[test]
    fn stage_snapshot_round_trip() {
        let mut body = SnapshotBody::with_capacity(1);
        body.push(7, 12.5, 30.0, 3);
        let mut m = BytesMut::new();
        m.put_u16_le(513);
        m.put_u8(4);
        m.put_u8(2);
        m.put_u16_le(encode_pos(100.25));
        m.put_u16_le(encode_pos(7.5));
        let frame = body.stage_frame(1, 2, 3, &m, 1);
        assert_eq!(frame.len(), SNAPSHOT_HEADER_LEN + SNAPSHOT_ENTRY_LEN + 2 + MONSTER_ENTRY_LEN);
        let s = Snapshot::decode(&frame).unwrap();
        assert_eq!(s.entities, vec![(7, 12.5, 30.0, 3)]);
        assert_eq!(s.monsters, vec![MonsterEntry { slot: 513, generation: 4, kind: 2, x: 100.25, y: 7.5 }]);
        assert!(Snapshot::decode(&frame[..frame.len() - 1]).is_none());
        let empty = body.stage_frame(1, 2, 3, &[], 0);
        assert!(Snapshot::decode(&empty).unwrap().monsters.is_empty());
    }

    #[test]
    fn fixed_point_precision_is_one_64th_cell() {
        for v in [0.0f32, 0.01, 1.0 / 64.0, 133.337, 1023.98] {
            assert!((decode_pos(encode_pos(v)) - v).abs() <= 1.0 / 128.0 + f32::EPSILON, "{v}");
        }
        assert_eq!(encode_pos(-3.0), 0, "negative clamps");
    }

    #[test]
    fn binary_input_decodes() {
        let mut b = vec![INPUT_KIND];
        b.extend_from_slice(&7u32.to_le_bytes());
        b.extend_from_slice(&32767i16.to_le_bytes());
        b.extend_from_slice(&(-16384i16).to_le_bytes());
        let (seq, dx, dy) = decode_input(&b).unwrap();
        assert_eq!(seq, 7);
        assert_eq!(dx, 1.0);
        assert!((dy + 0.5).abs() < 1e-4);
        assert!(decode_input(&b[..8]).is_none());
    }

    #[test]
    fn client_messages_parse() {
        let m: ClientMsg = serde_json::from_str(r#"{"type":"hub_join"}"#).unwrap();
        assert_eq!(m, ClientMsg::HubJoin { channel: None });
        let m: ClientMsg = serde_json::from_str(r#"{"type":"hub_join","channel":2}"#).unwrap();
        assert_eq!(m, ClientMsg::HubJoin { channel: Some(2) });
        let m: ClientMsg = serde_json::from_str(r#"{"type":"hub_input","seq":3,"dx":1,"dy":0}"#).unwrap();
        assert_eq!(m, ClientMsg::HubInput { seq: 3, dx: 1.0, dy: 0.0 });
        let m: ClientMsg = serde_json::from_str(r#"{"type":"mission_select","mission":"m"}"#).unwrap();
        assert_eq!(m, ClientMsg::MissionSelect { mission: "m".into(), public: true }, "public by default");
        let m: ClientMsg = serde_json::from_str(r#"{"type":"lobby_join","code":"AB2CD"}"#).unwrap();
        assert_eq!(m, ClientMsg::LobbyJoin { code: "AB2CD".into() });
    }
}
