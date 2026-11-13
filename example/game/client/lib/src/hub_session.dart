import 'dart:async';
import 'dart:convert';
import 'dart:js_interop';
import 'dart:js_interop_unsafe';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:game_client/src/api.dart';
import 'package:game_client/src/engine.dart';
import 'package:game_client/src/interpolation.dart';
import 'package:game_client/src/prediction.dart';
import 'package:game_client/src/protocol.dart';
import 'package:game_client/src/walk_grid.dart';
import 'package:web/web.dart' as web;
import 'package:ws/ws.dart';

final class _Npc {
  _Npc(this.index, Map<String, Object?> j)
    : id = j['id']! as String,
      name = j['name']! as String,
      sprite = j['sprite']! as String,
      kind = (j['kind'] as String?) ?? 'talk',
      cutin = j['cutin'] as String?,
      text = j['text'] as String?,
      x = (j['x']! as num).toDouble() + 0.5,
      y = (j['y']! as num).toDouble() + 0.5,
      facing = (j['facing'] as num?)?.toInt() ?? 4,
      radius = (j['interactRadius'] as num?)?.toDouble() ?? 3;

  final int index;
  final String id;
  final String name;
  final String sprite;
  final String kind;
  final String? cutin;
  final String? text;
  final double x;
  final double y;
  final int facing;
  final double radius;

  /// NPCs are client-side entities; negative ids never collide with characters.
  double get entityId => -(index + 1).toDouble();

  bool inReach(double px, double py) {
    final dx = px - x, dy = py - y;
    return dx * dx + dy * dy <= radius * radius;
  }
}

/// A stage monster as the server last described it: a dead-reckoning
/// record (see [Reckoning]). Its identity is `(slot, generation)`: a new
/// generation on the same slot is a different monster.
final class _Mob {
  _Mob(this.slot, this.generation, this.kind);

  /// Engine ids of monsters: above every character id, below 2^24 (exact in
  /// float32), and positive (NPCs are negative).
  static int entityIdOf(int slot) => 0x800000 + slot;

  final int slot;
  final int generation;
  final int kind;
  double get id => entityIdOf(slot).toDouble();

  // Record: position (1/64 cell), velocity (1/256 cell/tick), stamp tick.
  int x = 0;
  int y = 0;
  int vx = 0;
  int vy = 0;
  int stamp = 0;
  int facing = 4;

  /// The record changed since it was last handed to the engine.
  bool dirty = true;

  /// Removed by the server at this tick; dropped once the render clock
  /// gets there (it renders slightly in the past).
  int? removeAt;

  /// Wall-clock seconds: attack / flinch animations play until then; a
  /// killed monster plays its death where it stood before it goes.
  double attackUntil = 0;
  double hitUntil = 0;
  double? dieAt;
  int sentAction = -1;

  void set(int x, int y, int vx, int vy, int stamp) {
    this
      ..x = x
      ..y = y
      ..vx = vx
      ..vy = vy
      ..stamp = stamp
      ..dirty = true;
    // Facing from the velocity, with a dead zone so push-out nudges in a
    // settled crowd do not spin the sprite.
    if (vx * vx + vy * vy >= 9) facing = facingOf(vx.toDouble(), vy.toDouble());
  }

  bool get moving => vx * vx + vy * vy >= 9;
}

final class _Peer {
  _Peer(this.name, this.archetype, this.look);
  final String name;
  final String archetype;
  Look look;
}

/// One connected hub session: network, prediction, interpolation and the
/// render loop feeding the engine façade.
final class HubSession {
  HubSession(
    this.api,
    this.view, {
    required this.assetsBase,
    required String characterName,
    String archetype = 'novice',
    Look look = const Look(),
  }) : _myName = characterName,
       _myArchetype = archetype,
       _myLook = look;

  final Api api;
  final GameView view;
  final String assetsBase;

  late final WebSocketClient<Uint8List> _ws;
  final _subs = <StreamSubscription<Object?>>[];

  int? _me;
  String _myName;
  String _myArchetype;
  Look _myLook;

  /// The stylist window is open (NPC id), and a restyle is in flight.
  String? _stylist;
  bool _styling = false;
  String _channel = '-';

  /// In a stage instance (not the hub).
  bool _inStage = false;

  /// Current lobby (`LobbyState`), or null.
  LobbyInfo? _lobby;
  String _missionName = '';
  int _tickHz = 20;
  String _mapId = '';
  String _mapName = '';
  int _mapEpoch = -1;
  bool _loading = true;
  int _loadToken = 0;

  LocalPredictor? _predictor;
  late ServerClock _clock = ServerClock(tickHz: _tickHz);
  final _tracks = <int, RemoteTrack>{};
  final _lastSeen = <int, double>{};
  final _peers = <int, _Peer>{};

  /// Combat: last vitals per player, own progress, attack animations
  /// (wall-clock end and facing) of characters.
  final _vitals = <int, PlayerVitals>{};
  Progress? _progress;
  final _attackUntil = <int, double>{};
  final _attackFacing = <int, int>{};

  /// Own state from the vitals: 0 up, 1 downed, 2 dead.
  int get _myState => _vitals[_me]?.state ?? 0;

  /// Stage monsters by pool slot, and the kind table from `stage_joined`.
  final _mobs = <int, _Mob>{};
  var _monsterKinds = const <String>[];

  /// Tick of the last monster section applied; null until a keyframe
  /// (after a map change, or when a delta did not apply and a resync is
  /// pending).
  int? _mobTick;
  bool _resyncSent = false;
  var _npcs = <_Npc>[];
  _Npc? _nearNpc;

  final _keys = <String>{};
  int _stopSends = 0;

  /// Own skills (`skills` message) and the last pointer position (aim).
  Skills? _skills;
  (double, double)? _pointer;

  /// Slot keys: Q E R F Space, with Shift the second page; digits 1–0 too.
  static const _slotKeys = ['KeyQ', 'KeyE', 'KeyR', 'KeyF', 'Space'];
  static const _slotCaptions = ['Q', 'E', 'R', 'F', '␣', '⇧Q', '⇧E', '⇧R', '⇧F', '⇧␣'];
  double _acc = 0;
  double _lastFrame = 0;
  double _lastRender = 0;
  final _out = RowBuffer();

  /// Server tick that maps to render time 0 for the current map. Segment
  /// times are seconds relative to it, which keeps float32 precision in the
  /// shader no matter how long the server has been up.
  int? _epochTick;

  // Client-side settings (the engine owns the rest); persisted with them.
  int _frameCap = 0;
  static const _settingsKey = 'game.settings';

  // Stats.
  double _rttMs = 0;
  double _pingSentAt = 0;
  int _snapshots = 0;
  double _snapshotRate = 0;
  double _rateWindowStart = 0;
  int _lastBytesIn = 0;
  double _bytesInRate = 0;

  static double _now() => web.window.performance.now() / 1000;

  Future<void> start(String token) async {
    _ws = WebSocketClient.binary();
    _subs.add(_ws.messages.listen(_onMessage));
    _subs.add(
      _ws.states.listen((s) {
        if (s is WebSocketState$Open) {
          _send(ClientMsg.hubJoin());
        }
      }),
    );
    view.onUiEvent(_onUiEvent.toJS);
    _loadSettings();
    view.loadSkills('$assetsBase/client/skills').toDart.then((_) => _renderSkills()).catchError((Object e) {
      web.console.warn('skills unavailable: $e'.toJS);
    });
    web.window.addEventListener(
      'pointermove',
      ((web.PointerEvent e) {
        _pointer = (e.clientX.toDouble(), e.clientY.toDouble());
      }).toJS,
    );
    web.window.addEventListener('keydown', _onKeyDown.toJS);
    web.window.addEventListener('keyup', _onKeyUp.toJS);
    web.window.addEventListener('blur', ((web.Event _) => _keys.clear()).toJS);
    (web.document.querySelector('canvas')! as web.HTMLCanvasElement).addEventListener('click', _onClick.toJS);

    final loc = web.window.location;
    final scheme = loc.protocol == 'https:' ? 'wss' : 'ws';
    await _ws.connect('$scheme://${loc.host}/api/ws?token=${Uri.encodeQueryComponent(token)}');
    Timer.periodic(const Duration(seconds: 2), (_) {
      _pingSentAt = _now();
      _send(ClientMsg.ping());
    });
    // Read-only probe for scripted browser tests and bots.
    globalContext.setProperty(
      'gameDebug'.toJS,
      (() {
        final p = _predictor;
        return jsonEncode({'map': _mapId, 'loading': _loading, 'x': p?.x, 'y': p?.y, 'peers': _peers.length});
      }).toJS,
    );
    _frame(0);
  }

  void _send(Uint8List frame) {
    if (_ws.state is! WebSocketState$Open) return;
    _ws.push(frame).ignore();
  }

  // ---------------------------------------------------------------- network

  void _onMessage(Uint8List data) {
    if (data.isEmpty) return;
    if (Snapshot.isSnapshot(data[0])) {
      final s = Snapshot.decode(data);
      if (s != null) _onSnapshot(s);
      return;
    }
    final ServerMsg? msg;
    try {
      msg = ServerMsg.decode(data);
    } on Object catch (e) {
      web.console.warn('bad frame 0x${data[0].toRadixString(16)}: $e'.toJS);
      return;
    }
    switch (msg) {
      case null || Connected() || HubChannels():
        break;
      case Notification(:final message):
        view.toast(message);
      case final PlayerVitals v:
        _onVitals(v);
      case final Combat c:
        _onCombat(c);
      case final Skills s:
        _skills = s;
        _renderSkills();
      case final Cast c:
        _onCast(c);
      case final Progress pr:
        final before = _progress?.level;
        _progress = pr;
        if (before != null && pr.level > before) {
          view.toast('Level up! Lv ${pr.level} ${_title(_myArchetype, pr.level)} — K: skills');
        }
        _renderStatus();
      case Pong():
        if (_pingSentAt > 0) _rttMs = (_now() - _pingSentAt) * 1000;
      case HubJoined(:final entityId, :final channelName, :final tickRate):
        _me = entityId;
        _channel = channelName;
        _tickHz = tickRate;
        _clock = ServerClock(tickHz: _tickHz);
        view.setLightRadius(0);
        _vitals.clear();
        view.setStatus('');
        if (_inStage) {
          _inStage = false;
          view.showPanel('');
        }
      case StageJoined():
        _me = msg.entityId;
        _channel = 'Stage-${msg.stageId}';
        _tickHz = msg.tickRate;
        _clock = ServerClock(tickHz: _tickHz);
        _monsterKinds = msg.monsters;
        _vitals.clear();
        view.setLightRadius(msg.lightRadius);
        _inStage = true;
        _renderStagePanel();
      case LobbyState(:final lobby):
        _lobby = lobby;
        _missionName = lobby.mission.name;
        _renderLobby();
      case LobbyLeft(:final reason):
        _lobby = null;
        if (!_inStage) view.showPanel('');
        switch (reason) {
          case 'kicked':
            view.toast('You were removed from the party.');
          case 'replaced':
            view.toast('Your party moved to another tab.');
          case 'launched':
            view.toast('$_missionName is starting!');
        }
      case HubError(:final message) when _stylist != null:
        _styling = false;
        _showStylist(error: message);
      case LobbyError(:final message) || HubError(:final message):
        view.toast(message);
      case PlayerLook(:final entityId, :final look):
        _onPlayerLook(entityId, look);
      case final MapEnter enter:
        _onMapEnter(enter).catchError((Object e, StackTrace st) {
          web.console.error('map_enter failed: $e\n$st'.toJS);
          view.toast('Could not load ${enter.map.id}: $e');
        });
      case PlayerAppeared(:final player):
        _addPeer(player);
        if (_inStage) _renderStagePanel();
      case PlayerVanished(:final entityId):
        _removePeer(entityId);
        if (_inStage) _renderStagePanel();
      case PortalDenied(:final targetMap):
        view.toast('$targetMap is reached through the mission board.');
      case NpcDialog():
        _onNpcDialog(msg);
      case HubLeft(:final reason):
        view.toast('Left the hub ($reason).');
      case Displaced():
        view.toast('Signed in elsewhere; this tab is disconnected.');
        _ws.close().ignore();
    }
  }

  void _onSnapshot(Snapshot s) {
    if (s.mapEpoch != _mapEpoch) return; // from the previous map
    final now = _now();
    _snapshots++;
    _clock.onSnapshot(s.tick, now);
    _epochTick ??= s.tick;
    for (var i = 0; i < s.length; i++) {
      final id = s.ids[i];
      if (id == _me) {
        if (!_loading) _predictor?.reconcile(s.ackSeq, s.xs[i], s.ys[i]);
        continue;
      }
      (_tracks[id] ??= RemoteTrack(id)).push(s.tick, s.xs[i], s.ys[i], s.facings[i]);
      _lastSeen[id] = now;
    }
    if (s.mobBaseline >= 0) _applyMonsters(s);
  }

  /// Applies a monster section; a delta against a frame this client did not
  /// apply asks the server for a keyframe and waits for it.
  void _applyMonsters(Snapshot s) {
    if (s.mobBaseline == 0) {
      for (final m in _mobs.values) {
        m.removeAt ??= s.tick;
      }
      _resyncSent = false;
    } else if (_mobTick == null || s.tick - s.mobBaseline != _mobTick) {
      _outOfSync();
      return;
    }
    for (var i = 0; i < s.monsterCount; i++) {
      final slot = s.mobSlots[i];
      var m = _mobs[slot];
      switch (s.mobOps[i]) {
        case MobOp.spawn:
          // A reused slot (or a keyframe) is a new monster: snap, never
          // slide from the old one.
          if (m != null) view.removeEntity(m.id);
          m = _mobs[slot] = _Mob(slot, s.mobGenerations[i], s.mobKinds[i]);
          m.set(s.mobXs[i], s.mobYs[i], s.mobVxs[i], s.mobVys[i], s.tick);
        case MobOp.move when m != null && m.removeAt == null:
          if (m.dieAt == null) m.set(s.mobXs[i], s.mobYs[i], s.mobVxs[i], s.mobVys[i], s.tick);
        case MobOp.nudge when m != null && m.removeAt == null && m.dieAt != null:
          break;
        case MobOp.nudge when m != null && m.removeAt == null:
          m.set(m.x + s.mobXs[i], m.y + s.mobYs[i], s.mobVxs[i], s.mobVys[i], s.tick);
        case MobOp.remove when m != null:
          m.removeAt ??= s.tick;
        default:
          // An update for a monster this client does not hold.
          _outOfSync();
          return;
      }
    }
    _mobTick = s.tick;
  }

  // ------------------------------------------------------------ combat

  void _onVitals(PlayerVitals v) {
    final wasUp = (_vitals[v.entityId]?.state ?? 0) == 0;
    _vitals[v.entityId] = v;
    _applyHealth(v);
    if (v.entityId == _me) {
      if (wasUp && v.state == 1) view.toast('You are down! Stand by an ally to be revived.');
      _renderStatus();
    }
  }

  void _applyHealth(PlayerVitals v) {
    view.setHealth(v.entityId.toDouble(), v.maxHp > 0 ? v.hp / v.maxHp : double.nan);
  }

  void _onCombat(Combat c) {
    final now = _now();
    for (final s in c.swings) {
      final mob = s.entityId >= monsterEntityBase ? _mobs[s.entityId - monsterEntityBase] : null;
      if (mob != null) {
        mob
          ..attackUntil = now + 0.5
          ..facing = s.facing;
      } else {
        _attackUntil[s.entityId] = now + 0.45;
        _attackFacing[s.entityId] = s.facing;
        _tracks[s.entityId]?.resetSent();
      }
    }
    for (final h in c.hits) {
      final isMob = h.target >= monsterEntityBase;
      view.showDamage(h.target.toDouble(), h.amount, h.flags, !isMob);
      if (!isMob) continue;
      final mob = _mobs[h.target - monsterEntityBase];
      if (mob == null) continue;
      if (h.isKill) {
        mob.dieAt ??= now;
      } else if (!h.isMiss) {
        mob.hitUntil = now + 0.25;
      }
    }
  }

  /// Character action: sitting while down, the swing for a moment after an
  /// attack, otherwise walk / idle.
  int _charAction(int id, bool moving, double now) {
    if ((_vitals[id]?.state ?? 0) != 0) return Act.sit;
    if (moving) return Act.walk;
    return now < (_attackUntil[id] ?? 0) ? Act.attack : Act.idle;
  }

  void _renderStatus() {
    final me = _me;
    final v = me == null ? null : _vitals[me];
    if (!_inStage || v == null) {
      view.setStatus('');
      return;
    }
    final pr = _progress;
    final level = pr?.level ?? 1;
    view.setStatus(
      jsonEncode({
        'name': _myName,
        'title': _title(_myArchetype, level),
        'level': level,
        'hp': v.hp,
        'maxHp': v.maxHp,
        'sp': v.sp,
        'maxSp': v.maxSp,
        'exp': pr?.exp ?? 0,
        'expToNext': pr?.expToNext ?? 0,
        'state': v.state,
      }),
    );
  }

  /// Class title at a level (tiers at 30 and 60, see COMBAT.md).
  static String _title(String archetype, int level) {
    const titles = {
      'swordsman': ['Swordsman', 'Crusader', 'Paladin'],
      'mage': ['Mage', 'Wizard', 'High Wizard'],
      'archer': ['Archer', 'Hunter', 'Sniper'],
      'acolyte': ['Acolyte', 'Priest', 'High Priest'],
    };
    final t = titles[archetype];
    if (t == null) return archetype;
    return t[level < 30 ? 0 : level < 60 ? 1 : 2];
  }

  void _outOfSync() {
    _mobTick = null;
    if (_resyncSent) return;
    _resyncSent = true;
    _send(ClientMsg.resync());
  }

  void _clearMobs() {
    for (final m in _mobs.values) {
      view.removeEntity(m.id);
    }
    _mobs.clear();
    _mobTick = null;
    _resyncSent = false;
  }

  Future<void> _onMapEnter(MapEnter msg) async {
    final token = ++_loadToken;
    _loading = true;
    final map = msg.map;
    _mapEpoch = msg.mapEpoch;
    _mapId = map.id;
    _mapName = map.name;
    final x = msg.x, y = msg.y;
    view.closeDialog();
    for (final id in _tracks.keys.toList()) {
      _removePeer(id);
    }
    _clearMobs();
    for (final npc in _npcs) {
      view.setLabel(npc.entityId, null, '');
    }
    _clock.reset();
    _epochTick = null;
    msg.players.forEach(_addPeer);

    final base = '$assetsBase/client/locations/$_mapId';
    final results = await Future.wait([
      api.json('$base/manifest.json'),
      api.bytes('$assetsBase/server/locations/$_mapId/walkability.bin'),
      view.loadLocation('$base/manifest.json').toDart,
    ]);
    if (token != _loadToken) return; // a newer map_enter won the race
    final manifest = results[0]! as Map<String, Object?>;
    final grid = WalkGrid.parse(results[1]! as Uint8List);

    view.setCamera(jsonEncode(map.camera));
    final p = _predictor;
    if (p == null) {
      _predictor = LocalPredictor(grid, x, y, rules: MoveRules(tickHz: _tickHz));
    } else {
      p.reset(grid, x, y);
    }
    _npcs = [
      for (final (i, n) in ((manifest['npcs'] as List?) ?? const []).indexed) _Npc(i, n as Map<String, Object?>),
    ];
    // NPCs never move: one row each per map, then the GPU owns them.
    for (final npc in _npcs) {
      view.setLabel(npc.entityId, npc.name, 'npc');
      _out.add(
        id: npc.entityId,
        x0: npc.x,
        y0: npc.y,
        t0: 0,
        x1: npc.x,
        y1: npc.y,
        t1: 0,
        facing: npc.facing,
        sprite: view.spriteIndex(npc.sprite),
        action: Act.idle,
      );
    }
    _out.flush(view);
    // loadLocation cleared the engine's entities; resend every peer.
    for (final t in _tracks.values) {
      t.resetSent();
    }
    for (final m in _mobs.values) {
      m.dirty = true;
    }
    // Labels and looks were cleared with the entities.
    final me = _me;
    if (me != null) {
      view.setLabel(me.toDouble(), _myName, 'me');
      _applyLook(me, _myLook);
    }
    for (final e in _peers.entries) {
      view.setLabel(e.key.toDouble(), e.value.name, '');
      _applyLook(e.key, e.value.look);
    }
    _vitals.values.forEach(_applyHealth);
    _renderStatus();
    final bgm = map.bgm;
    view.playBgm(bgm == null ? null : '$assetsBase/client/$bgm');
    _loading = false;
    if (_inStage) {
      _renderStagePanel();
      view.toast('$_mapName: the mission has begun. Walk to the exit or use the panel to return.');
    } else if (msg.viaPortal == null) {
      view.toast('Welcome to $_mapName ($_channel). WASD to move, F to talk.');
    }
  }

  void _addPeer(PlayerInfo p) {
    final id = p.entityId;
    final name = p.name;
    final archetype = p.archetype;
    if (id == _me) {
      _myName = name;
      _myArchetype = archetype;
      _myLook = p.look;
      _applyLook(id, p.look);
      return;
    }
    _peers[id] = _Peer(name, archetype, p.look);
    _applyLook(id, p.look);
    final t = _tracks[id] ??= RemoteTrack(id);
    if (t.isEmpty) {
      t
        ..x = p.x
        ..y = p.y
        ..facing = p.facing;
    }
    _lastSeen[id] = _now();
    view.setLabel(id.toDouble(), name, '');
  }

  void _applyLook(int id, Look l) => view.setLook(id.toDouble(), l.gender, l.hair, l.hairColor, l.bodyColor);

  void _onPlayerLook(int id, Look look) {
    if (id == _me) {
      _myLook = look;
      if (_stylist != null) {
        _stylist = null;
        _styling = false;
        view.closeScreen();
        view.toast('Looking sharp!');
      }
    } else {
      _peers[id]?.look = look;
    }
    _applyLook(id, look);
  }

  /// The stylist window over the game (hair style, hair and clothes dyes).
  void _showStylist({String? error}) {
    final npc = _npcs.where((n) => n.id == _stylist).firstOrNull;
    view.showStylist(
      jsonEncode({
        'title': npc?.name ?? 'Stylist',
        'text': npc?.text ?? 'Pick a new style.',
        'look': _myLook.toUi(),
        'busy': _styling,
        'error': ?error,
      }),
    );
  }

  void _removePeer(int id) {
    _peers.remove(id);
    _tracks.remove(id);
    _lastSeen.remove(id);
    view.setLabel(id.toDouble(), null, '');
    view.removeEntity(id.toDouble());
  }

  // ---------------------------------------------------------------- input

  void _onKeyDown(web.KeyboardEvent e) {
    // `is` on JS interop types checks only the Dart representation; use isA.
    if (e.target.isA<web.HTMLInputElement>()) return;
    _keys.add(e.code);
    // On a stage F is a skill slot; in town it talks to the nearest NPC.
    if (e.code == 'KeyF' && !e.repeat && !_inStage) _interactNearest();
    if (_inStage && (e.code == 'Space' || e.code == 'Tab')) e.preventDefault();
    if (e.code == 'KeyK' && !e.repeat) view.toggleSkillTree(-1);
    if (e.code == 'Escape') view.toggleSkillTree(0);
    if (e.code == 'F10') {
      e.preventDefault();
      _toggleSettings();
    }
  }

  void _onKeyUp(web.KeyboardEvent e) => _keys.remove(e.code);

  void _onClick(web.MouseEvent e) {
    final id = view.pickEntity(e.clientX.toDouble(), e.clientY.toDouble());
    if (id.isNaN || id >= 0) return;
    final npc = _npcs.where((n) => n.entityId == id).firstOrNull;
    if (npc == null) return;
    final p = _predictor;
    if (p != null && npc.inReach(p.x, p.y)) {
      _send(ClientMsg.npcInteract(npc.id));
    } else {
      view.toast('Walk closer to ${npc.name}.');
    }
  }

  void _interactNearest() {
    final npc = _nearNpc;
    if (npc != null && !view.dialogOpen()) _send(ClientMsg.npcInteract(npc.id));
  }

  void _onNpcDialog(NpcDialog msg) {
    if (msg.kind == 'stylist') {
      view.closeDialog();
      _stylist = msg.npc;
      _styling = false;
      _showStylist();
      return;
    }
    final missions = msg.missions;
    final isBoard = msg.kind == 'mission_board';
    final npc = _npcs.where((n) => n.id == msg.npc).firstOrNull;
    _settingsOpen = false;
    final inLobby = _lobby != null;
    final cutin = npc?.cutin;
    final cutinUrl = cutin == null ? null : (cutin.startsWith('/') || cutin.startsWith('http')) ? cutin : '$assetsBase/client/$cutin';
    view.showDialog(
      jsonEncode({
        'id': 'npc:${msg.npc}',
        'title': msg.name,
        if (cutinUrl != null) 'cutin': cutinUrl,
        'text': npc?.text ?? (!isBoard
            ? 'Hello there.'
            : inLobby
            ? 'You are already in a party. Leave it to pick another mission.'
            : 'Pick a mission. Parties of up to four.'),
        'options': [
          if (isBoard && !inLobby)
            for (final m in missions) ...[
              {
                'id': 'public:${m.id}',
                'label': '${m.name}: find a party',
                'detail': '${m.description} Joins an open public party or starts one (up to ${m.maxPlayers}).',
              },
              {
                'id': 'private:${m.id}',
                'label': '${m.name}: private party',
                'detail': 'Only players with your party code can join.',
              },
            ],
        ],
        if (isBoard && !inLobby) 'input': {'id': 'code', 'label': 'Join', 'placeholder': 'Party code', 'maxLength': 8},
      }),
    );
  }

  /// Party lobby panel: members with ready state, ready/start/leave.
  void _renderLobby() {
    final lobby = _lobby;
    if (lobby == null) return;
    final members = lobby.members;
    final mission = lobby.mission;
    final me = members.where((m) => m.entityId == _me).firstOrNull;
    final leader = me?.leader ?? false;
    final othersReady = members.where((m) => !m.leader).every((m) => m.ready);
    final ready = me?.ready ?? false;
    view.showPanel(
      jsonEncode({
        'id': 'lobby',
        'title': mission.name,
        'subtitle':
            '${lobby.public ? 'Public' : 'Private'} party · code ${lobby.code} · ${members.length}/${mission.maxPlayers}',
        'rows': [
          for (final m in members)
            {
              'id': '${m.entityId}',
              'text': m.name,
              'detail': m.entityId == _me ? '(you)' : null,
              'tag': m.leader ? 'leader' : (m.ready ? 'ready' : 'not ready'),
              'tagKind': m.leader ? 'lead' : (m.ready ? 'ok' : 'wait'),
              if (leader && m.entityId != _me) 'action': {'id': 'kick', 'label': 'Kick'},
            },
        ],
        'actions': [
          if (leader)
            {'id': 'start', 'label': othersReady ? 'Start' : 'Waiting…', 'primary': true, 'disabled': !othersReady}
          else
            {'id': ready ? 'unready' : 'ready', 'label': ready ? 'Not ready' : 'Ready', 'primary': !ready},
          {'id': 'leave', 'label': 'Leave'},
        ],
      }),
    );
  }

  /// In-stage panel: who is here and the way back.
  void _renderStagePanel() {
    view.showPanel(
      jsonEncode({
        'id': 'stage',
        'title': _missionName.isEmpty ? _mapName : _missionName,
        'subtitle': '$_channel · $_mapName',
        'rows': [
          {'text': _myName, 'detail': '(you)'},
          for (final p in _peers.values) {'text': p.name},
        ],
        'actions': [
          {'id': 'return', 'label': 'Return to town'},
        ],
      }),
    );
  }

  void _onPanelAction(String panel, String action, String? row) {
    switch ((panel, action)) {
      case ('lobby', 'ready'):
        _send(ClientMsg.lobbyReady(true));
      case ('lobby', 'unready'):
        _send(ClientMsg.lobbyReady(false));
      case ('lobby', 'start'):
        _send(ClientMsg.lobbyStart());
      case ('lobby', 'leave'):
        _send(ClientMsg.lobbyLeave());
      case ('lobby', 'kick'):
        final id = int.tryParse(row ?? '');
        if (id != null) _send(ClientMsg.lobbyKick(id));
      case ('stage', 'return'):
        _send(ClientMsg.stageLeave());
    }
  }

  void _onUiEvent(String json) {
    final e = jsonDecode(json) as Map<String, Object?>;
    switch (e['type']) {
      case 'dialog_choice':
        final option = e['option']! as String;
        if (option == 'code') {
          _send(ClientMsg.lobbyJoin('${e['value']}'));
        } else if (option.startsWith('public:') || option.startsWith('private:')) {
          final i = option.indexOf(':');
          _send(ClientMsg.missionSelect(option.substring(i + 1), public: option.startsWith('public:')));
        }
        view.closeDialog();
      case 'panel_action':
        _onPanelAction(e['panel']! as String, e['action']! as String, e['row'] as String?);
      case 'setting':
        _applySetting(e['id']! as String, e['value']);
      case 'dialog_close':
        if (e['dialog'] == 'settings') _settingsOpen = false;
      case 'style_apply':
        if (_stylist == null || _styling) return;
        _styling = true;
        _send(
          ClientMsg.styleSet(
            _myLook.copyWith(
              hair: (e['hair']! as num).toInt(),
              hairColor: (e['hairColor']! as num).toInt(),
              bodyColor: (e['bodyColor']! as num).toInt(),
            ),
          ),
        );
        _showStylist();
      case 'style_cancel':
        _stylist = null;
        _styling = false;
      case 'skill_learn':
        _send(ClientMsg.skillLearn(e['skill']! as String));
      case 'skill_slot':
        _send(ClientMsg.skillSlot((e['slot']! as num).toInt(), e['skill']! as String));
    }
  }

  // ---------------------------------------------------------------- skills

  void _renderSkills() {
    final s = _skills;
    if (s == null) return;
    view.setSkills(
      jsonEncode({
        'points': s.points,
        'levels': s.levels,
        'hotbar': s.hotbar,
        'level': s.level,
        'cls': _myArchetype,
        'keys': _slotCaptions,
      }),
    );
  }

  void _onCast(Cast c) {
    final own = c.caster == _me;
    view.castSkill(
      jsonEncode({
        'caster': c.caster,
        'skill': c.skill,
        'x': c.x,
        'y': c.y,
        'castMs': c.castMs,
        'cooldownMs': c.cooldownMs,
        'level': own ? (_skills?.levels[c.skill] ?? 1) : 1,
        'own': own,
      }),
    );
  }

  /// Skill slots held this tick (bit i = slot i); only on a stage, standing
  /// up, with no window open.
  int _heldSlots() {
    if (!_inStage || _myState != 0 || view.dialogOpen()) return 0;
    final shift = _keys.contains('ShiftLeft') || _keys.contains('ShiftRight');
    var bits = 0;
    for (var i = 0; i < _slotKeys.length; i++) {
      if (_keys.contains(_slotKeys[i])) bits |= 1 << (shift ? i + 5 : i);
    }
    for (var i = 0; i < 10; i++) {
      if (_keys.contains('Digit${(i + 1) % 10}')) bits |= 1 << i;
    }
    return bits;
  }

  /// Pointer on the ground relative to the player, in 1/8 m (0,0 = none).
  (int, int) _aim() {
    final ptr = _pointer, p = _predictor;
    if (ptr == null || p == null) return (0, 0);
    final g = view.pickGround(ptr.$1, ptr.$2);
    if (g == null) return (0, 0);
    // Cells are 0.5 m: 4 aim units per cell.
    return (((g.$1 - p.x) * 4).round().clamp(-127, 127), ((g.$2 - p.y) * 4).round().clamp(-127, 127));
  }

  // ---------------------------------------------------------------- settings

  Map<String, Object?> _settings = {};
  bool _settingsOpen = false;

  void _loadSettings() {
    try {
      final raw = web.window.localStorage.getItem(_settingsKey);
      if (raw != null) _settings = (jsonDecode(raw) as Map).cast<String, Object?>();
    } on Object catch (_) {
      _settings = {};
    }
    // The engine ignores keys it does not know (the client's own ones).
    view.configure(jsonEncode(_settings));
    _frameCap = (_settings['frameCap'] as num?)?.toInt() ?? 0;
    _clock.bias = ((_settings['interpBiasMs'] as num?) ?? 0) / 1000;
  }

  void _applySetting(String id, Object? value) {
    _settings[id] = value;
    switch (id) {
      case 'frameCap':
        _frameCap = (value! as num).toInt();
      case 'interpBiasMs':
        _clock.bias = (value! as num) / 1000;
      default:
        view.configure(jsonEncode({id: value}));
    }
    try {
      web.window.localStorage.setItem(_settingsKey, jsonEncode(_settings));
    } on Object catch (_) {
      // Private mode or blocked storage: settings last for this session.
    }
  }

  void _toggleSettings() {
    if (view.dialogOpen()) {
      view.closeDialog();
      _settingsOpen = false;
      return;
    }
    _settingsOpen = true;
    final e = jsonDecode(view.configure('{}')) as Map<String, Object?>;
    Map<String, Object?> range(String id, String label, num min, num max, num step, Object? value, [String? hint]) => {
      'id': id,
      'label': label,
      'type': 'range',
      'min': min,
      'max': max,
      'step': step,
      'value': value,
      'hint': ?hint,
    };
    view.showSettings(
      jsonEncode({
        'title': 'Settings',
        'fields': [
          {
            'id': 'frameCap',
            'label': 'Frame rate cap',
            'type': 'select',
            'value': _frameCap,
            'options': [
              {'value': 0, 'label': 'Display'},
              {'value': 30, 'label': '30'},
              {'value': 60, 'label': '60'},
              {'value': 90, 'label': '90'},
            ],
          },
          range('renderScale', 'Render scale', 0.25, 1, 0.05, e['renderScale'], 'Fraction of screen pixels drawn.'),
          range('maxPixelRatio', 'Max pixel ratio', 1, 3, 0.25, e['maxPixelRatio']),
          {'id': 'pixelated', 'label': 'Pixelated upscale', 'type': 'toggle', 'value': e['pixelated']},
          range(
            'interpBiasMs',
            'Extra buffering, ms',
            0,
            200,
            10,
            (_clock.bias * 1000).round(),
            'Smoother remote movement on unstable links, at the cost of latency.',
          ),
          range(
            'stepHz',
            'Monster step rate, Hz',
            0,
            30,
            1,
            e['stepHz'],
            '0 = smooth. Low values give a stepped retro look.',
          ),
          range('highStepHz', 'Player/boss step rate, Hz', 0, 60, 1, e['highStepHz'], '0 = smooth.'),
          range('animLodNear', 'Full-rate animation radius, m', 4, 40, 1, e['animLodNear']),
          range('animLodFar', 'Reduced animation beyond, m', 8, 80, 1, e['animLodFar']),
          {'id': 'water', 'label': 'Animated water', 'type': 'toggle', 'value': e['water']},
          {'id': 'props', 'label': 'Props (buildings, trees)', 'type': 'toggle', 'value': e['props']},
          {'id': 'cutout', 'label': 'See-through buildings', 'type': 'toggle', 'value': e['cutout']},
          range('cutoutRadius', 'See-through radius, m', 0.5, 4, 0.1, e['cutoutRadius']),
          {'id': 'fog', 'label': 'Animated fog beyond the light', 'type': 'toggle', 'value': e['fog']},
        ],
      }),
    );
  }

  /// WASD / arrows → world direction (x east, y north), turned by the camera
  /// yaw so "up" always walks away from the camera.
  (double, double) _inputDir() {
    var sx = 0.0, sy = 0.0;
    if (_keys.contains('KeyW') || _keys.contains('ArrowUp')) sy += 1;
    if (_keys.contains('KeyS') || _keys.contains('ArrowDown')) sy -= 1;
    if (_keys.contains('KeyD') || _keys.contains('ArrowRight')) sx += 1;
    if (_keys.contains('KeyA') || _keys.contains('ArrowLeft')) sx -= 1;
    if (sx == 0 && sy == 0) return (0, 0);
    if (view.dialogOpen()) return (0, 0);
    final len = math.sqrt(sx * sx + sy * sy);
    sx /= len;
    sy /= len;
    final yaw = view.cameraYaw();
    final c = math.cos(yaw), s = math.sin(yaw);
    // Yaw is clockwise from north: rotate the screen vector clockwise.
    return quantizeDir(sx * c + sy * s, -sx * s + sy * c);
  }

  // ---------------------------------------------------------------- loop

  void _tick() {
    final p = _predictor;
    if (p == null || _loading) return;
    // Downed or fallen: the server ignores movement; do not predict any.
    final (dx, dy) = _myState == 0 ? _inputDir() : (0.0, 0.0);
    final moving = dx != 0 || dy != 0;
    final slots = _heldSlots();
    if (moving || slots != 0) _stopSends = 3;
    if (moving || slots != 0 || _stopSends > 0) {
      if (!moving && slots == 0) _stopSends--;
      final (ax, ay) = slots != 0 ? _aim() : (0, 0);
      final seq = p.step(dx, dy);
      _ws.push(encodeInput(seq, dx, dy, aimX: ax, aimY: ay, slots: slots)).ignore();
    } else {
      p.idle();
    }
  }

  /// Render-clock seconds for a (fractional) server tick on this map.
  double _sec(num tick) => (tick - (_epochTick ?? 0)) / _tickHz;

  void _frame(num _) {
    web.window.requestAnimationFrame(_frame.toJS);
    final now = _now();
    final dt = _lastFrame == 0 ? 0.0 : math.min(0.25, now - _lastFrame);
    _lastFrame = now;
    final tickDt = 1 / _tickHz;
    _acc += dt;
    while (_acc >= tickDt) {
      _acc -= tickDt;
      _tick();
    }
    final p = _predictor;
    if (p == null || _loading) {
      view.renderFrame(0);
      return;
    }
    // Frame cap: input and prediction keep ticking, only drawing is skipped.
    if (_frameCap > 0 && now - _lastRender < 1 / _frameCap - 0.002) return;
    final renderDt = _lastRender == 0 ? dt : math.min(0.25, now - _lastRender);
    _lastRender = now;

    // The local player is the only entity written every frame.
    final (mx, my) = p.render(_acc / tickDt, renderDt);
    final me = _me ?? 0;
    _out.add(
      id: me.toDouble(),
      x0: mx,
      y0: my,
      t0: 0,
      x1: mx,
      y1: my,
      t1: 0,
      facing: !p.moving && now < (_attackUntil[me] ?? 0) ? _attackFacing[me]! : p.facing,
      sprite: _spriteFor(me, _myArchetype, _myLook),
      action: _charAction(me, p.moving, now),
      flags: Row.flagFocus | Row.flagHigh,
    );

    // Remote players: a row only when the bracketing snapshot pair, facing
    // or action changes (about once per server tick while moving, never
    // while standing). The shader interpolates in between.
    final rt = _clock.renderTick(now);
    final stale = <int>[];
    for (final t in _tracks.values) {
      if (now - (_lastSeen[t.id] ?? now) > 3) {
        stale.add(t.id);
        continue;
      }
      t.segment(rt);
      final action = _charAction(t.id, t.moving, now);
      final peer = _peers[t.id];
      final sprite = _spriteFor(t.id, peer?.archetype ?? 'novice', peer?.look);
      final still = t.x0 == t.x1 && t.y0 == t.y1;
      if (t.sentFacing == t.facing && t.sentAction == action && t.sentSprite == sprite) {
        if (still ? (t.sentX == t.x1 && t.sentY == t.y1) : (t.sentTick0 == t.tick0 && t.sentTick1 == t.tick1)) {
          continue;
        }
      }
      _out.add(
        id: t.id.toDouble(),
        x0: t.x0,
        y0: t.y0,
        t0: _sec(t.tick0),
        x1: t.x1,
        y1: t.y1,
        t1: still ? _sec(t.tick0) : _sec(t.tick1),
        facing: t.facing,
        sprite: sprite,
        action: action,
        flags: Row.flagHigh,
      );
      t
        ..sentTick0 = t.tick0
        ..sentTick1 = t.tick1
        ..sentFacing = t.facing
        ..sentAction = action
        ..sentSprite = sprite
        ..sentX = still ? t.x1 : double.nan
        ..sentY = still ? t.y1 : double.nan;
    }
    stale.forEach(_removePeer);

    // Monsters: one row per record change. The row is the record's
    // dead-reckoning line from `stamp − back` to `stamp + horizon`; the
    // shader walks it, so a monster on a straight path costs nothing here.
    final removed = <int>[];
    for (final m in _mobs.values) {
      final at = m.removeAt;
      final dieAt = m.dieAt;
      // A killed monster finishes its death animation before it goes.
      if (at != null && rt >= at && (dieAt == null || now - dieAt > 0.75)) {
        removed.add(m.slot);
        continue;
      }
      final action = dieAt != null
          ? Act.die
          : now < m.attackUntil
          ? Act.attack
          : now < m.hitUntil
          ? Act.hit
          : m.moving
          ? Act.walk
          : Act.idle;
      if (!m.dirty && action == m.sentAction) continue;
      m.sentAction = action;
      final sprite = _mobSprite(m.kind);
      final px = m.x / Reckoning.posOne, py = m.y / Reckoning.posOne;
      final vx = m.vx / Reckoning.velOne, vy = m.vy / Reckoning.velOne;
      const back = Reckoning.back, ahead = Reckoning.horizon;
      if (dieAt != null) {
        // Frozen where it stood at the render clock.
        final u = (rt - m.stamp).clamp(-back, ahead);
        final (x, y) = (px + vx * u, py + vy * u);
        _out.add(
          id: m.id,
          x0: x,
          y0: y,
          t0: _sec(rt),
          x1: x,
          y1: y,
          t1: _sec(rt),
          facing: m.facing,
          sprite: sprite,
          action: action,
          flags: Row.flagFade,
        );
        m.dirty = false;
        continue;
      }
      final moving = m.vx != 0 || m.vy != 0;
      _out.add(
        id: m.id,
        x0: moving ? px - vx * back : px,
        y0: moving ? py - vy * back : py,
        t0: _sec(moving ? m.stamp - back : m.stamp),
        x1: moving ? px + vx * ahead : px,
        y1: moving ? py + vy * ahead : py,
        t1: _sec(moving ? m.stamp + ahead : m.stamp),
        facing: m.facing,
        sprite: sprite,
        action: action,
        flags: Row.flagFade,
      );
      m.dirty = false;
    }
    for (final slot in removed) {
      view.removeEntity(_mobs.remove(slot)!.id);
    }

    _nearNpc = null;
    for (final npc in _npcs) {
      if (npc.inReach(p.x, p.y)) _nearNpc = npc;
    }
    if (_nearNpc == null && view.dialogOpen() && !_settingsOpen) view.closeDialog();
    view.setHint(_nearNpc != null && !view.dialogOpen() ? 'F: talk to ${_nearNpc!.name}' : null);

    _out.flush(view);
    view.renderFrame(_clock.synced ? _sec(rt) : 0);
    _updateStats(now, p);
  }

  final _spriteCache = <String, int>{};

  /// Sprite for a character: the dyeable body of its gender (hair and dyes
  /// come from `setLook`); without the customization pack,
  /// `<archetype>_m|_f` when the atlas has it, else the novice placeholder.
  int _spriteFor(int id, String archetype, Look? look) {
    final gender = look?.gender ?? (id.isEven ? 0 : 1);
    final body = _spriteCache['$_mapId:body:$gender'] ??= view.bodySpriteIndex(gender);
    if (body >= 0) return body;
    final g = gender == 0 ? 'm' : 'f';
    final key = '$_mapId:$archetype:$g';
    return _spriteCache[key] ??= () {
      final s = view.spriteIndex('${archetype}_$g');
      return s >= 0 ? s : view.spriteIndex('novice_$g');
    }();
  }

  /// Sprite of a monster kind in the current location atlas (-1 when the
  /// atlas lacks it; the engine skips such rows).
  int _mobSprite(int kind) {
    final name = kind < _monsterKinds.length ? _monsterKinds[kind] : '';
    return _spriteCache['$_mapId:mob:$name'] ??= view.spriteIndex(name);
  }

  void _updateStats(double now, LocalPredictor p) {
    if (now - _rateWindowStart >= 1) {
      final span = now - _rateWindowStart;
      _snapshotRate = _snapshots / span;
      final bytesIn = _ws.metrics.bytesReceived;
      _bytesInRate = (bytesIn - _lastBytesIn) / span;
      _lastBytesIn = bytesIn;
      _snapshots = 0;
      _rateWindowStart = now;
    }
    final stats = jsonDecode(view.stats()) as Map<String, Object?>;
    view.updateHud(
      '$_mapName · $_channel · ${_peers.length + 1} here'
      '${_inStage ? ' · ${_mobs.length} monsters in view' : ''}\n'
      'rtt ${_rttMs.toStringAsFixed(0)} ms · interp ${(_clock.delay * 1000).toStringAsFixed(0)} ms'
      ' (jitter ${(_clock.jitter * 1000).toStringAsFixed(1)})\n'
      'snapshots ${_snapshotRate.toStringAsFixed(0)}/s · in ${(_bytesInRate / 1024).toStringAsFixed(1)} KB/s'
      ' · pending inputs ${p.pendingCount}\n'
      'fps ${stats['fps']} · draw calls ${stats['drawCalls']} · units ${stats['units']}',
    );
  }
}
