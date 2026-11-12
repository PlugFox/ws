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
      x = (j['x']! as num).toDouble() + 0.5,
      y = (j['y']! as num).toDouble() + 0.5,
      facing = (j['facing'] as num?)?.toInt() ?? 4,
      radius = (j['interactRadius'] as num?)?.toDouble() ?? 3;

  final int index;
  final String id;
  final String name;
  final String sprite;
  final String kind;
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

final class _Peer {
  _Peer(this.name, this.archetype);
  final String name;
  final String archetype;
}

/// One connected hub session: network, prediction, interpolation and the
/// render loop feeding the engine façade.
final class HubSession {
  HubSession(this.api, this.view, {required this.assetsBase, required String characterName}) : _myName = characterName;

  final Api api;
  final GameView view;
  final String assetsBase;

  late final WebSocketClient<Uint8List> _ws;
  final _subs = <StreamSubscription<Object?>>[];

  int? _me;
  String _myName;
  String _myArchetype = 'novice';
  String _channel = '-';

  /// In a stage instance (not the hub).
  bool _inStage = false;

  /// Current lobby (`lobby_state.lobby`), or null.
  Map<String, Object?>? _lobby;
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
  var _npcs = <_Npc>[];
  _Npc? _nearNpc;

  final _keys = <String>{};
  int _stopSends = 0;
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
          _send({'type': 'hub_join'});
        }
      }),
    );
    view.onUiEvent(_onUiEvent.toJS);
    _loadSettings();
    web.window.addEventListener('keydown', _onKeyDown.toJS);
    web.window.addEventListener('keyup', _onKeyUp.toJS);
    web.window.addEventListener('blur', ((web.Event _) => _keys.clear()).toJS);
    (web.document.querySelector('canvas')! as web.HTMLCanvasElement).addEventListener('click', _onClick.toJS);

    final loc = web.window.location;
    final scheme = loc.protocol == 'https:' ? 'wss' : 'ws';
    await _ws.connect('$scheme://${loc.host}/api/ws?token=${Uri.encodeQueryComponent(token)}');
    Timer.periodic(const Duration(seconds: 2), (_) {
      _pingSentAt = _now();
      _send({'type': 'ping'});
    });
    // Read-only probe for scripted browser tests and bots.
    globalContext.setProperty('gameDebug'.toJS, (() {
      final p = _predictor;
      return jsonEncode({'map': _mapId, 'loading': _loading, 'x': p?.x, 'y': p?.y, 'peers': _peers.length});
    }).toJS);
    _frame(0);
  }

  void _send(Map<String, Object?> msg) {
    if (_ws.state is! WebSocketState$Open) return;
    _ws.push(utf8.encode(jsonEncode(msg))).ignore();
  }

  // ---------------------------------------------------------------- network

  void _onMessage(Uint8List data) {
    if (data.isEmpty) return;
    if (data[0] == Snapshot.kind) {
      final s = Snapshot.decode(data);
      if (s != null) _onSnapshot(s);
      return;
    }
    if (data[0] != 0x7B) return; // '{'
    final msg = jsonDecode(utf8.decode(data)) as Map<String, Object?>;
    switch (msg['type']) {
      case 'connected':
        break;
      case 'pong':
        if (_pingSentAt > 0) _rttMs = (_now() - _pingSentAt) * 1000;
      case 'hub_joined':
        _me = (msg['entity_id']! as num).toInt();
        _channel = msg['channel_name']! as String;
        _tickHz = (msg['tick_rate']! as num).toInt();
        _clock = ServerClock(tickHz: _tickHz);
        if (_inStage) {
          _inStage = false;
          view.showPanel('');
        }
      case 'stage_joined':
        _me = (msg['entity_id']! as num).toInt();
        _channel = 'Stage-${msg['stage_id']}';
        _tickHz = (msg['tick_rate']! as num).toInt();
        _clock = ServerClock(tickHz: _tickHz);
        _inStage = true;
        _renderStagePanel();
      case 'lobby_state':
        _lobby = msg['lobby']! as Map<String, Object?>;
        _missionName = (_lobby!['mission']! as Map<String, Object?>)['name']! as String;
        _renderLobby();
      case 'lobby_left':
        _lobby = null;
        if (!_inStage) view.showPanel('');
        switch (msg['reason']) {
          case 'kicked':
            view.toast('You were removed from the party.');
          case 'replaced':
            view.toast('Your party moved to another tab.');
          case 'launched':
            view.toast('$_missionName is starting!');
        }
      case 'lobby_error':
        view.toast('${msg['message']}');
      case 'map_enter':
        _onMapEnter(msg).catchError((Object e, StackTrace st) {
          web.console.error('map_enter failed: $e\n$st'.toJS);
          view.toast('Could not load ${(msg['map'] as Map?)?['id']}: $e');
        });
      case 'player_appeared':
        _addPeer(msg['player']! as Map<String, Object?>);
        if (_inStage) _renderStagePanel();
      case 'player_vanished':
        _removePeer((msg['entity_id']! as num).toInt());
        if (_inStage) _renderStagePanel();
      case 'portal_denied':
        view.toast('${msg['target_map']} is reached through the mission board.');
      case 'npc_dialog':
        _onNpcDialog(msg);
      case 'hub_error':
        view.toast('${msg['message']}');
      case 'hub_left':
        view.toast('Left the hub (${msg['reason']}).');
      case 'displaced':
        view.toast('Signed in elsewhere; this tab is disconnected.');
        _ws.close().ignore();
      default:
        break;
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
  }

  Future<void> _onMapEnter(Map<String, Object?> msg) async {
    final token = ++_loadToken;
    _loading = true;
    final map = msg['map']! as Map<String, Object?>;
    _mapEpoch = (msg['map_epoch']! as num).toInt();
    _mapId = map['id']! as String;
    _mapName = map['name']! as String;
    final x = (msg['x']! as num).toDouble(), y = (msg['y']! as num).toDouble();
    view.closeDialog();
    for (final id in _tracks.keys.toList()) {
      _removePeer(id);
    }
    for (final npc in _npcs) {
      view.setLabel(npc.entityId, null, '');
    }
    _clock.reset();
    _epochTick = null;
    for (final p in (msg['players']! as List).cast<Map<String, Object?>>()) {
      _addPeer(p);
    }

    final base = '$assetsBase/client/locations/$_mapId';
    final results = await Future.wait([
      api.json('$base/manifest.json'),
      api.bytes('$assetsBase/server/locations/$_mapId/walkability.bin'),
      view.loadLocation('$base/manifest.json').toDart,
    ]);
    if (token != _loadToken) return; // a newer map_enter won the race
    final manifest = results[0]! as Map<String, Object?>;
    final grid = WalkGrid.parse(results[1]! as Uint8List);

    view.setCamera(jsonEncode(map['camera']));
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
    final me = _me;
    if (me != null) view.setLabel(me.toDouble(), _myName, 'me');
    for (final e in _peers.entries) {
      view.setLabel(e.key.toDouble(), e.value.name, '');
    }
    final bgm = map['bgm'] as String?;
    view.playBgm(bgm == null ? null : '$assetsBase/client/$bgm');
    _loading = false;
    if (_inStage) {
      _renderStagePanel();
      view.toast('$_mapName: the mission has begun. Walk to the exit or use the panel to return.');
    } else if (msg['via_portal'] == null) {
      view.toast('Welcome to $_mapName ($_channel). WASD to move, F to talk.');
    }
  }

  void _addPeer(Map<String, Object?> p) {
    final id = (p['entity_id']! as num).toInt();
    final name = p['name']! as String;
    final archetype = p['archetype']! as String;
    if (id == _me) {
      _myName = name;
      _myArchetype = archetype;
      return;
    }
    _peers[id] = _Peer(name, archetype);
    final t = _tracks[id] ??= RemoteTrack(id);
    if (t.isEmpty) {
      t
        ..x = (p['x']! as num).toDouble()
        ..y = (p['y']! as num).toDouble()
        ..facing = (p['facing']! as num).toInt();
    }
    _lastSeen[id] = _now();
    view.setLabel(id.toDouble(), name, '');
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
    if (e.code == 'KeyF' && !e.repeat) _interactNearest();
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
      _send({'type': 'npc_interact', 'npc': npc.id});
    } else {
      view.toast('Walk closer to ${npc.name}.');
    }
  }

  void _interactNearest() {
    final npc = _nearNpc;
    if (npc != null && !view.dialogOpen()) _send({'type': 'npc_interact', 'npc': npc.id});
  }

  void _onNpcDialog(Map<String, Object?> msg) {
    final missions = (msg['missions']! as List).cast<Map<String, Object?>>();
    final isBoard = msg['kind'] == 'mission_board';
    _settingsOpen = false;
    final inLobby = _lobby != null;
    view.showDialog(
      jsonEncode({
        'id': 'npc:${msg['npc']}',
        'title': msg['name'],
        'text': !isBoard
            ? 'Hello there.'
            : inLobby
            ? 'You are already in a party. Leave it to pick another mission.'
            : 'Pick a mission. Parties of up to four.',
        'options': [
          if (isBoard && !inLobby)
            for (final m in missions) ...[
              {
                'id': 'public:${m['id']}',
                'label': '${m['name']}: find a party',
                'detail': '${m['description']} Joins an open public party or starts one (up to ${m['max_players']}).',
              },
              {
                'id': 'private:${m['id']}',
                'label': '${m['name']}: private party',
                'detail': 'Only players with your party code can join.',
              },
            ],
        ],
        if (isBoard && !inLobby)
          'input': {'id': 'code', 'label': 'Join', 'placeholder': 'Party code', 'maxLength': 8},
      }),
    );
  }

  /// Party lobby panel: members with ready state, ready/start/leave.
  void _renderLobby() {
    final lobby = _lobby;
    if (lobby == null) return;
    final members = (lobby['members']! as List).cast<Map<String, Object?>>();
    final mission = lobby['mission']! as Map<String, Object?>;
    final me = members.where((m) => m['entity_id'] == _me).firstOrNull;
    final leader = me?['leader'] == true;
    final othersReady = members.where((m) => m['leader'] != true).every((m) => m['ready'] == true);
    final max = mission['max_players'];
    view.showPanel(
      jsonEncode({
        'id': 'lobby',
        'title': mission['name'],
        'subtitle': '${lobby['public'] == true ? 'Public' : 'Private'} party · code ${lobby['code']} · ${members.length}/$max',
        'rows': [
          for (final m in members)
            {
              'id': '${m['entity_id']}',
              'text': m['name'],
              'detail': m['entity_id'] == _me ? '(you)' : null,
              'tag': m['leader'] == true ? 'leader' : (m['ready'] == true ? 'ready' : 'not ready'),
              'tagKind': m['leader'] == true ? 'lead' : (m['ready'] == true ? 'ok' : 'wait'),
              if (leader && m['entity_id'] != _me) 'action': {'id': 'kick', 'label': 'Kick'},
            },
        ],
        'actions': [
          if (leader)
            {'id': 'start', 'label': othersReady ? 'Start' : 'Waiting…', 'primary': true, 'disabled': !othersReady}
          else
            {'id': me?['ready'] == true ? 'unready' : 'ready', 'label': me?['ready'] == true ? 'Not ready' : 'Ready', 'primary': me?['ready'] != true},
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
        _send({'type': 'lobby_ready', 'ready': true});
      case ('lobby', 'unready'):
        _send({'type': 'lobby_ready', 'ready': false});
      case ('lobby', 'start'):
        _send({'type': 'lobby_start'});
      case ('lobby', 'leave'):
        _send({'type': 'lobby_leave'});
      case ('lobby', 'kick'):
        final id = int.tryParse(row ?? '');
        if (id != null) _send({'type': 'lobby_kick', 'entity_id': id});
      case ('stage', 'return'):
        _send({'type': 'stage_leave'});
    }
  }

  void _onUiEvent(String json) {
    final e = jsonDecode(json) as Map<String, Object?>;
    switch (e['type']) {
      case 'dialog_choice':
        final option = e['option']! as String;
        if (option == 'code') {
          _send({'type': 'lobby_join', 'code': e['value']});
        } else if (option.startsWith('public:') || option.startsWith('private:')) {
          final i = option.indexOf(':');
          _send({'type': 'mission_select', 'mission': option.substring(i + 1), 'public': option.startsWith('public:')});
        }
        view.closeDialog();
      case 'panel_action':
        _onPanelAction(e['panel']! as String, e['action']! as String, e['row'] as String?);
      case 'setting':
        _applySetting(e['id']! as String, e['value']);
      case 'dialog_close':
        if (e['dialog'] == 'settings') _settingsOpen = false;
    }
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
          range('stepHz', 'Monster step rate, Hz', 0, 30, 1, e['stepHz'], '0 = smooth. Low values give a stepped retro look.'),
          range('highStepHz', 'Player/boss step rate, Hz', 0, 60, 1, e['highStepHz'], '0 = smooth.'),
          range('animLodNear', 'Full-rate animation radius, m', 4, 40, 1, e['animLodNear']),
          range('animLodFar', 'Reduced animation beyond, m', 8, 80, 1, e['animLodFar']),
          {'id': 'water', 'label': 'Animated water', 'type': 'toggle', 'value': e['water']},
          {'id': 'props', 'label': 'Props (buildings, trees)', 'type': 'toggle', 'value': e['props']},
          {'id': 'cutout', 'label': 'See-through buildings', 'type': 'toggle', 'value': e['cutout']},
          range('cutoutRadius', 'See-through radius, m', 0.5, 4, 0.1, e['cutoutRadius']),
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
    final (dx, dy) = _inputDir();
    final moving = dx != 0 || dy != 0;
    if (moving) _stopSends = 3;
    if (moving || _stopSends > 0) {
      if (!moving) _stopSends--;
      final seq = p.step(dx, dy);
      _ws.push(encodeInput(seq, dx, dy)).ignore();
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
      facing: p.facing,
      sprite: _spriteFor(me, _myArchetype),
      action: p.moving ? Act.walk : Act.idle,
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
      final action = t.moving ? Act.walk : Act.idle;
      final sprite = _spriteFor(t.id, _peers[t.id]?.archetype ?? 'novice');
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

  /// Sprite for a character: `<archetype>_m|_f` when the atlas has it, else
  /// the novice placeholder (gender is not stored yet; id parity stands in).
  int _spriteFor(int id, String archetype) {
    final g = id.isEven ? 'm' : 'f';
    final key = '$_mapId:$archetype:$g';
    return _spriteCache[key] ??= () {
      final s = view.spriteIndex('${archetype}_$g');
      return s >= 0 ? s : view.spriteIndex('novice_$g');
    }();
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
      '$_mapName · $_channel · ${_peers.length + 1} here\n'
      'rtt ${_rttMs.toStringAsFixed(0)} ms · interp ${(_clock.delay * 1000).toStringAsFixed(0)} ms'
      ' (jitter ${(_clock.jitter * 1000).toStringAsFixed(1)})\n'
      'snapshots ${_snapshotRate.toStringAsFixed(0)}/s · in ${(_bytesInRate / 1024).toStringAsFixed(1)} KB/s'
      ' · pending inputs ${p.pendingCount}\n'
      'fps ${stats['fps']} · draw calls ${stats['drawCalls']} · units ${stats['units']}',
    );
  }
}
