import 'dart:convert';
import 'dart:math' as math;
import 'dart:typed_data';

// Binary wire protocol, mirror of the server's `world/wire.rs` and
// `world/protocol.rs` (the layouts are documented there). Every frame starts
// with a one-byte kind (0xFF escapes to a two-byte kind); fields are
// little-endian, `var` is an unsigned LEB128 varint, `str` is a varint
// length plus UTF-8, `code` is an index into [wireCodes] (0xFF + str for
// anything else), `pos` is u16 cells in 1/64 fixed point.

/// Error codes and reasons sent as one byte; same order as the server.
const wireCodes = <String>[
  'bad_message',
  'hub_unavailable',
  'no_character',
  'hub_busy',
  'not_in_hub',
  'no_such_npc',
  'npc_too_far',
  'unknown_mission',
  'evicted',
  'requested',
  'replaced',
  'kicked',
  'launched',
  'left',
  'disbanded',
  'disconnected',
  'stage_requires_mission',
  'already_in_lobby',
  'not_in_lobby',
  'no_such_lobby',
  'lobby_full',
  'not_leader',
  'not_ready',
  'stage_full',
  'mission_unavailable',
  'no_such_member',
  'stage_busy',
  'stage_gone',
  'channel_full',
  'hub_full',
  'no_such_channel',
  'not_in_stage',
];

/// Message kinds (first byte).
abstract final class Kind {
  static const hubSnapshot = 0x01;
  static const input = 0x02;
  static const stageSnapshot = 0x03;
  static const connected = 0x10;
  static const pong = 0x11;
  static const displaced = 0x12;
  static const notification = 0x13;
  static const hubJoined = 0x20;
  static const mapEnter = 0x21;
  static const playerAppeared = 0x22;
  static const playerVanished = 0x23;
  static const portalDenied = 0x24;
  static const hubChannels = 0x25;
  static const npcDialog = 0x26;
  static const hubLeft = 0x27;
  static const hubError = 0x28;
  static const playerLook = 0x29;
  static const playerVitals = 0x2A;
  static const combat = 0x2B;
  static const progress = 0x2C;
  static const skills = 0x2D;
  static const cast = 0x2E;
  static const zoneEnd = 0x2F;
  static const stageJoined = 0x30;
  static const lobbyState = 0x31;
  static const lobbyLeft = 0x32;
  static const lobbyError = 0x33;
  static const stageEnd = 0x34;
  static const boss = 0x35;
  static const buffs = 0x36;

  static const ping = 0x40;
  static const hubJoin = 0x41;
  static const hubLeave = 0x42;
  static const hubChannelsRequest = 0x43;
  static const npcInteract = 0x44;
  static const missionSelect = 0x45;
  static const lobbyJoin = 0x46;
  static const lobbyReady = 0x47;
  static const lobbyStart = 0x48;
  static const lobbyLeave = 0x49;
  static const lobbyKick = 0x4A;
  static const stageLeave = 0x4B;
  static const resync = 0x4C;
  static const styleSet = 0x4D;
  static const skillLearn = 0x4E;
  static const skillReset = 0x4F;
  static const skillSlot = 0x50;

  static const extended = 0xFF;
}

/// Thrown by [WireReader] on a truncated or malformed field.
final class WireFormatError implements Exception {
  const WireFormatError(this.message);
  final String message;
  @override
  String toString() => 'WireFormatError: $message';
}

/// Cursor over one frame.
final class WireReader {
  WireReader(this.bytes) : _d = ByteData.sublistView(bytes);

  final Uint8List bytes;
  final ByteData _d;
  int offset = 0;

  int get remaining => bytes.length - offset;
  bool get isEmpty => offset >= bytes.length;

  void _need(int n) {
    if (offset + n > bytes.length) throw const WireFormatError('truncated');
  }

  int u8() {
    _need(1);
    return bytes[offset++];
  }

  int i8() {
    _need(1);
    return _d.getInt8(offset++);
  }

  int u16() {
    _need(2);
    final v = _d.getUint16(offset, Endian.little);
    offset += 2;
    return v;
  }

  int u32() {
    _need(4);
    final v = _d.getUint32(offset, Endian.little);
    offset += 4;
    return v;
  }

  double f32() {
    _need(4);
    final v = _d.getFloat32(offset, Endian.little);
    offset += 4;
    return v;
  }

  /// Varint up to 2^53 (exact in a double under dart2js).
  int varint() {
    var v = 0;
    var mul = 1;
    for (var i = 0; i < 8; i++) {
      final b = u8();
      v += (b & 0x7F) * mul;
      if (b < 0x80) return v;
      mul *= 128;
    }
    throw const WireFormatError('varint too long');
  }

  /// Zigzag varint (to ±2^52).
  int zigzag() {
    final v = varint();
    return v.isEven ? v ~/ 2 : -(v + 1) ~/ 2;
  }

  Uint8List take(int n) {
    _need(n);
    final s = Uint8List.sublistView(bytes, offset, offset + n);
    offset += n;
    return s;
  }

  String str() => utf8.decode(take(varint()));

  String? optStr() {
    final n = varint();
    return n == 0 ? null : utf8.decode(take(n - 1));
  }

  String code() {
    final i = u8();
    if (i == 0xFF) return str();
    if (i >= wireCodes.length) throw const WireFormatError('unknown code');
    return wireCodes[i];
  }

  double pos() => u16() / 64;

  bool boolean() => switch (u8()) {
    0 => false,
    1 => true,
    _ => throw const WireFormatError('bad bool'),
  };

  List<T> list<T>(T Function(WireReader r) item) {
    final n = varint();
    if (n > remaining) throw const WireFormatError('list longer than the frame');
    return [for (var i = 0; i < n; i++) item(this)];
  }
}

/// Growable frame builder.
final class WireWriter {
  WireWriter(int kind) {
    u8(kind);
  }

  Uint8List _b = Uint8List(32);
  int _len = 0;

  void _grow(int n) {
    if (_len + n <= _b.length) return;
    _b = Uint8List(math.max(_b.length * 2, _len + n))..setRange(0, _len, _b);
  }

  void u8(int v) {
    _grow(1);
    _b[_len++] = v;
  }

  void varint(int v) {
    while (v >= 0x80) {
      u8((v & 0x7F) | 0x80);
      v = v ~/ 128;
    }
    u8(v);
  }

  void str(String s) {
    final bytes = utf8.encode(s);
    varint(bytes.length);
    _grow(bytes.length);
    _b.setRange(_len, _len + bytes.length, bytes);
    _len += bytes.length;
  }

  void boolean(bool v) => u8(v ? 1 : 0);

  void look(Look l) {
    u8(l.gender);
    u8(l.hair);
    u8(l.hairColor);
    u8(l.bodyColor);
  }

  Uint8List done() => Uint8List.sublistView(_b, 0, _len);
}

// ------------------------------------------------------------ client → server

Uint8List _bare(int kind) => Uint8List.fromList([kind]);

abstract final class ClientMsg {
  static Uint8List ping() => _bare(Kind.ping);

  /// `channel` null lets the server pick.
  static Uint8List hubJoin([int? channel]) =>
      (WireWriter(Kind.hubJoin)..varint(channel == null ? 0 : channel + 1)).done();
  static Uint8List hubLeave() => _bare(Kind.hubLeave);
  static Uint8List hubChannels() => _bare(Kind.hubChannelsRequest);
  static Uint8List npcInteract(String npc) => (WireWriter(Kind.npcInteract)..str(npc)).done();
  static Uint8List missionSelect(String mission, {required bool public}) =>
      (WireWriter(Kind.missionSelect)
            ..str(mission)
            ..boolean(public))
          .done();
  static Uint8List lobbyJoin(String code) => (WireWriter(Kind.lobbyJoin)..str(code)).done();
  static Uint8List lobbyReady(bool ready) => (WireWriter(Kind.lobbyReady)..boolean(ready)).done();
  static Uint8List lobbyStart() => _bare(Kind.lobbyStart);
  static Uint8List lobbyLeave() => _bare(Kind.lobbyLeave);
  static Uint8List lobbyKick(int entityId) => (WireWriter(Kind.lobbyKick)..varint(entityId)).done();
  static Uint8List stageLeave() => _bare(Kind.stageLeave);

  /// The last monster delta did not apply: ask for a keyframe.
  static Uint8List resync() => _bare(Kind.resync);

  /// Restyle at a stylist NPC (the server keeps the gender).
  static Uint8List styleSet(Look look) => (WireWriter(Kind.styleSet)..look(look)).done();

  /// Spend a skill point on [skill].
  static Uint8List skillLearn(String skill) => (WireWriter(Kind.skillLearn)..str(skill)).done();

  /// Every skill point back (in town only).
  static Uint8List skillReset() => _bare(Kind.skillReset);

  /// Put a learned active skill in a hotbar slot ('' clears it).
  static Uint8List skillSlot(int slot, String skill) =>
      (WireWriter(Kind.skillSlot)
            ..u8(slot)
            ..str(skill))
          .done();
}

/// Character appearance: gender (0 male, 1 female), hair style (1..12),
/// hair dye (0..15) and clothes dye (0..7). The engine draws it from the
/// customization pack (`GameView.setLook`).
final class Look {
  const Look({this.gender = 0, this.hair = 1, this.hairColor = 0, this.bodyColor = 0});

  /// From the REST character JSON (`look: {gender, hair_style, hair_color, body_color}`).
  factory Look.fromJson(Object? j) {
    if (j is! Map) return const Look();
    int v(String k, int d) => (j[k] as num?)?.toInt() ?? d;
    return Look(gender: v('gender', 0), hair: v('hair_style', 1), hairColor: v('hair_color', 0), bodyColor: v('body_color', 0));
  }

  final int gender;
  final int hair;
  final int hairColor;
  final int bodyColor;

  Look copyWith({int? hair, int? hairColor, int? bodyColor}) =>
      Look(gender: gender, hair: hair ?? this.hair, hairColor: hairColor ?? this.hairColor, bodyColor: bodyColor ?? this.bodyColor);

  Map<String, int> toUi() => {'gender': gender, 'hair': hair, 'hairColor': hairColor, 'bodyColor': bodyColor};

  static Look _read(WireReader r) => Look(gender: r.u8(), hair: r.u8(), hairColor: r.u8(), bodyColor: r.u8());

  @override
  bool operator ==(Object other) =>
      other is Look && other.gender == gender && other.hair == hair && other.hairColor == hairColor && other.bodyColor == bodyColor;

  @override
  int get hashCode => Object.hash(gender, hair, hairColor, bodyColor);

  @override
  String toString() => 'Look($gender, $hair, $hairColor, $bodyColor)';
}

/// Input (client → server): `u8 kind=0x02, u32 seq, i16 dx, i16 dy` with the
/// direction scaled by 32767, then `i8 aimX, i8 aimY` (from the player in
/// 1/8 m, 0,0 = no aim) and `u16 slots` (skill slots held, bit i = slot i).
Uint8List encodeInput(int seq, double dx, double dy, {int aimX = 0, int aimY = 0, int slots = 0}) {
  final d = ByteData(13)
    ..setUint8(0, Kind.input)
    ..setUint32(1, seq, Endian.little)
    ..setInt16(5, (dx.clamp(-1.0, 1.0) * 32767).round(), Endian.little)
    ..setInt16(7, (dy.clamp(-1.0, 1.0) * 32767).round(), Endian.little)
    ..setInt8(9, aimX.clamp(-127, 127))
    ..setInt8(10, aimY.clamp(-127, 127))
    ..setUint16(11, slots & 0xFFFF, Endian.little);
  return d.buffer.asUint8List();
}

/// Inverse of the wire quantisation, so prediction uses exactly the
/// direction the server will see.
(double, double) quantizeDir(double dx, double dy) =>
    ((dx.clamp(-1.0, 1.0) * 32767).round() / 32767, (dy.clamp(-1.0, 1.0) * 32767).round() / 32767);

/// 8-way facing from a direction, 0 = north (+y), clockwise (same as server).
int facingOf(double dx, double dy) {
  final step = (math.atan2(dx, dy) / (math.pi / 4)).round();
  return ((step % 8) + 8) % 8;
}

// ------------------------------------------------------------ server → client

final class MapInfo {
  MapInfo._(this.id, this.name, this.kind, this.width, this.height, this.cellSize, this.camera, this.bgm);

  final String id;
  final String name;

  /// `hub`, `indoor` or `field`.
  final String kind;
  final int width;
  final int height;
  final double cellSize;

  /// Camera constraint in the engine's JSON shape (`setCamera`).
  final Map<String, Object?> camera;
  final String? bgm;

  static MapInfo _read(WireReader r) {
    final id = r.str(), name = r.str();
    final kind = switch (r.u8()) {
      0 => 'hub',
      1 => 'indoor',
      2 => 'field',
      _ => throw const WireFormatError('location kind'),
    };
    final width = r.varint(), height = r.varint(), cell = r.f32();
    final camera = switch (r.u8()) {
      0 => <String, Object?>{
        'type': 'free',
        'pitch': r.f32(),
        'min_zoom': r.f32(),
        'max_zoom': r.f32(),
        'default_zoom': r.f32(),
      },
      1 => <String, Object?>{'type': 'fixed', 'yaw': r.f32(), 'pitch': r.f32(), 'zoom': r.f32()},
      _ => throw const WireFormatError('camera'),
    };
    return MapInfo._(id, name, kind, width, height, cell, camera, r.optStr());
  }
}

final class PlayerInfo {
  PlayerInfo._(this.entityId, this.name, this.archetype, this.x, this.y, this.facing, this.look, this.level);
  final int entityId;
  final String name;
  final String archetype;
  final double x;
  final double y;
  final int facing;
  final Look look;

  /// Character level (its tier picks the body).
  final int level;

  static PlayerInfo _read(WireReader r) =>
      PlayerInfo._(r.varint(), r.str(), r.str(), r.pos(), r.pos(), r.u8(), Look._read(r), r.varint());
}

final class MissionInfo {
  MissionInfo._(this.id, this.name, this.description, this.map, this.maxPlayers);
  final String id;
  final String name;
  final String description;
  final String map;
  final int maxPlayers;

  static MissionInfo _read(WireReader r) => MissionInfo._(r.str(), r.str(), r.str(), r.str(), r.u8());
}

final class LobbyMember {
  LobbyMember._(this.entityId, this.name, this.archetype, this.ready, this.leader);
  final int entityId;
  final String name;
  final String archetype;
  final bool ready;
  final bool leader;
}

final class LobbyInfo {
  LobbyInfo._(this.lobbyId, this.code, this.mission, this.public, this.members);
  final int lobbyId;
  final String code;
  final MissionInfo mission;
  final bool public;
  final List<LobbyMember> members;
}

final class ChannelInfo {
  ChannelInfo._(this.id, this.name, this.population, this.capacity);
  final int id;
  final String name;
  final int population;
  final int capacity;
}

/// Server → client control messages ([ServerMsg.decode]).
sealed class ServerMsg {
  const ServerMsg();

  /// Null for snapshots and unknown kinds; throws [WireFormatError] on a
  /// malformed known message.
  static ServerMsg? decode(Uint8List frame) {
    final r = WireReader(frame);
    final kind = r.u8();
    final ServerMsg? msg = switch (kind) {
      Kind.connected => Connected(r.varint(), r.take(16)),
      Kind.pong => Pong(r.zigzag()),
      Kind.displaced => Displaced(r.str()),
      Kind.notification => Notification(r.str()),
      Kind.hubJoined => HubJoined(r.varint(), r.str(), r.varint(), r.varint()),
      Kind.mapEnter => MapEnter(MapInfo._read(r), r.u16(), r.pos(), r.pos(), r.optStr(), r.list(PlayerInfo._read)),
      Kind.playerAppeared => PlayerAppeared(PlayerInfo._read(r)),
      Kind.playerVanished => PlayerVanished(r.varint()),
      Kind.portalDenied => PortalDenied(r.str(), r.str(), r.code()),
      Kind.hubChannels => HubChannels(r.list((r) => ChannelInfo._(r.varint(), r.str(), r.varint(), r.varint()))),
      Kind.npcDialog => NpcDialog(r.str(), r.str(), r.str(), r.list(MissionInfo._read)),
      Kind.hubLeft => HubLeft(r.code()),
      Kind.hubError => HubError(r.code(), r.str()),
      Kind.playerLook => PlayerLook(r.varint(), Look._read(r)),
      Kind.playerVitals => PlayerVitals(
        r.varint(),
        r.varint(),
        r.varint(),
        r.varint(),
        r.varint(),
        r.u8(),
        r.varint(),
        r.varint(),
        r.varint(),
        r.varint(),
      ),
      Kind.combat => Combat(
        r.list((r) => Swing._(r.varint(), r.u8())),
        r.list((r) => Hit._(r.varint(), r.varint(), r.u8())),
      ),
      Kind.progress => Progress(r.varint(), r.varint(), r.varint(), r.varint()),
      Kind.skills => Skills(
        r.varint(),
        r.varint(),
        {for (final (id, lvl) in r.list((r) => (r.str(), r.u8()))) id: lvl},
        r.list((r) => r.str()),
      ),
      Kind.cast => Cast(r.varint(), r.str(), r.pos(), r.pos(), r.varint(), r.varint(), r.list(Telegraph._read), r.u8()),
      Kind.zoneEnd => ZoneEnd(r.varint()),
      Kind.stageEnd => StageEnd(r.boolean(), r.varint(), r.varint(), r.varint(), r.varint()),
      Kind.boss => Boss(r.varint(), r.str(), r.varint(), r.varint()),
      Kind.buffs => Buffs([for (final (id, ms) in r.list((r) => (r.str(), r.varint()))) (skill: id, ms: ms)]),
      Kind.stageJoined => StageJoined(r.varint(), r.str(), r.varint(), r.varint(), r.list((r) => r.str()), r.f32()),
      Kind.lobbyState => LobbyState(
        LobbyInfo._(
          r.varint(),
          r.str(),
          MissionInfo._read(r),
          r.boolean(),
          r.list((r) {
            final id = r.varint(), name = r.str(), archetype = r.str(), flags = r.u8();
            return LobbyMember._(id, name, archetype, flags & 1 != 0, flags & 2 != 0);
          }),
        ),
      ),
      Kind.lobbyLeft => LobbyLeft(r.code()),
      Kind.lobbyError => LobbyError(r.code(), r.str()),
      _ => null,
    };
    if (msg == null) return null;
    if (!r.isEmpty) throw const WireFormatError('trailing bytes');
    return msg;
  }
}

final class Connected extends ServerMsg {
  const Connected(this.userId, this.sessionId);
  final int userId;
  final Uint8List sessionId;
}

final class Pong extends ServerMsg {
  const Pong(this.ts);
  final int ts;
}

final class Displaced extends ServerMsg {
  const Displaced(this.reason);
  final String reason;
}

final class Notification extends ServerMsg {
  const Notification(this.message);
  final String message;
}

final class HubJoined extends ServerMsg {
  const HubJoined(this.channel, this.channelName, this.tickRate, this.entityId);
  final int channel;
  final String channelName;
  final int tickRate;
  final int entityId;
}

final class MapEnter extends ServerMsg {
  const MapEnter(this.map, this.mapEpoch, this.x, this.y, this.viaPortal, this.players);
  final MapInfo map;
  final int mapEpoch;
  final double x;
  final double y;
  final String? viaPortal;
  final List<PlayerInfo> players;
}

final class PlayerAppeared extends ServerMsg {
  const PlayerAppeared(this.player);
  final PlayerInfo player;
}

final class PlayerVanished extends ServerMsg {
  const PlayerVanished(this.entityId);
  final int entityId;
}

final class PortalDenied extends ServerMsg {
  const PortalDenied(this.portal, this.targetMap, this.reason);
  final String portal;
  final String targetMap;
  final String reason;
}

final class HubChannels extends ServerMsg {
  const HubChannels(this.channels);
  final List<ChannelInfo> channels;
}

final class NpcDialog extends ServerMsg {
  const NpcDialog(this.npc, this.name, this.kind, this.missions);
  final String npc;
  final String name;
  final String kind;
  final List<MissionInfo> missions;
}

final class HubLeft extends ServerMsg {
  const HubLeft(this.reason);
  final String reason;
}

final class HubError extends ServerMsg {
  const HubError(this.code, this.message);
  final String code;
  final String message;
}

/// A player on the map (possibly this one) restyled.
final class PlayerLook extends ServerMsg {
  const PlayerLook(this.entityId, this.look);
  final int entityId;
  final Look look;
}

/// Health and spell points of a player on the map (stages).
final class PlayerVitals extends ServerMsg {
  const PlayerVitals(
    this.entityId,
    this.hp,
    this.maxHp,
    this.sp,
    this.maxSp,
    this.state, [
    this.status = 0,
    this.tempHp = 0,
    this.light = 0,
    this.level = 0,
  ]);
  final int entityId;
  final int hp;
  final int maxHp;
  final int sp;
  final int maxSp;

  /// 0 up, 1 downed (an ally can revive), 2 dead (back at the next wave).
  final int state;

  /// Condition bits ([MobStatus], as on monsters).
  final int status;
  final int tempHp;

  /// Extra light radius from buffs, cells.
  final int light;
  final int level;
}

/// Entity id of a monster in combat events (= its engine id).
const monsterEntityBase = 0x800000;

/// An attack animation starts.
final class Swing {
  const Swing._(this.entityId, this.facing);
  final int entityId;
  final int facing;
}

/// A number over `target`: damage, heal or a miss.
final class Hit {
  const Hit._(this.target, this.amount, this.flags);
  final int target;
  final int amount;
  final int flags;

  static const critical = 1;
  static const miss = 2;
  static const kill = 4;
  static const heal = 8;

  bool get isCritical => flags & critical != 0;
  /// Blocked by a ward, a shield or a ranged ward (MISS and HEAL together).
  bool get isGuard => flags & (miss | heal) == (miss | heal);
  bool get isMiss => flags & miss != 0 && !isGuard;
  bool get isKill => flags & kill != 0;
  bool get isHeal => flags & heal != 0 && !isGuard;

  /// Damage type (server `DamageType` order): 0 physical, 1 fire, 2 cold,
  /// 3 lightning, 4 earth, 5 poison, 6 radiant, 7 necrotic, 8 force.
  int get damageType => flags >> 4;
}

/// Attacks and numbers of one server tick on the map.
final class Combat extends ServerMsg {
  const Combat(this.swings, this.hits);
  final List<Swing> swings;
  final List<Hit> hits;
}

/// This character's level and experience into it.
final class Progress extends ServerMsg {
  const Progress(this.level, this.exp, this.expToNext, this.gained);
  final int level;
  final int exp;
  final int expToNext;
  final int gained;
}

/// The local character's learned skills, unspent points and hotbar.
final class Skills extends ServerMsg {
  const Skills(this.level, this.points, this.levels, this.hotbar);

  /// Character level (gates of the tree).
  final int level;
  final int points;
  final Map<String, int> levels;

  /// Skill id per slot; '' = free.
  final List<String> hotbar;
}

/// A player starts a skill aimed at [x], [y] (cells): wind-up [castMs], then
/// [cooldownMs]; [areas] are where its effects land, as the server resolved
/// them. An empty [skill] cancels the caster's wind-up.
final class Cast extends ServerMsg {
  const Cast(this.caster, this.skill, this.x, this.y, this.castMs, this.cooldownMs, [this.areas = const [], this.level = 1]);
  final int caster;
  final String skill;
  final double x;
  final double y;
  final int castMs;
  final int cooldownMs;
  final List<Telegraph> areas;

  /// Skill level (1 for monsters; 0 on a cancel).
  final int level;
  bool get isCancel => skill.isEmpty;
}

/// A skill area on the ground (cells).
final class Telegraph {
  const Telegraph(this.id, this.shape, this.look, this.x, this.y, this.dir, this.a, this.b, this.spanMs, this.follow);

  static Telegraph _read(WireReader r) =>
      Telegraph(r.varint(), r.u8(), r.u8(), r.pos(), r.pos(), r.u8(), r.pos(), r.pos(), r.varint(), r.varint());

  /// Lasting ground (ended early by [ZoneEnd]); 0 = a one-off strike.
  final int id;

  /// 0 circle ([a] radius), 1 box ([a] along [dir] × [b] across, centred on
  /// [x], [y]), 2 cone (apex [x], [y], [a] radius, [b] degrees).
  final int shape;

  /// 0 damage, 1 heal, 2 fire, 3 swamp, 4 ward, 5 cold, 6 lightning, 7 holy,
  /// 8 trap, 9 taunt, 10 barrier, 11 buff, 12 earth, 13 curse.
  final int look;
  final double x;
  final double y;

  /// 1/256 turns, counter-clockwise from +x.
  final int dir;
  final double a;
  final double b;
  final int spanMs;

  /// Entity it moves with (0 = none).
  final int follow;

  Map<String, Object> toJson() =>
      {'id': id, 'shape': shape, 'look': look, 'x': x, 'y': y, 'dir': dir, 'a': a, 'b': b, 'spanMs': spanMs, 'follow': follow};
}

/// Ground [id] (a ward, a trap) ended before its time.
final class ZoneEnd extends ServerMsg {
  const ZoneEnd(this.id);
  final int id;
}

/// The mission is over: the result screen; everyone leaves after [leaveMs].
final class StageEnd extends ServerMsg {
  const StageEnd(this.victory, this.secs, this.kills, this.exp, this.leaveMs);
  final bool victory;
  final int secs;
  final int kills;
  final int exp;
  final int leaveMs;
}

/// The boss's bar ([hp] 0: gone).
final class Boss extends ServerMsg {
  const Boss(this.entityId, this.name, this.hp, this.maxHp);
  final int entityId;
  final String name;
  final int hp;
  final int maxHp;
}

/// Own running buffs: skill id and ms left (0 = until toggled off).
final class Buffs extends ServerMsg {
  const Buffs(this.buffs);
  final List<({String skill, int ms})> buffs;
}

final class StageJoined extends ServerMsg {
  const StageJoined(this.stageId, this.mission, this.tickRate, this.entityId, this.monsters, this.lightRadius);
  final int stageId;
  final String mission;
  final int tickRate;
  final int entityId;

  /// Monster kind ids; a monster's `kind` byte indexes this list.
  final List<String> monsters;

  /// Cells; 0 when the stage has no monsters.
  final double lightRadius;
}

final class LobbyState extends ServerMsg {
  const LobbyState(this.lobby);
  final LobbyInfo lobby;
}

final class LobbyLeft extends ServerMsg {
  const LobbyLeft(this.reason);
  final String reason;
}

final class LobbyError extends ServerMsg {
  const LobbyError(this.code, this.message);
  final String code;
  final String message;
}

// ------------------------------------------------------------ snapshots

/// Monster delta ops (`horde/view.rs` on the server).
abstract final class MobOp {
  /// `x`/`y` are deltas (1/64 cell) from the known position.
  static const nudge = 0;

  /// `x`/`y` are absolute (1/64 cell).
  static const move = 1;
  static const spawn = 2;
  static const remove = 3;
}

/// Monster conditions (server `combat::Condition` bits).
abstract final class MobStatus {
  static const slowed = 1;
  static const frozen = 2;
  static const stunned = 4;
  static const blinded = 8;
  static const rooted = 16;
  static const petrified = 32;
  static const burning = 64;
  static const poisoned = 128;

  /// −2 AC in a swamp (drawn as [slowed], which comes with it).
  static const mired = 256;

  /// Hunts whoever provoked it.
  static const taunted = 512;

  /// Cannot move or act: the animation holds.
  static const held = frozen | stunned | petrified;
}

/// Dead reckoning (server `horde::view`): a record `(p, v, stamp)` is drawn
/// at `p + v · clamp(t − stamp, −back, horizon)`. Positions are 1/64 cell,
/// velocities 1/256 cell per tick.
abstract final class Reckoning {
  static const posOne = 64.0;
  static const velOne = 256.0;
  static const horizon = 16;
  static const back = 8;
}

/// Binary hub snapshot (`kind=0x01`): `u16 map_epoch, u32 tick,
/// u32 ack_seq, u16 count`, then `count × {u32 entity_id, u16 x, u16 y,
/// u8 facing}`. Stage snapshots (`kind=0x03`) append the monster section:
/// `var baseline` (0 = keyframe, else `tick − baseline` is the frame it
/// applies to), `var count`, then entries in slot order with
/// `var head = gap << 3 | s << 2 | op` (see [MobOp]); `s = 1` puts a `var`
/// status ([MobStatus] bits) right after the head.
final class Snapshot {
  Snapshot._(this.mapEpoch, this.tick, this.ackSeq, this.ids, this.xs, this.ys, this.facings);

  static const int headerLength = 13;
  static const int entryLength = 9;

  static bool isSnapshot(int firstByte) => firstByte == Kind.hubSnapshot || firstByte == Kind.stageSnapshot;

  final int mapEpoch;
  final int tick;
  final int ackSeq;
  final Uint32List ids;
  final Float32List xs;
  final Float32List ys;
  final Uint8List facings;

  /// Monster section: -1 in hub snapshots, 0 for a keyframe, else how many
  /// ticks back the frame it is a delta against is.
  int mobBaseline = -1;

  /// Monster entries (parallel arrays, slot order). For [MobOp.spawn]
  /// `mobGenerations`/`mobKinds` are set; `mobXs`/`mobYs` are raw 1/64-cell
  /// values (deltas for [MobOp.nudge]); velocities in 1/256 cell per tick.
  Uint8List mobOps = Uint8List(0);
  Uint16List mobSlots = Uint16List(0);
  Uint8List mobGenerations = Uint8List(0);
  Uint8List mobKinds = Uint8List(0);
  Int32List mobXs = Int32List(0);
  Int32List mobYs = Int32List(0);
  Int8List mobVxs = Int8List(0);
  Int8List mobVys = Int8List(0);

  /// [MobStatus] bits when the entry carries them, else -1 (unchanged).
  Int32List mobStatuses = Int32List(0);

  int get length => ids.length;
  int get monsterCount => mobOps.length;

  /// Returns null for anything that is not a well-formed snapshot.
  static Snapshot? decode(Uint8List b) {
    if (b.length < headerLength || !isSnapshot(b[0])) return null;
    try {
      final r = WireReader(b)..offset = 1;
      final mapEpoch = r.u16(), tick = r.u32(), ackSeq = r.u32(), count = r.u16();
      if (r.remaining < count * entryLength) return null;
      final ids = Uint32List(count);
      final xs = Float32List(count);
      final ys = Float32List(count);
      final facings = Uint8List(count);
      for (var i = 0; i < count; i++) {
        ids[i] = r.u32();
        xs[i] = r.pos();
        ys[i] = r.pos();
        facings[i] = r.u8();
      }
      final s = Snapshot._(mapEpoch, tick, ackSeq, ids, xs, ys, facings);
      if (b[0] == Kind.stageSnapshot) s._readMonsters(r);
      return r.isEmpty ? s : null;
    } on WireFormatError {
      return null;
    }
  }

  void _readMonsters(WireReader r) {
    mobBaseline = r.varint();
    final n = r.varint();
    if (n > r.remaining) throw const WireFormatError('monster count');
    mobOps = Uint8List(n);
    mobSlots = Uint16List(n);
    mobGenerations = Uint8List(n);
    mobKinds = Uint8List(n);
    mobXs = Int32List(n);
    mobYs = Int32List(n);
    mobVxs = Int8List(n);
    mobVys = Int8List(n);
    mobStatuses = Int32List(n);
    var prev = -1;
    for (var i = 0; i < n; i++) {
      final head = r.varint();
      final slot = prev + 1 + (head >> 3);
      if (slot > 0xFFFF) throw const WireFormatError('slot');
      prev = slot;
      final op = head & 3;
      mobOps[i] = op;
      mobSlots[i] = slot;
      if (head & 4 != 0) {
        if (op == MobOp.remove) throw const WireFormatError('status on remove');
        final status = r.varint();
        if (status > 0xFFFF) throw const WireFormatError('status');
        mobStatuses[i] = status;
      } else {
        mobStatuses[i] = -1;
      }
      switch (op) {
        case MobOp.nudge:
          mobXs[i] = r.i8();
          mobYs[i] = r.i8();
        case MobOp.move:
          mobXs[i] = r.u16();
          mobYs[i] = r.u16();
        case MobOp.spawn:
          mobGenerations[i] = r.u8();
          mobKinds[i] = r.u8();
          mobXs[i] = r.u16();
          mobYs[i] = r.u16();
        case MobOp.remove:
          continue;
      }
      mobVxs[i] = r.i8();
      mobVys[i] = r.i8();
    }
  }
}
