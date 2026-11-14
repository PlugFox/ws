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
  static const emote = 0x37;
  static const pose = 0x38;
  static const flag = 0x39;
  static const inventory = 0x3A;
  static const itemDefs = 0x3B;
  static const shop = 0x3C;
  static const itemUsed = 0x3D;
  static const playerGear = 0x3E;
  static const loot = 0x3F;
  static const combo = 0x60;
  static const revive = 0x61;
  static const bonus = 0x68;
  static const missionInfo = 0x6C;
  static const stageClock = 0x6D;
  static const fog = 0x6E;
  static const partyInvite = 0x70;
  static const playerInspect = 0x71;
  static const chat = 0x7A;
  static const friends = 0x78;
  static const friendNote = 0x79;
  static const trade = 0x74;
  static const bossCast = 0x6A;
  static const bossZones = 0x6B;
  static const bossShout = 0x6F;
  static const bossBar = 0x72;
  static const feedbackResult = 0x62;
  static const playerLink = 0x73;
  static const resumable = 0x75;
  static const modifierDefs = 0x7B;
  static const modifiers = 0x7C;
  static const runStats = 0x81;
  static const runRewards = 0x82;

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
  static const emoteReq = 0x51;
  static const poseReq = 0x52;
  static const flagReq = 0x53;
  static const shopBuy = 0x54;
  static const shopSell = 0x55;
  static const equip = 0x56;
  static const unequip = 0x57;
  static const itemUse = 0x58;
  static const partyReq = 0x59;
  static const inspectReq = 0x5A;
  static const chatReq = 0x5E;
  static const friendReq = 0x5D;
  static const tradeReq = 0x5B;
  static const feedbackReq = 0x5C;
  static const stageResume = 0x5F;
  static const lobbyModifiers = 0x77;
  static const missionStart = 0x63;

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

  /// A cell coordinate in 1/64 cell (the reader's `pos`).
  void pos(double v) {
    final q = (v * 64).round().clamp(0, 0xFFFF);
    u8(q & 0xFF);
    u8(q >> 8);
  }

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

  /// Quick start (server `lobby.rs` `quick_start`): alone, launches
  /// [mission] with these modifiers at once; a party leader starts the
  /// party's run (everyone ready).
  static Uint8List missionStart(String mission, {int tier = 0, int mask = 0}) =>
      (WireWriter(Kind.missionStart)
            ..str(mission)
            ..u8(tier)
            ..varint(mask))
          .done();

  /// Leader: the run's difficulty [tier] and curse [mask] (bit i = the
  /// modifier the server's table gives bit i).
  static Uint8List lobbyModifiers(int tier, int mask) =>
      (WireWriter(Kind.lobbyModifiers)
            ..u8(tier)
            ..varint(mask))
          .done();
  static Uint8List lobbyLeave() => _bare(Kind.lobbyLeave);
  static Uint8List lobbyKick(int entityId) => (WireWriter(Kind.lobbyKick)..varint(entityId)).done();
  static Uint8List stageLeave() => _bare(Kind.stageLeave);

  /// Back into the stage the character is link-dead in (true), or forfeit
  /// it (false); either way the server puts the player somewhere.
  static Uint8List stageResume({required bool accept}) => (WireWriter(Kind.stageResume)..boolean(accept)).done();

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

  /// Play emote [emote] (the RO emotion index) over the own character.
  static Uint8List emote(int emote) => (WireWriter(Kind.emoteReq)..u8(emote)).done();

  /// Sit down (true) or stand up (in town only).
  static Uint8List pose({required bool sitting}) => (WireWriter(Kind.poseReq)..u8(sitting ? 1 : 0)).done();

  /// A ping flag of kind [flag] (0..9) on cell ([x], [y]) for everyone on the map.
  static Uint8List flag(int flag, double x, double y) =>
      (WireWriter(Kind.flagReq)
            ..u8(flag)
            ..pos(x)
            ..pos(y))
          .done();

  /// Buy [count] of [item] from the merchant [npc] (standing at it).
  static Uint8List shopBuy(String npc, String item, int count) =>
      (WireWriter(Kind.shopBuy)
            ..str(npc)
            ..str(item)
            ..varint(count))
          .done();

  /// Sell [count] of [item] from the bag to the merchant [npc].
  static Uint8List shopSell(String npc, String item, int count) =>
      (WireWriter(Kind.shopSell)
            ..str(npc)
            ..str(item)
            ..varint(count))
          .done();

  /// Wear [item] from the bag (in town); [slot] picks an accessory slot,
  /// null = the item's own.
  static Uint8List equip(String item, [int? slot]) =>
      (WireWriter(Kind.equip)
            ..str(item)
            ..u8(slot ?? 0xFF))
          .done();

  /// Take off what is in equipment [slot] ([EquipSlot]; in town).
  static Uint8List unequip(int slot) => (WireWriter(Kind.unequip)..u8(slot)).done();

  /// Drink a potion (on a mission).
  static Uint8List itemUse(String item) => (WireWriter(Kind.itemUse)..str(item)).done();

  /// Invite the player [entityId] (on the map) into the own party: the
  /// leader, or a player in no party; they get a [PartyInvite].
  static Uint8List partyInvite(int entityId) =>
      (WireWriter(Kind.partyReq)
            ..u8(0)
            ..varint(entityId))
          .done();

  /// Answer the [PartyInvite] of [from] (the inviter's entity id).
  static Uint8List partyAnswer(int from, {required bool accept}) =>
      (WireWriter(Kind.partyReq)
            ..u8(1)
            ..varint(from)
            ..boolean(accept))
          .done();

  /// Ask for the worn gear of the player [entityId] ([PlayerInspect]).
  static Uint8List inspect(int entityId) => (WireWriter(Kind.inspectReq)..varint(entityId)).done();

  /// Say [text] to the players around (local chat; the server cleans it,
  /// cuts it at [chatMaxChars] and rate limits it).
  static Uint8List chat(String text) => (WireWriter(Kind.chatReq)..str(text)).done();

  /// Friends (`FRIEND`): ask for the list ([Friends] comes back).
  static Uint8List friendList() => (WireWriter(Kind.friendReq)..u8(0)).done();

  /// Ask the player [entityId] (on the map) to be friends; if they had
  /// asked first, you are friends at once.
  static Uint8List friendAdd(int entityId) =>
      (WireWriter(Kind.friendReq)
            ..u8(1)
            ..varint(entityId))
          .done();

  /// Ask the player whose character is called [name].
  static Uint8List friendAddName(String name) =>
      (WireWriter(Kind.friendReq)
            ..u8(2)
            ..str(name))
          .done();

  /// Accept / decline the request of [account].
  static Uint8List friendAccept(int account) =>
      (WireWriter(Kind.friendReq)
            ..u8(3)
            ..varint(account))
          .done();
  static Uint8List friendDecline(int account) =>
      (WireWriter(Kind.friendReq)
            ..u8(4)
            ..varint(account))
          .done();

  /// End the friendship with [account], or withdraw a request to them.
  static Uint8List friendRemove(int account) =>
      (WireWriter(Kind.friendReq)
            ..u8(5)
            ..varint(account))
          .done();

  /// Trade (`TRADE`, in town): ask the player [entityId] (near by).
  static Uint8List tradeRequest(int entityId) =>
      (WireWriter(Kind.tradeReq)
            ..u8(0)
            ..varint(entityId))
          .done();

  /// Answer the trade request to you.
  static Uint8List tradeRespond({required bool accept}) => (WireWriter(Kind.tradeReq)..u8(accept ? 1 : 2)).done();

  /// Close the trade window, or withdraw / turn down a request.
  static Uint8List tradeCancel() => (WireWriter(Kind.tradeReq)..u8(3)).done();

  /// Freeze the own offer ("OK"); any change unlocks both sides.
  static Uint8List tradeLock() => (WireWriter(Kind.tradeReq)..u8(4)).done();

  /// Both locked: agree to the exchange ("Trade").
  static Uint8List tradeConfirm() => (WireWriter(Kind.tradeReq)..u8(5)).done();

  /// Put [zeny] on the table (0 takes it off).
  static Uint8List tradeZeny(int zeny) =>
      (WireWriter(Kind.tradeReq)
            ..u8(6)
            ..varint(zeny))
          .done();

  /// Put [count] of [item] on the table (0 takes it off).
  static Uint8List tradeItem(String item, int count) =>
      (WireWriter(Kind.tradeReq)
            ..u8(7)
            ..str(item)
            ..varint(count))
          .done();

  /// A bug report or suggestion (`FEEDBACK`, at the Scribe or a Kafra):
  /// [kind] `bug` | `suggestion` | `balance` | `other`, [priority] `low` |
  /// `normal` | `high` | `critical`; the server checks the lengths
  /// ([FeedbackLimits]) and answers [FeedbackResult].
  static Uint8List feedback({
    required String kind,
    required String priority,
    required String title,
    required String body,
    required String clientVersion,
    required String userAgent,
  }) =>
      (WireWriter(Kind.feedbackReq)
            ..str(kind)
            ..str(priority)
            ..str(title)
            ..str(body)
            ..str(clientVersion)
            ..str(userAgent))
          .done();
}

/// Lengths the server accepts for feedback (server `crate::feedback`).
abstract final class FeedbackLimits {
  static const titleMin = 4;
  static const titleMax = 80;
  static const bodyMin = 20;
  static const bodyMax = 4000;
  static const userAgentMax = 300;
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
    return Look(
      gender: v('gender', 0),
      hair: v('hair_style', 1),
      hairColor: v('hair_color', 0),
      bodyColor: v('body_color', 0),
    );
  }

  final int gender;
  final int hair;
  final int hairColor;
  final int bodyColor;

  Look copyWith({int? hair, int? hairColor, int? bodyColor}) => Look(
    gender: gender,
    hair: hair ?? this.hair,
    hairColor: hairColor ?? this.hairColor,
    bodyColor: bodyColor ?? this.bodyColor,
  );

  Map<String, int> toUi() => {'gender': gender, 'hair': hair, 'hairColor': hairColor, 'bodyColor': bodyColor};

  static Look _read(WireReader r) => Look(gender: r.u8(), hair: r.u8(), hairColor: r.u8(), bodyColor: r.u8());

  @override
  bool operator ==(Object other) =>
      other is Look &&
      other.gender == gender &&
      other.hair == hair &&
      other.hairColor == hairColor &&
      other.bodyColor == bodyColor;

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
  PlayerInfo._(
    this.entityId,
    this.name,
    this.archetype,
    this.x,
    this.y,
    this.facing,
    this.look,
    this.level,
    this.weapon,
  );
  final int entityId;
  final String name;
  final String archetype;
  final double x;
  final double y;
  final int facing;
  final Look look;

  /// Character level (its tier picks the body).
  final int level;

  /// Held weapon id (`sword`, `bow`, ...; '' for townsfolk).
  final String weapon;

  static PlayerInfo _read(WireReader r) =>
      PlayerInfo._(r.varint(), r.str(), r.str(), r.pos(), r.pos(), r.u8(), Look._read(r), r.varint(), r.str());
}

/// Equipment slot indices (the server's `items::Slot`, wire order).
abstract final class EquipSlot {
  static const upperHead = 0;
  static const midHead = 1;
  static const lowerHead = 2;
  static const armour = 3;
  static const weapon = 4;
  static const shield = 5;
  static const garment = 6;
  static const shoes = 7;
  static const accessory1 = 8;
  static const accessory2 = 9;
  static const count = 10;
}

/// Weapon family of a weapon item id (what the engine draws and swings:
/// `setWeapon`), or null for none / unknown.
String? weaponFamilyOf(String weapon) => switch (weapon) {
  'knife' || 'cutter' || 'main_gauche' || 'stiletto' => 'dagger',
  'sword' || 'falchion' || 'blade' => 'sword',
  'katana' || 'claymore' => 'two_hand_sword',
  'javelin' || 'pike' => 'spear',
  'axe' => 'axe',
  'battle_axe' => 'two_hand_axe',
  'club' || 'mace' => 'mace',
  'bow' || 'composite_bow' || 'great_bow' => 'bow',
  'rod' || 'wand' || 'staff' => 'staff',
  'bible' => 'book',
  _ => null,
};

/// One row of the item catalogue ([ItemDefs]).
final class ItemDef {
  ItemDef._(this.id, this.name, this.kind, this.slot, this.price, this.classes, this.desc);
  final String id;
  final String name;

  /// 0 etc, 1 consumable, 2 weapon, 3 armour, 4 shield, 5 headgear,
  /// 6 garment, 7 shoes, 8 accessory.
  final int kind;

  /// [EquipSlot] it is worn in; null = not worn.
  final int? slot;
  final int price;

  /// Classes that may use it: bit 0 swordsman, 1 archer, 2 mage, 3 acolyte.
  final int classes;

  /// Tooltip text (the engine's `markup()`).
  final String desc;

  static ItemDef _read(WireReader r) {
    final id = r.str(), name = r.str(), kind = r.u8(), slot = r.u8();
    return ItemDef._(id, name, kind, slot == 0xFF ? null : slot, r.varint(), r.u8(), r.str());
  }
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
      // The server version came later: older servers leave it out.
      Kind.connected => Connected(r.varint(), r.take(16), r.isEmpty ? '' : r.str()),
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
        r.list((r) => Swing._(r.varint(), r.u8(), r.varint())),
        r.list((r) => Hit._(r.varint(), r.varint(), r.u8())),
      ),
      Kind.progress => Progress(r.varint(), r.varint(), r.varint(), r.varint()),
      Kind.skills => Skills(r.varint(), r.varint(), {
        for (final (id, lvl) in r.list((r) => (r.str(), r.u8()))) id: lvl,
      }, r.list((r) => r.str())),
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
      Kind.emote => Emote(r.varint(), r.u8()),
      Kind.pose => Pose(r.varint(), r.u8()),
      Kind.flag => FlagPing(r.varint(), r.u8(), r.pos(), r.pos()),
      Kind.inventory => Inventory(
        r.varint(),
        [for (final (id, n) in r.list((r) => (r.str(), r.varint()))) (id: id, count: n)],
        [for (var i = 0; i < EquipSlot.count; i++) r.str()],
        r.str(),
      ),
      Kind.itemDefs => ItemDefs(r.list(ItemDef._read)),
      Kind.shop => Shop(r.str(), r.str(), r.list((r) => r.str())),
      Kind.itemUsed => ItemUsed(r.str(), r.varint()),
      Kind.playerGear => PlayerGear(r.varint(), r.str()),
      Kind.loot => Loot(r.varint(), r.pos(), r.pos(), r.str(), r.varint(), rare: r.u8() & 1 != 0),
      Kind.combo => Combo(r.varint(), r.u8(), r.varint(), r.varint()),
      Kind.revive => Revive(r.varint(), r.u8(), r.varint()),
      Kind.bonus => switch (r.u8()) {
        0 => BonusSpawn(r.varint(), r.str(), r.pos(), r.pos(), r.varint()),
        1 => BonusGone(r.varint(), r.str(), r.varint()),
        _ => throw const WireFormatError('bonus op'),
      },
      Kind.missionInfo => MissionBriefing(MissionBrief._read(r)),
      Kind.stageClock => StageClock(r.varint()),
      Kind.fog => switch (r.u8()) {
        final op && (0 || 1) => PoisonFog(
          begun: op == 0,
          nowMs: r.varint(),
          startMs: r.varint(),
          shrinkMs: r.varint(),
          x: r.pos(),
          y: r.pos(),
          fromRadius: r.pos(),
          toRadius: r.pos(),
        ),
        2 => FogExposure(r.u8(), r.u8()),
        _ => throw const WireFormatError('fog op'),
      },
      Kind.partyInvite => switch (r.u8()) {
        0 => PartyInvite(r.varint(), r.str(), r.str(), r.varint()),
        1 => PartyAnswer(r.varint(), r.str(), accepted: r.boolean()),
        _ => throw const WireFormatError('party op'),
      },
      Kind.playerInspect => PlayerInspect(r.varint(), r.str(), r.str(), r.varint(), Look._read(r), [
        for (var i = 0; i < EquipSlot.count; i++) r.str(),
      ]),
      Kind.chat => switch (r.u8()) {
        0 => ChatLine(r.varint(), r.str(), r.str()),
        1 => Announcement(r.str()),
        _ => throw const WireFormatError('chat op'),
      },
      Kind.friends => Friends(r.list(FriendInfo._read)),
      Kind.friendNote => FriendNote(r.u8(), r.varint(), r.str()),
      Kind.trade => switch (r.u8()) {
        0 => TradeRequest(r.varint(), r.str(), r.varint()),
        1 => TradeState(r.varint(), r.str(), TradeOffer._read(r), TradeOffer._read(r), r.u8()),
        2 => TradeClosed(r.code(), r.str()),
        _ => throw const WireFormatError('trade op'),
      },
      Kind.feedbackResult => FeedbackResult(r.varint(), r.str(), r.str()),
      Kind.playerLink => PlayerLink(r.varint(), online: r.boolean()),
      Kind.resumable => Resumable(r.varint(), r.str(), r.varint()),
      Kind.modifierDefs => ModifierDefs(r.list(ModifierDef._read), r.list(RewardStep._read)),
      Kind.modifiers => switch (r.u8()) {
        0 => ModifierState(r.u8(), r.u8(), r.varint(), r.varint(), r.varint(), r.varint(), r.varint(), r.varint()),
        1 => ModifierReward(r.u8(), r.varint(), r.varint(), r.varint(), r.varint(), [
          for (var n = r.varint(); n > 0; n--) (r.str(), r.varint()),
        ]),
        _ => throw const WireFormatError('modifiers op'),
      },
      Kind.runStats => RunStats(r.boolean(), r.varint(), r.varint(), r.list(RunSeat._read), r.list(RunMark._read)),
      Kind.runRewards => RunRewards(r.u8(), r.varint(), r.varint(), r.varint(), r.varint(), r.varint(), [
        for (var n = r.varint(); n > 0; n--) (r.str(), r.varint()),
      ]),
      Kind.bossCast => BossCast(r.varint(), r.str(), r.str(), r.varint(), r.u8()),
      Kind.bossZones => BossZones(r.varint(), r.str(), r.list(BossZone._read)),
      Kind.bossShout => BossShout(r.varint(), r.str(), r.str(), r.u8()),
      Kind.bossBar => BossBar(
        r.varint(),
        r.str(),
        r.u8(),
        r.varint(),
        r.varint(),
        r.varint(),
        r.u8(),
        r.list((r) => r.u8()),
        r.u8(),
      ),
      _ => null,
    };
    if (msg == null) return null;
    if (!r.isEmpty) throw const WireFormatError('trailing bytes');
    return msg;
  }
}

final class Connected extends ServerMsg {
  const Connected(this.userId, this.sessionId, this.serverVersion);
  final int userId;
  final Uint8List sessionId;

  /// `0.1.0+1a2b3c4d` (package version + git commit); empty if not sent.
  final String serverVersion;
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
  const Swing._(this.entityId, this.facing, this.target);
  final int entityId;
  final int facing;

  /// The entity struck (its arrow flies there); 0 = none.
  final int target;
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
  const Cast(
    this.caster,
    this.skill,
    this.x,
    this.y,
    this.castMs,
    this.cooldownMs, [
    this.areas = const [],
    this.level = 1,
  ]);
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

  Map<String, Object> toJson() => {
    'id': id,
    'shape': shape,
    'look': look,
    'x': x,
    'y': y,
    'dir': dir,
    'a': a,
    'b': b,
    'spanMs': spanMs,
    'follow': follow,
  };
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

/// An entity on the recipient's map plays an emote (players and residents).
final class Emote extends ServerMsg {
  const Emote(this.entityId, this.emote);
  final int entityId;
  final int emote;
}

/// A player on the stage lost its connection ([online] false: link-dead,
/// still on the map for the stage's link grace) or got it back.
final class PlayerLink extends ServerMsg {
  const PlayerLink(this.entityId, {required this.online});
  final int entityId;
  final bool online;
}

/// The selected character is link-dead in stage [stageId] ([mission] is
/// its name) for [leftMs] more: answer [ClientMsg.stageResume].
final class Resumable extends ServerMsg {
  const Resumable(this.stageId, this.mission, this.leftMs);
  final int stageId;
  final String mission;
  final int leftMs;
}

/// One row of the mission modifier table (server `world::modifiers`): the
/// difficulty slider ([bit] 0xFF, one [steps] entry per tier) or a curse
/// (its [bit] in a mask). The mission window draws only from these.
final class ModifierDef {
  ModifierDef._(this.id, this.bit, this.name, this.desc, this.icon, this.risk, this.minLevel, this.locked, this.steps);
  static const slider = 0xFF;
  final String id;
  final int bit;
  final String name;
  final String desc;
  final String icon;
  final int risk;
  final int minLevel;
  final bool locked;
  final List<(String label, String detail)> steps;

  static ModifierDef _read(WireReader r) => ModifierDef._(
    r.str(),
    r.u8(),
    r.str(),
    r.str(),
    r.str(),
    r.varint(),
    r.varint(),
    r.boolean(),
    r.list((r) => (r.str(), r.str())),
  );

  Map<String, Object?> toJson() => {
    'id': id,
    'bit': bit,
    'name': name,
    'desc': desc,
    'icon': icon,
    'risk': risk,
    'minLevel': minLevel,
    'locked': locked,
    'steps': [for (final (label, detail) in steps) {'label': label, 'detail': detail}],
  };
}

/// The clear bonus at one risk score.
final class RewardStep {
  const RewardStep(this.zenyPct, this.expPct, this.chest);
  final int zenyPct;
  final int expPct;
  final int chest;
  static RewardStep _read(WireReader r) => RewardStep(r.varint(), r.varint(), r.varint());
  Map<String, Object?> toJson() => {'zenyPct': zenyPct, 'expPct': expPct, 'chest': chest};
}

/// The mission modifier table and the bonus by risk (`MODIFIER_DEFS`).
final class ModifierDefs extends ServerMsg {
  const ModifierDefs(this.defs, this.rewards);
  final List<ModifierDef> defs;
  final List<RewardStep> rewards;
}

/// A lobby's ([scope] 0) or the stage's ([scope] 1) modifiers, with the
/// clear bonus they pay and each kill's loot share ([lootPct]).
final class ModifierState extends ServerMsg {
  const ModifierState(this.scope, this.tier, this.mask, this.risk, this.zenyPct, this.expPct, this.chest, this.lootPct);
  final int scope;
  final int tier;
  final int mask;
  final int risk;
  final int zenyPct;
  final int expPct;
  final int chest;
  final int lootPct;

  Map<String, Object?> toJson() => {
    'tier': tier,
    'mask': mask,
    'risk': risk,
    'zenyPct': zenyPct,
    'expPct': expPct,
    'chest': chest,
    'lootPct': lootPct,
  };
}

/// The clear bonus of a won run with modifiers (already in the bag).
final class ModifierReward extends ServerMsg {
  const ModifierReward(this.tier, this.mask, this.risk, this.zeny, this.exp, this.items);
  final int tier;
  final int mask;
  final int risk;
  final int zeny;
  final int exp;
  final List<(String item, int count)> items;
}

/// One party member's run (`RUN_STATS`; server `protocol/results.rs`).
final class RunSeat {
  const RunSeat(
    this.entityId,
    this.name,
    this.cls,
    this.level,
    this.flags,
    this.dealt,
    this.taken,
    this.healed,
    this.kills,
    this.assists,
    this.downs,
    this.deaths,
    this.revives,
    this.bestCombo,
    this.mvps,
    this.exp,
    this.buckets,
    this.skills,
  );

  static RunSeat _read(WireReader r) => RunSeat(
    r.varint(),
    r.str(),
    r.str(),
    r.varint(),
    r.u8(),
    r.varint(),
    r.varint(),
    r.varint(),
    r.varint(),
    r.varint(),
    r.varint(),
    r.varint(),
    r.varint(),
    r.varint(),
    r.varint(),
    r.varint(),
    r.list((r) => (r.varint(), r.varint(), r.varint())),
    r.list((r) => (r.str(), r.varint(), r.varint())),
  );

  /// [flags] bits.
  static const left = 1;
  static const linkDead = 2;

  final int entityId;
  final String name;
  final String cls;
  final int level;
  final int flags;
  final int dealt;
  final int taken;
  final int healed;
  final int kills;
  final int assists;
  final int downs;
  final int deaths;
  final int revives;
  final int bestCombo;
  final int mvps;
  final int exp;

  /// Per time bucket: damage dealt, damage taken, kills.
  final List<(int dealt, int taken, int kills)> buckets;

  /// Skill id (`attack`, `other`), damage, damaging hits; the most damaging first.
  final List<(String skill, int dealt, int hits)> skills;

  /// The engine's `ResultsSeat`.
  Map<String, Object> toJson() => {
    'id': entityId,
    'name': name,
    'cls': cls,
    'level': level,
    'left': flags & left != 0,
    'linkDead': flags & linkDead != 0,
    'dealt': dealt,
    'taken': taken,
    'healed': healed,
    'kills': kills,
    'assists': assists,
    'downs': downs,
    'deaths': deaths,
    'revives': revives,
    'bestCombo': bestCombo,
    'mvps': mvps,
    'exp': exp,
    'buckets': [for (final (d, t, k) in buckets) [d, t, k]],
    'skills': [for (final (s, d, h) in skills) [s, d, h]],
  };
}

/// A moment on a run's timeline: `kind` 0 boss slain, 1 elite slain, 2 a
/// member down, 3 a member out, 4 boss arrived; `seat` 255 = nobody.
final class RunMark {
  const RunMark(this.secs, this.kind, this.seat, this.label);
  static RunMark _read(WireReader r) => RunMark(r.varint(), r.u8(), r.u8(), r.str());
  final int secs;
  final int kind;
  final int seat;
  final String label;
  Map<String, Object> toJson() => {'secs': secs, 'kind': kind, 'seat': seat, 'label': label};
}

/// The party's statistics of a finished run (after `STAGE_END`).
final class RunStats extends ServerMsg {
  const RunStats(this.victory, this.secs, this.bucketSecs, this.seats, this.marks);
  final bool victory;
  final int secs;
  final int bucketSecs;
  final List<RunSeat> seats;
  final List<RunMark> marks;
}

/// The recipient's own take of a finished run (`seat` in [RunStats]): the
/// run's experience (bonus included), zeny from kills and drops, the
/// victory bonus, the modifiers' clear bonus and everything picked up.
final class RunRewards extends ServerMsg {
  const RunRewards(this.seat, this.exp, this.zeny, this.clearZeny, this.modExp, this.modZeny, this.items);
  final int seat;
  final int exp;
  final int zeny;
  final int clearZeny;
  final int modExp;
  final int modZeny;
  final List<(String item, int count)> items;

  Map<String, Object> toJson() => {
    'exp': exp,
    'zeny': zeny,
    'clearZeny': clearZeny,
    'modExp': modExp,
    'modZeny': modZeny,
    'items': [for (final (i, n) in items) [i, n]],
  };
}

/// An entity sits down (`pose` 1) or stands up (0).
final class Pose extends ServerMsg {
  const Pose(this.entityId, this.pose);
  final int entityId;
  final int pose;
  bool get sitting => pose == 1;
}

/// The own belongings: zeny, bag stacks and the ten equipment slots
/// (item ids by [EquipSlot], '' = nothing). On join and after each change.
final class Inventory extends ServerMsg {
  const Inventory(this.zeny, this.items, this.equip, [this.statsJson = '']);
  final int zeny;
  final List<({String id, int count})> items;
  final List<String> equip;

  /// Derived numbers with this gear (JSON: `ac`, `weapon {to_hit, damage,
  /// interval_ms}`, ...), as the character list's `stats`.
  final String statsJson;

  int count(String id) => items.where((s) => s.id == id).fold(0, (n, s) => n + s.count);
}

/// The item catalogue (on join).
final class ItemDefs extends ServerMsg {
  const ItemDefs(this.items);
  final List<ItemDef> items;
}

/// A merchant's goods (talking to a `shop` NPC); prices in [ItemDefs].
final class Shop extends ServerMsg {
  const Shop(this.npc, this.name, this.items);
  final String npc;
  final String name;
  final List<String> items;
}

/// The own character drank [item]; the next potion waits [cooldownMs].
final class ItemUsed extends ServerMsg {
  const ItemUsed(this.item, this.cooldownMs);
  final String item;
  final int cooldownMs;
}

/// A player on the map now holds [weapon] (an item id).
final class PlayerGear extends ServerMsg {
  const PlayerGear(this.entityId, this.weapon);
  final int entityId;
  final String weapon;
}

/// The monster [entityId] (at cell [x], [y]) dropped [count] of [item] for
/// the own character ([item] `zeny`: zeny); it is in the bag by the next
/// [Inventory]. [rare] drops deserve a toast.
final class Loot extends ServerMsg {
  const Loot(this.entityId, this.x, this.y, this.item, this.count, {this.rare = false});
  final int entityId;
  final double x;
  final double y;
  final String item;
  final int count;
  final bool rare;

  bool get isZeny => item == 'zeny';
}

/// The own kill combo (`combat::combo` on the server): [count] kills in a
/// row at style [rank] (0 none, 1 D, 2 C, 3 B, 4 A, 5 S, 6 SS, 7 SSS); it
/// drops in [leftMs] unless a kill refills it to [windowMs]. [count] 0: it
/// dropped.
final class Combo extends ServerMsg {
  const Combo(this.count, this.rank, this.windowMs, this.leftMs);
  final int count;
  final int rank;
  final int windowMs;
  final int leftMs;

  /// Rank letters by code.
  static const ranks = ['', 'D', 'C', 'B', 'A', 'S', 'SS', 'SSS'];

  String get rankName => ranks[rank.clamp(0, ranks.length - 1)];
}

/// A downed player [entityId] on the map: an ally's revive is [progress]
/// percent done (5 % steps), and they bleed out in [leftMs] (0: no longer
/// downed).
final class Revive extends ServerMsg {
  const Revive(this.entityId, this.progress, this.leftMs);
  final int entityId;
  final int progress;
  final int leftMs;

  bool get isDown => leftMs > 0;
}

/// A pickup bonus [bonus] (`storm_gust`, `zeny`, ...; `hub/bonus.rs`)
/// floats at cell ([x], [y]) on the stage; it bursts in [lifeMs] unless
/// a player walks into it (blinking over its last [blinkMs]).
final class BonusSpawn extends ServerMsg {
  const BonusSpawn(this.id, this.bonus, this.x, this.y, this.lifeMs);
  final int id;
  final String bonus;
  final double x;
  final double y;
  final int lifeMs;

  /// Same as the server's `BONUS_BLINK_MS`.
  static const blinkMs = 5000;
}

/// Pickup bonus [id] is gone: the player [entityId] took it (0: it expired).
final class BonusGone extends ServerMsg {
  const BonusGone(this.id, this.bonus, this.entityId);
  final int id;
  final String bonus;
  final int entityId;

  bool get expired => entityId == 0;
}

/// What a mission spawns and drops (`MISSION_INFO`): built by the server
/// from the live monster catalogue and the mission's wave rules, after a
/// mission giver's dialog and on joining its stage.
final class MissionBriefing extends ServerMsg {
  const MissionBriefing(this.brief);
  final MissionBrief brief;
}

/// Time into the run of the own stage (the timeline).
final class StageClock extends ServerMsg {
  const StageClock(this.elapsedMs);
  final int elapsedMs;
}

/// The overtime poison fog of the own stage (`FOG` op 0/1, `hub/fog.rs`):
/// its safe circle around cell ([x], [y]) closes linearly from
/// [fromRadius] to [toRadius] cells over [shrinkMs] from run time
/// [startMs], then holds; [nowMs] is the run clock when it was sent.
/// [begun]: it rises right now (warn the player); otherwise it is the
/// state for a player joining a fogged stage.
final class PoisonFog extends ServerMsg {
  const PoisonFog({
    required this.begun,
    required this.nowMs,
    required this.startMs,
    required this.shrinkMs,
    required this.x,
    required this.y,
    required this.fromRadius,
    required this.toRadius,
  });
  final bool begun;
  final int nowMs;
  final int startMs;
  final int shrinkMs;
  final double x;
  final double y;
  final double fromRadius;
  final double toRadius;

  /// Safe radius (cells) at run time [ms].
  double radiusAt(int ms) {
    final t = shrinkMs <= 0 ? 1.0 : ((ms - startMs) / shrinkMs).clamp(0.0, 1.0);
    return fromRadius + (toRadius - fromRadius) * t;
  }

  /// The engine's `setPoisonFog` JSON (cells, ms).
  Map<String, Object> toJson() => {
    'x': x,
    'y': y,
    'from': fromRadius,
    'to': toRadius,
    'nowMs': nowMs,
    'startMs': startMs,
    'shrinkMs': shrinkMs,
  };
}

/// Entity id of the first town resident (server `RESIDENT_ENTITY_BASE`):
/// townsfolk look like players but are not (no player menu).
const residentEntityBase = 0x700000;

/// [name] (entity [from]) invites the recipient into their party for
/// [mission] (`PARTY_INVITE` op 0); answer with [ClientMsg.partyAnswer]
/// within [ttlMs].
final class PartyInvite extends ServerMsg {
  const PartyInvite(this.from, this.name, this.mission, this.ttlMs);
  final int from;
  final String name;
  final String mission;
  final int ttlMs;
}

/// The recipient's invitation to [name] (entity [entityId]) was answered
/// (`PARTY_INVITE` op 1).
final class PartyAnswer extends ServerMsg {
  const PartyAnswer(this.entityId, this.name, {required this.accepted});
  final int entityId;
  final String name;
  final bool accepted;
}

/// Another player's worn gear (`PLAYER_INSPECT`, the answer to
/// [ClientMsg.inspect]): [equip] holds [EquipSlot.count] item ids in slot
/// order, '' = nothing.
final class PlayerInspect extends ServerMsg {
  const PlayerInspect(this.entityId, this.name, this.archetype, this.level, this.look, this.equip);
  final int entityId;
  final String name;
  final String archetype;
  final int level;
  final Look look;
  final List<String> equip;
}

/// Longest chat line the server keeps (`chat::CHAT_MAX_CHARS`).
const chatMaxChars = 120;

/// A scripted elite or boss starts [ability] (`hub::boss`): the cast bar
/// shows [name] for [castMs]. An empty [ability] ends its cast early
/// ([interrupted]: the party broke it).
final class BossCast extends ServerMsg {
  const BossCast(this.caster, this.ability, this.name, this.castMs, this.flags);
  final int caster;
  final String ability;
  final String name;
  final int castMs;

  /// 1 a channel burst damage breaks, 2 shielded, 4 interrupted.
  final int flags;
  bool get channel => flags & 1 != 0;
  bool get shielded => flags & 2 != 0;
  bool get interrupted => flags & 4 != 0;
}

/// The zones of a boss ability, each resolving after its own delay.
final class BossZones extends ServerMsg {
  const BossZones(this.caster, this.ability, this.zones);
  final int caster;
  final String ability;
  final List<BossZone> zones;
}

/// One zone of [BossZones]: [area] (shape 3 = ring: `a` outer radius, `b`
/// inner) resolving [delayMs] after the frame; [safe]: a sanctuary.
final class BossZone {
  const BossZone(this.area, this.delayMs, this.flags);
  static BossZone _read(WireReader r) => BossZone(Telegraph._read(r), r.varint(), r.u8());
  final Telegraph area;
  final int delayMs;
  final int flags;
  bool get safe => flags & 1 != 0;

  /// A bar centred on its caster (a cross): it fills from the middle out.
  bool get centre => flags & 2 != 0;

  Map<String, Object> toJson() => {...area.toJson(), 'delayMs': delayMs, 'safe': safe, 'centre': centre};
}

/// A boss's yell over its head and in the chat log ([style] 0 elite, 1 boss).
final class BossShout extends ServerMsg {
  const BossShout(this.entityId, this.name, this.text, this.style);
  final int entityId;
  final String name;
  final String text;
  final int style;
}

/// The encounter bar of an elite or boss ([rank] 0: gone).
final class BossBar extends ServerMsg {
  const BossBar(
    this.entityId,
    this.name,
    this.rank,
    this.hp,
    this.maxHp,
    this.shield,
    this.phase,
    this.pips,
    this.flags,
  );
  final int entityId;
  final String name;

  /// 0 gone, 1 elite, 2 boss.
  final int rank;
  final int hp;
  final int maxHp;

  /// Absorb shield left.
  final int shield;

  /// 0-based phase; [pips]: HP % at which each later phase begins.
  final int phase;
  final List<int> pips;
  final int flags;
  bool get enraged => flags & 1 != 0;

  Map<String, Object> toJson() => {
    'id': entityId,
    'name': name,
    'rank': rank,
    'hp': hp,
    'maxHp': maxHp,
    'shield': shield,
    'phase': phase,
    'pips': pips,
    'enraged': enraged,
  };
}

/// A player's local chat line (`CHAT` op 0): over their head ([entityId])
/// and in the chat log. Reaches the players around them in town, the
/// whole party on a mission; the speaker gets their own line back.
final class ChatLine extends ServerMsg {
  const ChatLine(this.entityId, this.name, this.text);
  final int entityId;
  final String name;
  final String text;
}

/// A server announcement (`CHAT` op 1: the admin API, restart countdown):
/// a banner and a chat log line.
final class Announcement extends ServerMsg {
  const Announcement(this.text);
  final String text;
}

/// The recipient's net seconds inside the poison fog ([secs]); at
/// [deathSecs] it kills outright.
final class FogExposure extends ServerMsg {
  const FogExposure(this.secs, this.deathSecs);
  final int secs;
  final int deathSecs;

  bool get fatal => secs >= deathSecs;
}

final class MissionBrief {
  const MissionBrief({
    required this.mission,
    required this.map,
    required this.timeLimitSecs,
    required this.levelMin,
    required this.levelMax,
    required this.levelEverySecs,
    required this.monsters,
  });

  factory MissionBrief._read(WireReader r) => MissionBrief(
    mission: r.str(),
    map: r.str(),
    timeLimitSecs: r.varint(),
    levelMin: r.varint(),
    levelMax: r.varint(),
    levelEverySecs: r.varint(),
    monsters: r.list(MonsterBrief._read),
  );

  final String mission;
  final String map;
  final int timeLimitSecs;

  /// Monster level band; also the recommended player levels.
  final int levelMin;
  final int levelMax;

  /// Monsters gain a level this often.
  final int levelEverySecs;

  /// Every kind that can spawn before the time limit, by unlock time.
  final List<MonsterBrief> monsters;

  /// For the engine's mission window and timeline.
  Map<String, Object?> toJson() => {
    'mission': mission,
    'map': map,
    'timeLimit': timeLimitSecs,
    'levels': [levelMin, levelMax],
    'levelEvery': levelEverySecs,
    'monsters': [for (final m in monsters) m.toJson()],
  };
}

final class MonsterBrief {
  const MonsterBrief({
    required this.id,
    required this.name,
    required this.archetype,
    required this.element,
    required this.size,
    required this.unlockSecs,
    required this.levelMin,
    required this.levelMax,
    required this.hpMin,
    required this.hpMax,
    required this.packMin,
    required this.packMax,
    required this.drops,
  });

  factory MonsterBrief._read(WireReader r) => MonsterBrief(
    id: r.str(),
    name: r.str(),
    archetype: r.str(),
    element: r.str(),
    size: r.str(),
    unlockSecs: r.varint(),
    levelMin: r.varint(),
    levelMax: r.varint(),
    hpMin: r.varint(),
    hpMax: r.varint(),
    packMin: r.varint(),
    packMax: r.varint(),
    drops: r.list((r) => (item: r.str(), rate: r.varint(), min: r.varint(), max: r.varint())),
  );

  final String id;
  final String name;

  /// `swarm`, `chaser`, `tank`, `elite`, `boss`.
  final String archetype;
  final String element;
  final String size;
  final int unlockSecs;
  final int levelMin;
  final int levelMax;
  final int hpMin;
  final int hpMax;

  /// Pack size range the game master sends it in.
  final int packMin;
  final int packMax;

  /// [rate] in hundredths of a percent (10000 = always); [item] `zeny` = zeny.
  final List<({String item, int rate, int min, int max})> drops;

  Map<String, Object?> toJson() => {
    'id': id,
    'name': name,
    'archetype': archetype,
    'element': element,
    'size': size,
    'unlock': unlockSecs,
    'levels': [levelMin, levelMax],
    'hp': [hpMin, hpMax],
    'pack': [packMin, packMax],
    'drops': [
      for (final d in drops) {'item': d.item, 'rate': d.rate, 'min': d.min, 'max': d.max},
    ],
  };
}

/// A ping flag placed by [entityId] on cell ([x], [y]).
final class FlagPing extends ServerMsg {
  const FlagPing(this.entityId, this.flag, this.x, this.y);
  final int entityId;
  final int flag;
  final double x;
  final double y;
}

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

/// How the recipient relates to a [FriendInfo] account.
enum FriendLink {
  friend,

  /// They asked the recipient (accept / decline).
  incoming,

  /// The recipient asked them (waiting).
  outgoing,
}

/// A row of the friends list (account level).
final class FriendInfo {
  const FriendInfo({
    required this.account,
    required this.link,
    required this.name,
    required this.online,
    required this.archetype,
    required this.level,
    required this.map,
    required this.channel,
    required this.characters,
  });

  static FriendInfo _read(WireReader r) => FriendInfo(
    account: r.varint(),
    link: switch (r.u8()) {
      0 => FriendLink.friend,
      1 => FriendLink.incoming,
      2 => FriendLink.outgoing,
      _ => throw const WireFormatError('friend link'),
    },
    name: r.str(),
    online: r.boolean(),
    archetype: r.str(),
    level: r.varint(),
    map: r.str(),
    channel: r.varint(),
    characters: r.list((r) => r.varint()),
  );

  final int account;
  final FriendLink link;

  /// The character played now (online), else the last one played.
  final String name;
  final bool online;
  final String archetype;
  final int level;

  /// Map display name ('' offline, and for requests).
  final String map;

  /// Hub channel; 0 = on a mission (or offline).
  final int channel;

  /// Every character of the account: a player entity id is a character
  /// id, so the client tells a friend on the map from these.
  final List<int> characters;
}

/// The recipient's whole friends list (`FRIENDS`), after any change.
final class Friends extends ServerMsg {
  const Friends(this.friends);
  final List<FriendInfo> friends;
}

/// `FRIEND_NOTE` ops (a toast).
abstract final class FriendNoteOp {
  static const online = 0;
  static const request = 1;
  static const accepted = 2;
  static const sent = 3;
  static const removed = 4;
}

/// Something happened on the friends list ([FriendNoteOp]): [name] is the
/// other side's character.
final class FriendNote extends ServerMsg {
  const FriendNote(this.op, this.account, this.name);
  final int op;
  final int account;
  final String name;
}

/// One side of a player trade: stacks from the bag and zeny.
final class TradeOffer {
  const TradeOffer(this.zeny, this.items);
  final int zeny;
  final List<({String id, int count})> items;

  static TradeOffer _read(WireReader r) =>
      TradeOffer(r.varint(), [for (final (id, n) in r.list((r) => (r.str(), r.varint()))) (id: id, count: n)]);

  int count(String id) => items.where((s) => s.id == id).fold(0, (n, s) => n + s.count);

  Map<String, Object?> toJson() => {
    'zeny': zeny,
    'items': [
      for (final s in items) {'id': s.id, 'count': s.count},
    ],
  };
}

/// The player [entityId] ([name], level [level]) asks to trade
/// ([ClientMsg.tradeRespond]).
final class TradeRequest extends ServerMsg {
  const TradeRequest(this.entityId, this.name, this.level);
  final int entityId;
  final String name;
  final int level;
}

/// The open trade with [entityId] ([name]): both offers and who locked
/// ("OK") and confirmed ("Trade"). After every change.
final class TradeState extends ServerMsg {
  const TradeState(this.entityId, this.name, this.mine, this.theirs, this.flags);
  final int entityId;
  final String name;
  final TradeOffer mine;
  final TradeOffer theirs;
  final int flags;

  bool get myLock => flags & 1 != 0;
  bool get theirLock => flags & 2 != 0;
  bool get myConfirm => flags & 4 != 0;
  bool get theirConfirm => flags & 8 != 0;
}

/// The trade (or request) ended: [reason] is `done`, `cancelled`,
/// `declined`, `expired`, `too_far`, `left`, `map_changed`, `mission` or a
/// `trade_*` error; [message] may be empty (just close the prompt).
final class TradeClosed extends ServerMsg {
  const TradeClosed(this.reason, this.message);
  final String reason;
  final String message;
}

/// The answer to [ClientMsg.feedback]: stored as report [id] ([code]
/// empty), or refused ([id] 0, [code] `feedback_*` or `npc_too_far`);
/// [message] is for the player.
final class FeedbackResult extends ServerMsg {
  const FeedbackResult(this.id, this.code, this.message);
  final int id;
  final String code;
  final String message;

  bool get ok => code.isEmpty && id > 0;
}
