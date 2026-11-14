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
//! 0x10 CONNECTED       var user_id, 16 bytes session_id, [str server_version]
//!                      (the version is left out when empty)
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
//! 0x2B COMBAT          var n, n × {var entity_id, u8 facing, var target}
//!                      (swings; target = entity struck, 0 none)
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
//!                      var n, n × {var entity_id, str name, str archetype, u8 flags,
//!                      var level, var offline_secs}
//!                      (the party: the account's current or last character;
//!                      flags: 2 leader, 4 online, 8 on a mission; bit 1 was
//!                      the retired ready flag, always 0)
//! 0x32 LOBBY_LEFT      code reason
//! 0x33 LOBBY_ERROR     code, str message
//! 0x37 EMOTE           var entity_id, u8 emote   (everyone on the entity's map,
//!                      the sender too)
//! 0x38 POSE            var entity_id, u8 pose (0 stand, 1 sit)   (everyone on
//!                      the map; after MAP_ENTER, one POSE 1 per entity already
//!                      sitting there)
//! 0x39 FLAG            var entity_id, u8 flag (0..=9), pos x, pos y
//!                      (a ping marker; everyone on the map, the sender too)
//! 0x3A INVENTORY       var zeny, var n, n × {str item, var count},
//!                      10 × str equipped (slot order of `items::Slot`;
//!                      empty = nothing), str stats (JSON: armour class,
//!                      weapon to-hit and damage with this gear).
//!                      Item strings here (and in every item field of
//!                      SHOP_SELL, EQUIP, ITEM_USE, TRADE, INSPECT) are
//!                      instance keys (`items::key`): the catalogue id,
//!                      then `+<refine>` and `[<card>,…]` when the copy
//!                      has them; one stack per key
//! 0x3B ITEM_DEFS       var n, n × {str id, str name, u8 kind, u8 slot
//!                      (0xFF none), var price, u8 classes, str description}
//!                      (the catalogue; once per instance join)
//! 0x3C SHOP            str npc, str name, var n, n × str item (a merchant
//!                      the recipient talked to; prices in ITEM_DEFS)
//! 0x3D ITEM_USED       str item, var cooldown_ms (the recipient drank it)
//! 0x3E PLAYER_GEAR     var entity_id, str weapon (a player on the map
//!                      changed weapon)
//! 0x8B PLAYER_HEADGEAR var entity_id, var n, n × str item (the headgear a
//!                      player on the map wears, any slot; on arrival for
//!                      everyone there, and after each change)
//! 0x3F LOOT            var entity_id (the monster), pos x, pos y, str item
//!                      (`zeny` = zeny), var count, u8 flags (1 rare)
//!                      (a kill dropped this for the recipient; already in
//!                      the bag at the next INVENTORY)
//! 0x60 COMBO           var count, u8 rank (0 none, 1 D, 2 C, 3 B, 4 A,
//!                      5 S, 6 SS, 7 SSS), var window_ms, var left_ms
//!                      (the recipient's kill combo, on change; count 0 =
//!                      it dropped. 0x60.. continues the server → client
//!                      kinds: 0x10..0x3F are taken)
//! 0x61 REVIVE          var entity_id, u8 progress (0..=100, % of an ally's
//!                      revive, 5 % steps), var left_ms (bleed-out left;
//!                      0 = no longer downed) (everyone on the map: on going
//!                      down, as the progress moves, on getting up or dying)
//! 0x68 BONUS           u8 op: 0 appeared: var id, str bonus, pos x, pos y,
//!                      var life_ms (it bursts after that; the client blinks
//!                      it over its last `BONUS_BLINK_MS`); 1 gone: var id,
//!                      str bonus, var entity_id (the player who took it;
//!                      0 = it expired). Pickup bonuses on a stage
//!                      (`hub::bonus`), to everyone on the map, and the live
//!                      ones to a player joining it
//! 0x6C MISSION_INFO    str mission, str map, var time_limit_secs,
//!                      var level_min, var level_max, var level_every_secs,
//!                      var n, n × {str id, str name, str archetype,
//!                      str element, str size, var unlock_secs,
//!                      var level_min, var level_max, var hp_min, var hp_max,
//!                      var pack_min, var pack_max,
//!                      var m, m × {str item, var rate (1/100 %), var min,
//!                      var max}} (`missions::brief`: what a mission spawns
//!                      and drops, from the mission's roster catalogue; after
//!                      NPC_DIALOG of a mission giver, one per mission, and
//!                      after STAGE_JOINED), then var band_min,
//!                      var band_recommended, var band_max (player levels),
//!                      var light_permille, var e, e × {str id, str name,
//!                      var s, s × var start_secs, var warn_secs,
//!                      var duration_secs} (the map's timed events)
//! 0x92 MISSION_LIST, 0x93 ENVIRONMENT
//!                      the mission ladder's cards and a run's live light
//!                      and map events: see `protocol/mission.rs`
//! 0x6D STAGE_CLOCK     var elapsed_ms (time into the run; on joining a
//!                      stage and every few seconds, for the timeline)
//! 0x6E FOG             u8 op: 0 the overtime poison fog rises (warn),
//!                      1 its state (to a player joining it): var now_ms
//!                      (run clock when sent), var start_ms, var shrink_ms
//!                      (the safe circle closes linearly from start_ms over
//!                      shrink_ms, then holds), pos x, pos y (its centre,
//!                      walkable), pos from_radius, pos to_radius (cells);
//!                      2 the recipient's exposure: u8 secs, u8 death_secs
//!                      (net seconds inside; at death_secs they die). Ops
//!                      0 and 1 to everyone on the stage (`hub::fog`)
//! 0x7B MODIFIER_DEFS, 0x7C MODIFIERS
//!                      mission modifiers (`world::modifiers`): the table,
//!                      a lobby's or stage's selection, the clear bonus:
//!                      see `protocol/modifiers.rs` for their bodies
//! 0x81 RUN_STATS, 0x82 RUN_REWARDS
//!                      a finished run's party statistics and the
//!                      recipient's rewards, after STAGE_END: see
//!                      `protocol/results.rs`
//! 0x8D SHOP_DONE       var spent, var earned (a SHOP_DEAL went through;
//!                      the new INVENTORY comes before it)
//! 0x6A BOSS_CAST, 0x6B BOSS_ZONES, 0x6F BOSS_SHOUT, 0x72 BOSS_BAR
//!                      scripted elite and boss fights (`hub::boss`): see
//!                      `protocol/boss.rs` for their bodies
//! 0xA0 MONSTER_AFFIXES, 0xA1 AFFIX_EVENT, 0xA2 AFFIX_DEFS
//!                      monster affixes / stars (`combat::affixes`,
//!                      `hub::affix`): see `protocol/affixes.rs`
//! 0xA5 MONSTER_HP      var n, n × {var entity_id, u8 hp (1..=255 of its
//!                      max, rounded up; 0 = gone)}: the bars under the
//!                      stage's elites and bosses (`hub::plates`), on
//!                      change and every few seconds, to everyone on the map
//! 0x70 PARTY_INVITE    u8 op: 0 offer (to the invitee): var from (the
//!                      inviter's entity id), str name, str mission (its
//!                      name), var ttl_ms (answer within; then it lapses);
//!                      1 answer (to the inviter): var entity_id (the
//!                      invitee), str name, bool accepted (`lobby.rs`)
//! 0x71 PLAYER_INSPECT  var entity_id, str name, str archetype, var level,
//!                      look, 10 × str equipped (slot order of
//!                      `items::Slot`; empty = nothing): another player's
//!                      worn gear, the answer to INSPECT
//! 0x7A CHAT            u8 op: 0 say: var entity_id, str name, str text
//!                      (a player's line, to the players around them:
//!                      `chat::CHAT_RANGE` cells on a hub map, the whole
//!                      stage on a mission; the speaker too);
//!                      1 announce: str text (a server announcement, to
//!                      everyone connected: the admin API)
//! 0x78 FRIENDS         var n, n × friend (the recipient's whole list: on
//!                      FRIEND list, after every change, when a friend
//!                      moves or goes offline; `friends::Friends::list`)
//! 0x79 FRIEND_NOTE     u8 op, var account, str name (a toast: 0 a friend
//!                      came online, 1 asks to be friends, 2 now friends,
//!                      3 your request went out, 4 off your list;
//!                      `friends::note`), after the FRIENDS it changed
//! 0x73 PLAYER_LINK     var entity_id, bool online (a stage member's socket
//!                      dropped (false: link-dead, still on the map for the
//!                      stage's link grace) or came back (true); to everyone
//!                      on the map, and one false per link-dead player to a
//!                      player arriving there)
//! 0x75 RESUMABLE       var stage_id, str mission (its name), var left_ms
//!                      (the answer to HUB_JOIN when the selected character
//!                      is link-dead in a running stage: the client offers
//!                      "Return to battle" and answers with STAGE_RESUME;
//!                      the stage keeps the fighter for left_ms more)
//! 0x74 TRADE           u8 op (`hub::trade`, player-to-player, in town):
//!                      0 request (to the asked): var entity_id, str name,
//!                      var level (answer with TRADE accept / decline);
//!                      1 state (to both, after every change): var
//!                      entity_id (the partner), str name, offer mine,
//!                      offer theirs, u8 flags (1 I locked, 2 they locked,
//!                      4 I confirmed, 8 they confirmed);
//!                      2 closed: code reason (`done`, `cancelled`,
//!                      `declined`, `expired`, `too_far`, `left`,
//!                      `map_changed`, `mission` or a `trade_*` error),
//!                      str message (may be empty: just close the prompt)
//!                      offer = var zeny, var n, n × {str item, var count}
//!
//! friend  = var account, u8 link (0 friend, 1 asks you, 2 you asked),
//!           str name (character played now, else the last one), bool
//!           online, str archetype, var level, str map (display name; empty
//!           offline or for requests), var channel (0 = mission / offline),
//!           var m, m × var character id (every character of the account:
//!           entity ids of players are character ids)
//! map     = str id, str name, u8 kind (0 hub, 1 indoor, 2 field), var width,
//!           var height, f32 cell_size, camera, opt str bgm
//! camera  = u8 0 + f32 pitch, min_zoom, max_zoom, default_zoom (free)
//!         | u8 1 + f32 yaw, pitch, zoom (fixed)
//! player  = var entity_id, str name, str archetype, pos x, pos y, u8 facing,
//!           look, var level, str weapon (held weapon id)
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
//! 0x85 CAST_PREFS      u8 flags (bit 0: manual aiming, the pointer picks
//!                      the monster and places ground skills), u16 auto
//!                      (auto-cast slots, bit i = slot i: they only fire at
//!                      a target that needs them). Kept by the connection,
//!                      sent again on change
//! 0x40 PING
//! 0x41 HUB_JOIN        var channel + 1 (0 = server picks)
//! 0x42 HUB_LEAVE
//! 0x43 HUB_CHANNELS
//! 0x44 NPC_INTERACT    str npc
//! 0x45 MISSION_SELECT  str mission, bool public
//! 0x46 LOBBY_JOIN      str code
//! 0x47 (retired: LOBBY_READY; the leader starts at any time; ignored)
//! 0x48 LOBBY_START
//! 0x49 LOBBY_LEAVE
//! 0x4A LOBBY_KICK      var entity_id
//! 0x4B STAGE_LEAVE
//! 0x4C RESYNC          the last monster delta did not apply: send a keyframe
//! 0x4D STYLE_SET       look (at a stylist; the gender is kept)
//! 0x4E SKILL_LEARN     str skill (one level up)
//! 0x4F SKILL_RESET     (in the hub only: every point back)
//! 0x50 SKILL_SLOT      u8 slot, str skill (empty = clear)
//! 0x51 EMOTE           u8 emote (0..=127; at most one per 0.5 s)
//! 0x52 POSE            u8 pose (0 stand, 1 sit; sitting in hubs only, any
//!                      movement input stands the player up)
//! 0x53 FLAG            u8 flag (0..=9), pos x, pos y (within 40 cells of the
//!                      player; at most one per second)
//! 0x54 SHOP_BUY        str npc, str item, var count (at a merchant)
//! 0x55 SHOP_SELL       str npc, str item, var count (at a merchant)
//! 0x8C SHOP_DEAL       str npc, var n, n × {str item, var count} (buy),
//!                      var m, m × {str item, var count} (sell) (the shop
//!                      window's cart, at a merchant, all or nothing:
//!                      sales first; at most 20 lines a side, 1..=999
//!                      each; answered by SHOP_DONE or HUB_ERROR)
//! 0x56 EQUIP           str item, u8 slot (0xFF = its own slot; town only)
//! 0x57 UNEQUIP         u8 slot (town only)
//! 0x58 ITEM_USE        str item (a potion, on a mission)
//! 0x59 PARTY           u8 op: 0 invite: var entity_id (a player on the
//!                      map; the leader, or anyone not in a party);
//!                      1 answer: var from (the inviter), bool accept
//! 0x5A INSPECT         var entity_id (a player on the map: their gear)
//! 0x5E CHAT            str text (local chat; `chat::sanitize` cleans it,
//!                      at most 4 lines then one per 1.5 s)
//! 0x5D FRIEND          u8 op: 0 list; 1 add: var entity_id (a player on
//!                      the map); 2 add by name: str character name;
//!                      3 accept: var account; 4 decline: var account;
//!                      5 remove (or withdraw a request): var account.
//!                      Errors come as HUB_ERROR `friend_*`
//! 0x5B TRADE           u8 op: 0 request: var entity_id (a player near
//!                      by); 1 accept; 2 decline; 3 cancel; 4 lock (OK);
//!                      5 confirm (Trade); 6 zeny: var amount; 7 item:
//!                      str item, var count (on the table; 0 takes it
//!                      off). Errors come as HUB_ERROR `trade_*`
//! 0x5C FEEDBACK        str kind (bug | suggestion | balance | other), str
//!                      priority (low | normal | high | critical), str
//!                      title, str body, str client_version, str user_agent
//!                      (at the Scribe or a Kafra; `crate::feedback` has the
//!                      limits). Answered by FEEDBACK_RESULT
//! 0x77 LOBBY_MODIFIERS u8 tier, var mask (the leader; `protocol/modifiers.rs`)
//! 0x63 MISSION_START   str mission, u8 tier, var mask (quick start, `lobby.rs`
//!                      `quick_start`: alone at a Kafra it launches a party
//!                      of one at once; a party leader starts the party's
//!                      run under the lobby rules)
//! 0x5F STAGE_RESUME    bool accept (after RESUMABLE, or right after a
//!                      reconnect from a stage: true re-attaches to the
//!                      link-dead fighter, false forfeits it; either way
//!                      the player ends up somewhere: the stage, else town)
//! ```
//!
//! Town residents (simulated townsfolk in hub channels) look like players:
//! they come in `MAP_ENTER.players`, snapshots, `EMOTE` and `POSE`, with
//! entity ids from `RESIDENT_ENTITY_BASE` (0x700000) up, and never vanish.
//!
//! `map_epoch` bumps on every map change of the recipient; the client drops
//! snapshots whose epoch differs from the last `MAP_ENTER`.

use super::horde::view::MonsterDelta;
use super::map::{CameraConstraint, LocationKind, MapData};
use super::missions::{self, DropBrief, EventBrief, MissionBrief, MissionInfo, MonsterBrief};
use super::trade::ShopOrder;
use super::wire::{PutWire, Reader};
use bytes::{BufMut, Bytes, BytesMut};
use uuid::Uuid;

pub use super::wire::{decode_pos, encode_pos};

mod affixes;
mod boss;
mod mission;
mod modifiers;
mod results;
pub use modifiers::{ModifierInfo, ModifierStep, RewardStep};
pub use affixes::{affix_state, AffixEvent, AffixInfo, MonsterAffix};
pub use boss::{cast_flags, zone_flags, BossBarInfo, BossZone};
pub use mission::{hazard_ability, phase as env_phase};
pub use results::{mark, seat_flags, RunBucket, RunMark, RunSeat};

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
    pub const ZONE_END: u8 = 0x2F;
    pub const STAGE_JOINED: u8 = 0x30;
    pub const LOBBY_STATE: u8 = 0x31;
    pub const LOBBY_LEFT: u8 = 0x32;
    pub const LOBBY_ERROR: u8 = 0x33;
    pub const STAGE_END: u8 = 0x34;
    pub const BOSS: u8 = 0x35;
    pub const BUFFS: u8 = 0x36;
    pub const EMOTE: u8 = 0x37;
    pub const POSE: u8 = 0x38;
    pub const FLAG: u8 = 0x39;
    pub const INVENTORY: u8 = 0x3A;
    pub const ITEM_DEFS: u8 = 0x3B;
    pub const SHOP: u8 = 0x3C;
    pub const ITEM_USED: u8 = 0x3D;
    pub const PLAYER_GEAR: u8 = 0x3E;
    pub const PLAYER_HEADGEAR: u8 = 0x8B;
    pub const PLAYER_MOUNT: u8 = 0x90;
    pub const LOOT: u8 = 0x3F;
    pub const COMBO: u8 = 0x60;
    pub const REVIVE: u8 = 0x61;
    pub const BONUS: u8 = 0x68;
    pub const MISSION_INFO: u8 = 0x6C;
    /// Mission platform (`protocol/mission.rs`).
    pub const MISSION_LIST: u8 = 0x92;
    pub const ENVIRONMENT: u8 = 0x93;
    pub const STAGE_CLOCK: u8 = 0x6D;
    pub const FOG: u8 = 0x6E;
    pub const BOSS_CAST: u8 = 0x6A;
    pub const BOSS_ZONES: u8 = 0x6B;
    pub const BOSS_SHOUT: u8 = 0x6F;
    pub const BOSS_BAR: u8 = 0x72;
    pub const PARTY_INVITE: u8 = 0x70;
    pub const PLAYER_INSPECT: u8 = 0x71;
    pub const CHAT: u8 = 0x7A;
    pub const FRIENDS: u8 = 0x78;
    pub const FRIEND_NOTE: u8 = 0x79;
    pub const TRADE: u8 = 0x74;
    pub const FEEDBACK_RESULT: u8 = 0x62;
    pub const PLAYER_LINK: u8 = 0x73;
    pub const RESUMABLE: u8 = 0x75;
    pub const MODIFIER_DEFS: u8 = 0x7B;
    pub const MODIFIERS: u8 = 0x7C;
    pub const RUN_STATS: u8 = 0x81;
    pub const RUN_REWARDS: u8 = 0x82;
    pub const SHOP_DONE: u8 = 0x8D;
    pub const MONSTER_AFFIXES: u8 = 0xA0;
    pub const AFFIX_EVENT: u8 = 0xA1;
    pub const AFFIX_DEFS: u8 = 0xA2;
    pub const MONSTER_HP: u8 = 0xA5;

    pub const PING: u8 = 0x40;
    pub const HUB_JOIN: u8 = 0x41;
    pub const HUB_LEAVE: u8 = 0x42;
    pub const HUB_CHANNELS_REQ: u8 = 0x43;
    pub const NPC_INTERACT: u8 = 0x44;
    pub const MISSION_SELECT: u8 = 0x45;
    pub const LOBBY_JOIN: u8 = 0x46;
    /// Retired (the ready check is gone): never sent, ignored as unknown.
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
    pub const EMOTE_REQ: u8 = 0x51;
    pub const POSE_REQ: u8 = 0x52;
    pub const MOUNT_REQ: u8 = 0x91;
    pub const FLAG_REQ: u8 = 0x53;
    pub const SHOP_BUY: u8 = 0x54;
    pub const SHOP_SELL: u8 = 0x55;
    pub const SHOP_DEAL: u8 = 0x8C;
    pub const EQUIP: u8 = 0x56;
    pub const UNEQUIP: u8 = 0x57;
    pub const ITEM_USE: u8 = 0x58;
    pub const PARTY_REQ: u8 = 0x59;
    pub const INSPECT_REQ: u8 = 0x5A;
    pub const CHAT_REQ: u8 = 0x5E;
    pub const FRIEND_REQ: u8 = 0x5D;
    pub const TRADE_REQ: u8 = 0x5B;
    pub const FEEDBACK_REQ: u8 = 0x5C;
    pub const STAGE_RESUME: u8 = 0x5F;
    pub const LOBBY_MODIFIERS: u8 = 0x77;
    pub const MISSION_START: u8 = 0x63;
    pub const CAST_PREFS: u8 = 0x85;
}

/// Highest valid emote id.
pub const MAX_EMOTE: u8 = 127;
/// Highest valid flag (ping marker) id.
pub const MAX_FLAG: u8 = 9;
/// [`ServerMsg::PlayerMount`] values.
pub mod mount {
    pub const NONE: u8 = 0;
    /// A Peco Peco (Grand Peco): the Crusader's and Paladin's bird.
    pub const PECO: u8 = 1;
}
/// [`ServerMsg::Pose`] values.
pub mod pose {
    pub const STAND: u8 = 0;
    pub const SIT: u8 = 1;
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
    /// Character level (the client draws the body of its tier).
    pub level: u32,
    /// Held weapon id (`combat::weapon`), for its sprite and sounds.
    pub weapon: String,
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
        b.put_var(self.level.into());
        b.put_str(&self.weapon);
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
            level: r.var32()?,
            weapon: r.str()?,
        })
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct LobbyMemberInfo {
    /// The member's current character (their last one while offline).
    pub entity_id: u32,
    pub name: String,
    pub archetype: String,
    pub leader: bool,
    /// A socket of theirs is in the world.
    pub online: bool,
    /// On a mission (the leader's start leaves them out).
    pub away: bool,
    pub level: u32,
    /// Seconds since they went offline (0 while online).
    pub offline_secs: u32,
}

/// [`LobbyMemberInfo`] flag bits on the wire (bit 1, the retired ready
/// flag, stays unused).
pub mod member_flags {
    pub const LEADER: u8 = 2;
    pub const ONLINE: u8 = 4;
    pub const AWAY: u8 = 8;
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

/// One row of the item catalogue (`ITEM_DEFS`).
#[derive(Clone, Debug, PartialEq)]
pub struct ItemDef {
    pub id: String,
    pub name: String,
    /// `items::Kind::code`: 0 etc, 1 consumable, 2 weapon, 3 armour,
    /// 4 shield, 5 headgear, 6 garment, 7 shoes, 8 accessory.
    pub kind: u8,
    /// Equipment slot index, 0xFF = not worn.
    pub slot: u8,
    pub price: u32,
    /// Classes that may use it (bit `Class as u8`).
    pub classes: u8,
    /// Tooltip text (`**bold**`, `[ok]…[/]`).
    pub desc: String,
}

impl ItemDef {
    fn put(&self, b: &mut BytesMut) {
        b.put_str(&self.id);
        b.put_str(&self.name);
        b.put_u8(self.kind);
        b.put_u8(self.slot);
        b.put_var(self.price.into());
        b.put_u8(self.classes);
        b.put_str(&self.desc);
    }

    fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(Self { id: r.str()?, name: r.str()?, kind: r.u8()?, slot: r.u8()?, price: r.var32()?, classes: r.u8()?, desc: r.str()? })
    }

    /// The whole catalogue, as sent on join.
    pub fn catalogue() -> Vec<ItemDef> {
        super::combat::items::catalogue()
            .iter()
            .map(|e| ItemDef {
                id: e.id.into(),
                name: e.name.into(),
                kind: e.kind.code(),
                slot: e.slot().map_or(0xFF, |s| s as u8),
                price: e.price,
                classes: e.classes,
                desc: e.desc.clone(),
            })
            .collect()
    }
}

/// Entity id of a monster in combat events: above every character id and
/// exact in a float32 (the client's engine id for the slot).
pub const MONSTER_ENTITY_BASE: u32 = 0x80_0000;
/// Entity id of the first town resident (simulated townsfolk): above every
/// character id, below the monsters.
pub const RESIDENT_ENTITY_BASE: u32 = 0x70_0000;

/// An attack animation starts (player or monster) facing `facing`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Swing {
    pub entity_id: u32,
    pub facing: u8,
    /// The entity struck (its arrow or bolt flies there); 0 = none.
    pub target: u32,
}

/// Bits of [`HitEvent::flags`]; the high nibble is the damage type.
pub mod hit {
    pub const CRITICAL: u8 = 1;
    pub const MISS: u8 = 2;
    pub const KILL: u8 = 4;
    pub const HEAL: u8 = 8;
    /// Blocked by a ward, a shield or a ranged ward (MISS and HEAL never
    /// come together otherwise).
    pub const GUARD: u8 = MISS | HEAL;
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

fn put_brief(b: &mut BytesMut, m: &MissionBrief) {
    b.put_str(&m.mission);
    b.put_str(&m.map);
    b.put_var(m.time_limit_secs.into());
    b.put_var(m.levels.0.into());
    b.put_var(m.levels.1.into());
    b.put_var(m.level_every_secs.into());
    put_list(b, &m.monsters, |b, k| {
        b.put_str(&k.id);
        b.put_str(&k.name);
        b.put_str(&k.archetype);
        b.put_str(&k.element);
        b.put_str(&k.size);
        b.put_var(k.unlock_secs.into());
        b.put_var(k.levels.0.into());
        b.put_var(k.levels.1.into());
        b.put_var(k.hp.0.into());
        b.put_var(k.hp.1.into());
        b.put_var(k.pack.0.into());
        b.put_var(k.pack.1.into());
        put_list(b, &k.drops, |b, d| {
            b.put_str(&d.item);
            b.put_var(d.rate.into());
            b.put_var(d.min.into());
            b.put_var(d.max.into());
        });
    });
    // Mission platform (trailing): the player band, the light, the events.
    for v in [m.band.0, m.band.1, m.band.2, m.light_permille] {
        b.put_var(v.into());
    }
    put_list(b, &m.events, |b, e| {
        b.put_str(&e.id);
        b.put_str(&e.name);
        put_list(b, &e.starts, |b, s| b.put_var((*s).into()));
        b.put_var(e.warn_secs.into());
        b.put_var(e.duration_secs.into());
    });
}

fn read_brief(r: &mut Reader<'_>) -> Option<MissionBrief> {
    Some(MissionBrief {
        mission: r.str()?,
        map: r.str()?,
        time_limit_secs: r.var32()?,
        levels: (r.var32()?, r.var32()?),
        level_every_secs: r.var32()?,
        monsters: read_list(r, |r| {
            Some(MonsterBrief {
                id: r.str()?,
                name: r.str()?,
                archetype: r.str()?,
                element: r.str()?,
                size: r.str()?,
                unlock_secs: r.var32()?,
                levels: (r.var32()?, r.var32()?),
                hp: (r.var32()?, r.var32()?),
                pack: (r.var32()?, r.var32()?),
                drops: read_list(r, |r| Some(DropBrief { item: r.str()?, rate: r.var32()?, min: r.var32()?, max: r.var32()? }))?,
            })
        })?,
        band: (r.var32()?, r.var32()?, r.var32()?),
        light_permille: r.var32()?,
        events: read_list(r, |r| {
            Some(EventBrief {
                id: r.str()?,
                name: r.str()?,
                starts: read_list(r, |r| r.var32())?,
                warn_secs: r.var32()?,
                duration_secs: r.var32()?,
            })
        })?,
    })
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

/// A skill area on the ground (cells).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Telegraph {
    /// Lasting ground (ended early by `zone_end`); 0 = a one-off strike.
    pub id: u32,
    /// 0 circle (`a` radius), 1 box (`a` along `dir` × `b` across,
    /// centred on `x, y`), 2 cone (apex `x, y`, `a` radius, `b` degrees).
    pub shape: u8,
    /// How it looks ([`look`]).
    pub look: u8,
    pub x: f32,
    pub y: f32,
    /// Direction in 1/256 turns, counter-clockwise from +x.
    pub dir: u8,
    pub a: f32,
    pub b: f32,
    /// How long it acts after the wind-up (hits, ground).
    pub span_ms: u32,
    /// Entity it moves with (0 = none): the caster's aura, a homing hit.
    pub follow: u32,
}

/// [`Telegraph::look`] values.
pub mod look {
    pub const DAMAGE: u8 = 0;
    pub const HEAL: u8 = 1;
    pub const FIRE: u8 = 2;
    pub const SWAMP: u8 = 3;
    pub const WARD: u8 = 4;
    pub const COLD: u8 = 5;
    pub const LIGHTNING: u8 = 6;
    pub const HOLY: u8 = 7;
    pub const TRAP: u8 = 8;
    pub const TAUNT: u8 = 9;
    pub const BARRIER: u8 = 10;
    pub const BUFF: u8 = 11;
    pub const EARTH: u8 = 12;
    pub const CURSE: u8 = 13;
    pub const POISON: u8 = 14;
    pub const DARK: u8 = 15;
    /// Themed ground of locations (mission platform hazards, boss beats);
    /// clients draw their own decal colour, default vfx.
    pub const SAND: u8 = 16;
    pub const SLUDGE: u8 = 17;
    /// Moonlight petals.
    pub const PETALS: u8 = 18;
    /// A talisman's glyph.
    pub const GLYPH: u8 = 19;
    /// A golden glint.
    pub const GOLD: u8 = 20;
    /// Sludge ripples rolling out (something moving under the sewage).
    pub const RIPPLE: u8 = 21;
}

/// Server → client control messages.
#[derive(Clone, Debug, PartialEq)]
pub enum ServerMsg {
    /// First frame on a socket; `server_version` as `crate::version`
    /// (empty = not sent).
    Connected { user_id: u32, session_id: Uuid, server_version: String },
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
    /// (e.g. a combat field, which is entered through a Kafra employee or a
    /// mission board).
    PortalDenied {
        portal: String,
        target_map: String,
        reason: String,
    },
    HubChannels { channels: Vec<ChannelInfo> },
    /// Answer to `npc_interact`. `missions` is filled for mission givers
    /// (kinds `kafra` and `mission_board`).
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
    /// `status`: condition bits (as on monsters); `light`: extra light
    /// radius from buffs, cells.
    PlayerVitals { entity_id: u32, hp: u32, max_hp: u32, sp: u32, max_sp: u32, state: u8, status: u32, temp_hp: u32, light: u32, level: u32 },
    /// Attacks and numbers of one tick on the recipient's map.
    Combat { swings: Vec<Swing>, hits: Vec<HitEvent> },
    /// The recipient's level and experience into it; `gained` since the
    /// last progress message (kills, or negative-free penalties as 0).
    Progress { level: u32, exp: u64, exp_to_next: u64, gained: u64 },
    /// The recipient's learned skills, unspent points and hotbar (on join
    /// and after every change).
    Skills { level: u32, points: u32, skills: Vec<(String, u8)>, hotbar: Vec<String> },
    /// A player starts a skill aimed at `x, y`: wind-up `cast_ms`, then
    /// `cooldown_ms`; `areas` are where its effects will land, as the server
    /// resolved them. An empty `skill` cancels the caster's wind-up.
    /// `level`: the skill level (how many hits a storm shows; 1 for monsters).
    Cast { caster: u32, skill: String, x: f32, y: f32, cast_ms: u32, cooldown_ms: u32, areas: Vec<Telegraph>, level: u8 },
    /// Ground `id` (a ward, a trap) ended before its time.
    ZoneEnd { id: u32 },
    /// The mission is over for the recipient's map: the result screen, and
    /// everyone leaves through the exit after `leave_ms`.
    StageEnd { victory: bool, secs: u32, kills: u32, exp: u64, leave_ms: u32 },
    /// The boss's bar (`hp` 0: it is gone).
    Boss { entity_id: u32, name: String, hp: u32, max_hp: u32 },
    /// The recipient's running buffs: skill id and time left.
    Buffs { buffs: Vec<(String, u32)> },
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
    /// The recipient is no longer in a party: `left`, `kicked`, `expired`
    /// (offline too long). A party outlives its missions, so `launched`
    /// is no longer sent.
    LobbyLeft { reason: String },
    /// Lobby errors: `already_in_lobby`, `not_in_lobby`, `no_such_lobby`,
    /// `lobby_full`, `not_leader`, `stage_full`, `mission_unavailable`,
    /// `party_mission`, `not_in_hub` (`not_ready` is retired).
    LobbyError { code: String, message: String },
    /// An entity on the recipient's map (possibly the recipient) plays an
    /// emote.
    Emote { entity_id: u32, emote: u8 },
    /// An entity on the recipient's map sat down (1) or stood up (0).
    Pose { entity_id: u32, pose: u8 },
    /// A player on the recipient's map rides `mount` ([`mount`]; `NONE`:
    /// got off), `speed` percent faster in town (and from the mount in a
    /// stage: what the rider's own client predicts with). Sent on arrival
    /// for everyone riding there, the recipient included, then on changes.
    PlayerMount { entity_id: u32, mount: u8, speed: u8 },
    /// A player on the recipient's map pinged a spot.
    Flag { entity_id: u32, flag: u8, x: f32, y: f32 },
    /// The recipient's belongings: zeny, bag stacks and the ten equipment
    /// slots (item ids, empty = nothing). On join and after every change.
    Inventory { zeny: u64, items: Vec<(String, u32)>, equip: Vec<String>, stats: String },
    /// The item catalogue (on join).
    ItemDefs { items: Vec<ItemDef> },
    /// A merchant's goods (answer to `npc_interact` at a `shop` NPC).
    Shop { npc: String, name: String, items: Vec<String> },
    /// The recipient's merchant order went through.
    ShopDone { spent: u64, earned: u64 },
    /// The recipient drank `item`; the next potion waits `cooldown_ms`.
    ItemUsed { item: String, cooldown_ms: u32 },
    /// A player on the recipient's map now holds `weapon`.
    PlayerGear { entity_id: u32, weapon: String },
    /// A player on the recipient's map wears `items` on the head (item
    /// ids, '' slots left out).
    PlayerHeadgear { entity_id: u32, items: Vec<String> },
    /// The monster `entity_id` (at `x`, `y`) dropped `count` of `item`
    /// (`zeny`: zeny) for the recipient; `rare` drops get a toast.
    Loot { entity_id: u32, x: f32, y: f32, item: String, count: u32, rare: bool },
    /// The recipient's kill combo (`combat::combo`): `count` kills in a
    /// row at style `rank`; it drops `left_ms` from now unless another kill
    /// refills it to `window_ms`. Count 0: the combo dropped.
    Combo { count: u32, rank: u8, window_ms: u32, left_ms: u32 },
    /// A downed player on the map: an ally's revive is `progress` percent
    /// done, and they bleed out in `left_ms` (0: no longer downed).
    Revive { entity_id: u32, progress: u8, left_ms: u32 },
    /// A pickup bonus `bonus` (its id, e.g. `storm_gust`) appeared at
    /// `(x, y)`; it bursts in `life_ms` unless someone walks over it.
    Bonus { id: u32, bonus: String, x: f32, y: f32, life_ms: u32 },
    /// Pickup bonus `id` is gone: `entity_id` took it (0 = it expired).
    BonusGone { id: u32, bonus: String, entity_id: u32 },
    /// What a mission spawns and drops (the mission window, the timeline).
    MissionInfo { brief: MissionBrief },
    /// Every mission of the ladder as a card (`missions::card`).
    MissionList { missions: Vec<missions::MissionCard> },
    /// The run's base light radius changed (cells; `hub::env`).
    EnvLight { radius: f32, fade_ms: u32 },
    /// A map event warns, begins or ends ([`env_phase`]).
    EnvEvent { phase: u8, id: String, name: String, text: String, start_ms: u32, duration_ms: u32 },
    /// Shake the camera (`intensity` 1..=255 over `duration_ms`).
    EnvShake { intensity: u8, duration_ms: u32 },
    /// Seconds into the run, in ms (the recipient's stage).
    StageClock { elapsed_ms: u32 },
    /// The overtime poison fog (`hub::fog`): its safe circle around
    /// `(x, y)` closes linearly from `from_radius` to `to_radius` (cells)
    /// over `shrink_ms` from run time `start_ms`, then holds; `now_ms` is
    /// the run clock as it was sent. `begun`: it rises right now (warn).
    Fog { begun: bool, now_ms: u32, start_ms: u32, shrink_ms: u32, x: f32, y: f32, from_radius: f32, to_radius: f32 },
    /// The recipient's net seconds inside the fog; at `death_secs` it kills.
    FogExposure { secs: u8, death_secs: u8 },
    /// The recipient's friends and requests (`friends`).
    Friends { friends: Vec<super::friends::FriendInfo> },
    /// A friends toast (`friends::note` ops).
    FriendNote { op: u8, account: u32, name: String },
    /// The player `entity_id` asks the recipient to trade.
    TradeRequest { entity_id: u32, name: String, level: u32 },
    /// The recipient's open trade with `entity_id`: both offers and the
    /// lock / confirm flags (1 mine locked, 2 theirs, 4 mine confirmed,
    /// 8 theirs).
    TradeState { entity_id: u32, name: String, mine: super::trade::Offer, theirs: super::trade::Offer, flags: u8 },
    /// The recipient's trade (or request) ended; `message` may be empty.
    TradeClosed { reason: String, message: String },
    /// `from` (entity id, `name`) invites the recipient into their party
    /// for `mission`; the offer lapses after `ttl_ms`.
    PartyInvite { from: u32, name: String, mission: String, ttl_ms: u32 },
    /// The recipient's invitation to `entity_id` (`name`) was answered.
    PartyAnswer { entity_id: u32, name: String, accepted: bool },
    /// Another player's worn gear (the answer to `inspect`): ten slot
    /// item ids in `items::Slot` order, empty = nothing.
    PlayerInspect { entity_id: u32, name: String, archetype: String, level: u32, look: Look, equip: Vec<String> },
    /// A player's chat line (`entity_id` = their character, `name` its
    /// name): over their head and in the chat log.
    Chat { entity_id: u32, name: String, text: String },
    /// A server announcement (banner and chat log).
    Announce { text: String },
    /// A scripted elite or boss starts `ability` (`hub::boss`): the cast
    /// bar shows `name` for `cast_ms`; an empty `ability` ends its cast
    /// early ([`cast_flags`]).
    BossCast { caster: u32, ability: String, name: String, cast_ms: u32, flags: u8 },
    /// The danger (and safe) zones of a boss ability, each resolving after
    /// its own delay: rings, crosses, sweeps, charges, pools.
    BossZones { caster: u32, ability: String, zones: Vec<BossZone> },
    /// A boss's yell (`style` 0 elite, 1 boss): a bubble and a log line.
    BossShout { entity_id: u32, name: String, text: String, style: u8 },
    /// The encounter bar of an elite or boss (rank 0: it is gone).
    BossBar(BossBarInfo),
    /// The answer to `FEEDBACK`: stored as report `id` (`code` empty), or
    /// refused (`id` 0, `code` as `feedback::FeedbackError::code`);
    /// `message` is for the player.
    FeedbackResult { id: u32, code: String, message: String },
    /// A player on the recipient's stage lost its connection (`online`
    /// false: link-dead, kept for the link grace) or got it back.
    PlayerLink { entity_id: u32, online: bool },
    /// HP bytes of elites and bosses (`(entity_id, 1..=255)`, 0 = gone):
    /// the bars under them.
    MonsterHp { bars: Vec<(u32, u8)> },
    /// The selected character is link-dead in stage `stage_id` (mission
    /// display name `mission`) for `left_ms` more: answer `stage_resume`.
    Resumable { stage_id: u32, mission: String, left_ms: u32 },
    /// The mission modifier table (`modifiers::defs_msg`).
    ModifierDefs { defs: Vec<ModifierInfo>, rewards: Vec<RewardStep> },
    /// A lobby's or stage's modifiers (`scope`: `modifiers::scope`) with
    /// the clear bonus they pay and each kill's loot share.
    Modifiers { scope: u8, tier: u8, mask: u32, risk: u32, zeny_pct: u32, exp_pct: u32, chest: u32, loot_pct: u32 },
    /// The clear bonus of a won run with modifiers (already in the bag).
    ModifierReward { tier: u8, mask: u32, risk: u32, zeny: u64, exp: u64, items: Vec<(String, u32)> },
    /// The party's statistics of a finished run (`hub/stats.rs`).
    RunStats { victory: bool, secs: u32, bucket_secs: u32, seats: Vec<RunSeat>, marks: Vec<RunMark> },
    /// The recipient's own take of a finished run: `seat` in `RunStats`.
    RunRewards { seat: u8, exp: u64, zeny: u64, clear_zeny: u64, mod_exp: u64, mod_zeny: u64, items: Vec<(String, u32)> },
    /// Affixed monsters by `(slot, generation)` (upserts; `protocol/affixes.rs`).
    MonsterAffixes { monsters: Vec<MonsterAffix> },
    /// One-off moments of affix tricks (blinks, summons, splits, frenzy).
    AffixEvents { events: Vec<AffixEvent> },
    /// The affix table (`combat::affixes::defs_msg`) and the reward per star.
    AffixDefs { defs: Vec<AffixInfo>, ranks: Vec<String>, exp_pct: u32, drop_pct: u32, extra_pct: u32, hp_pct: u32 },
}

/// The client blinks a pickup bonus over this last stretch of its life.
pub const BONUS_BLINK_MS: u32 = 5000;

impl ServerMsg {
    pub fn encode(&self) -> Bytes {
        let mut b = BytesMut::with_capacity(64);
        match self {
            m @ (ServerMsg::BossCast { .. } | ServerMsg::BossZones { .. } | ServerMsg::BossShout { .. } | ServerMsg::BossBar(_)) => {
                boss::encode(m, &mut b);
            }
            m @ (ServerMsg::ModifierDefs { .. } | ServerMsg::Modifiers { .. } | ServerMsg::ModifierReward { .. }) => {
                modifiers::encode(m, &mut b);
            }
            m @ (ServerMsg::RunStats { .. } | ServerMsg::RunRewards { .. }) => results::encode(m, &mut b),
            m @ (ServerMsg::MonsterAffixes { .. } | ServerMsg::AffixEvents { .. } | ServerMsg::AffixDefs { .. }) => affixes::encode(m, &mut b),
            m @ (ServerMsg::MissionList { .. } | ServerMsg::EnvLight { .. } | ServerMsg::EnvEvent { .. } | ServerMsg::EnvShake { .. }) => mission::encode(m, &mut b),
            ServerMsg::Connected { user_id, session_id, server_version } => {
                b.put_u8(kind::CONNECTED);
                b.put_var((*user_id).into());
                b.put_slice(session_id.as_bytes());
                if !server_version.is_empty() {
                    b.put_str(server_version);
                }
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
            ServerMsg::PlayerVitals { entity_id, hp, max_hp, sp, max_sp, state, status, temp_hp, light, level } => {
                b.put_u8(kind::PLAYER_VITALS);
                for v in [*entity_id, *hp, *max_hp, *sp, *max_sp] {
                    b.put_var(v.into());
                }
                b.put_u8(*state);
                for v in [*status, *temp_hp, *light, *level] {
                    b.put_var(v.into());
                }
            }
            ServerMsg::Combat { swings, hits } => {
                b.put_u8(kind::COMBAT);
                put_list(&mut b, swings, |b, s| {
                    b.put_var(s.entity_id.into());
                    b.put_u8(s.facing);
                    b.put_var(s.target.into());
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
            ServerMsg::Cast { caster, skill, x, y, cast_ms, cooldown_ms, areas, level } => {
                b.put_u8(kind::CAST);
                b.put_var((*caster).into());
                b.put_str(skill);
                b.put_pos(*x);
                b.put_pos(*y);
                b.put_var((*cast_ms).into());
                b.put_var((*cooldown_ms).into());
                put_list(&mut b, areas, |b, t| {
                    b.put_var(t.id.into());
                    b.put_u8(t.shape);
                    b.put_u8(t.look);
                    b.put_pos(t.x);
                    b.put_pos(t.y);
                    b.put_u8(t.dir);
                    b.put_pos(t.a);
                    b.put_pos(t.b);
                    b.put_var(t.span_ms.into());
                    b.put_var(t.follow.into());
                });
                b.put_u8(*level);
            }
            ServerMsg::StageEnd { victory, secs, kills, exp, leave_ms } => {
                b.put_u8(kind::STAGE_END);
                b.put_bool(*victory);
                b.put_var((*secs).into());
                b.put_var((*kills).into());
                b.put_var(*exp);
                b.put_var((*leave_ms).into());
            }
            ServerMsg::Boss { entity_id, name, hp, max_hp } => {
                b.put_u8(kind::BOSS);
                b.put_var((*entity_id).into());
                b.put_str(name);
                b.put_var((*hp).into());
                b.put_var((*max_hp).into());
            }
            ServerMsg::Buffs { buffs } => {
                b.put_u8(kind::BUFFS);
                put_list(&mut b, buffs, |b, (skill, ms)| {
                    b.put_str(skill);
                    b.put_var((*ms).into());
                });
            }
            ServerMsg::ZoneEnd { id } => {
                b.put_u8(kind::ZONE_END);
                b.put_var((*id).into());
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
                    let flags = [(m.leader, member_flags::LEADER), (m.online, member_flags::ONLINE), (m.away, member_flags::AWAY)];
                    b.put_u8(flags.iter().filter(|f| f.0).fold(0, |a, f| a | f.1));
                    b.put_var(m.level.into());
                    b.put_var(m.offline_secs.into());
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
            ServerMsg::Emote { entity_id, emote } => {
                b.put_u8(kind::EMOTE);
                b.put_var((*entity_id).into());
                b.put_u8(*emote);
            }
            ServerMsg::Pose { entity_id, pose } => {
                b.put_u8(kind::POSE);
                b.put_var((*entity_id).into());
                b.put_u8(*pose);
            }
            ServerMsg::PlayerMount { entity_id, mount, speed } => {
                b.put_u8(kind::PLAYER_MOUNT);
                b.put_var((*entity_id).into());
                b.put_u8(*mount);
                b.put_u8(*speed);
            }
            ServerMsg::PlayerLink { entity_id, online } => {
                b.put_u8(kind::PLAYER_LINK);
                b.put_var((*entity_id).into());
                b.put_bool(*online);
            }
            ServerMsg::MonsterHp { bars } => {
                b.put_u8(kind::MONSTER_HP);
                put_list(&mut b, bars, |b, &(id, hp)| {
                    b.put_var(id.into());
                    b.put_u8(hp);
                });
            }
            ServerMsg::Resumable { stage_id, mission, left_ms } => {
                b.put_u8(kind::RESUMABLE);
                b.put_var((*stage_id).into());
                b.put_str(mission);
                b.put_var((*left_ms).into());
            }
            ServerMsg::Flag { entity_id, flag, x, y } => {
                b.put_u8(kind::FLAG);
                b.put_var((*entity_id).into());
                b.put_u8(*flag);
                b.put_pos(*x);
                b.put_pos(*y);
            }
            ServerMsg::Inventory { zeny, items, equip, stats } => {
                b.put_u8(kind::INVENTORY);
                b.put_var(*zeny);
                put_list(&mut b, items, |b, (id, n)| {
                    b.put_str(id);
                    b.put_var((*n).into());
                });
                for i in 0..super::combat::items::EQUIP_SLOTS {
                    b.put_str(equip.get(i).map_or("", String::as_str));
                }
                b.put_str(stats);
            }
            ServerMsg::ItemDefs { items } => {
                b.put_u8(kind::ITEM_DEFS);
                put_list(&mut b, items, |b, d| d.put(b));
            }
            ServerMsg::Shop { npc, name, items } => {
                b.put_u8(kind::SHOP);
                b.put_str(npc);
                b.put_str(name);
                put_list(&mut b, items, |b, i| b.put_str(i));
            }
            ServerMsg::ShopDone { spent, earned } => {
                b.put_u8(kind::SHOP_DONE);
                b.put_var(*spent);
                b.put_var(*earned);
            }
            ServerMsg::ItemUsed { item, cooldown_ms } => {
                b.put_u8(kind::ITEM_USED);
                b.put_str(item);
                b.put_var((*cooldown_ms).into());
            }
            ServerMsg::PlayerGear { entity_id, weapon } => {
                b.put_u8(kind::PLAYER_GEAR);
                b.put_var((*entity_id).into());
                b.put_str(weapon);
            }
            ServerMsg::PlayerHeadgear { entity_id, items } => {
                b.put_u8(kind::PLAYER_HEADGEAR);
                b.put_var((*entity_id).into());
                put_list(&mut b, items, |b, i| b.put_str(i));
            }
            ServerMsg::PartyInvite { from, name, mission, ttl_ms } => {
                b.put_u8(kind::PARTY_INVITE);
                b.put_u8(0);
                b.put_var((*from).into());
                b.put_str(name);
                b.put_str(mission);
                b.put_var((*ttl_ms).into());
            }
            ServerMsg::PartyAnswer { entity_id, name, accepted } => {
                b.put_u8(kind::PARTY_INVITE);
                b.put_u8(1);
                b.put_var((*entity_id).into());
                b.put_str(name);
                b.put_u8(u8::from(*accepted));
            }
            ServerMsg::PlayerInspect { entity_id, name, archetype, level, look, equip } => {
                b.put_u8(kind::PLAYER_INSPECT);
                b.put_var((*entity_id).into());
                b.put_str(name);
                b.put_str(archetype);
                b.put_var((*level).into());
                look.put(&mut b);
                for i in 0..super::combat::items::EQUIP_SLOTS {
                    b.put_str(equip.get(i).map_or("", String::as_str));
                }
            }
            ServerMsg::Loot { entity_id, x, y, item, count, rare } => {
                b.put_u8(kind::LOOT);
                b.put_var((*entity_id).into());
                b.put_pos(*x);
                b.put_pos(*y);
                b.put_str(item);
                b.put_var((*count).into());
                b.put_u8(*rare as u8);
            }
            ServerMsg::Combo { count, rank, window_ms, left_ms } => {
                b.put_u8(kind::COMBO);
                b.put_var((*count).into());
                b.put_u8(*rank);
                b.put_var((*window_ms).into());
                b.put_var((*left_ms).into());
            }
            ServerMsg::Revive { entity_id, progress, left_ms } => {
                b.put_u8(kind::REVIVE);
                b.put_var((*entity_id).into());
                b.put_u8(*progress);
                b.put_var((*left_ms).into());
            }
            ServerMsg::Bonus { id, bonus, x, y, life_ms } => {
                b.put_u8(kind::BONUS);
                b.put_u8(0);
                b.put_var((*id).into());
                b.put_str(bonus);
                b.put_pos(*x);
                b.put_pos(*y);
                b.put_var((*life_ms).into());
            }
            ServerMsg::BonusGone { id, bonus, entity_id } => {
                b.put_u8(kind::BONUS);
                b.put_u8(1);
                b.put_var((*id).into());
                b.put_str(bonus);
                b.put_var((*entity_id).into());
            }
            ServerMsg::MissionInfo { brief } => {
                b.put_u8(kind::MISSION_INFO);
                put_brief(&mut b, brief);
            }
            ServerMsg::StageClock { elapsed_ms } => {
                b.put_u8(kind::STAGE_CLOCK);
                b.put_var((*elapsed_ms).into());
            }
            ServerMsg::Fog { begun, now_ms, start_ms, shrink_ms, x, y, from_radius, to_radius } => {
                b.put_u8(kind::FOG);
                b.put_u8(if *begun { 0 } else { 1 });
                b.put_var((*now_ms).into());
                b.put_var((*start_ms).into());
                b.put_var((*shrink_ms).into());
                b.put_pos(*x);
                b.put_pos(*y);
                b.put_pos(*from_radius);
                b.put_pos(*to_radius);
            }
            ServerMsg::FogExposure { secs, death_secs } => {
                b.put_u8(kind::FOG);
                b.put_u8(2);
                b.put_u8(*secs);
                b.put_u8(*death_secs);
            }
            ServerMsg::Friends { friends } => {
                b.put_u8(kind::FRIENDS);
                b.put_var(friends.len() as u64);
                for f in friends {
                    f.put(&mut b);
                }
            }
            ServerMsg::FriendNote { op, account, name } => {
                b.put_u8(kind::FRIEND_NOTE);
                b.put_u8(*op);
                b.put_var((*account).into());
                b.put_str(name);
            }
            ServerMsg::TradeRequest { entity_id, name, level } => {
                b.put_u8(kind::TRADE);
                b.put_u8(0);
                b.put_var((*entity_id).into());
                b.put_str(name);
                b.put_var((*level).into());
            }
            ServerMsg::TradeState { entity_id, name, mine, theirs, flags } => {
                b.put_u8(kind::TRADE);
                b.put_u8(1);
                b.put_var((*entity_id).into());
                b.put_str(name);
                mine.put(&mut b);
                theirs.put(&mut b);
                b.put_u8(*flags);
            }
            ServerMsg::TradeClosed { reason, message } => {
                b.put_u8(kind::TRADE);
                b.put_u8(2);
                b.put_code(reason);
                b.put_str(message);
            }
            ServerMsg::Chat { entity_id, name, text } => {
                b.put_u8(kind::CHAT);
                b.put_u8(0);
                b.put_var((*entity_id).into());
                b.put_str(name);
                b.put_str(text);
            }
            ServerMsg::Announce { text } => {
                b.put_u8(kind::CHAT);
                b.put_u8(1);
                b.put_str(text);
            }
            ServerMsg::FeedbackResult { id, code, message } => {
                b.put_u8(kind::FEEDBACK_RESULT);
                b.put_var((*id).into());
                b.put_str(code);
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
            kind::BOSS_CAST | kind::BOSS_ZONES | kind::BOSS_SHOUT | kind::BOSS_BAR => boss::decode(k, &mut r)?,
            kind::MODIFIER_DEFS | kind::MODIFIERS => modifiers::decode(k, &mut r)?,
            kind::RUN_STATS | kind::RUN_REWARDS => results::decode(k, &mut r)?,
            kind::MONSTER_AFFIXES | kind::AFFIX_EVENT | kind::AFFIX_DEFS => affixes::decode(k, &mut r)?,
            kind::MISSION_LIST | kind::ENVIRONMENT => mission::decode(k, &mut r)?,
            kind::CONNECTED => ServerMsg::Connected {
                user_id: r.var32()?,
                session_id: Uuid::from_slice(r.bytes(16)?).ok()?,
                server_version: if r.is_empty() { String::new() } else { r.str()? },
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
                status: r.var32()?,
                temp_hp: r.var32()?,
                light: r.var32()?,
                level: r.var32()?,
            },
            kind::COMBAT => ServerMsg::Combat {
                swings: read_list(&mut r, |r| Some(Swing { entity_id: r.var32()?, facing: r.u8()?, target: r.var32()? }))?,
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
                areas: read_list(&mut r, |r| {
                    Some(Telegraph {
                        id: r.var32()?,
                        shape: r.u8()?,
                        look: r.u8()?,
                        x: r.pos()?,
                        y: r.pos()?,
                        dir: r.u8()?,
                        a: r.pos()?,
                        b: r.pos()?,
                        span_ms: r.var32()?,
                        follow: r.var32()?,
                    })
                })?,
                level: r.u8()?,
            },
            kind::ZONE_END => ServerMsg::ZoneEnd { id: r.var32()? },
            kind::STAGE_END => ServerMsg::StageEnd {
                victory: r.bool()?,
                secs: r.var32()?,
                kills: r.var32()?,
                exp: r.var()?,
                leave_ms: r.var32()?,
            },
            kind::BOSS => ServerMsg::Boss { entity_id: r.var32()?, name: r.str()?, hp: r.var32()?, max_hp: r.var32()? },
            kind::BUFFS => ServerMsg::Buffs { buffs: read_list(&mut r, |r| Some((r.str()?, r.var32()?)))? },
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
                        Some(LobbyMemberInfo {
                            entity_id,
                            name,
                            archetype,
                            leader: flags & member_flags::LEADER != 0,
                            online: flags & member_flags::ONLINE != 0,
                            away: flags & member_flags::AWAY != 0,
                            level: r.var32()?,
                            offline_secs: r.var32()?,
                        })
                    })?,
                },
            },
            kind::LOBBY_LEFT => ServerMsg::LobbyLeft { reason: r.code()? },
            kind::LOBBY_ERROR => ServerMsg::LobbyError { code: r.code()?, message: r.str()? },
            kind::EMOTE => ServerMsg::Emote { entity_id: r.var32()?, emote: r.u8()? },
            kind::POSE => ServerMsg::Pose { entity_id: r.var32()?, pose: r.u8()? },
            kind::PLAYER_MOUNT => ServerMsg::PlayerMount { entity_id: r.var32()?, mount: r.u8()?, speed: r.u8()? },
            kind::PLAYER_LINK => ServerMsg::PlayerLink { entity_id: r.var32()?, online: r.bool()? },
            kind::MONSTER_HP => ServerMsg::MonsterHp { bars: read_list(&mut r, |r| Some((r.var32()?, r.u8()?)))? },
            kind::RESUMABLE => ServerMsg::Resumable { stage_id: r.var32()?, mission: r.str()?, left_ms: r.var32()? },
            kind::FLAG => ServerMsg::Flag { entity_id: r.var32()?, flag: r.u8()?, x: r.pos()?, y: r.pos()? },
            kind::INVENTORY => ServerMsg::Inventory {
                zeny: r.var()?,
                items: read_list(&mut r, |r| Some((r.str()?, r.var32()?)))?,
                equip: (0..super::combat::items::EQUIP_SLOTS).map(|_| r.str()).collect::<Option<_>>()?,
                stats: r.str()?,
            },
            kind::ITEM_DEFS => ServerMsg::ItemDefs { items: read_list(&mut r, ItemDef::read)? },
            kind::SHOP => ServerMsg::Shop { npc: r.str()?, name: r.str()?, items: read_list(&mut r, |r| r.str())? },
            kind::SHOP_DONE => ServerMsg::ShopDone { spent: r.var()?, earned: r.var()? },
            kind::ITEM_USED => ServerMsg::ItemUsed { item: r.str()?, cooldown_ms: r.var32()? },
            kind::PLAYER_GEAR => ServerMsg::PlayerGear { entity_id: r.var32()?, weapon: r.str()? },
            kind::PLAYER_HEADGEAR => ServerMsg::PlayerHeadgear { entity_id: r.var32()?, items: read_list(&mut r, |r| r.str())? },
            kind::PARTY_INVITE => match r.u8()? {
                0 => ServerMsg::PartyInvite { from: r.var32()?, name: r.str()?, mission: r.str()?, ttl_ms: r.var32()? },
                1 => ServerMsg::PartyAnswer { entity_id: r.var32()?, name: r.str()?, accepted: r.bool()? },
                _ => return None,
            },
            kind::PLAYER_INSPECT => ServerMsg::PlayerInspect {
                entity_id: r.var32()?,
                name: r.str()?,
                archetype: r.str()?,
                level: r.var32()?,
                look: Look::read(&mut r)?,
                equip: (0..super::combat::items::EQUIP_SLOTS).map(|_| r.str()).collect::<Option<_>>()?,
            },
            kind::LOOT => ServerMsg::Loot {
                entity_id: r.var32()?,
                x: r.pos()?,
                y: r.pos()?,
                item: r.str()?,
                count: r.var32()?,
                rare: r.u8()? & 1 != 0,
            },
            kind::COMBO => ServerMsg::Combo { count: r.var32()?, rank: r.u8()?, window_ms: r.var32()?, left_ms: r.var32()? },
            kind::REVIVE => ServerMsg::Revive { entity_id: r.var32()?, progress: r.u8()?, left_ms: r.var32()? },
            kind::BONUS => match r.u8()? {
                0 => ServerMsg::Bonus { id: r.var32()?, bonus: r.str()?, x: r.pos()?, y: r.pos()?, life_ms: r.var32()? },
                1 => ServerMsg::BonusGone { id: r.var32()?, bonus: r.str()?, entity_id: r.var32()? },
                _ => return None,
            },
            kind::MISSION_INFO => ServerMsg::MissionInfo { brief: read_brief(&mut r)? },
            kind::STAGE_CLOCK => ServerMsg::StageClock { elapsed_ms: r.var32()? },
            kind::FOG => match r.u8()? {
                op @ (0 | 1) => ServerMsg::Fog {
                    begun: op == 0,
                    now_ms: r.var32()?,
                    start_ms: r.var32()?,
                    shrink_ms: r.var32()?,
                    x: r.pos()?,
                    y: r.pos()?,
                    from_radius: r.pos()?,
                    to_radius: r.pos()?,
                },
                2 => ServerMsg::FogExposure { secs: r.u8()?, death_secs: r.u8()? },
                _ => return None,
            },
            kind::FRIENDS => ServerMsg::Friends { friends: read_list(&mut r, super::friends::FriendInfo::read)? },
            kind::FRIEND_NOTE => ServerMsg::FriendNote { op: r.u8()?, account: r.var32()?, name: r.str()? },
            kind::TRADE => match r.u8()? {
                0 => ServerMsg::TradeRequest { entity_id: r.var32()?, name: r.str()?, level: r.var32()? },
                1 => ServerMsg::TradeState {
                    entity_id: r.var32()?,
                    name: r.str()?,
                    mine: super::trade::Offer::read(&mut r)?,
                    theirs: super::trade::Offer::read(&mut r)?,
                    flags: r.u8()?,
                },
                2 => ServerMsg::TradeClosed { reason: r.code()?, message: r.str()? },
                _ => return None,
            },
            kind::CHAT => match r.u8()? {
                0 => ServerMsg::Chat { entity_id: r.var32()?, name: r.str()?, text: r.str()? },
                1 => ServerMsg::Announce { text: r.str()? },
                _ => return None,
            },
            kind::FEEDBACK_RESULT => ServerMsg::FeedbackResult { id: r.var32()?, code: r.str()?, message: r.str()? },
            _ => return None,
        };
        r.is_empty().then_some(msg)
    }
}

/// How a player casts (`CAST_PREFS`): the connection keeps it and hands
/// it to the instance with every input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CastPrefs {
    /// The pointer picks the monster and places ground skills.
    pub manual_aim: bool,
    /// Auto-cast slots (bit i = slot i).
    pub auto: u16,
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
    /// Manual aiming and the auto-cast slots.
    CastPrefs(CastPrefs),
    /// Talk to an NPC on the current map; must be within its radius.
    NpcInteract { npc: String },
    /// Pick a mission at a Kafra employee or mission board the player is
    /// standing at: joins
    /// the oldest public lobby of that mission with room (or opens one), or
    /// opens a private lobby when `public` is false.
    MissionSelect { mission: String, public: bool },
    /// Quick start (`LobbyDirectory::quick_start`): alone, at a mission
    /// giver, launches `mission` with these modifiers at once; a party
    /// leader starts the party's run on `mission` (switching the party to
    /// it), with the members in town.
    MissionStart { mission: String, tier: u8, mask: u32 },
    /// Join a lobby by its code (public or private), from anywhere in the hub.
    LobbyJoin { code: String },
    /// Leader only, at any time: the members in town go along.
    LobbyStart,
    /// Leader only: the run's difficulty tier and curse mask
    /// (`world::modifiers`).
    LobbyModifiers { tier: u8, mask: u32 },
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
    /// Play an emote (`0..=MAX_EMOTE`; others are ignored).
    Emote { emote: u8 },
    /// Sit down (1, hubs only) or stand up (0).
    Pose { pose: u8 },
    /// Mount (true) or get off the Peco Peco: needs a learned mount skill
    /// (Peco Peco Ride), anywhere; answered with `player_mount`.
    Mount { on: bool },
    /// Ping a spot near the player with marker `flag` (`0..=MAX_FLAG`).
    Flag { flag: u8, x: f32, y: f32 },
    /// Buy `count` of `item` from the merchant `npc` (standing at it).
    ShopBuy { npc: String, item: String, count: u32 },
    /// Sell `count` of `item` from the bag to the merchant `npc`.
    ShopSell { npc: String, item: String, count: u32 },
    /// The shop window's cart at the merchant `npc`: buys and sells at once.
    ShopDeal { npc: String, order: ShopOrder },
    /// Wear `item` from the bag; `slot` picks an accessory slot (0xFF =
    /// the item's own). Town only.
    Equip { item: String, slot: u8 },
    /// Take off what is in `slot`. Town only.
    Unequip { slot: u8 },
    /// Drink a potion (missions only).
    ItemUse { item: String },
    /// Invite a player on the map into the sender's party.
    PartyInvite { entity_id: u32 },
    /// Answer the invitation of `from` (the inviter's entity id).
    PartyAnswer { from: u32, accept: bool },
    /// Ask for a player's worn gear (`player_inspect`).
    Inspect { entity_id: u32 },
    /// Say something to the players around (local chat).
    Chat { text: String },
    /// Friends list requests (`friends::FriendOp`).
    Friend { op: super::friends::FriendOp },
    /// A player-to-player trade step (`trade::TradeOp`).
    Trade { op: super::trade::TradeOp },
    /// A bug report or suggestion (`crate::feedback`), unchecked.
    Feedback { draft: crate::feedback::Draft },
    /// Back into the stage the character is link-dead in (`accept`), or
    /// forfeit it; then the town.
    StageResume { accept: bool },
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
            ClientMsg::CastPrefs(p) => {
                b.put_u8(kind::CAST_PREFS);
                b.put_u8(u8::from(p.manual_aim));
                b.put_u16_le(p.auto);
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
            ClientMsg::LobbyStart => b.put_u8(kind::LOBBY_START),
            ClientMsg::MissionStart { mission, tier, mask } => {
                b.put_u8(kind::MISSION_START);
                b.put_str(mission);
                b.put_u8(*tier);
                b.put_var((*mask).into());
            }
            ClientMsg::LobbyModifiers { tier, mask } => {
                b.put_u8(kind::LOBBY_MODIFIERS);
                b.put_u8(*tier);
                b.put_var((*mask).into());
            }
            ClientMsg::LobbyLeave => b.put_u8(kind::LOBBY_LEAVE),
            ClientMsg::LobbyKick { entity_id } => {
                b.put_u8(kind::LOBBY_KICK);
                b.put_var((*entity_id).into());
            }
            ClientMsg::StageLeave => b.put_u8(kind::STAGE_LEAVE),
            ClientMsg::StageResume { accept } => {
                b.put_u8(kind::STAGE_RESUME);
                b.put_bool(*accept);
            }
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
            ClientMsg::Emote { emote } => {
                b.put_u8(kind::EMOTE_REQ);
                b.put_u8(*emote);
            }
            ClientMsg::Pose { pose } => {
                b.put_u8(kind::POSE_REQ);
                b.put_u8(*pose);
            }
            ClientMsg::Mount { on } => {
                b.put_u8(kind::MOUNT_REQ);
                b.put_bool(*on);
            }
            ClientMsg::Flag { flag, x, y } => {
                b.put_u8(kind::FLAG_REQ);
                b.put_u8(*flag);
                b.put_pos(*x);
                b.put_pos(*y);
            }
            ClientMsg::ShopBuy { npc, item, count } | ClientMsg::ShopSell { npc, item, count } => {
                b.put_u8(if matches!(self, ClientMsg::ShopBuy { .. }) { kind::SHOP_BUY } else { kind::SHOP_SELL });
                b.put_str(npc);
                b.put_str(item);
                b.put_var((*count).into());
            }
            ClientMsg::ShopDeal { npc, order } => {
                b.put_u8(kind::SHOP_DEAL);
                b.put_str(npc);
                order.put(&mut b);
            }
            ClientMsg::Equip { item, slot } => {
                b.put_u8(kind::EQUIP);
                b.put_str(item);
                b.put_u8(*slot);
            }
            ClientMsg::Unequip { slot } => {
                b.put_u8(kind::UNEQUIP);
                b.put_u8(*slot);
            }
            ClientMsg::ItemUse { item } => {
                b.put_u8(kind::ITEM_USE);
                b.put_str(item);
            }
            ClientMsg::PartyInvite { entity_id } => {
                b.put_u8(kind::PARTY_REQ);
                b.put_u8(0);
                b.put_var((*entity_id).into());
            }
            ClientMsg::PartyAnswer { from, accept } => {
                b.put_u8(kind::PARTY_REQ);
                b.put_u8(1);
                b.put_var((*from).into());
                b.put_u8(u8::from(*accept));
            }
            ClientMsg::Inspect { entity_id } => {
                b.put_u8(kind::INSPECT_REQ);
                b.put_var((*entity_id).into());
            }
            ClientMsg::Chat { text } => {
                b.put_u8(kind::CHAT_REQ);
                b.put_str(text);
            }
            ClientMsg::Friend { op } => {
                b.put_u8(kind::FRIEND_REQ);
                op.put(&mut b);
            }
            ClientMsg::Trade { op } => {
                b.put_u8(kind::TRADE_REQ);
                op.put(&mut b);
            }
            ClientMsg::Feedback { draft } => {
                b.put_u8(kind::FEEDBACK_REQ);
                for f in [&draft.kind, &draft.priority, &draft.title, &draft.body, &draft.client_version, &draft.user_agent] {
                    b.put_str(f);
                }
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
                kind::CAST_PREFS => ClientMsg::CastPrefs(CastPrefs { manual_aim: r.u8()? & 1 != 0, auto: r.u16()? }),
                kind::NPC_INTERACT => ClientMsg::NpcInteract { npc: r.str()? },
                kind::MISSION_SELECT => ClientMsg::MissionSelect { mission: r.str()?, public: r.bool()? },
                kind::LOBBY_JOIN => ClientMsg::LobbyJoin { code: r.str()? },
                kind::LOBBY_START => ClientMsg::LobbyStart,
                kind::MISSION_START => ClientMsg::MissionStart { mission: r.str()?, tier: r.u8()?, mask: r.var32()? },
                kind::LOBBY_MODIFIERS => ClientMsg::LobbyModifiers { tier: r.u8()?, mask: r.var32()? },
                kind::LOBBY_LEAVE => ClientMsg::LobbyLeave,
                kind::LOBBY_KICK => ClientMsg::LobbyKick { entity_id: r.var32()? },
                kind::STAGE_LEAVE => ClientMsg::StageLeave,
                kind::STAGE_RESUME => ClientMsg::StageResume { accept: r.bool()? },
                kind::RESYNC => ClientMsg::Resync,
                kind::STYLE_SET => ClientMsg::StyleSet { look: Look::read(r)? },
                kind::SKILL_LEARN => ClientMsg::SkillLearn { skill: r.str()? },
                kind::SKILL_RESET => ClientMsg::SkillReset,
                kind::SKILL_SLOT => ClientMsg::SkillSlot { slot: r.u8()?, skill: r.str()? },
                kind::EMOTE_REQ => ClientMsg::Emote { emote: r.u8()? },
                kind::POSE_REQ => ClientMsg::Pose { pose: r.u8()? },
                kind::MOUNT_REQ => ClientMsg::Mount { on: r.bool()? },
                kind::FLAG_REQ => ClientMsg::Flag { flag: r.u8()?, x: r.pos()?, y: r.pos()? },
                kind::SHOP_BUY => ClientMsg::ShopBuy { npc: r.str()?, item: r.str()?, count: r.var32()? },
                kind::SHOP_SELL => ClientMsg::ShopSell { npc: r.str()?, item: r.str()?, count: r.var32()? },
                kind::SHOP_DEAL => ClientMsg::ShopDeal { npc: r.str()?, order: ShopOrder::read(r)? },
                kind::EQUIP => ClientMsg::Equip { item: r.str()?, slot: r.u8()? },
                kind::UNEQUIP => ClientMsg::Unequip { slot: r.u8()? },
                kind::ITEM_USE => ClientMsg::ItemUse { item: r.str()? },
                kind::PARTY_REQ => match r.u8()? {
                    0 => ClientMsg::PartyInvite { entity_id: r.var32()? },
                    1 => ClientMsg::PartyAnswer { from: r.var32()?, accept: r.bool()? },
                    _ => return None,
                },
                kind::INSPECT_REQ => ClientMsg::Inspect { entity_id: r.var32()? },
                kind::CHAT_REQ => ClientMsg::Chat { text: r.str()? },
                kind::FRIEND_REQ => ClientMsg::Friend { op: super::friends::FriendOp::read(r)? },
                kind::TRADE_REQ => ClientMsg::Trade { op: super::trade::TradeOp::read(r)? },
                kind::FEEDBACK_REQ => ClientMsg::Feedback {
                    draft: crate::feedback::Draft {
                        kind: r.str()?,
                        priority: r.str()?,
                        title: r.str()?,
                        body: r.str()?,
                        client_version: r.str()?,
                        user_agent: r.str()?,
                    },
                },
                _ => return None,
            })
        };
        // The retired ready flag (`LOBBY_READY`) is unknown now: ignored.
        let known = k != kind::LOBBY_READY && matches!(k, INPUT_KIND | kind::PING..=kind::ITEM_USE | kind::CHAT_REQ | kind::PARTY_REQ | kind::INSPECT_REQ | kind::FRIEND_REQ | kind::TRADE_REQ | kind::FEEDBACK_REQ | kind::STAGE_RESUME | kind::LOBBY_MODIFIERS | kind::MISSION_START | kind::CAST_PREFS | kind::SHOP_DEAL | kind::MOUNT_REQ);
        if !known {
            return Err(Unknown);
        }
        match body(&mut r) {
            Some(msg) if r.is_empty() => Ok(msg),
            _ => Err(Malformed),
        }
    }
}

/// Player block of a snapshot (shared by every recipient on a map, or
/// rebuilt per recipient where sight is limited). Reusable via `clear`.
#[derive(Default)]
pub struct SnapshotBody {
    buf: BytesMut,
    count: u16,
}

impl SnapshotBody {
    pub fn with_capacity(entities: usize) -> Self {
        Self { buf: BytesMut::with_capacity(entities * SNAPSHOT_ENTRY_LEN), count: 0 }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
        self.count = 0;
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
        m.put_var(513 << 3 | 2);
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
            vec![DeltaEntry::Spawn { slot: 513, generation: 4, kind: 2, x: 6416, y: 480, vx: 10, vy: -10, status: None }]
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
            ClientMsg::CastPrefs(CastPrefs { manual_aim: true, auto: 0b1001 }),
            ClientMsg::CastPrefs(CastPrefs::default()),
            ClientMsg::SkillLearn { skill: "bolt".into() },
            ClientMsg::SkillReset,
            ClientMsg::SkillSlot { slot: 4, skill: "slide".into() },
            ClientMsg::NpcInteract { npc: "board".into() },
            ClientMsg::MissionSelect { mission: "m".into(), public: false },
            ClientMsg::LobbyJoin { code: "AB2CD".into() },
            ClientMsg::LobbyStart,
            ClientMsg::MissionStart { mission: "m".into(), tier: 2, mask: 0b100_0001 },
            ClientMsg::LobbyModifiers { tier: 7, mask: 0b101_0001 },
            ClientMsg::LobbyLeave,
            ClientMsg::LobbyKick { entity_id: 300 },
            ClientMsg::StageLeave,
            ClientMsg::Resync,
            ClientMsg::StyleSet { look: Look { gender: 1, hair: 12, hair_color: 15, body_color: 7 } },
            ClientMsg::Emote { emote: 127 },
            ClientMsg::Pose { pose: 1 },
            ClientMsg::Mount { on: true },
            ClientMsg::Flag { flag: 9, x: 120.5, y: 33.25 },
            ClientMsg::ShopBuy { npc: "tool".into(), item: "red_potion".into(), count: 12 },
            ClientMsg::ShopSell { npc: "tool".into(), item: "jellopy".into(), count: 300 },
            ClientMsg::Equip { item: "clip".into(), slot: 9 },
            ClientMsg::Unequip { slot: 3 },
            ClientMsg::ItemUse { item: "red_potion".into() },
            ClientMsg::Chat { text: "hi там 👋".into() },
            ClientMsg::PartyInvite { entity_id: 300 },
            ClientMsg::PartyAnswer { from: 300, accept: false },
            ClientMsg::Inspect { entity_id: 7 },
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
        assert_eq!(ClientMsg::decode(&[kind::LOBBY_READY, 1]), Err(ClientDecodeError::Unknown), "the retired ready flag");
        assert_eq!(ClientMsg::decode(&[kind::LOBBY_JOIN]), Err(ClientDecodeError::Malformed));
        assert_eq!(ClientMsg::decode(&[kind::LOBBY_START, 0]), Err(ClientDecodeError::Malformed), "trailing");
        assert_eq!(ClientMsg::decode(&[kind::HUB_JOIN, 0xFF, 0xFF, 0x7F]), Err(ClientDecodeError::Malformed));
        assert_eq!(ClientMsg::decode(&[kind::EMOTE_REQ]), Err(ClientDecodeError::Malformed));
        assert_eq!(ClientMsg::decode(&[kind::POSE_REQ, 1, 0]), Err(ClientDecodeError::Malformed), "trailing");
        assert_eq!(ClientMsg::decode(&[kind::FLAG_REQ, 1, 0, 0]), Err(ClientDecodeError::Malformed), "short pos");
        assert_eq!(ClientMsg::decode(&[0x6F]), Err(ClientDecodeError::Unknown));
        assert_eq!(ClientMsg::decode(&[kind::UNEQUIP]), Err(ClientDecodeError::Malformed));
        assert_eq!(ClientMsg::decode(&[kind::PARTY_REQ, 2, 1]), Err(ClientDecodeError::Malformed), "unknown party op");
        assert_eq!(ClientMsg::decode(&[kind::INSPECT_REQ]), Err(ClientDecodeError::Malformed));
        assert_eq!(ClientMsg::decode(b"{\"type\":\"ping\"}"), Err(ClientDecodeError::Unknown), "JSON is gone");
    }

    #[test]
    fn server_messages_round_trip() {
        let mission = missions::MISSIONS[0].clone();
        let player = PlayerInfo { entity_id: 5, name: "Ann".into(), archetype: "novice".into(), x: 10.5, y: 3.25, facing: 2, look: Look { gender: 1, hair: 7, hair_color: 3, body_color: 5 }, level: 31, weapon: "katana".into() };
        let all = vec![
            ServerMsg::Connected { user_id: 9, session_id: Uuid::from_u128(0x1234), server_version: String::new() },
            ServerMsg::Connected { user_id: 9, session_id: Uuid::from_u128(0x1234), server_version: "0.1.0+abc".into() },
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
            ServerMsg::PlayerVitals { entity_id: 5, hp: 40, max_hp: 122, sp: 0, max_sp: 131, state: 1, status: 640, temp_hp: 3, light: 4, level: 31 },
            ServerMsg::StageEnd { victory: true, secs: 421, kills: 1234, exp: 98765, leave_ms: 10000 },
            ServerMsg::Boss { entity_id: 0x800005, name: "Baphomet".into(), hp: 2000, max_hp: 2790 },
            ServerMsg::Buffs { buffs: vec![("haste".into(), 4500), ("endure".into(), 900)] },
            ServerMsg::Combat {
                swings: vec![Swing { entity_id: 5, facing: 3, target: 0x80_0001 }],
                hits: vec![HitEvent { target: MONSTER_ENTITY_BASE + 7, amount: 13, flags: hit::CRITICAL | hit::KILL | 0x30 }],
            },
            ServerMsg::Progress { level: 31, exp: 87_000, exp_to_next: 97_000, gained: 353 },
            ServerMsg::Skills { level: 12, points: 3, skills: vec![("bolt".into(), 4), ("slide".into(), 1)], hotbar: vec!["bolt".into(), String::new()] },
            ServerMsg::Cast {
                caster: 300,
                skill: "bolt".into(),
                x: 10.5,
                y: 3.25,
                cast_ms: 540,
                cooldown_ms: 1000,
                areas: vec![Telegraph { id: 9, shape: 1, look: look::FIRE, x: 10.5, y: 3.25, dir: 64, a: 8.0, b: 1.0, span_ms: 6000, follow: 0 }],
                level: 3,
            },
            ServerMsg::Cast { caster: 300, skill: String::new(), x: 0.0, y: 0.0, cast_ms: 0, cooldown_ms: 0, areas: vec![], level: 0 },
            ServerMsg::ZoneEnd { id: 9 },
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
                        LobbyMemberInfo { entity_id: 5, name: "Ann".into(), archetype: "novice".into(), leader: true, online: true, away: false, level: 12, offline_secs: 0 },
                        LobbyMemberInfo { entity_id: 6, name: "Bo".into(), archetype: "mage".into(), leader: false, online: false, away: true, level: 3, offline_secs: 86_400 },
                    ],
                },
            },
            ServerMsg::LobbyLeft { reason: "some_new_reason".into() },
            ServerMsg::LobbyError { code: "lobby_full".into(), message: "full".into() },
            ServerMsg::Emote { entity_id: RESIDENT_ENTITY_BASE + 3, emote: 46 },
            ServerMsg::Pose { entity_id: 5, pose: pose::SIT },
            ServerMsg::PlayerMount { entity_id: 5, mount: mount::PECO, speed: 25 },
            ServerMsg::Flag { entity_id: 5, flag: 3, x: 140.75, y: 12.5 },
            ServerMsg::Inventory {
                zeny: 1_234_567,
                items: vec![("red_potion".into(), 5), ("jellopy".into(), 300)],
                equip: ["", "", "", "cotton_shirt", "sword", "guard", "", "", "clip", ""].map(String::from).to_vec(),
                stats: r#"{"ac":14}"#.into(),
            },
            ServerMsg::ItemDefs { items: ItemDef::catalogue() },
            ServerMsg::Shop { npc: "dega".into(), name: "Dega".into(), items: vec!["sword".into(), "knife".into()] },
            ServerMsg::ItemUsed { item: "red_potion".into(), cooldown_ms: 1000 },
            ServerMsg::PlayerGear { entity_id: 5, weapon: "katana".into() },
            ServerMsg::PlayerHeadgear { entity_id: 5, items: vec!["hat".into(), "sunglasses".into()] },
            ServerMsg::PlayerHeadgear { entity_id: 5, items: Vec::new() },
            ServerMsg::Loot { entity_id: MONSTER_ENTITY_BASE + 7, x: 12.5, y: 3.25, item: "jellopy".into(), count: 2, rare: false },
            ServerMsg::Loot { entity_id: 5, x: 0.0, y: 0.0, item: "zeny".into(), count: 1500, rare: true },
            ServerMsg::Combo { count: 187, rank: 5, window_ms: 3000, left_ms: 2950 },
            ServerMsg::Combo { count: 0, rank: 0, window_ms: 0, left_ms: 0 },
            ServerMsg::Revive { entity_id: 5, progress: 45, left_ms: 17_250 },
            ServerMsg::Revive { entity_id: 5, progress: 0, left_ms: 0 },
            ServerMsg::Bonus { id: 7, bonus: "storm_gust".into(), x: 40.5, y: 12.25, life_ms: 30_000 },
            ServerMsg::BonusGone { id: 7, bonus: "storm_gust".into(), entity_id: 101 },
            ServerMsg::BonusGone { id: 8, bonus: "zeny".into(), entity_id: 0 },
            ServerMsg::MissionInfo { brief: golden::golden_brief() },
            ServerMsg::MissionInfo { brief: MissionBrief { monsters: Vec::new(), ..golden::golden_brief() } },
            ServerMsg::StageClock { elapsed_ms: 0 },
            ServerMsg::Fog { begun: true, now_ms: 720_000, start_ms: 720_000, shrink_ms: 150_000, x: 200.5, y: 180.25, from_radius: 160.0, to_radius: 10.0 },
            ServerMsg::Fog { begun: false, now_ms: 800_000, start_ms: 720_000, shrink_ms: 150_000, x: 0.0, y: 0.0, from_radius: 0.0, to_radius: 0.0 },
            ServerMsg::FogExposure { secs: 12, death_secs: 30 },
            ServerMsg::Chat { entity_id: 300, name: "Ann".into(), text: "hi там 👋".into() },
            ServerMsg::Announce { text: "Server restart in 5 minutes.".into() },
            ServerMsg::PartyInvite { from: 300, name: "Ann".into(), mission: "Poring Island Survival".into(), ttl_ms: 30_000 },
            ServerMsg::PartyAnswer { entity_id: 301, name: "Bo".into(), accepted: true },
            ServerMsg::PartyAnswer { entity_id: 301, name: "Bo".into(), accepted: false },
            ServerMsg::PlayerInspect {
                entity_id: 300,
                name: "Ann".into(),
                archetype: "mage".into(),
                level: 44,
                look: Look { gender: 1, hair: 7, hair_color: 3, body_color: 5 },
                equip: vec!["cap".into(), String::new(), String::new(), String::new(), "rod".into(), String::new(), String::new(), String::new(), String::new(), "clip".into()],
            },
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
                        level: 31,
                        weapon: "bow".into(),
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
                        members: vec![LobbyMemberInfo { entity_id: 9, name: "Bo".into(), archetype: "novice".into(), leader: true, online: true, away: false, level: 1, offline_secs: 0 }],
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
            ("player_vitals", ServerMsg::PlayerVitals { entity_id: 300, hp: 40, max_hp: 122, sp: 7, max_sp: 131, state: 1, status: 640, temp_hp: 3, light: 4, level: 31 }.encode()),
            (
                "combat",
                ServerMsg::Combat {
                    swings: vec![Swing { entity_id: 300, facing: 6, target: MONSTER_ENTITY_BASE + 7 }],
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
            (
                "cast",
                ServerMsg::Cast {
                    caster: 300,
                    skill: "bolt".into(),
                    x: 10.5,
                    y: 3.25,
                    cast_ms: 540,
                    cooldown_ms: 1000,
                    areas: vec![Telegraph { id: 9, shape: 1, look: look::FIRE, x: 10.5, y: 3.25, dir: 64, a: 8.0, b: 1.0, span_ms: 6000, follow: 300 }],
                    level: 3,
                }
                .encode(),
            ),
            ("zone_end", ServerMsg::ZoneEnd { id: 9 }.encode()),
            ("stage_end", ServerMsg::StageEnd { victory: true, secs: 421, kills: 1234, exp: 98765, leave_ms: 10000 }.encode()),
            ("boss", ServerMsg::Boss { entity_id: 0x800005, name: "Baph".into(), hp: 2000, max_hp: 2790 }.encode()),
            ("buffs", ServerMsg::Buffs { buffs: vec![("haste".into(), 4500)] }.encode()),
            (
                "inventory",
                ServerMsg::Inventory {
                    zeny: 300,
                    items: vec![("red_potion".into(), 5)],
                    equip: ["", "", "", "cotton_shirt", "sword", "", "", "", "", ""].map(String::from).to_vec(),
                    stats: r#"{"ac":12}"#.into(),
                }
                .encode(),
            ),
            (
                "item_defs",
                ServerMsg::ItemDefs {
                    items: vec![ItemDef { id: "cap".into(), name: "Cap".into(), kind: 5, slot: 0, price: 4000, classes: 11, desc: "+1 AC".into() }],
                }
                .encode(),
            ),
            ("shop", ServerMsg::Shop { npc: "dega".into(), name: "Dega".into(), items: vec!["sword".into()] }.encode()),
            ("item_used", ServerMsg::ItemUsed { item: "red_potion".into(), cooldown_ms: 1000 }.encode()),
            ("player_gear", ServerMsg::PlayerGear { entity_id: 300, weapon: "bow".into() }.encode()),
            ("player_headgear", ServerMsg::PlayerHeadgear { entity_id: 300, items: vec!["hat".into(), "flu_mask".into()] }.encode()),
            (
                "loot",
                ServerMsg::Loot { entity_id: 0x800005, x: 10.5, y: 3.25, item: "knife".into(), count: 1, rare: true }.encode(),
            ),
            ("combo", ServerMsg::Combo { count: 187, rank: 5, window_ms: 3000, left_ms: 2950 }.encode()),
            ("revive", ServerMsg::Revive { entity_id: 300, progress: 45, left_ms: 17_250 }.encode()),
            ("bonus", ServerMsg::Bonus { id: 7, bonus: "zeny".into(), x: 10.5, y: 3.25, life_ms: 30_000 }.encode()),
            ("bonus_gone", ServerMsg::BonusGone { id: 7, bonus: "zeny".into(), entity_id: 300 }.encode()),
            ("mission_info", ServerMsg::MissionInfo { brief: golden_brief() }.encode()),
            ("stage_clock", ServerMsg::StageClock { elapsed_ms: 90_500 }.encode()),
            (
                "fog",
                ServerMsg::Fog { begun: true, now_ms: 720_000, start_ms: 720_000, shrink_ms: 150_000, x: 10.5, y: 3.25, from_radius: 40.0, to_radius: 10.0 }
                    .encode(),
            ),
            ("fog_exposure", ServerMsg::FogExposure { secs: 12, death_secs: 30 }.encode()),
            ("chat", ServerMsg::Chat { entity_id: 300, name: "Ann".into(), text: "hi!".into() }.encode()),
            ("announce", ServerMsg::Announce { text: "bye".into() }.encode()),
            ("party_invite", ServerMsg::PartyInvite { from: 300, name: "Ann".into(), mission: "M".into(), ttl_ms: 30_000 }.encode()),
            ("party_answer", ServerMsg::PartyAnswer { entity_id: 300, name: "Ann".into(), accepted: true }.encode()),
            (
                "player_inspect",
                ServerMsg::PlayerInspect {
                    entity_id: 300,
                    name: "Ann".into(),
                    archetype: "novice".into(),
                    level: 12,
                    look: Look { gender: 1, hair: 5, hair_color: 10, body_color: 6 },
                    equip: (0..10).map(|i| if i == 4 { "sword".to_string() } else { String::new() }).collect(),
                }
                .encode(),
            ),
        ]
    }

    /// A one-monster brief (the `mission_info` golden on both sides).
    pub fn golden_brief() -> MissionBrief {
        MissionBrief {
            mission: "m".into(),
            map: "f".into(),
            time_limit_secs: 900,
            levels: (1, 20),
            level_every_secs: 90,
            monsters: vec![MonsterBrief {
                id: "p".into(),
                name: "P".into(),
                archetype: "swarm".into(),
                element: "water".into(),
                size: "small".into(),
                unlock_secs: 0,
                levels: (1, 10),
                hp: (5, 21),
                pack: (6, 12),
                drops: vec![DropBrief { item: "apple".into(), rate: 300, min: 1, max: 2 }],
            }],
            band: (1, 3, 15),
            light_permille: 700,
            events: vec![EventBrief { id: "tide".into(), name: "Tide".into(), starts: vec![120, 420], warn_secs: 15, duration_secs: 40 }],
        }
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
            ("shop_buy", ClientMsg::ShopBuy { npc: "dega".into(), item: "sword".into(), count: 2 }.encode()),
            ("shop_sell", ClientMsg::ShopSell { npc: "dega".into(), item: "sword".into(), count: 1 }.encode()),
            ("equip", ClientMsg::Equip { item: "clip".into(), slot: 0xFF }.encode()),
            ("unequip", ClientMsg::Unequip { slot: 5 }.encode()),
            ("item_use", ClientMsg::ItemUse { item: "red_potion".into() }.encode()),
            ("chat_req", ClientMsg::Chat { text: "hi".into() }.encode()),
            ("party_invite_req", ClientMsg::PartyInvite { entity_id: 300 }.encode()),
            ("party_answer_req", ClientMsg::PartyAnswer { from: 300, accept: true }.encode()),
            ("inspect_req", ClientMsg::Inspect { entity_id: 300 }.encode()),
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
        m.put_var(3 << 3 | 2);
        m.put_slice(&[5, 1]);
        m.put_u16_le(640);
        m.put_u16_le(1280);
        m.put_slice(&[16, 0xF0]);
        // NUDGE with a status byte (frozen).
        m.put_var(1 << 2);
        m.put_u8(2);
        m.put_slice(&[0xFE, 3, 0, 0x80]);
        m.put_var(35 << 3 | 1);
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
            "210166054669656c64029003020000003f00000034c200002041000020420000a04106612e6f676702006000900000010902426f066e6f76696365c000000106010402031f03626f77",
        ),
        (
            "lobby_state",
            "310105414232434416706f72696e675f69736c616e645f737572766976616c16506f72696e672049736c616e6420537572766976616c48486f6c64206f757420616761696e73742067726f77696e67207761766573206f6e20506f72696e672049736c616e6420756e74696c2074686520626f73732073686f77732075702e0a7072745f66696c6430380800010902426f066e6f76696365060100",
        ),
        ("lobby_left", "32ff06637573746f6d"),
        ("pong", "1105"),
        ("player_look", "29ac02000c0f07"),
        ("player_vitals", "2aac02287a07830101800503041f"),
        ("combat", "2b01ac0206878080040287808004c80115ac020002"),
        ("progress", "2c1fd8a705e8f505e102"),
        ("skills", "2d0c030104626f6c74040204626f6c7400"),
        ("cast", "2eac0204626f6c74a002d0009c04e80701090102a002d0004000024000f02eac0203"),
        ("zone_end", "2f09"),
        ("stage_end", "3401a503d209cd8306904e"),
        ("boss", "35858080040442617068d00fe615"),
        ("buffs", "36010568617374659423"),
        ("inventory", "3aac02010a7265645f706f74696f6e050000000c636f74746f6e5f73686972740573776f72640000000000097b226163223a31327d"),
        ("item_defs", "3b0103636170034361700500a01f0b052b31204143"),
        ("shop", "3c04646567610444656761010573776f7264"),
        ("item_used", "3d0a7265645f706f74696f6ee807"),
        ("player_gear", "3eac0203626f77"),
        ("player_headgear", "8bac02020368617408666c755f6d61736b"),
        ("loot", "3f85808004a002d000056b6e6966650101"),
        ("combo", "60bb0105b8178617"),
        ("revive", "61ac022de28601"),
        ("bonus", "680007047a656e79a002d000b0ea01"),
        ("bonus_gone", "680107047a656e79ac02"),
        ("mission_info", "6c016d0166840701145a010170015005737761726d05776174657205736d616c6c00010a0515060c01056170706c65ac02010201030fbc0501047469646504546964650278a4030f28"),
        ("stage_clock", "6d84c305"),
        ("fog", "6e0080f92b80f92bf09309a002d000000a8002"),
        ("fog_exposure", "6e020c1e"),
        ("chat", "7a00ac0203416e6e03686921"),
        ("announce", "7a0103627965"),
        ("party_invite", "7000ac0203416e6e014db0ea01"),
        ("party_answer", "7001ac0203416e6e01"),
        ("player_inspect", "71ac0203416e6e066e6f766963650c01050a06000000000573776f72640000000000"),
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
        ("shop_buy", "5404646567610573776f726402"),
        ("shop_sell", "5504646567610573776f726401"),
        ("equip", "5604636c6970ff"),
        ("unequip", "5705"),
        ("item_use", "580a7265645f706f74696f6e"),
        ("chat_req", "5e026869"),
        ("party_invite_req", "5900ac02"),
        ("party_answer_req", "5901ac0201"),
        ("inspect_req", "5aac02"),
        ("stage_snapshot", "030400640000000c000000010009000000600080000201041a05018002000510f00402fe0300809902ffff00007f0103"),
    ];

    /// Quick start (0x63). Change only together with `client_test.dart`
    /// (`mission start golden`).
    #[test]
    fn mission_start_matches_the_dart_golden() {
        let b = ClientMsg::MissionStart { mission: "m1".into(), tier: 2, mask: 65 }.encode();
        assert_eq!(hex(&b), "63026d310241");
        assert_eq!(ClientMsg::decode(&b), Ok(ClientMsg::MissionStart { mission: "m1".into(), tier: 2, mask: 65 }));
    }

    /// Cast preferences (0x85). Change only together with `client_test.dart`
    /// (`cast prefs golden`).
    #[test]
    fn cast_prefs_match_the_dart_golden() {
        let prefs = CastPrefs { manual_aim: true, auto: 0x8009 };
        let b = ClientMsg::CastPrefs(prefs).encode();
        assert_eq!(hex(&b), "85010980");
        assert_eq!(ClientMsg::decode(&b), Ok(ClientMsg::CastPrefs(prefs)));
    }

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
                Spawn { slot: 3, generation: 5, kind: 1, x: 640, y: 1280, vx: 16, vy: -16, status: None },
                Nudge { slot: 4, dx: -2, dy: 3, vx: 0, vy: -128, status: Some(2) },
                Move { slot: 40, x: 65535, y: 0, vx: 127, vy: 1, status: None },
                Remove { slot: 41 },
            ]
        );
    }

    /// Trade frames (0x74 TRADE, 0x5B TRADE): a list of their own.
    /// Change these only together with `client_test.dart` (`trade goldens`).
    pub fn trade() -> Vec<(&'static str, Bytes)> {
        use crate::world::trade::{Offer, TradeOp};
        let mine = Offer { items: vec![("red_potion".into(), 3), ("sword".into(), 1)], zeny: 1500 };
        vec![
            ("trade_request", ServerMsg::TradeRequest { entity_id: 300, name: "Ann".into(), level: 12 }.encode()),
            ("trade_state", ServerMsg::TradeState { entity_id: 300, name: "Ann".into(), mine, theirs: Offer::default(), flags: 0b0110 }.encode()),
            ("trade_closed", ServerMsg::TradeClosed { reason: "done".into(), message: "Trade complete.".into() }.encode()),
            ("trade_req", ClientMsg::Trade { op: TradeOp::Request { entity_id: 300 } }.encode()),
            ("trade_accept", ClientMsg::Trade { op: TradeOp::Accept }.encode()),
            ("trade_decline", ClientMsg::Trade { op: TradeOp::Decline }.encode()),
            ("trade_cancel", ClientMsg::Trade { op: TradeOp::Cancel }.encode()),
            ("trade_lock", ClientMsg::Trade { op: TradeOp::Lock }.encode()),
            ("trade_confirm", ClientMsg::Trade { op: TradeOp::Confirm }.encode()),
            ("trade_zeny", ClientMsg::Trade { op: TradeOp::Zeny(1500) }.encode()),
            ("trade_item", ClientMsg::Trade { op: TradeOp::Item { item: "red_potion".into(), count: 3 } }.encode()),
        ]
    }

    const TRADE_EXPECTED: &[(&str, &str)] = &[
        ("trade_request", "7400ac0203416e6e0c"),
        ("trade_state", "7401ac0203416e6edc0b020a7265645f706f74696f6e030573776f726401000006"),
        ("trade_closed", "7402ff04646f6e650f547261646520636f6d706c6574652e"),
        ("trade_req", "5b00ac02"),
        ("trade_accept", "5b01"),
        ("trade_decline", "5b02"),
        ("trade_cancel", "5b03"),
        ("trade_lock", "5b04"),
        ("trade_confirm", "5b05"),
        ("trade_zeny", "5b06dc0b"),
        ("trade_item", "5b070a7265645f706f74696f6e03"),
    ];

    #[test]
    fn trade_frames_round_trip_and_match_the_dart_goldens() {
        for (name, b) in trade() {
            println!("(\"{name}\", \"{}\"),", hex(&b));
            if ["trade_request", "trade_state", "trade_closed"].contains(&name) {
                let m = ServerMsg::decode(&b).unwrap();
                assert_eq!(m.encode(), b, "{name}");
                assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none(), "truncated {name}");
            } else {
                let m = ClientMsg::decode(&b).unwrap();
                assert_eq!(m.encode(), b, "{name}");
                if b.len() > 2 {
                    assert_eq!(ClientMsg::decode(&b[..b.len() - 1]), Err(ClientDecodeError::Malformed), "truncated {name}");
                }
            }
        }
        assert_eq!(ClientMsg::decode(&[kind::TRADE_REQ, 8]), Err(ClientDecodeError::Malformed), "unknown op");
        assert_eq!(ClientMsg::decode(&[kind::TRADE_REQ]), Err(ClientDecodeError::Malformed), "no op");
        let got: Vec<(&str, String)> = trade().into_iter().map(|(n, b)| (n, hex(&b))).collect();
        let want: Vec<(&str, String)> = TRADE_EXPECTED.iter().map(|(n, h)| (*n, h.to_string())).collect();
        assert_eq!(got, want);
    }

    /// Merchant cart frames (0x8C SHOP_DEAL, 0x8D SHOP_DONE): a list of
    /// their own. Change these only together with `client_test.dart`
    /// (`shop goldens`).
    pub fn shop() -> Vec<(&'static str, Bytes)> {
        let order = ShopOrder { buy: vec![("red_potion".into(), 12)], sell: vec![("jellopy".into(), 300), ("sword+7".into(), 1)] };
        vec![
            ("shop_deal", ClientMsg::ShopDeal { npc: "tool".into(), order }.encode()),
            ("shop_done", ServerMsg::ShopDone { spent: 600, earned: 1250 }.encode()),
        ]
    }

    const SHOP_EXPECTED: &[(&str, &str)] = &[("shop_deal", "8c04746f6f6c010a7265645f706f74696f6e0c02076a656c6c6f7079ac020773776f72642b3701"), ("shop_done", "8dd804e209")];

    #[test]
    fn shop_frames_round_trip_and_match_the_dart_goldens() {
        for (name, b) in shop() {
            println!("(\"{name}\", \"{}\"),", hex(&b));
            if name == "shop_done" {
                assert_eq!(ServerMsg::decode(&b).unwrap().encode(), b);
                assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none(), "truncated {name}");
            } else {
                assert_eq!(ClientMsg::decode(&b).unwrap().encode(), b);
                assert_eq!(ClientMsg::decode(&b[..b.len() - 1]), Err(ClientDecodeError::Malformed), "truncated {name}");
            }
        }
        let got: Vec<(&str, String)> = shop().into_iter().map(|(n, b)| (n, hex(&b))).collect();
        let want: Vec<(&str, String)> = SHOP_EXPECTED.iter().map(|(n, h)| (*n, h.to_string())).collect();
        assert_eq!(got, want);
    }

    /// Friends frames (0x78 FRIENDS, 0x79 FRIEND_NOTE, 0x5D FRIEND): a
    /// list of their own so the shared list above stays untouched. Change
    /// these only together with `client_test.dart` (`friends goldens`).
    pub fn friends() -> Vec<(&'static str, Bytes)> {
        use crate::world::friends::{FriendInfo, FriendOp};
        let ann = FriendInfo {
            account: 7,
            link: 0,
            name: "Ann".into(),
            online: true,
            class: "mage".into(),
            level: 12,
            map: "Prontera".into(),
            channel: 2,
            characters: vec![300, 301],
        };
        let bob = FriendInfo { account: 9, link: 1, name: "Bob".into(), online: false, class: "archer".into(), level: 3, map: String::new(), channel: 0, characters: vec![5] };
        vec![
            ("friends", ServerMsg::Friends { friends: vec![ann, bob] }.encode()),
            ("friend_note", ServerMsg::FriendNote { op: 0, account: 7, name: "Ann".into() }.encode()),
            ("friend_list", ClientMsg::Friend { op: FriendOp::List }.encode()),
            ("friend_add", ClientMsg::Friend { op: FriendOp::Add { entity_id: 300 } }.encode()),
            ("friend_add_name", ClientMsg::Friend { op: FriendOp::AddName { name: "Ann".into() } }.encode()),
            ("friend_accept", ClientMsg::Friend { op: FriendOp::Accept { account: 7 } }.encode()),
            ("friend_decline", ClientMsg::Friend { op: FriendOp::Decline { account: 7 } }.encode()),
            ("friend_remove", ClientMsg::Friend { op: FriendOp::Remove { account: 7 } }.encode()),
        ]
    }

    const FRIENDS_EXPECTED: &[(&str, &str)] = &[
        ("friends", "7802070003416e6e01046d6167650c0850726f6e746572610202ac02ad02090103426f6200066172636865720300000105"),
        ("friend_note", "79000703416e6e"),
        ("friend_list", "5d00"),
        ("friend_add", "5d01ac02"),
        ("friend_add_name", "5d0203416e6e"),
        ("friend_accept", "5d0307"),
        ("friend_decline", "5d0407"),
        ("friend_remove", "5d0507"),
    ];

    #[test]
    fn friend_frames_round_trip_and_match_the_dart_goldens() {
        for (name, b) in friends() {
            if name.starts_with("friend_") && name != "friend_note" {
                let m = ClientMsg::decode(&b).unwrap();
                assert_eq!(m.encode(), b, "{name}");
                assert_eq!(ClientMsg::decode(&b[..b.len() - 1]), Err(ClientDecodeError::Malformed), "truncated {name}");
            } else {
                let m = ServerMsg::decode(&b).unwrap();
                assert_eq!(m.encode(), b, "{name}");
                assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none(), "truncated {name}");
            }
        }
        assert_eq!(ClientMsg::decode(&[kind::FRIEND_REQ, 6]), Err(ClientDecodeError::Malformed), "unknown op");
        let got: Vec<(&str, String)> = friends().into_iter().map(|(n, b)| (n, hex(&b))).collect();
        let want: Vec<(&str, String)> = FRIENDS_EXPECTED.iter().map(|(n, h)| (*n, h.to_string())).collect();
        assert_eq!(got, want);
    }

    /// Feedback frames (0x5C FEEDBACK, 0x62 FEEDBACK_RESULT) and the
    /// versioned CONNECTED: a list of their own. Change these only together
    /// with `client_test.dart` (`feedback goldens`).
    pub fn feedback() -> Vec<(&'static str, Bytes)> {
        let draft = crate::feedback::Draft {
            kind: "bug".into(),
            priority: "high".into(),
            title: "Stuck".into(),
            body: "Line one\nLine two".into(),
            client_version: "0.27".into(),
            user_agent: "UA".into(),
        };
        vec![
            (
                "connected_version",
                ServerMsg::Connected { user_id: 9, session_id: Uuid::from_u128(0x1234), server_version: "0.1.0+ab".into() }.encode(),
            ),
            ("feedback_result", ServerMsg::FeedbackResult { id: 300, code: String::new(), message: "Thanks!".into() }.encode()),
            (
                "feedback_refused",
                ServerMsg::FeedbackResult { id: 0, code: "feedback_too_soon".into(), message: "Wait.".into() }.encode(),
            ),
            ("feedback_req", ClientMsg::Feedback { draft }.encode()),
        ]
    }

    const FEEDBACK_EXPECTED: &[(&str, &str)] = &[
        ("connected_version", "10090000000000000000000000000000123408302e312e302b6162"),
        ("feedback_result", "62ac0200075468616e6b7321"),
        ("feedback_refused", "620011666565646261636b5f746f6f5f736f6f6e05576169742e"),
        ("feedback_req", "5c03627567046869676805537475636b114c696e65206f6e650a4c696e652074776f04302e3237025541"),
    ];

    #[test]
    fn feedback_frames_round_trip_and_match_the_dart_goldens() {
        for (name, b) in feedback() {
            if name == "feedback_req" {
                let m = ClientMsg::decode(&b).unwrap();
                assert_eq!(m.encode(), b, "{name}");
                assert_eq!(ClientMsg::decode(&b[..b.len() - 1]), Err(ClientDecodeError::Malformed), "truncated {name}");
            } else {
                let m = ServerMsg::decode(&b).unwrap();
                assert_eq!(m.encode(), b, "{name}");
                assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none(), "truncated {name}");
            }
        }
        let got: Vec<(&str, String)> = feedback().into_iter().map(|(n, b)| (n, hex(&b))).collect();
        let want: Vec<(&str, String)> = FEEDBACK_EXPECTED.iter().map(|(n, h)| (*n, h.to_string())).collect();
        assert_eq!(got, want);
    }

    /// Mount frames (0x90 PLAYER_MOUNT, 0x91 MOUNT_REQ). Change these only
    /// together with `client_test.dart` (`mount goldens`).
    #[test]
    fn mount_frames_match_the_dart_goldens() {
        let b = ServerMsg::PlayerMount { entity_id: 300, mount: mount::PECO, speed: 25 }.encode();
        assert_eq!(hex(&b), "90ac020119");
        assert_eq!(ServerMsg::decode(&b).unwrap().encode(), b);
        assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none());
        let req = ClientMsg::Mount { on: false }.encode();
        assert_eq!(hex(&req), "9100");
        assert_eq!(ClientMsg::decode(&req), Ok(ClientMsg::Mount { on: false }));
        assert_eq!(ClientMsg::decode(&[kind::MOUNT_REQ]), Err(ClientDecodeError::Malformed));
    }

    /// Disconnect frames (0x73 PLAYER_LINK, 0x75 RESUMABLE, 0x5F
    /// STAGE_RESUME). Change these only together with `client_test.dart`
    /// (`link goldens`).
    #[test]
    fn link_frames_round_trip_and_match_the_dart_goldens() {
        let server = [
            ("player_link", ServerMsg::PlayerLink { entity_id: 300, online: false }.encode(), "73ac0200"),
            (
                "resumable",
                ServerMsg::Resumable { stage_id: 7, mission: "Poring".into(), left_ms: 45_000 }.encode(),
                "750706506f72696e67c8df02",
            ),
        ];
        for (name, b, want) in server {
            assert_eq!(hex(&b), want, "{name}");
            assert_eq!(ServerMsg::decode(&b).unwrap().encode(), b, "{name}");
            assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none(), "truncated {name}");
        }
        let resume = ClientMsg::StageResume { accept: true }.encode();
        assert_eq!(hex(&resume), "5f01");
        assert_eq!(ClientMsg::decode(&resume), Ok(ClientMsg::StageResume { accept: true }));
        assert_eq!(ClientMsg::decode(&[kind::STAGE_RESUME]), Err(ClientDecodeError::Malformed));
    }

    /// 0xA5 MONSTER_HP. Change this only together with `client_test.dart`
    /// (`monster hp golden`).
    #[test]
    fn monster_hp_matches_the_dart_golden() {
        let b = ServerMsg::MonsterHp { bars: vec![(MONSTER_ENTITY_BASE + 7, 255), (MONSTER_ENTITY_BASE + 300, 0)] }.encode();
        assert_eq!(hex(&b), "a50287808004ff ac828004 00".replace(' ', ""));
        assert_eq!(ServerMsg::decode(&b).unwrap().encode(), b);
        assert!(ServerMsg::decode(&b[..b.len() - 1]).is_none());
    }
}
