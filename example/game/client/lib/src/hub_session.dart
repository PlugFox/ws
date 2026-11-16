import 'dart:async';
import 'dart:convert';
import 'dart:js_interop';
import 'dart:js_interop_unsafe';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:game_client/src/api.dart';
import 'package:game_client/src/auto_use.dart';
import 'package:game_client/src/cast_pose.dart';
import 'package:game_client/src/contacts.dart';
import 'package:game_client/src/changelog.dart';
import 'package:game_client/src/leaderboard.dart';
import 'package:game_client/src/online.dart';
import 'package:game_client/src/classes.dart';
import 'package:game_client/src/click_move.dart';
import 'package:game_client/src/engine.dart';
import 'package:game_client/src/env_events.dart';
import 'package:game_client/src/feedback.dart';
import 'package:game_client/src/guide.dart';
import 'package:game_client/src/input.dart';
import 'package:game_client/src/input_devices.dart';
import 'package:game_client/src/interpolation.dart';
import 'package:game_client/src/key_bindings.dart';
import 'package:game_client/src/prediction.dart';
import 'package:game_client/src/parties.dart';
import 'package:game_client/src/protocol.dart';
import 'package:game_client/src/refine.dart';
import 'package:game_client/src/sheet.dart';
import 'package:game_client/src/tips.dart';
import 'package:game_client/src/update.dart';
import 'package:game_client/src/version.dart';
import 'package:game_client/src/walk_grid.dart';
import 'package:web/web.dart' as web;
import 'package:ws/ws.dart';

/// Input slot bit of the travel key (server `cast::TRAVEL_SLOT`).
const travelSlot = 15;

/// After a finished run's countdown, how long the client waits for the
/// server to send it home before asking itself (`STAGE_LEAVE`), ms.
const returnGraceMs = 2500;

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

  /// Faces its target while the attack plays (a crowd push may move it).
  int attackFacing = 4;
  double hitUntil = 0;
  double? dieAt;
  int sentAction = -1;

  /// [MobStatus] bits.
  int status = 0;

  /// Burrowed as last drawn, and when its digging is next heard.
  bool burrowed = false;
  double digAt = 0;

  /// Engine row flags and animation speed of the conditions.
  int get statusFlags => statusRowFlags(status);

  /// Held monsters stop mid-frame; slowed ones animate at half speed.
  double get animSpeed => status & MobStatus.held != 0
      ? 0.001
      : status & MobStatus.slowed != 0
      ? 0.5
      : 1;

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

/// Engine row flags of condition bits ([MobStatus]), for monsters and
/// players alike.
int statusRowFlags(int status) =>
    (status & MobStatus.frozen != 0 ? Row.flagFrozen : 0) |
    (status & MobStatus.petrified != 0 ? Row.flagPetrified : 0) |
    (status & MobStatus.stunned != 0 ? Row.flagStunned : 0) |
    (status & MobStatus.slowed != 0 ? Row.flagSlowed : 0) |
    (status & MobStatus.blinded != 0 ? Row.flagBlinded : 0) |
    (status & MobStatus.burning != 0 ? Row.flagBurning : 0) |
    (status & MobStatus.poisoned != 0 ? Row.flagPoisoned : 0) |
    (status & MobStatus.taunted != 0 ? Row.flagTaunted : 0) |
    (status & MobStatus.silenced != 0 ? Row.flagSilenced : 0) |
    (status & MobStatus.burrowed != 0 ? Row.flagBurrowed : 0) |
    (status & MobStatus.cloaked != 0 ? Row.flagCloaked : 0);

final class _Peer {
  _Peer(this.name, this.archetype, this.appearance);
  final String name;
  final String archetype;

  /// How it is drawn, whole (`PlayerInfo.appearance`, then
  /// `player_appearance`): body tier, look, weapon, headgear, mount.
  Appearance appearance;

  Look get look => appearance.look;
  int get tier => appearance.tier;
  bool get riding => appearance.riding;

  /// Character level (`PlayerInfo.level`), for the player menu.
  int level = 1;

  /// Their connection dropped (`player_link`): still on the stage, marked.
  bool linkDead = false;

  /// Town pets (`pet:<sprite>`) are drawn with a location sprite, no look.
  String? get petSprite => archetype.startsWith('pet:') ? archetype.substring(4) : null;
}

/// One connected hub session: network, prediction, interpolation and the
/// render loop feeding the engine façade.
final class HubSession {
  HubSession(
    this.api,
    this.view, {
    required this.assetsBase,
    required this.characterId,
    required String characterName,
    String archetype = 'novice',
    int level = 1,
    Look look = const Look(),
    Map<String, Object?> stats = const {},
    int exp = 0,
  }) : _myName = characterName,
       _myArchetype = archetype,
       _myLevel = level,
       _myLook = look,
       _stats = stats,
       _myExp = exp;

  final Api api;
  final GameView view;
  final String assetsBase;

  /// The selected character (`POST /api/characters/{id}/select`).
  final int characterId;

  late final WebSocketClient<Uint8List> _ws;
  final _subs = <StreamSubscription<Object?>>[];

  int? _me;
  String _myName;
  String _myArchetype;
  Look _myLook;

  /// Character level at sign-in; `Progress` / `Skills` carry the live one.
  final int _myLevel;

  /// Character stats at sign-in (`max_hp`, `max_sp`, `exp_to_next`, ...)
  /// and experience: the town's Basic Info until the server sends vitals.
  Map<String, Object?> _stats;
  final int _myExp;

  /// The player's live combat numbers (`PLAYER_SHEET`: buffs, auras and
  /// conditions included); the Equipment window prefers them to [_stats].
  Map<String, Object?>? _liveSheet;

  /// The player the inspect window was opened on, their gear and their
  /// live numbers; [_inspectTimer] asks again while the window shows them.
  int? _inspectId;
  PlayerInspect? _inspected;
  Map<String, Object?>? _inspectedSheet;
  Timer? _inspectTimer;

  /// The stylist window is open (NPC id), and a restyle is in flight.
  String? _stylist;

  /// The smith the refine window is for (id, name) while it is open, and
  /// the row of the attempt on its way (null: none; its answer plays in the
  /// window, which then lists the item under its new key).
  ({String id, String name})? _refiner;
  String? _refining;

  /// The Hatter whose recipe window is open (NPC id), and a craft in flight.
  String? _hatter;
  bool _crafting = false;

  /// The Kafra's party list was asked for: the answer opens it.
  bool _partiesAsked = false;
  bool _styling = false;
  String _channel = '-';

  /// In a stage instance (not the hub).
  bool _inStage = false;

  /// Cards dropped on this run, shown (turned over) back in town.
  final _newCards = <String>[];

  /// Players online (beside the town's minimap, the Ranking tab's footer).
  late final _pulse = OnlinePoller(
    () => api.json('/api/online'),
    (_) => _showOnline(),
    every: const Duration(seconds: 30),
  );

  /// Pages of the Ranking tab (`leaderboard_query`).
  late final _board = LeaderboardClient(
    api.json,
    (String j) => view.setLeaderboard(j),
    me: () => _me,
    online: () => _pulse.last?.down == false ? _pulse.last!.online : null,
  );

  /// Own belongings (`inventory`), the catalogue and the open merchant.
  Inventory? _inventory;
  final _itemDefs = <String, ItemDef>{};
  String? _shopNpc;

  /// The account's party (`LobbyState`), or null.
  LobbyInfo? _lobby;

  /// When we last asked to form or join a party (a mission pick, a code,
  /// an invitation): the party that forms then opens the Party window.
  double _partyAsked = 0;
  String _missionName = '';

  /// The town greeting listed the keys already (once per visit).
  bool _welcomed = false;

  /// The own stage's mission id (the timeline) and the mission giver's
  /// offer (the mission window).
  String _stageMission = '';

  /// The run's map events warned of or running (their banners and edge
  /// warnings come down on a new instance: [_endEnvEvents]).
  final _envEvents = EnvEventLog();

  /// Mission modifiers (server `world::modifiers`): the table, the lobby's
  /// choice, a choice made before a party exists (sent once our own new
  /// lobby arrives), and a won run's clear bonus for the result screen.
  ModifierDefs? _modDefs;
  ModifierState? _lobbyMods;
  (int tier, int mask)? _pendingMods;

  /// The last mission's end and party statistics, until `RUN_REWARDS`
  /// completes the result screen.
  StageEnd? _stageEnd;

  /// The end-of-run "Return to town" call is up (until the hub takes us),
  /// and the backstop that asks to leave if the server's own move is late.
  bool _returnCall = false;
  Timer? _returnTimer;
  RunStats? _runStats;

  /// The last mission briefing (the loading screen's tips).
  MissionBrief? _brief;
  List<MissionInfo> _offer = const [];
  String _offerFrom = '';

  /// The mission ladder of the last mission giver (`MISSION_LIST`): the
  /// expedition window's cards.
  List<MissionCard> _ladder = const [];

  /// The expedition window is open (a late `MISSION_LIST` redraws it).
  bool _missionsShown = false;
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

  /// Skill wind-ups (wall-clock end of the cast pose) and the end of the
  /// combat stance after a swing or a cast; the character faces
  /// [_attackFacing] until then while it stands.
  final _castUntil = <int, double>{};
  final _stanceUntil = <int, double>{};

  /// Casts that end in a throw (alchemist flasks, potions, spheres): the
  /// toss plays after the wind-up (`cast_pose.dart`).
  final _throws = <int, CastPose>{};

  /// Own state from the vitals: 0 up, 1 downed, 2 dead.
  int get _myState => _vitals[_me]?.state ?? 0;

  /// Merchant-line companions on the map (`COMPANIONS` records) by entity
  /// id: homunculi walk in the snapshot rows, plants and sphere mines stand
  /// where their record says ([_rootedRows]).
  final _companions = <int, CompanionInfo>{};

  /// When each record arrived (wall clock): a rooted one's `expiresMs`
  /// counts from there.
  final _companionSince = <int, double>{};

  /// A companion's flinch after a hit on it, and its death (the fall plays,
  /// then a rooted one goes; a homunculus leaves the snapshots).
  final _hitUntil = <int, double>{};
  final _companionDown = <int, double>{};

  /// The last row of each rooted companion (sprite, action, facing packed).
  final _rootedSent = <int, int>{};

  /// The own homunculus panel (`COMPANION_STATE`) and when it came.
  CompanionState? _homState;
  double _homStateAt = 0;
  double _homDrawn = 0;

  /// Its name and look while it is not out (no record to read them from).
  String _homName = '';
  bool _homEvolved = false;

  /// When the own Call Homunculus slot was last pressed (the chooser opens
  /// when the server answers that no type is chosen yet).
  double _callHomAt = -10;
  bool _callHomHeld = false;
  bool _homChooser = false;

  /// The chooser is open to change the type (it stays open while panels
  /// with the current type come in).
  bool _homChanging = false;

  /// The companions pack is asked for (once a session).
  bool _companionsAsked = false;

  /// The entities whose Pushcarts the engine draws (appearance `cart`).
  final Set<int> _carts = {};

  /// Light radius of the stage before buffs (cells), and the row flags last
  /// sent per remote player (conditions).
  double _baseLight = 0;
  final _sentFlags = <int, int>{};

  /// Stage monsters by pool slot, and the kind table from `stage_joined`.
  final _mobs = <int, _Mob>{};

  /// Affixed monsters by slot (server `MONSTER_AFFIXES`), kept until that
  /// monster dies; a spawn of the same generation shows them.
  final _affixes = <int, MonsterAffix>{};
  var _monsterKinds = const <String>[];

  /// Tick of the last monster section applied; null until a keyframe
  /// (after a map change, or when a delta did not apply and a resync is
  /// pending).
  int? _mobTick;
  bool _resyncSent = false;
  var _npcs = <_Npc>[];
  _Npc? _nearNpc;

  final _keys = <String>{};

  /// Keys pressed since the last input tick: a tap shorter than a tick
  /// (a slow frame, a quick finger) still casts.
  final _tapped = <String>{};

  /// Gamepad and touch (COMBAT.md §4.2), polled once per tick; auto-cast
  /// slots (bit i = slot i, persisted with the settings).
  late final _input = GameInput(view)
    ..onSettings = _toggleMenu
    ..onInteract = _interactNearest
    ..onPlayerMenu = _nearestPlayerMenu
    ..onAutoToggle = _toggleAutoCast;
  int _autoCast = 0;
  int _stopSends = 0;

  /// Settings → Manual aiming: the mouse picks the monster and places
  /// ground skills (a gamepad or touch keeps automatic targeting).
  bool _manualAim = false;

  /// `CAST_PREFS` as last sent on this connection (`(manual, auto)`).
  (bool, int)? _sentCastPrefs;

  /// Sends `CAST_PREFS` when it changed: manual aiming counts only while
  /// the aim comes from the mouse.
  void _syncCastPrefs() {
    final input = _input.state;
    final prefs = (_manualAim && !input.hasAim && !_input.touchOn, _autoCast);
    if (prefs == _sentCastPrefs) return;
    _sentCastPrefs = prefs;
    _ws.push(ClientMsg.castPrefs(manualAim: prefs.$1, auto: prefs.$2)).ignore();
  }

  /// `USE_PREFS` as last sent on this connection (`(hp, sp)` percent).
  (int, int)? _sentUsePrefs;

  /// Settings → Auto-use potions: the thresholds the server drinks at
  /// ([usePrefsOf]); sent on connect and on change.
  void _syncUsePrefs() {
    if (_ws.state is! WebSocketState$Open) return;
    final prefs = usePrefsOf(_settings);
    if (prefs == _sentUsePrefs) return;
    _sentUsePrefs = prefs;
    _ws.push(ClientMsg.usePrefs(hp: prefs.$1, sp: prefs.$2)).ignore();
  }

  /// Own skills (`skills` message) and the last pointer position (aim).
  Skills? _skills;
  (double, double)? _pointer;

  /// Rebindable keys (hotbar 1–0, emotes Alt+1–0, flags Ctrl+1–0, ...).
  Bindings _binds = Bindings(const {});

  /// Peers whose name is hidden in the town fog.
  final _unlit = <int>{};

  /// Entities sitting (players, townsfolk), from `pose` messages.
  final _sitting = <int>{};

  /// Arrows nocked by archers on the stage (Arrow Change; `quiver`):
  /// damage type ids, the plain arrow left out.
  final _quivers = <int, int>{};

  /// The own running buffs (`buffs`), redrawn with the arrow chip.
  List<({String skill, int ms})> _buffs = const [];

  /// The own arrow, kept between stages (Arrow Change nocks it again).
  int _myArrow = 0;

  /// The own character's appearance as the server states it (after every
  /// `map_enter`, then on changes); null until the first one.
  Appearance? _mine;

  /// The own character rides, [_rideSpeed] percent faster (from [_mine]);
  /// [_statusPace] is what conditions leave of the pace (slowed, held).
  bool get _riding => _mine?.riding ?? false;
  int get _rideSpeed => _mine?.speed ?? 0;
  double _statusPace = 1;

  /// A gamepad / touch press of the mount slot was down last tick (edges).
  bool _mountHeld = false;

  /// When the current map was entered (`_now`): mount puffs only after it.
  double _mapSince = 0;

  /// Light radius of the town map in cells (manifest `lightRadius`; 0 = none).
  double _townLight = 0;
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
  int _lastBytesOut = 0;
  double _bytesOutRate = 0;
  double _lastNetStats = 0;

  /// Listeners, the ping timer and the frame loop, undone by [stop].
  final _listeners = <(web.EventTarget, String, JSFunction)>[];
  Timer? _pingTimer;
  bool _stopped = false;
  final _exit = Completer<String>();

  /// Completes when the player leaves the world: `characters` (back to
  /// the character slots), `logout`, or `ended` (the session cannot go on:
  /// expired, signed in elsewhere; [exitNotice] says why).
  Future<String> get exited => _exit.future;

  /// Why the session ended on its own, for the sign-in screen.
  String? exitNotice;

  /// The socket is down and being retried: input is frozen (no prediction
  /// drift) and the engine shows the reconnecting plate until the server
  /// puts us back into an instance.
  bool _offline = false;

  /// The socket was open before: the next open is a reconnect.
  bool _wasOpen = false;

  /// The character is being selected again after a server restart.
  bool _reselecting = false;

  /// The server offered the stage the character is link-dead in
  /// (`resumable`): the "Return to battle" dialog is up.
  Resumable? _resumeOffer;

  /// Close codes after which reconnecting would only repeat the refusal:
  /// 4008 kicked, 4009 signed in elsewhere (or taken over by a newer tab),
  /// 4010 another build (`update.dart`: reload).
  static const _finalCloseCodes = {4008, 4009, closeUpdateRequired};

  static double _now() => web.window.performance.now() / 1000;

  /// Connects and enters the world. Every attempt (the first and each
  /// reconnect) builds its URL with a current access token, renewed through
  /// the refresh token when it is about to expire.
  Future<void> start() async {
    final loc = web.window.location;
    final scheme = loc.protocol == 'https:' ? 'wss' : 'ws';
    _ws = WebSocketClient.binary(
      options: WebSocketOptions(
        request: (url, attempt) async {
          // A failed attempt may be a refused token (the browser hides
          // the 401): the first retry renews it outright, and so does
          // every fourth after (a renewal rotates the refresh token, so not
          // on every attempt while the server is away).
          final token = await api.freshToken(force: attempt == 1 || (attempt > 0 && attempt % 4 == 0));
          // The server refuses another build (`update.dart`).
          return WebSocketRequest(url.replace(queryParameters: {'token': token, 'build': clientBuildId}));
        },
        reconnect: WebSocketReconnect(
          maxDelay: const Duration(seconds: 8),
          shouldReconnect: (cause, attempt) => switch (cause) {
            WebSocketException(cause: SessionExpired()) => false,
            WebSocketClosedException(:final code?) when _finalCloseCodes.contains(code) => false,
            _ => true,
          },
        ),
        // Snapshots flow every tick: a few quiet seconds mean a dead link
        // (the browser may take minutes to notice on its own).
        heartbeat: WebSocketHeartbeat(
          interval: const Duration(seconds: 4),
          timeout: const Duration(seconds: 4),
          ping: ClientMsg.ping,
        ),
      ),
    );
    _subs.add(_ws.messages.listen(_onMessage));
    _subs.add(_ws.states.listen(_onState));
    view.onUiEvent(_onUiEvent.toJS);
    _loadSettings();
    view.loadSkills('$assetsBase/client/skills').toDart.then((_) => _renderSkills()).catchError((Object e) {
      web.console.warn('skills unavailable: $e'.toJS);
    });
    view.loadEmotes('$assetsBase/client/emotes').toDart.catchError((Object e) {
      web.console.warn('emotes unavailable: $e'.toJS);
      return null;
    });
    void listen(web.EventTarget target, String type, JSFunction fn) {
      target.addEventListener(type, fn);
      _listeners.add((target, type, fn));
    }

    listen(
      web.window,
      'pointermove',
      ((web.PointerEvent e) {
        _pointer = _clientXY(e);
        _hoverDirty = true;
      }).toJS,
    );
    listen(web.window, 'pointerup', _onMouseUp.toJS);
    listen(web.window, 'keydown', _onKeyDown.toJS);
    listen(web.window, 'keyup', _onKeyUp.toJS);
    listen(
      web.window,
      'blur',
      ((web.Event _) {
        _keys.clear();
        // A button released outside the window never says so.
        _steerHeld = _walker.steering = false;
      }).toJS,
    );
    listen(web.document.querySelector('canvas')! as web.HTMLCanvasElement, 'click', _onClick.toJS);
    // The player menu: right-click (a click, not a camera drag), long
    // press on touch (also the joystick zone's `contextmenu`).
    final canvas = web.document.querySelector('canvas')! as web.HTMLCanvasElement;
    listen(canvas, 'contextmenu', _onContextMenu.toJS);
    listen(canvas, 'pointerdown', _onCanvasDown.toJS);
    listen(canvas, 'pointermove', _onCanvasMove.toJS);
    listen(canvas, 'pointerup', _onCanvasUp.toJS);
    listen(canvas, 'pointercancel', ((web.PointerEvent _) => _cancelHold()).toJS);

    await _ws.connect('$scheme://${loc.host}/api/ws');
    _pingTimer = Timer.periodic(const Duration(seconds: 2), (_) {
      _pingSentAt = _now();
      _send(ClientMsg.ping());
    });
    _pulse.start();
    // Read-only probe for scripted browser tests and bots.
    globalContext.setProperty(
      'gameDebug'.toJS,
      (() {
        final p = _predictor;
        final v = _vitals[_me];
        return jsonEncode({
          'map': _mapId,
          'loading': _loading,
          'x': p?.x,
          'y': p?.y,
          'peers': _peers.length,
          'channel': _channel,
          'offline': _offline,
          'hp': v?.hp,
          'maxHp': v?.maxHp,
          // Stage monsters in view: kind id and cell (1/64 on the wire).
          'mobs': [
            for (final m in _mobs.values)
              if (m.removeAt == null)
                {'kind': m.kind < _monsterKinds.length ? _monsterKinds[m.kind] : '', 'x': m.x / 64, 'y': m.y / 64},
          ],
          // Click to move: the walk under way and where it ends; the NPCs.
          'walking': _walker.goal?.name,
          'walkEnd': ?switch (_walker.end) {
            (final x, final y) => [x, y],
            null => null,
          },
          'npcs': [
            for (final n in _npcs) {'id': n.id, 'name': n.name, 'x': n.x, 'y': n.y},
          ],
        });
      }).toJS,
    );
    // The ground cell under a client point (`[x, y]` or null): lets a
    // scripted test find where to click.
    globalContext.setProperty(
      'gameGroundAt'.toJS,
      ((JSNumber x, JSNumber y) => switch (view.pickGround(x.toDartDouble, y.toDartDouble)) {
        (final gx, final gy) => [gx.toJS, gy.toJS].toJS,
        null => null,
      }).toJS,
    );
    _frame(0);
  }

  /// [_send] for a frame that may form or join a party (see [_partyAsked]).
  void _askParty(Uint8List frame) {
    _partyAsked = _now();
    _send(frame);
  }

  void _send(Uint8List frame) {
    if (_ws.state is! WebSocketState$Open) return;
    _ws.push(frame).ignore();
  }

  // ------------------------------------------------------------ connection

  void _onState(WebSocketState s) {
    switch (s) {
      case WebSocketState$Open():
        // A reconnect from a mission goes straight back to the fighter (the
        // stage keeps it link-dead for a while); anything else asks for the
        // town, where the server offers a stage the character is still in.
        final back = _wasOpen && _inStage;
        _wasOpen = true;
        _send(back ? ClientMsg.stageResume(accept: true) : ClientMsg.hubJoin());
        if (_offline) _showOffline(0, text: 'Reconnected — rejoining…');
      case WebSocketState$Reconnecting(:final attempt, :final cause):
        if (!_wasOpen) return;
        web.console.warn('connection lost ($attempt): $cause'.toJS);
        _goOffline(attempt);
      case WebSocketState$Connecting(:final attempt) when attempt > 0 && _wasOpen:
        _goOffline(attempt);
      case WebSocketState$Disconnected(:final closeCode, :final closeReason, :final error) when !_stopped:
        // Not retried: the session cannot go on here.
        final expired = error?.cause is SessionExpired;
        final refused = refusedBy(closeCode, closeReason);
        if (refused != null) pageUpdates?.refused(refused);
        _end(switch (closeCode) {
          _ when expired => '${error!.cause}',
          closeUpdateRequired => 'A new version of the game is out: reload the page to play.',
          4009 => 'You signed in somewhere else, so this tab was disconnected.',
          4008 => 'You were disconnected by the server.',
          _ => 'The connection to the server was lost.',
        });
      default:
        break;
    }
  }

  /// The socket dropped: freeze input (the server stopped hearing it) and
  /// show the plate. In a mission the character holds on link-dead.
  void _goOffline(int attempt) {
    if (!_offline) {
      _offline = true;
      _keys.clear();
      _cancelWalk();
    }
    _showOffline(attempt);
  }

  void _showOffline(int attempt, {String? text}) => view.setConnection(
    jsonEncode({
      'attempt': attempt,
      'text': ?text,
      if (_inStage) 'sub': 'Your character holds its ground for a minute.',
    }),
  );

  /// Back in an instance after a drop: the plate goes, input resumes.
  void _backOnline() {
    if (!_offline) return;
    _offline = false;
    view.setConnection('');
  }

  /// The session ended on its own: back to the sign-in with [why].
  void _end(String why) {
    if (_exit.isCompleted) return;
    exitNotice = why;
    _leave('ended').ignore();
  }

  /// The selected character is link-dead in a mission: offer it.
  void _offerResume(Resumable r) {
    if (_offline) {
      // A reconnect from town that finds a stage: just go back.
      _send(ClientMsg.stageResume(accept: true));
      return;
    }
    _resumeOffer = r;
    _npcDialog = false;
    _dropRefiner();
    final secs = (r.leftMs / 1000).ceil();
    view.showDialog(
      jsonEncode({
        'id': 'resume',
        'title': 'Return to battle',
        'text':
            'Your character is still fighting in ${r.mission}. '
            'Return within $secs s, or leave the mission and go to town.',
        'options': [
          {'id': 'resume:yes', 'label': 'Return to battle', 'detail': 'Back to your party, where you left off.'},
          {'id': 'resume:no', 'label': 'Leave the mission', 'detail': 'Go to town; the party fights on without you.'},
        ],
      }),
    );
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
      case Connected(:final serverVersion):
        this.serverVersion = serverVersion;
        // A new connection knows nothing of how we cast yet.
        _sentCastPrefs = null;
        _sentUsePrefs = null;
        _syncUsePrefs();
      case null || HubChannels():
        break;
      case final FeedbackResult r:
        _onFeedbackResult(r);
      case Notification(:final message):
        view.toast(message);
      case final PlayerVitals v:
        _onVitals(v);
      case final Combat c:
        _onCombat(c);
      case final Skills s:
        _skills = s;
        _renderSkills();
        _syncCart();
      case final Cast c:
        _onCast(c);
      case ZoneEnd(:final id):
        _predictor?.drift.remove(id);
        view.endZone(id);
      case final SkillStrikes s:
        view.skillStrikes(
          jsonEncode({
            'caster': s.caster,
            'skill': s.skill,
            'radius': s.radius,
            'strikes': [
              for (final p in s.strikes) {'x': p.x, 'y': p.y, 'delayMs': p.delayMs},
            ],
          }),
        );
      case final StageEnd e:
        _onStageEnd(e);
      case final Boss b:
        _bossId = b.hp <= 0 ? null : b.entityId;
        _renderMarks();
        view.setBoss(b.hp <= 0 ? '' : jsonEncode({'id': b.entityId, 'name': b.name, 'hp': b.hp, 'maxHp': b.maxHp}));
      case Buffs(:final buffs):
        _buffs = buffs;
        _renderBuffs();
      case Quiver(:final entityId, :final element):
        _onQuiver(entityId, element);
      case final Progress pr:
        final before = _progress?.level;
        _progress = pr;
        if (before != null && pr.level > before) {
          view.toast('Level up! Lv ${pr.level} ${_title(_myArchetype, pr.level)} — K: skills');
        }
        _renderStatus();
      case final BossCast c:
        view.bossCast(
          jsonEncode({
            'caster': c.caster,
            'ability': c.ability,
            'name': c.name,
            'castMs': c.castMs,
            'channel': c.channel,
            'shielded': c.shielded,
            'interrupted': c.interrupted,
          }),
        );
      case final BossZones z:
        // Belts (drifting ground): the local prediction rides them too.
        for (final a in z.zones) {
          if (!a.drift || a.area.id == 0) continue;
          final t = a.area;
          _predictor?.drift.put(t.id, circle: t.shape == 0, x: t.x, y: t.y, dir: t.dir, along: t.a, across: t.b, speed: a.driftSpeed);
        }
        view.bossZones(
          jsonEncode({
            'caster': z.caster,
            'ability': z.ability,
            'zones': [for (final a in z.zones) a.toJson()],
          }),
        );
      case BossShout(:final entityId, :final name, :final text, :final style):
        view.bossShout(entityId.toDouble(), name, text, style);
      case final AffixDefs d:
        view.setAffixDefs(jsonEncode(d.toJson()));
      case MonsterAffixes(:final monsters):
        for (final a in monsters) {
          _affixes[a.slot] = a;
          final m = _mobs[a.slot];
          if (m != null && m.generation == a.generation && m.dieAt == null) _showAffixes(m);
        }
      case AffixEvents(:final events):
        for (final e in events) {
          view.affixEvent(e.entity.toDouble(), e.name, e.x, e.y);
        }
      case Companions(:final list):
        _onCompanions(list);
      case final CompanionState s:
        _onCompanionState(s);
      case MonsterHp(:final bars):
        // The bars under elites and bosses; the engine keeps each until
        // its plate shows (or the monster is gone: NaN).
        for (final (id, hp) in bars) {
          view.setHealth(id.toDouble(), hp == 0 ? double.nan : hp / 255);
        }
      case final BossBar b:
        view.setBossBar(jsonEncode(b.toJson()));
        // The newest encounter is the boss mark on the minimap.
        final id = b.rank == 0 ? (_bossId == b.entityId ? null : _bossId) : b.entityId;
        if (id != _bossId) {
          _bossId = id;
          _renderMarks();
        }
      case Pong():
        if (_pingSentAt > 0) _rttMs = (_now() - _pingSentAt) * 1000;
      case HubJoined(:final entityId, :final channelName, :final tickRate):
        _backOnline();
        _resumeOffer = null;
        _me = entityId;
        _syncCart();
        view.setChat(true);
        _channel = channelName;
        _tickHz = tickRate;
        _clock = ServerClock(tickHz: _tickHz);
        view.setLightRadius(_townLight);
        _baseLight = 0;
        view.setBoss('');
        view.setBossBar('');
        _bossId = null;
        _affixes.clear();
        _buffs = const [];
        _quivers.clear();
        view.setBuffs('[]');
        view.setCombo(0, 0, 0, 0);
        _combo = 0;
        _stageMission = '';
        _endEnvEvents();
        view.setMissionClock('', 0, false);
        view.setMissionModifiers(jsonEncode({'scope': 'stage'}));
        _vitals.clear();
        _hideReturnCall();
        if (_inStage) {
          _inStage = false;
          // The cards found on the run, the first one turned over.
          if (_newCards.isNotEmpty) {
            view.showCard(_newCards.first, true);
            _newCards.clear();
          }
        }
        _renderParty();
        _showOnline();
        _devMission();
        _shopNpc = null;
        view.showShop('');
        // A new instance: any trade there was is over.
        _trading = false;
        _tradePending = false;
        view.showTrade('');
        _renderInventory();
      case StageJoined():
        _backOnline();
        _resumeOffer = null;
        _me = msg.entityId;
        _syncCart();
        view.setChat(true);
        _channel = 'Stage-${msg.stageId}';
        _tickHz = msg.tickRate;
        _clock = ServerClock(tickHz: _tickHz);
        _monsterKinds = msg.monsters;
        _stageMission = msg.mission;
        _endEnvEvents();
        _vitals.clear();
        // A fresh arrival carries no timed effect (COMBAT.md §4.6).
        _buffs = const [];
        _quivers.clear();
        view.setBuffs('[]');
        // The arrow nocked on the last stage, nocked again (the server
        // ignores it without Arrow Change).
        if (_myArrow != 0) _send(ClientMsg.quiver(_myArrow));
        _baseLight = msg.lightRadius;
        view.setLightRadius(msg.lightRadius);
        _inStage = true;
        _showOnline();
        _renderStagePanel();
        _shopNpc = null;
        view.showShop('');
        // A new instance: any trade there was is over.
        _trading = false;
        _tradePending = false;
        view.showTrade('');
        _renderInventory();
      case LobbyState(:final lobby):
        // The party is the account's: it comes back on every sign-in and
        // after every mission. Only a party we just asked for opens the
        // window; the rest updates the tab quietly.
        final news = _lobby?.lobbyId != lobby.lobbyId && _now() - _partyAsked < 30;
        if (news) _partyAsked = 0;
        _lobby = lobby;
        _missionName = lobby.mission.name;
        _sendPendingMods();
        if (!_inStage) _renderLobby(quiet: !news);
        _renderModifiers();
      case LobbyLeft(:final reason):
        _lobby = null;
        _lobbyMods = null;
        _renderModifiers();
        _renderMarks();
        if (!_inStage) _renderParty();
        switch (reason) {
          case 'kicked':
            view.toast('You were removed from the party.');
          case 'expired':
            view.toast('You were away too long and left the party.');
          case 'replaced':
            view.toast('Your party moved to another tab.');
        }
      // The server restarted while we were away: the session it has for
      // us has no character. Select ours again and go on.
      case HubError(:final code) when code == 'no_character' && _offline && !_reselecting:
        _reselecting = true;
        api
            .select(characterId)
            .then((_) => _send(_inStage ? ClientMsg.stageResume(accept: true) : ClientMsg.hubJoin()))
            .catchError((Object e) => _end('Could not re-enter the world: $e'))
            .whenComplete(() => _reselecting = false);
      case HubError(:final code, :final message) when code == 'chat_too_fast':
        view.chatNotice(message);
      case HubError(:final message) when _stylist != null:
        _styling = false;
        _showStylist(error: message);
      case TradeRequest() || TradeState() || TradeClosed():
        _onTrade(msg);
      case HubError(:final code, :final message) when _tradePending && code.startsWith('trade_'):
        // The request did not go out (too far, busy): drop the prompt.
        _tradePending = false;
        view.showTrade('');
        view.toast(message);
      case HubError(:final message) when _shopNpc != null:
        view.shopNote(message);
        view.toast(message);
      case HubError(:final message) when _crafting && _hatter != null:
        _crafting = false;
        view.recipesResult(jsonEncode({'error': message}));
      case HubError(:final message) when _refining != null && _refiner != null:
        // The attempt did not go through (no ore, too far): the window says so.
        _refining = null;
        view.refineResult(jsonEncode({'error': message}));
      case LobbyError(:final message) || HubError(:final message):
        view.toast(message);
        _refining = null;
      case final Inventory inv:
        _inventory = inv;
        // Gear changed the derived numbers (armour class, weapon).
        if (inv.statsJson.isNotEmpty) {
          final fresh = jsonDecode(inv.statsJson);
          if (fresh is Map<String, Object?>) _stats = {..._stats, ...fresh};
        }
        _renderInventory();
        _renderStatus();
        // The smith's list follows the bag (loot, a sale) while it is open;
        // an attempt's answer redraws it with the outcome.
        if (_refiner != null && _refining == null && view.dialogOpen()) _showRefiner();
      case ItemDefs(:final items):
        _itemDefs
          ..clear()
          ..addEntries(items.map((d) => MapEntry(d.id, d)));
        view.setItems(
          jsonEncode({
            'icons': '$assetsBase/client/items/icons',
            'cards': '$assetsBase/client/cards',
            'items': [
              for (final d in items)
                {
                  'id': d.id,
                  'name': d.name,
                  'kind': d.kind,
                  'slot': d.slot,
                  'price': d.price,
                  'classes': d.classes,
                  'desc': d.desc,
                  'rarity': d.rarity,
                  if (d.sockets > 0) 'sockets': d.sockets,
                  if (d.card > 0) ...{'card': d.card, 'prefix': d.prefix, 'suffix': d.suffix, 'monster': d.monster},
                },
            ],
          }),
        );
      case Shop(:final npc, :final name, :final items, :final buyPct, :final sellPct):
        view.closeDialog();
        _shopNpc = npc;
        // Discount / Overcharge: the window strikes the shop's price out.
        view.showShop(jsonEncode({'npc': npc, 'name': name, 'items': items, 'buyPct': buyPct, 'sellPct': sellPct}));
      case Recipes(:final npc, :final name, :final recipes):
        view.closeDialog();
        _hatter = npc;
        _crafting = false;
        // A recipe book of a skill (the Alchemist's Pharmacy) has no NPC
        // to walk away from: it stays open until closed.
        final giver = _npcs.where((n) => n.id == npc).firstOrNull;
        _npcDialog = giver != null;
        view.showRecipes(
          jsonEncode({
            'npc': npc,
            'name': name,
            'text': ?giver?.text,
            if (giver == null) ...{'kind': 'Recipe book', 'made': 'Brewed: {item}. It is in your bag.'},
            'recipes': [for (final r in recipes) r.toJson()],
          }),
        );
      case Crafted(:final item):
        _crafting = false;
        if (_hatter != null) {
          view.recipesResult(jsonEncode({'crafted': item}));
        } else {
          view.toast('Made: ${_itemDefs[item]?.name ?? item}.');
        }
      case Parties(:final parties):
        if (!_partiesAsked || _inStage) return;
        _partiesAsked = false;
        final meAway = _lobby?.members.any((m) => m.entityId == _me && m.away) ?? false;
        _npcDialog = true;
        view.showDialog(jsonEncode(partiesDialog(parties, lobby: _lobby, leading: _leading, meAway: meAway)));
      case ShopDone(:final spent, :final earned):
        view.shopDone(spent, earned);
      case Refined(:final item, :final ok, :final refine, :final slip):
        final row = _refining;
        _refining = null;
        if (row == null || _refiner == null) {
          // The window was closed meanwhile: a toast tells how it went.
          final name = _itemDefs[splitKey(item).id]?.name ?? item;
          view.toast(
            ok ? 'Success! Your $name is now +$refine.' : 'The hammer slipped... your $name fell back to +$refine.',
          );
          return;
        }
        // `<slot | b>:<key>`: the level before, and the row it is now.
        final i = row.indexOf(':');
        view.refineResult(
          jsonEncode({
            'ok': ok,
            'item': item,
            'from': splitKey(row.substring(i + 1)).refine,
            'to': refine,
            'slip': slip,
          }),
        );
        _showRefiner(select: '${row.substring(0, i)}:$item');
      case Carded(:final inserted, :final item, :final card):
        view.cardCarded(item);
        final name = _itemDefs[card]?.name ?? card;
        view.toast(inserted ? '$name socketed.' : '$name taken out.');
      case ItemUsed(:final item):
        final d = _itemDefs[item];
        if (d != null) view.toast('${d.name}: ${d.desc.split('\n').first.replaceAll('**', '')}');
      case Loot(:final entityId, :final x, :final y, :final item, :final count, :final rare):
        // The engine floats it from the monster and adds it to the loot
        // feed (merged repeats; rare drops highlighted) and the chat's
        // loot log: no toast.
        view.showLoot(entityId.toDouble(), x, y, item, count, rare);
        // A card is turned over back in town (not in the fight).
        if (rare && _itemDefs[item]?.kind == 9 && _inStage && !_newCards.contains(item)) _newCards.add(item);
      case Combo(:final count, :final rank, :final windowMs, :final leftMs):
        _combo = count;
        view.setCombo(count, rank, windowMs, leftMs);
      case Revive(:final entityId, :final progress, :final isDown):
        // The ring over a downed player fills while an ally stands by.
        view.setRevive(entityId.toDouble(), isDown ? progress / 100 : double.nan);
      case BonusSpawn(:final id, :final bonus, :final x, :final y, :final lifeMs):
        // A pickup bubble on the stage (and its minimap mark).
        view.showBonus(id.toDouble(), bonus, x, y, lifeMs);
        _bonusAt[id] = (x, y);
      case BonusGone(:final id, :final entityId):
        // Taken by [entityId] (its burst and name over them) or burst.
        view.takeBonus(id.toDouble(), entityId.toDouble());
        _bonusAt.remove(id);
      case PlayerAppearance(:final entityId, :final appearance):
        _onAppearance(entityId, appearance);
      case final MapEnter enter:
        _onMapEnter(enter).catchError((Object e, StackTrace st) {
          view.endLoading();
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
        view.toast('$targetMap is reached through the Kafra employee in the town centre.');
      case Emote(:final entityId, :final emote):
        view.showEmote(entityId.toDouble(), emote);
      case ChatLine(:final entityId, :final name, :final text):
        view.chatLine(entityId.toDouble(), name, text, entityId == _me);
      case Announcement(:final text):
        view.announce(text);
      case Pose(:final entityId, :final sitting):
        if (sitting) {
          _sitting.add(entityId);
        } else {
          _sitting.remove(entityId);
        }
      case FlagPing(:final entityId, :final flag, :final x, :final y):
        view.showFlag(entityId.toDouble(), flag, x, y, entityId == _me ? _myName : (_peers[entityId]?.name ?? ''));
      case MissionBriefing(:final brief):
        _brief = brief;
        view.setMissionBrief(jsonEncode(brief.toJson()));
      case MissionList(:final missions):
        _ladder = missions;
        if (_missionsShown) _showMissions(null);
      case final ModifierDefs d:
        _modDefs = d;
        view.setModifierDefs(
          jsonEncode({
            'defs': [for (final x in d.defs) x.toJson()],
            'rewards': [for (final r in d.rewards) r.toJson()],
          }),
        );
      case final ModifierState m when m.scope == 0:
        _lobbyMods = m;
        _renderModifiers();
        if (_lobby != null && !_inStage) _renderLobby();
      case final ModifierState m:
        view.setMissionModifiers(jsonEncode({'scope': 'stage', 'state': m.toJson()}));
      // The clear bonus is on the result screen (`RUN_REWARDS`).
      case ModifierReward():
        break;
      case final RunStats s:
        _runStats = s;
      case final RunRewards r:
        _showResults(r);
      case StageClock(:final elapsedMs):
        if (_inStage && _stageMission.isNotEmpty) view.setMissionClock(_stageMission, elapsedMs, true);
      case final PoisonFog fog:
        // The overtime fog: the engine closes the circle on its own.
        view.setPoisonFog(jsonEncode({...fog.toJson(), 'warn': fog.begun}));
        if (fog.begun) view.toast('A poison fog is closing in! Stay inside the circle.');
      case EnvLight(:final radius):
        // The run's light changed (a darkness or storm event, a lit
        // brazier); the own light upgrades still add to it.
        _baseLight = radius;
        if (_inStage) view.setLightRadius(_baseLight + (_vitals[_me]?.light ?? 0));
      case final EnvEvent e:
        _onEnvEvent(e);
      case EnvShake(:final intensity, :final durationMs):
        if (_inStage) view.shakeCamera(intensity / 255, durationMs);
      case FogExposure(:final secs, :final deathSecs, :final fatal):
        view.setFogExposure(secs, deathSecs);
        if (fatal) view.toast('The poison fog has claimed you.');
      case final PartyInvite inv:
        _onPartyInvite(inv);
      case PartyAnswer(:final name, :final accepted):
        view.toast(accepted ? '$name joined your party.' : '$name declined your invitation.');
      case final PlayerInspect s:
        _onInspect(s);
      case final PlayerSheet s:
        _onSheet(s);
      case NpcDialog():
        _onNpcDialog(msg);
      case HubLeft(:final reason):
        view.toast('Left the hub ($reason).');
      case Displaced():
        // The 4009 close that follows ends the session (`_onState`).
        view.toast('Signed in elsewhere; this tab is disconnected.');
      case final Resumable r:
        _offerResume(r);
      case PlayerLink(:final entityId, :final online):
        final peer = _peers[entityId];
        if (peer != null && peer.linkDead == online) {
          peer.linkDead = !online;
          if (!_unlit.contains(entityId)) _showPeer(entityId, peer);
          if (_inStage && !_loading) view.toast(online ? '${peer.name} is back.' : '${peer.name} lost the connection.');
        }
      case Friends(:final friends):
        _onFriends(friends);
      case final FriendNote n:
        _onFriendNote(n);
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
      var t = _tracks[id];
      if (t == null) {
        t = _tracks[id] = RemoteTrack(id);
        // Back from beyond the fog: its name and look were dropped with it.
        final peer = _peers[id];
        if (peer != null) {
          _showPeer(id, peer);
        } else if (_companions.containsKey(id)) {
          _showCompanion(id);
        }
      }
      t.push(s.tick, s.xs[i], s.ys[i], s.facings[i]);
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
      final status = s.mobStatuses[i];
      switch (s.mobOps[i]) {
        case MobOp.spawn:
          // A reused slot (or a keyframe) is a new monster: snap, never
          // slide from the old one.
          if (m != null) view.removeEntity(m.id);
          m = _mobs[slot] = _Mob(slot, s.mobGenerations[i], s.mobKinds[i])..status = status < 0 ? 0 : status;
          m.set(s.mobXs[i], s.mobYs[i], s.mobVxs[i], s.mobVys[i], s.tick);
          if (_affixes[slot]?.generation == m.generation) _showAffixes(m);
        case MobOp.move when m != null && m.removeAt == null:
          if (m.dieAt == null) m.set(s.mobXs[i], s.mobYs[i], s.mobVxs[i], s.mobVys[i], s.tick);
          if (status >= 0) m.status = status;
        case MobOp.nudge when m != null && m.removeAt == null && m.dieAt != null:
          break;
        case MobOp.nudge when m != null && m.removeAt == null:
          m.set(m.x + s.mobXs[i], m.y + s.mobYs[i], s.mobVxs[i], s.mobVys[i], s.tick);
          if (status >= 0) m.status = status;
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

  /// An archer nocked another arrow (`quiver`): its shots and arrow skills
  /// fly with that tip; the own one shows by the buffs.
  void _onQuiver(int entityId, int element) {
    if (element == 0) {
      _quivers.remove(entityId);
    } else {
      _quivers[entityId] = element;
    }
    view.setQuiver(entityId.toDouble(), Quiver.names[element] ?? '');
    if (entityId == _me) {
      _myArrow = element;
      _renderBuffs();
    }
  }

  /// The own buff icons, and the arrow nocked (when not the plain one) as
  /// a chip of its colour.
  /// What a homunculus puts on its owner (server `companion::buff_id`):
  /// the skill whose icon stands for it, and its name.
  static const _homBuffs = {
    'hom:urgent_escape': ('increase_agi', 'Urgent Escape (homunculus): faster'),
    'hom:bulwark': ('endure', 'Amistr Bulwark (homunculus): AC +4'),
  };

  void _renderBuffs() {
    final arrow = _quivers[_me];
    view.setBuffs(
      jsonEncode([
        for (final b in _buffs)
          if (_homBuffs[b.skill] case (final icon, final title))
            {'skill': icon, 'ms': b.ms, 'title': title, 'source': 'your homunculus'}
          else
            // Skills, `item:` potions, `bonus:` pickups, `cond:` conditions.
            {'skill': b.skill, 'ms': b.ms},
        if (arrow != null)
          {
            'skill': 'arrow_change',
            'ms': 0,
            'tint': Quiver.tints[arrow],
            'title': Quiver.labels[arrow],
            'desc': 'Your basic attacks shoot this arrow: its element and effect.',
            'source': 'the arrows in your quiver',
          },
      ]),
    );
  }

  void _onVitals(PlayerVitals v) {
    final before = _vitals[v.entityId];
    final wasUp = (before?.state ?? 0) == 0;
    _vitals[v.entityId] = v;
    // A thief coming up out of the ground: the burst of earth of the
    // burrowing monsters (sinking in, Hiding's own puff plays).
    const veil = MobStatus.burrowed;
    if (before != null && before.status & veil != 0 && v.status & veil == 0) {
      view.unitSound(v.entityId.toDouble(), 'emerge');
    }
    // The party window's health bars, at most four times a second.
    if (_inStage && before?.hp != v.hp) {
      final now = _now();
      if (now - _partyDrawn > 0.25) {
        _partyDrawn = now;
        _renderStagePanel();
      }
    }
    _applyHealth(v);
    if (_companions.containsKey(v.entityId)) {
      _onCompanionVitals(v, wasUp);
      return;
    }
    // A level gained mid-fight: the burst (the server healed and threw
    // the monsters around back).
    if (before != null && before.level > 0 && v.level > before.level) view.levelUp(v.entityId.toDouble());
    if (v.entityId == _me) {
      if (_inStage && (before?.light ?? 0) != v.light) view.setLightRadius(_baseLight + v.light);
      // Predict what the server does: held or rooted stand, slowed walk at
      // half; feared, the server steers (it runs from the scare): no
      // prediction, the corrections carry the player.
      final wasFeared = (before?.status ?? 0) & MobStatus.feared != 0;
      if (v.status & MobStatus.feared != 0 && !wasFeared) view.toast('You are terrified! You flee and cannot fight.');
      final wasSilenced = (before?.status ?? 0) & MobStatus.silenced != 0;
      if (v.status & MobStatus.silenced != 0 && !wasSilenced) view.toast('You are silenced! No skills, basic attacks only.');
      _statusPace = v.status & (MobStatus.held | MobStatus.rooted | MobStatus.feared) != 0
          ? 0
          : v.status & MobStatus.slowed != 0
          ? 0.5
          : 1;
      _applyPace();
      if (wasUp && v.state == 1) view.toast('You are down! Stand by an ally to be revived.');
      _renderStatus();
    }
  }

  void _applyHealth(PlayerVitals v) {
    view.setHealth(v.entityId.toDouble(), v.maxHp > 0 ? v.hp / v.maxHp : double.nan);
    // Every fighter on the map is in the party: their mana under the HP.
    view.setMana(v.entityId.toDouble(), v.maxSp > 0 ? v.sp / v.maxSp : double.nan);
  }

  void _onCombat(Combat c) {
    final now = _now();
    for (final s in c.swings) {
      final mob = s.entityId >= monsterEntityBase ? _mobs[s.entityId - monsterEntityBase] : null;
      view.unitSound(s.entityId.toDouble(), 'attack');
      // Only bows loose an arrow. Every other basic attack is melee
      // (staves and books are clubs, monsters strike on contact); ranged
      // damage comes from skills, which bring their own effects.
      if (s.target != 0 && mob == null && (weaponFamily(s.entityId) == 'bow' || _defaultBow(s.entityId))) {
        // The nocked arrow (Arrow Change) flies with its element's tip.
        final tip = Quiver.names[_quivers[s.entityId] ?? 0];
        view.projectile(s.entityId.toDouble(), s.target.toDouble(), tip == null ? 'arrow' : 'arrow_$tip');
      }
      if (mob != null) {
        mob
          ..attackUntil = now + 0.5
          ..attackFacing = s.facing;
      } else {
        _attackUntil[s.entityId] = now + 0.45;
        _stanceUntil[s.entityId] = now + 0.45 + 4;
        _attackFacing[s.entityId] = s.facing;
        _tracks[s.entityId]?.resetSent();
      }
    }
    for (final h in c.hits) {
      final isMob = h.target >= monsterEntityBase;
      view.showDamage(h.target.toDouble(), h.amount, h.flags, !isMob);
      if (!isMob) {
        // A companion flinches and falls like a monster.
        if (_companions.containsKey(h.target) && !h.isMiss && h.flags & 8 == 0) {
          if (h.isKill) {
            if (!_companionDown.containsKey(h.target)) view.unitSound(h.target.toDouble(), 'die');
            _companionDown[h.target] ??= now;
          } else {
            view.unitSound(h.target.toDouble(), 'hit');
            _hitUntil[h.target] = now + 0.3;
          }
          _tracks[h.target]?.resetSent();
        }
        continue;
      }
      final mob = _mobs[h.target - monsterEntityBase];
      if (mob == null) continue;
      if (h.isKill) {
        if (mob.dieAt == null) view.unitSound(h.target.toDouble(), 'die');
        // Its stars go with it (the plate fades, the death blast is the
        // server's telegraph).
        if (mob.dieAt == null && _affixes.remove(mob.slot) != null) view.setAffixes(mob.id, 0, 0, '');
        mob.dieAt ??= now;
      } else if (!h.isMiss) {
        view.unitSound(h.target.toDouble(), 'hit');
        // Flinch (the server holds it as long: 0.3 s).
        mob.hitUntil = now + 0.3;
      }
    }
  }

  /// Character action: lying while down, the swing for a moment after an
  /// attack, otherwise walk / idle.
  /// Weapon family a character holds (its appearance; null: unknown).
  String? weaponFamily(int id) {
    final w = (id == _me ? _mine : _peers[id]?.appearance)?.weapon ?? '';
    return w.isEmpty ? null : w.split(':').first;
  }

  /// An archer whose weapon the server has not named yet holds the class's bow.
  bool _defaultBow(int id) => weaponFamily(id) == null && _peers[id]?.archetype == 'archer';

  int _charAction(int id, bool moving, double now) {
    // Down or dead: the fall, held on its last frame (revived: up again).
    if ((_vitals[id]?.state ?? 0) != 0) return Act.die;
    if (moving) return Act.walk;
    if (_sitting.contains(id)) return Act.sit;
    // A skill's wind-up pose, then (as after a swing) the combat stance.
    if (now < (_castUntil[id] ?? 0)) return Act.cast;
    // A thrown skill (a flask, a potion, a sphere): the toss as it lands.
    if (_throws[id]?.at(now) == CastPhase.release) return Act.attack;
    // With a weapon: the swing (the engine picks the one its class and
    // family draw the weapon in), then the combat stance for a while (RO
    // draws weapons only in those), then the plain stand.
    if (now < (_attackUntil[id] ?? 0)) return Act.attack2;
    if (_inStage && now < (_stanceUntil[id] ?? 0)) return Act.ready;
    return Act.idle;
  }

  /// Facing of a character: where it walks while it moves; standing, the
  /// target of its last swing or cast while it keeps the pose or the
  /// combat stance (else the stance would turn back to the last step).
  int _poseFacing(int id, bool moving, int facing, double now) {
    final f = _attackFacing[id];
    if (moving || f == null) return facing;
    final hold = math.max(math.max(_castUntil[id] ?? 0, _throws[id]?.end ?? 0), _inStage ? _stanceUntil[id] ?? 0 : _attackUntil[id] ?? 0);
    return now < hold ? f : facing;
  }

  /// Basic Info and the orbs: live vitals on a mission; in town, full
  /// HP / SP from the character's stats (nothing hurts there).
  void _renderStatus() {
    final me = _me;
    final v = me == null ? null : _vitals[me];
    final pr = _progress;
    final level = pr?.level ?? _skills?.level ?? _myLevel;
    int stat(String k) => (_stats[k] as num?)?.toInt() ?? 0;
    final maxHp = v?.maxHp ?? stat('max_hp');
    final maxSp = v?.maxSp ?? stat('max_sp');
    view.setGlossaryContext(jsonEncode(_glossaryContext(level)));
    view.setStatus(
      jsonEncode({
        'name': _myName,
        'title': _title(_myArchetype, level),
        'level': level,
        'hp': v?.hp ?? maxHp,
        'maxHp': maxHp,
        'sp': v?.sp ?? maxSp,
        'maxSp': maxSp,
        'exp': pr?.exp ?? _myExp,
        'expToNext': pr?.expToNext ?? stat('exp_to_next'),
        'state': v?.state ?? 0,
        'zeny': ?_inventory?.zeny,
        // The Equipment window's sheet.
        'sheet': _sheet(),
        // The Equipment window's turntable.
        'look': {..._myLook.toUi(), 'job': bodyJob(_myArchetype, tierOfLevel(level))},
      }),
    );
  }

  /// Combat numbers for the Equipment window: the server's live sheet
  /// (`PLAYER_SHEET`), else the gear's `stats` until it comes.
  Map<String, Object?> _sheet() {
    final live = _liveSheet;
    if (live != null) return sheetView(live);
    final w = _stats['weapon'];
    final weapon = w is Map ? w : const {};
    final interval = (weapon['interval_ms'] as num?)?.toDouble() ?? 0;
    String signed(Object? v) => v is num ? (v >= 0 ? '+$v' : '$v') : '-';
    return {
      'ac': ?_stats['ac'],
      'toHit': signed(weapon['to_hit']),
      'damage': ?weapon['damage'],
      'damageType': ?weapon['damage_type'],
      'attacksPerSec': interval > 0 ? (1000 / interval * ((_stats['attacks'] as num?) ?? 1)).toStringAsFixed(1) : null,
      'proficiency': signed(_stats['proficiency']),
      if (_stats['class_mod'] != null) 'classMod': signed(_stats['class_mod']),
      'spellDc': ?_stats['save_dc'],
      'saves': ?_stats['saves'],
      'k': ?_stats['k'],
    };
  }

  /// The player's numbers for the glossary tips (`GameView.setGlossaryContext`):
  /// AC, attack bonus, spell DC and saves with the gear (the server's `stats`).
  Map<String, Object?> _glossaryContext(int level) {
    final w = _stats['weapon'];
    final saves = _stats['saves'];
    // The live numbers (buffs, conditions) when the server sent them.
    final live = _liveSheet;
    if (live != null) {
      return {
        'level': level,
        'cls': _myArchetype,
        'k': ?live['k'],
        'proficiency': ?live['proficiency'],
        'classMod': ?live['class_mod'],
        'ac': ?live['ac'],
        'attackBonus': ?live['to_hit'],
        'spellDc': ?live['save_dc'],
        if (live['saves'] is Map) 'saves': live['saves'],
      };
    }
    return {
      'level': level,
      'cls': _myArchetype,
      'k': ?_stats['k'],
      'proficiency': ?_stats['proficiency'],
      'classMod': ?_stats['class_mod'],
      'ac': ?_stats['ac'],
      if (w is Map) 'attackBonus': ?w['to_hit'],
      'spellDc': ?_stats['save_dc'],
      if (saves is Map) 'saves': saves,
    };
  }

  /// Class title at a level (tiers at 30 and 60, see COMBAT.md).
  /// ` 3 d`, ` 5 h`, ` 12 min` (nothing under a minute): how long a party
  /// member has been offline.
  static String _ago(int secs) => switch (secs) {
    >= 86400 => ' ${secs ~/ 86400} d',
    >= 3600 => ' ${secs ~/ 3600} h',
    >= 60 => ' ${secs ~/ 60} min',
    _ => '',
  };

  static String _title(String archetype, int level) {
    const titles = {
      'swordsman': ['Swordsman', 'Crusader', 'Paladin'],
      'mage': ['Mage', 'Wizard', 'High Wizard'],
      'archer': ['Archer', 'Hunter', 'Sniper'],
      'acolyte': ['Acolyte', 'Priest', 'High Priest'],
      'thief': ['Thief', 'Assassin', 'Assassin Cross'],
      'merchant': ['Merchant', 'Alchemist', 'Creator'],
    };
    final t = titles[archetype];
    if (t == null) return archetype;
    return t[level < 30
        ? 0
        : level < 60
        ? 1
        : 2];
  }

  /// The loading screen for [map] (the engine shows it only if the load
  /// is slow, at once for the [first] map: nothing is behind it yet).
  void _beginLoading(MapInfo map, {required bool first}) {
    final town = !_inStage;
    final mods = _modDefs?.defs ?? const <ModifierDef>[];
    view.beginLoading(
      jsonEncode({
        'title': map.name,
        'subtitle': town ? (_channel == '-' ? '' : _channel) : _missionName,
        'map': map.id,
        'kind': map.kind,
        'now': first,
        'tips': loadingTips(
          (interact: _binds.interact?.caption ?? 'F', sit: _binds.sit?.caption ?? 'C'),
          town: town,
          mission: _missionName,
          brief: _brief?.map == map.id ? _brief : null,
          missionTips: _ladder.where((c) => c.map == map.id).firstOrNull?.tips ?? const [],
          modifiers: mods,
        ),
      }),
    );
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
    _beginLoading(map, first: _mapId.isEmpty);
    _mapEpoch = msg.mapEpoch;
    _mapId = map.id;
    _mapName = map.name;
    final x = msg.x, y = msg.y;
    _cancelWalk();
    _bonusAt.clear();
    _portals = const [];
    view.showPlayerMenu('');
    _hoverDirty = true;
    view.closeDialog();
    for (final id in _tracks.keys.toList()) {
      _removePeer(id);
    }
    for (final id in _peers.keys.toList()) {
      _removePeer(id);
    }
    _sitting.clear();
    _clearMobs();
    for (final id in _companions.keys.toList()) {
      _dropCompanion(id);
    }
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
    // Fog of war in town: the server sends only what is inside the light.
    if (!_inStage) {
      _townLight = (manifest['lightRadius'] as num?)?.toDouble() ?? 0;
      view.setLightRadius(_townLight);
    }
    final p = _predictor;
    if (p == null) {
      _predictor = LocalPredictor(grid, x, y, rules: MoveRules(tickHz: _tickHz));
    } else {
      p.reset(grid, x, y);
    }
    _applyPace();
    _mapSince = _now();
    _npcs = [
      for (final (i, n) in ((manifest['npcs'] as List?) ?? const []).indexed) _Npc(i, n as Map<String, Object?>),
    ];
    // Portal triggers (`[x, y, r]`, cells): click targets.
    _portals = [
      for (final pt in (manifest['portals'] as List?) ?? const [])
        if ((pt as Map)['trigger'] case [final num px, final num py, final num r])
          (px.toDouble() + 0.5, py.toDouble() + 0.5, r.toDouble()),
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
      _drawAppearance(me);
    }
    for (final e in _peers.entries) {
      if (_unlit.contains(e.key)) {
        _drawAppearance(e.key);
      } else {
        _showPeer(e.key, e.value);
      }
    }
    for (final id in _companions.keys) {
      _showCompanion(id);
    }
    _rootedSent.clear();
    _vitals.values.forEach(_applyHealth);
    _renderStatus();
    final bgm = map.bgm;
    view.playBgm(bgm == null ? null : '$assetsBase/client/$bgm');
    _loading = false;
    view.endLoading();
    if (_devShop case final shop?) {
      _devShop = null;
      _devNpc = shop;
      _send(ClientMsg.npcInteract(shop));
    }
    if (_inStage) {
      _renderStagePanel();
      view.toast('$_mapName: the mission has begun. Walk to the exit or use the panel to return.');
    } else if (msg.viaPortal == null) {
      // The keys once per visit (never on touch); back from a mission or a
      // reconnect, a short greeting is enough.
      final first = !_welcomed;
      final keys = first && !_input.touchOn;
      _welcomed = true;
      // A brand-new character's first arrival after signing in: the guide
      // opens on its own (minimise or close it; the Kafra has it too).
      // (`?guide=welcome`: as if new, for e2e runs whose heroes start higher.)
      final fresh = (_progress?.level ?? _myLevel) <= 1 || Uri.base.queryParameters['guide'] == 'welcome';
      if (first && fresh && _settings['guideAuto'] != false) _showGuide(welcome: true);
      view.toast(
        keys
            ? 'Welcome to $_mapName ($_channel). WASD to move, ${_binds.interact?.caption ?? 'F'} or Space to talk, '
                  'Alt+1–0 emotes, Ctrl+1–0 flags, ${_binds.sit?.caption ?? 'C'} to sit. F10: settings and keys.'
            : 'Welcome to $_mapName ($_channel).',
      );
    }
  }

  void _addPeer(PlayerInfo p) {
    final id = p.entityId;
    if (id == _me) {
      _myName = p.name;
      _myArchetype = p.archetype;
      _onAppearance(id, p.appearance);
      return;
    }
    final peer = _peers[id] = _Peer(p.name, p.archetype, p.appearance)..level = p.level;
    _syncCart();
    // Drawn from its first snapshot on (with fog in town, maybe never).
    if (_tracks.containsKey(id)) _showPeer(id, peer);
  }

  /// Name label and appearance of a peer (again after it comes back into view).
  void _showPeer(int id, _Peer peer) {
    view.setLabel(id.toDouble(), peer.name, peer.petSprite != null ? 'npc' : (peer.linkDead ? 'linkdead' : ''));
    _drawAppearance(id);
  }

  /// Hands a character's latest appearance to the engine, whole (hair,
  /// dyes, weapon, headgear): idempotent, so it is safe after a reload of
  /// the location, a fog re-entry or a repeated message. The body (class,
  /// tier, mount) is picked per frame from the same state (`_spriteFor`).
  /// Pets are plain sprites and get none.
  void _drawAppearance(int id) {
    if (_peers[id]?.petSprite != null) return;
    final a = id == _me ? (_mine ?? Appearance(cls: _myArchetype, look: _myLook)) : _peers[id]?.appearance;
    if (a == null) return;
    view.setAppearance(id.toDouble(), jsonEncode(a.toUi()));
  }

  /// A character's appearance as the server states it now (whole, see
  /// [Appearance]): stored, drawn, and for the own character the riding
  /// pace. Getting on or off a Peco Peco puffs and whooshes, unless it is
  /// the arrival roll call.
  void _onAppearance(int id, Appearance a) {
    final before = id == _me ? _mine : _peers[id]?.appearance;
    if (id == _me) {
      _mine = a;
      final restyled = _myLook != a.look;
      _myLook = a.look;
      _applyPace();
      if (restyled) {
        _renderStatus();
        if (_stylist != null) {
          _stylist = null;
          _styling = false;
          view.closeScreen();
          view.toast('Looking sharp!');
        }
      }
    } else {
      final peer = _peers[id];
      // Unknown here: its `player_appeared` (with this appearance) comes first.
      if (peer == null) return;
      peer.appearance = a;
    }
    _drawAppearance(id);
    _syncCart();
    // (The own character has no track: its prediction draws it.)
    if (before == null || before.riding == a.riding || (id != _me && !_tracks.containsKey(id)) || _now() - _mapSince < 1) return;
    view.unitSound(id.toDouble(), 'mount');
    if (id == _me)
      view.toast(a.riding ? 'You ride your Peco Peco: +${a.speed}% movement.' : 'You get off your Peco Peco.');
  }

  /// The stylist window over the game (hair style, hair and clothes dyes).
  void _showStylist({String? error}) {
    final npc = _npcs.where((n) => n.id == _stylist).firstOrNull;
    view.showStylist(
      jsonEncode({
        'title': npc?.name ?? 'Stylist',
        'text': npc?.text ?? 'Pick a new style.',
        'look': {..._myLook.toUi(), 'job': bodyJob(_myArchetype, _me == null ? 1 : _tierOf(_me!))},
        'busy': _styling,
        'error': ?error,
      }),
    );
  }

  void _removePeer(int id) {
    _peers.remove(id);
    _sitting.remove(id);
    _hidePeer(id);
    _syncCart();
  }

  // -------------------------------------------------------- companions

  /// Homunculus sprite ids by [HomType] (the companions pack, server
  /// `HomType::sprite`).
  static const _homSprites = ['', 'lif', 'amistr', 'filir', 'vanilmirth'];

  /// The skill the client treats specially: its slot opens the type
  /// chooser. (The cart follows the appearance, not the skill.)
  static const callHomunculusSkill = 'call_homunculus';

  /// `COMPANIONS`: new, changed and gone records.
  void _onCompanions(List<CompanionInfo> list) {
    if (list.isNotEmpty) _loadCompanionsPack();
    final now = _now();
    var mine = false;
    for (final c in list) {
      final id = c.entityId;
      if (c.ownerEntityId == _me && c.kind == CompanionClass.homunculus) {
        mine = true;
        _homName = c.name;
        _homEvolved = c.evolved;
      }
      if (c.gone) {
        _dropCompanion(id);
        continue;
      }
      _companions[id] = c;
      _companionSince[id] = now;
      _rootedSent.remove(id);
      _tracks[id]?.resetSent();
      _showCompanion(id);
    }
    if (mine) _renderHomunculus();
  }

  /// Name plate of a companion: its name, and on hover its level and owner.
  void _showCompanion(int id) {
    final c = _companions[id];
    if (c == null) return;
    final own = c.ownerEntityId == _me;
    view.setLabel(id.toDouble(), c.name, own ? 'ally own' : 'ally');
    final owner = own ? _myName : _peers[c.ownerEntityId]?.name;
    view.setLabelSub(
      id.toDouble(),
      jsonEncode([
        {'text': 'Lv. ${c.level}', 'color': '#9ff2e4'},
        if (owner != null) {'text': "$owner's", 'color': '#c8d0dc'},
      ]),
    );
  }

  void _dropCompanion(int id) {
    _companions.remove(id);
    _companionSince.remove(id);
    _companionDown.remove(id);
    _hitUntil.remove(id);
    _rootedSent.remove(id);
    _attackUntil.remove(id);
    _vitals.remove(id);
    _tracks.remove(id);
    _lastSeen.remove(id);
    view.setLabel(id.toDouble(), null, '');
    view.removeEntity(id.toDouble());
  }

  /// A companion's vitals: the fall when it drops, up again when revived;
  /// the own homunculus's plate follows its HP (four times a second).
  void _onCompanionVitals(PlayerVitals v, bool wasUp) {
    final id = v.entityId;
    if (v.state != 0 && wasUp) {
      if (!_companionDown.containsKey(id)) view.unitSound(id.toDouble(), 'die');
      _companionDown[id] ??= _now();
    } else if (v.state == 0) {
      _companionDown.remove(id);
    }
    _tracks[id]?.resetSent();
    final c = _companions[id];
    if (c != null && c.ownerEntityId == _me && c.kind == CompanionClass.homunculus) {
      final now = _now();
      if (now - _homDrawn > 0.25 || v.state != 0) _renderHomunculus();
    }
  }

  /// A companion's action: the fall, a strike, a flinch, a wind-up, walk / idle.
  int _companionAction(int id, bool moving, double now) {
    if ((_vitals[id]?.state ?? 0) != 0 || _companionDown.containsKey(id)) return Act.die;
    if (now < (_attackUntil[id] ?? 0)) return Act.attack;
    if (now < (_hitUntil[id] ?? 0)) return Act.hit;
    if (now < (_castUntil[id] ?? 0)) return Act.cast;
    return moving ? Act.walk : Act.idle;
  }

  /// Its sprite in the companions pack (-1 until the pack is in: no row).
  int _companionSprite(CompanionInfo c) {
    final key = '$_mapId:comp:${c.sprite}';
    final hit = _spriteCache[key];
    if (hit != null) return hit;
    final i = view.spriteIndex(c.sprite);
    if (i >= 0) _spriteCache[key] = i;
    return i;
  }

  int _allyFlags(CompanionInfo c) => Row.flagAlly | (c.ownerEntityId == _me ? Row.flagAllyOwn : 0);

  /// Plants and sphere mines: one standing row each, again only when the
  /// sprite, action or facing changes; at the end of its time a plant
  /// withers (the fall), then goes.
  void _rootedRows(double now) {
    List<int>? gone;
    for (final c in _companions.values) {
      if (!c.rooted) continue;
      final id = c.entityId;
      final since = _companionSince[id] ?? now;
      if (c.expiresMs > 0 && now - since >= c.expiresMs / 1000) _companionDown[id] ??= now;
      final down = _companionDown[id];
      if (down != null && now - down > 0.75) {
        (gone ??= []).add(id);
        continue;
      }
      final sprite = _companionSprite(c);
      if (sprite < 0) continue;
      final action = _companionAction(id, false, now);
      final facing = _attackFacing[id] ?? 0;
      final key = (sprite * 16 + action) * 8 + facing;
      if (_rootedSent[id] == key) continue;
      _rootedSent[id] = key;
      _out.add(
        id: id.toDouble(),
        x0: c.x,
        y0: c.y,
        t0: 0,
        x1: c.x,
        y1: c.y,
        t1: 0,
        facing: facing,
        sprite: sprite,
        action: action,
        flags: Row.flagHigh | _allyFlags(c) | statusRowFlags(_vitals[id]?.status ?? 0),
      );
    }
    gone?.forEach(_dropCompanion);
  }

  /// The companions pack, once (on the first companion or cart).
  void _loadCompanionsPack() {
    if (_companionsAsked) return;
    _companionsAsked = true;
    view.loadCompanions('$assetsBase/client/companions/').toDart.then((_) {
      // Rows skipped meanwhile (no sprite) go out now.
      for (final t in _tracks.values) {
        t.resetSent();
      }
      _rootedSent.clear();
      // Sprites the location atlas lacked may be in the pack now (the
      // field plants Plant Cultivation grows on any stage).
      _spriteCache.removeWhere((_, i) => i < 0);
      for (final m in _mobs.values) {
        m.dirty = true;
      }
    }).catchError((Object e) {
      _companionsAsked = false;
      web.console.warn('companions: $e'.toJS);
    });
  }

  /// `COMPANION_STATE`: the own homunculus panel. No type chosen yet and
  /// Call Homunculus just pressed: the chooser.
  void _onCompanionState(CompanionState s) {
    _homState = s;
    _homStateAt = _now();
    if (s.homType == HomType.none) {
      if (_now() - _callHomAt < 3 && !_homChooser) {
        _homChooser = true;
        _loadCompanionsPack();
        view.showHomunculusChooser(true);
      }
    } else if (_homChooser && !_homChanging) {
      _homChooser = false;
      view.showHomunculusChooser(false);
    }
    _renderHomunculus();
  }

  /// A press of the Call Homunculus slot (rising edge): remembered for the
  /// chooser, which opens at once when the server already said there is
  /// no type yet. The cast itself goes out with the slots as usual (out:
  /// the server makes it the focus / recall command).
  void _callHomPress(int slots) {
    final hotbar = _skills?.hotbar ?? const <String>[];
    var mask = 0;
    for (var i = 0; i < hotbar.length && i < Bindings.count; i++) {
      if (hotbar[i] == callHomunculusSkill) mask |= 1 << i;
    }
    final held = mask != 0 && slots & mask != 0;
    if (held && !_callHomHeld) {
      _callHomAt = _now();
      if (_homState?.homType == HomType.none && !_homChooser) {
        _homChooser = true;
        _loadCompanionsPack();
        view.showHomunculusChooser(true);
      }
    }
    _callHomHeld = held;
  }

  /// Price of a new homunculus type (server `companion::TYPE_CHANGE_ZENY`).
  static const homChangeZeny = 5000;

  /// The plate's Change button: the chooser again, to change the type (in
  /// town, for zeny; Bond starts again). The server has the last word.
  void _openHomChange() {
    final s = _homState;
    if (s == null || s.homType == HomType.none || _homChooser) return;
    final zeny = _inventory?.zeny ?? 0;
    final why = _inStage
        ? 'Change it in town.'
        : zeny < homChangeZeny
        ? 'Needs $homChangeZeny zeny (you have $zeny).'
        : null;
    _homChooser = true;
    _homChanging = true;
    _loadCompanionsPack();
    view.showHomunculusChooser(
      true,
      jsonEncode({'current': s.homType, 'price': homChangeZeny, 'ok': why == null, 'why': ?why}),
    );
  }

  /// The homunculus plate from the panel, its record and its vitals.
  void _renderHomunculus() {
    final s = _homState;
    if (s == null || s.homType == HomType.none || s.homType >= _homSprites.length) {
      view.setHomunculus('');
      return;
    }
    final now = _now();
    _homDrawn = now;
    final c = _companions.values.where((c) => c.ownerEntityId == _me && c.kind == CompanionClass.homunculus).firstOrNull;
    final v = c == null ? null : _vitals[c.entityId];
    final evolved = c?.evolved ?? _homEvolved;
    // The countdowns go on from when the panel came.
    final gone = ((now - _homStateAt) * 1000).round();
    int left(int ms) => ms <= 0 ? 0 : math.max(0, ms - gone);
    final level = _progress?.level ?? _skills?.level ?? _myLevel;
    view.setHomunculus(
      jsonEncode({
        'type': s.homType,
        'name': c?.name ?? _homName,
        'sprite': c?.sprite ?? '${_homSprites[s.homType]}${evolved ? '_h' : ''}',
        'level': s.level,
        'exp': s.exp,
        'expToNext': s.expToNext,
        'bond': s.bond,
        'stance': s.stance,
        'state': s.state,
        'backMs': left(s.backMs),
        'hp': v?.hp ?? 0,
        'maxHp': s.state == CompanionState.out ? (v?.maxHp ?? 0) : 0,
        'sp': s.sp,
        'maxSp': s.maxSp,
        'cooldownsMs': [for (final ms in s.cooldownsMs) left(ms)],
        'evolved': evolved,
        // The Evolve button from tier 3 on: what is still missing, or ready.
        if (!evolved && level >= 60) 'evolve': _evolveCheck(level, s.bond),
      }),
    );
  }

  /// The client's side of the evolve rule (design: Bond 75, a Stone of
  /// Sage, in town); the server has the last word.
  Map<String, Object> _evolveCheck(int level, int bond) {
    final missing = [
      if (bond < 75) 'Bond 75 (now $bond)',
      if ((_inventory?.count('stone_of_sage') ?? 0) < 1) 'a Stone of Sage (Pharmacy)',
      if (_inStage) 'to be in town',
    ];
    return missing.isEmpty
        ? {'ok': true, 'why': 'Evolve: stronger, a new look and a fourth skill. Uses one Stone of Sage.'}
        : {'ok': false, 'why': 'Needs ${missing.join(', ')}.'};
  }

  /// Every Pushcart on the map follows the appearances (0xA6 `cart`): the
  /// own one and every peer's, diffed against what the engine draws. A cart
  /// out of view goes with its owner on the engine's side.
  void _syncCart() {
    final me = _me;
    final want = <int>{
      if (me != null && (_mine?.cart ?? false)) me,
      for (final MapEntry(:key, :value) in _peers.entries)
        if (key != me && value.petSprite == null && value.appearance.cart) key,
    };
    for (final id in _carts.difference(want)) {
      view.setPushcart(id.toDouble(), false);
    }
    for (final id in want.difference(_carts)) {
      view.setPushcart(id.toDouble(), true);
    }
    if (want.isNotEmpty) _loadCompanionsPack();
    _carts
      ..clear()
      ..addAll(want);
  }

  /// Out of view (beyond the fog, or silent): the sprite goes, the peer
  /// stays known until `player_vanished`.
  void _hidePeer(int id) {
    _unlit.remove(id);
    _tracks.remove(id);
    _lastSeen.remove(id);
    view.setLabel(id.toDouble(), null, '');
    view.removeEntity(id.toDouble());
  }

  // ---------------------------------------------------------------- input

  void _onKeyDown(web.KeyboardEvent e) {
    // `is` on JS interop types checks only the Dart representation; use isA.
    if (e.target.isA<web.HTMLInputElement>() || e.target.isA<web.HTMLTextAreaElement>()) return;
    _keys.add(e.code);
    if (!e.repeat) _tapped.add(e.code);
    final b = _binds;
    // Emotes and flags take their keys from the browser (Alt / Ctrl + digit).
    final emote = b.emoteOf(e);
    if (emote >= 0) {
      e.preventDefault();
      if (!e.repeat) _send(ClientMsg.emote(b.emoteIds[emote]));
      return;
    }
    final flag = b.flagOf(e);
    if (flag >= 0) {
      e.preventDefault();
      if (!e.repeat) _placeFlag(flag);
      return;
    }
    if (e.repeat) {
      if (_inStage && (e.code == 'Space' || e.code == 'Tab')) e.preventDefault();
      return;
    }
    // On missions the travel key (Space by default) is the travel skill.
    final travelKey = _inStage && (b.travel?.matches(e) ?? false);
    if (travelKey) e.preventDefault();
    if (!travelKey && ((b.interact?.matches(e) ?? false) || (b.interact2?.matches(e) ?? false))) {
      if (b.interact2?.matches(e) ?? false) e.preventDefault();
      _interactNearest();
    }
    if (b.potion?.matches(e) ?? false) _quickPotion('hp');
    if (b.potion2?.matches(e) ?? false) _quickPotion('sp');
    if (b.items?.matches(e) ?? false) {
      e.preventDefault();
      view.toggleWindow('items');
    }
    if ((b.sit?.matches(e) ?? false) && !_inStage && _myState == 0) {
      final me = _me;
      if (me != null) _send(ClientMsg.pose(sitting: !_sitting.contains(me)));
    }
    if (b.slots.any((k) => k != null && k.matches(e))) e.preventDefault();
    if (_inStage && (e.code == 'Space' || e.code == 'Tab')) e.preventDefault();
    if (e.code == 'KeyK' && !e.repeat) view.toggleSkillTree(-1);
    if (b.equip?.matches(e) ?? false) {
      e.preventDefault();
      view.toggleWindow('equip');
    }
    if (b.info?.matches(e) ?? false) {
      e.preventDefault();
      view.toggleWindow('basic');
    }
    if (b.party?.matches(e) ?? false) {
      e.preventDefault();
      _toggleParty();
    }
    if (b.friends?.matches(e) ?? false) {
      e.preventDefault();
      view.toggleWindow('friends');
    }
    if (b.leaderboard?.matches(e) ?? false) {
      e.preventDefault();
      view.toggleWindow('leaderboard');
    }
    // Esc: the engine closes the topmost window first (and marks the event);
    // with nothing to close it opens the game menu.
    // Marked handled either way, so the engine does not close it again.
    if (e.code == 'Escape' && !e.defaultPrevented && !view.dialogOpen()) {
      e.preventDefault();
      _toggleMenu();
    }
    if (b.debug?.matches(e) ?? false) {
      e.preventDefault();
      final mode = view.cycleDebugMode();
      if (mode == 0) view.toast('Debug overlay off (F1)');
    }
    if (e.code == 'F10') {
      e.preventDefault();
      _toggleSettings();
    }
  }

  void _onKeyUp(web.KeyboardEvent e) => _keys.remove(e.code);

  /// Pointer position in client pixels. `package:web` types `clientX` as an
  /// int, but pointer events carry fractions (high-DPI, zoom), which a typed
  /// read rejects under dart2js; read the JS number as it is.
  static (double, double) _clientXY(web.MouseEvent e) {
    final o = e as JSObject;
    double at(String k) => o.getProperty<JSNumber>(k.toJS).toDartDouble;
    return (at('clientX'), at('clientY'));
  }

  /// A click that reached the canvas. The mouse acts on its press
  /// ([_onMouseDown]); this is a touch tap (the canvas's own, or the
  /// joystick zone's short tap): an NPC always (tap to talk), the rest with
  /// Settings → Tap to move.
  void _onClick(web.MouseEvent e) {
    // The click that ends a long press (the player menu) is not a tap,
    // nor the one that follows a mouse press.
    if (_now() < _suppressClickUntil || _now() - _mouseDownAt < 1) return;
    final (x, y) = _clientXY(e);
    _clickWorld(x, y, everything: _tapMove);
  }

  // ------------------------------------------------- click to walk, hover

  /// Click (tap) to move (COMBAT.md §4.2): the walk under way, if any, and
  /// the left button held on the ground (the walk follows the pointer).
  final _walker = Walker();
  bool _steerHeld = false;
  double _steerAt = 0;
  double _mouseDownAt = -10;

  /// Settings → Click to move (mouse, on by default) and Tap to move
  /// (touch, off by default: the joystick is the touch default and a
  /// missed skill button would walk the hero out of its stance).
  bool _clickMove = true;
  bool _tapMove = false;

  /// Live pickup bubbles (id → cell) and the portals of the map (centre,
  /// radius): click targets.
  final _bonusAt = <int, (double, double)>{};
  var _portals = const <(double, double, double)>[];

  /// Basic attack reach of the held weapon (cells, the server's `stats`).
  double get _weaponReach {
    final w = _stats['weapon'];
    return ((w is Map ? w['reach'] : null) as num?)?.toDouble() ?? 2.5;
  }

  /// Left button on the canvas (mouse): the same as a tap, and on the
  /// ground holding it steers.
  void _onMouseDown(web.PointerEvent e) {
    if (e.button != 0 || e.ctrlKey || e.metaKey) return;
    _mouseDownAt = _now();
    final (x, y) = _clientXY(e);
    if (_clickWorld(x, y, everything: _clickMove) == WalkGoal.ground) {
      _steerHeld = true;
      _walker.steering = true;
      _steerAt = _now();
      _hoverDirty = true;
    }
  }

  /// Left button released anywhere: the walk goes on to the last point.
  void _onMouseUp(web.PointerEvent e) {
    if (e.pointerType != 'mouse' || e.button != 0 || !_steerHeld) return;
    _steerHeld = false;
    _walker.steering = false;
    _hoverDirty = true;
    final end = _walker.end;
    if (_walker.active && end != null) view.markMove(end.$1, end.$2, 0);
  }

  /// A click or tap on the world at client (x, y): talk to the NPC under
  /// it (walking there first); with [everything] also walk into weapon
  /// reach of a monster, into a pickup bubble or a portal, or to the
  /// ground. Returns what it walks to (null: nothing).
  WalkGoal? _clickWorld(double x, double y, {required bool everything}) {
    final p = _predictor;
    if (p == null || _loading || _offline || _myState != 0 || view.dialogOpen()) return null;
    final id = view.pickEntity(x, y);
    final npc = id.isNaN || id >= 0 ? null : _npcs.where((n) => n.entityId == id).firstOrNull;
    if (npc != null) {
      if (npc.inReach(p.x, p.y)) {
        _cancelWalk();
        _send(ClientMsg.npcInteract(npc.id));
      } else {
        _walkTo(npc);
      }
      return WalkGoal.npc;
    }
    if (!everything) return null;
    final mob = !id.isNaN && id >= monsterEntityBase ? _mobs[id.toInt() - monsterEntityBase] : null;
    if (mob != null && mob.dieAt == null && mob.removeAt == null) {
      final (mx, my) = _mobAt(mob);
      // Centre to centre, so the reach to its edge (the server's rule) holds
      // for any body; a little inside it so a step back does not undo it.
      final reach = math.max(1.0, _weaponReach - 0.3);
      return _walkGoal(WalkGoal.monster, mx, my, reach: reach, target: (mob.slot, mob.generation))
          ? WalkGoal.monster
          : null;
    }
    final bonus = view.pickBonus(x, y);
    final at = bonus.isNaN ? null : _bonusAt[bonus.toInt()];
    if (at != null) {
      // The server takes it within 1.2 cells.
      return _walkGoal(WalkGoal.bonus, at.$1, at.$2, reach: 0.8, target: bonus.toInt()) ? WalkGoal.bonus : null;
    }
    final g = view.pickGround(x, y);
    if (g == null) return null;
    final portal = portalAt(_portals, g.$1, g.$2);
    if (portal != null) {
      final (px, py, r) = portal;
      if (!_walkGoal(WalkGoal.portal, px, py, reach: math.max(0.5, r - 0.75))) return null;
      view.markMove(px, py, 0);
      return WalkGoal.portal;
    }
    return _walkGoal(WalkGoal.ground, g.$1, g.$2) ? WalkGoal.ground : null;
  }

  /// Starts a walk; marks where a ground walk ends (tinted when it stops
  /// short of the click). False when there is no way at all.
  bool _walkGoal(WalkGoal goal, double tx, double ty, {double reach = 0, Object? target}) {
    final p = _predictor!;
    final end = _walker.start(p.grid, p.x, p.y, goal, tx, ty, reach: reach, target: target, now: _now());
    if (end == null) {
      if (goal != WalkGoal.ground) view.toast('No way there from here.');
      return false;
    }
    _steerHeld = false;
    _sitting.remove(_me);
    if (goal == WalkGoal.ground) {
      final short = (end.$1 - tx).abs() > 0.75 || (end.$2 - ty).abs() > 0.75;
      view.markMove(end.$1, end.$2, short ? 1 : 0);
    }
    return true;
  }

  /// Walks to [npc] (a clicked NPC out of reach) and talks on arrival.
  void _walkTo(_Npc npc) {
    final p = _predictor;
    if (p == null || _loading || _myState != 0) return;
    // A step inside the radius, so rounding never leaves us just outside.
    final reach = math.max(0.5, npc.radius - 0.35);
    if (_walker.start(p.grid, p.x, p.y, WalkGoal.npc, npc.x, npc.y, reach: reach, target: npc, now: _now()) == null) {
      view.toast('No way to ${npc.name} from here.');
      return;
    }
    _steerHeld = false;
    _sitting.remove(_me);
  }

  void _cancelWalk() {
    _walker.stop();
    _steerHeld = false;
    view.markMove(double.nan, double.nan, 0);
  }

  /// A monster's cell now (its record dead-reckoned to the render clock).
  (double, double) _mobAt(_Mob m) {
    final u = (_clock.renderTick(_now()) - m.stamp).clamp(-Reckoning.back, Reckoning.horizon);
    return (m.x / Reckoning.posOne + m.vx / Reckoning.velOne * u, m.y / Reckoning.posOne + m.vy / Reckoning.velOne * u);
  }

  /// This tick's world direction of the click-to-move walk: follows the
  /// pointer while the button is held, a monster while it lives; talks to
  /// an NPC on arrival, gives up when stuck.
  (double, double) _walkDir(LocalPredictor p) {
    final w = _walker;
    final now = _now();
    switch (w.goal) {
      case WalkGoal.ground when _steerHeld && now - _steerAt >= 0.12:
        _steerAt = now;
        final ptr = _pointer;
        final g = ptr == null ? null : view.pickGround(ptr.$1, ptr.$2);
        if (g != null) w.retarget(p.grid, p.x, p.y, g.$1, g.$2, now: now);
      case WalkGoal.monster:
        final (slot, gen) = w.target! as (int, int);
        final m = _mobs[slot];
        if (m == null || m.generation != gen || m.dieAt != null || m.removeAt != null) {
          _cancelWalk();
          return (0, 0);
        }
        if (now - _steerAt >= 0.3) {
          _steerAt = now;
          final (mx, my) = _mobAt(m);
          w.retarget(p.grid, p.x, p.y, mx, my, now: now, slack: 1);
        }
      case WalkGoal.bonus when !_bonusAt.containsKey(w.target):
        _cancelWalk();
        return (0, 0);
      default:
        break;
    }
    final npc = w.target is _Npc ? w.target! as _Npc : null;
    final status = w.next(p.x, p.y, now, stepLen: p.rules.step * p.pace);
    switch (status) {
      case WalkStatus.walking:
        return w.dx == 0 && w.dy == 0 ? (0, 0) : quantizeDir(w.dx, w.dy);
      case WalkStatus.arrived:
        if (npc != null && !view.dialogOpen()) _send(ClientMsg.npcInteract(npc.id));
      // Held up inside the talk radius (short of the margin aimed for):
      // close enough to talk.
      case WalkStatus.stuck when npc != null && npc.inReach(p.x, p.y):
        if (!view.dialogOpen()) _send(ClientMsg.npcInteract(npc.id));
      case WalkStatus.stuck:
        view.toast(npc != null ? 'Could not reach ${npc.name}.' : 'Could not get there.');
      case WalkStatus.idle:
        break;
    }
    return (0, 0);
  }

  /// The entity under the mouse changed since the last frame: light up
  /// NPCs (click to talk) and players (right-click menu), and pick the
  /// world cursor: talk over an NPC, attack over a living monster, pick
  /// over a pickup bubble.
  bool _hoverDirty = false;
  double _hoverId = double.nan;
  String _hoverCursor = '';

  void _updateHover() {
    if (!_hoverDirty) return;
    _hoverDirty = false;
    final ptr = _pointer;
    var id = double.nan;
    var cursor = '';
    if (ptr != null && !_input.touchOn && !_loading) {
      final hit = view.pickEntity(ptr.$1, ptr.$2);
      if (!hit.isNaN && (hit < 0 ? _npcs.any((n) => n.entityId == hit) : _isPlayer(hit.toInt()))) id = hit;
      if (!id.isNaN && id < 0) {
        cursor = 'talk';
      } else if (!hit.isNaN && hit >= monsterEntityBase && _mobs[hit.toInt() - monsterEntityBase]?.dieAt == null) {
        cursor = 'attack';
      } else if (!view.pickBonus(ptr.$1, ptr.$2).isNaN) {
        cursor = 'pick';
      } else if (_clickMove || _steerHeld) {
        // Click to move: the walking cursor over open ground, the door
        // over a portal.
        final g = view.pickGround(ptr.$1, ptr.$2);
        final grid = _predictor?.grid;
        if (g != null && portalAt(_portals, g.$1, g.$2) != null) {
          cursor = 'portal';
        } else if (_steerHeld || (g != null && (grid?.isWalkable(g.$1, g.$2) ?? false))) {
          cursor = 'walk';
        }
      }
    }
    if (cursor != _hoverCursor) view.setCursor(_hoverCursor = cursor);
    if (id == _hoverId || (id.isNaN && _hoverId.isNaN)) return;
    _hoverId = id;
    view.setHover(id, !id.isNaN && id < 0);
  }

  /// A real player other than us (not a townsperson or a pet).
  bool _isPlayer(int id) =>
      id != _me &&
      id > 0 &&
      id < companionEntityBase &&
      (_peers[id]?.petSprite == null) &&
      _peers.containsKey(id);

  // ------------------------------------------------------- player menu

  /// Right button down (mouse): a click opens the menu, a drag turns the camera.
  (double, double, double)? _rightDown;
  double _lastMouseRight = -10;

  /// Touch long press on the canvas: where, and its timer.
  Timer? _hold;
  (double, double)? _holdAt;
  double _suppressClickUntil = 0;

  void _onCanvasDown(web.PointerEvent e) {
    final (x, y) = _clientXY(e);
    if (e.pointerType == 'mouse') {
      if (e.button == 2) {
        _rightDown = (x, y, _now());
        _lastMouseRight = _now();
      }
      _onMouseDown(e);
      return;
    }
    _cancelHold();
    _holdAt = (x, y);
    _hold = Timer(const Duration(milliseconds: 500), () {
      final at = _holdAt;
      _cancelHold();
      if (at == null) return;
      _suppressClickUntil = _now() + 0.8;
      _openPlayerMenuAt(at.$1, at.$2);
    });
  }

  void _onCanvasMove(web.PointerEvent e) {
    final at = _holdAt;
    if (at == null) return;
    final (x, y) = _clientXY(e);
    if ((x - at.$1).abs() + (y - at.$2).abs() > 12) _cancelHold();
  }

  void _onCanvasUp(web.PointerEvent e) {
    _cancelHold();
    final down = _rightDown;
    if (e.pointerType != 'mouse' || e.button != 2 || down == null) return;
    _rightDown = null;
    _lastMouseRight = _now();
    final (x, y) = _clientXY(e);
    if ((x - down.$1).abs() + (y - down.$2).abs() < 8 && _now() - down.$3 < 0.6) _openPlayerMenuAt(x, y);
  }

  void _cancelHold() {
    _hold?.cancel();
    _hold = null;
    _holdAt = null;
  }

  /// `contextmenu` from touch (a native long press, or the joystick
  /// zone's); the mouse goes through [_onCanvasUp].
  void _onContextMenu(web.MouseEvent e) {
    e.preventDefault();
    if (_now() - _lastMouseRight < 1) return;
    final (x, y) = _clientXY(e);
    _cancelHold();
    _suppressClickUntil = _now() + 0.8;
    _openPlayerMenuAt(x, y);
  }

  void _openPlayerMenuAt(double x, double y) {
    if (_loading) return;
    final id = view.pickEntity(x, y);
    if (id.isNaN || !_isPlayer(id.toInt())) return;
    _openPlayerMenu(id.toInt(), x: x, y: y);
  }

  /// Gamepad Y in town: the menu of the nearest player in sight.
  void _nearestPlayerMenu() {
    final p = _predictor;
    if (p == null || _loading) return;
    if (view.playerMenuTarget() != 0) {
      view.showPlayerMenu('');
      return;
    }
    int? best;
    var bestD = 12.0 * 12.0;
    for (final id in _peers.keys) {
      final t = _tracks[id];
      if (t == null || !_isPlayer(id) || _unlit.contains(id)) continue;
      final x = t.x, y = t.y;
      final d = (x - p.x) * (x - p.x) + (y - p.y) * (y - p.y);
      if (d < bestD) {
        bestD = d;
        best = id;
      }
    }
    if (best == null) {
      view.toast('Nobody near enough.');
      return;
    }
    _openPlayerMenu(best);
  }

  /// The context menu on player [id]: what we may do with them now.
  void _openPlayerMenu(int id, {double? x, double? y}) {
    final peer = _peers[id];
    if (peer == null) return;
    final lobby = _lobby;
    final members = lobby?.members ?? const <LobbyMember>[];
    final lead = members.any((m) => m.entityId == _me && m.leader);
    final theirs = members.any((m) => m.entityId == id);
    final room = lobby == null || members.length < lobby.mission.maxPlayers;
    // Friends are by account; every character of the account is listed
    // (entity ids of players are character ids).
    final friend = _friends.where((f) => f.characters.contains(id)).firstOrNull;
    view.showPlayerMenu(
      jsonEncode({
        'target': id,
        'name': peer.name,
        'title': '${_title(peer.archetype, peer.level)} · Lv ${peer.level}',
        'x': ?x,
        'y': ?y,
        'items': [
          if (!_inStage) {'id': 'trade', 'label': 'Trade', 'detail': 'Swap items and zeny'},
          if (friend == null)
            {'id': 'friend', 'label': 'Add friend', 'detail': 'See when they are online'}
          else if (friend.link == FriendLink.outgoing)
            {'id': 'friend', 'label': 'Add friend', 'detail': 'Request sent', 'disabled': true},
          if (!_inStage && !theirs && (lobby == null || lead))
            {
              'id': 'party_invite',
              'label': 'Invite to party',
              'detail': room ? 'Team up for a mission' : 'The party is full',
              'disabled': !room,
            },
          if (!_inStage && lead && theirs)
            {'id': 'party_kick', 'label': 'Kick from party', 'detail': 'Remove them from the party'},
          {'id': 'inspect', 'label': 'View equipment', 'detail': 'Their gear and look'},
        ],
      }),
    );
  }

  /// Another player invited us: Join / Decline (closing it declines).
  int? _inviteFrom;

  void _onPartyInvite(PartyInvite inv) {
    if (_inStage) {
      _send(ClientMsg.partyAnswer(inv.from, accept: false));
      return;
    }
    if (_lobby != null) {
      // Already in a party (the server checked when it was sent).
      _send(ClientMsg.partyAnswer(inv.from, accept: false));
      return;
    }
    _inviteFrom = inv.from;
    _npcDialog = false;
    _dropRefiner();
    view.showDialog(
      jsonEncode({
        'id': 'party_invite',
        'title': 'Party invitation',
        'text':
            '${inv.name} invites you to their party for ${inv.mission}. '
            'Join within ${(inv.ttlMs / 1000).round()} s.',
        'options': [
          {
            'id': 'party:accept:${inv.from}',
            'label': 'Join the party',
            'detail': 'Team up: the leader starts the missions.',
          },
          {'id': 'party:decline:${inv.from}', 'label': 'Decline', 'detail': 'Stay where you are; ${inv.name} is told.'},
        ],
      }),
    );
  }

  /// Another player's gear (View equipment). A refresh's answer for a
  /// window closed meanwhile is dropped (it must not open it again).
  void _onInspect(PlayerInspect s) {
    if (s.entityId != _inspectId) return;
    if (_inspected != null && view.inspectTarget().toInt() != s.entityId) return;
    if (_inspected?.entityId != s.entityId) _inspectedSheet = null;
    _inspected = s;
    _showInspected();
  }

  /// A live sheet: the player's own (Equipment window, glossary) or the
  /// inspected player's.
  void _onSheet(PlayerSheet s) {
    final Object? json;
    try {
      json = jsonDecode(s.json);
    } on FormatException {
      return;
    }
    if (json is! Map<String, Object?>) return;
    if (s.entityId == _me) {
      _liveSheet = json;
      _renderStatus();
    } else if (s.entityId == _inspected?.entityId && view.inspectTarget().toInt() == s.entityId) {
      _inspectedSheet = json;
      _showInspected();
    }
  }

  void _showInspected() {
    final s = _inspected;
    if (s == null) return;
    final sheet = _inspectedSheet;
    view.showInspect(
      jsonEncode({
        'id': s.entityId,
        'name': s.name,
        'title': _title(s.archetype, s.level),
        'level': s.level,
        'look': {...s.look.toUi(), 'job': bodyJob(s.archetype, tierOfLevel(s.level))},
        'equip': s.equip,
        if (sheet != null) 'sheet': sheetView(sheet),
      }),
    );
  }

  /// View equipment on [target]: asks now, then once a second while the
  /// window shows them (their buffs and conditions come and go).
  void _inspect(int target) {
    _inspectId = target;
    _inspected = null;
    _inspectedSheet = null;
    _send(ClientMsg.inspect(target));
    _inspectTimer?.cancel();
    _inspectTimer = Timer.periodic(const Duration(seconds: 1), (t) {
      if (_inspected == null) {
        // The first answer is on its way (or the player was gone).
        if (t.tick > 5) t.cancel();
        return;
      }
      if (view.inspectTarget().toInt() != target || _inspectId != target) {
        t.cancel();
        if (_inspectId == target) _inspectId = null;
        return;
      }
      _send(ClientMsg.inspect(target));
    });
  }

  /// A flag where the pointer is on the ground (else under the player).
  void _placeFlag(int kind) {
    final p = _predictor;
    if (p == null || _loading) return;
    final ptr = _pointer;
    final g = ptr == null || _input.touchOn ? null : view.pickGround(ptr.$1, ptr.$2);
    final (x, y) = g ?? (p.x, p.y);
    _send(ClientMsg.flag(kind, x, y));
  }

  void _interactNearest() {
    final npc = _nearNpc;
    if (npc != null && !view.dialogOpen()) _send(ClientMsg.npcInteract(npc.id));
  }

  /// The online count beside the minimap: in town only.
  void _showOnline() {
    final s = _pulse.last;
    view.setOnline(_inStage || s == null || s.down ? 0 : s.online);
    // A deploy behind this tab: offer the reload (`update.dart`).
    pageUpdates?.sawServer(s?.build);
  }

  /// A map event of the run (server `hub/env.rs`): a toast as it warns,
  /// begins and ends. Its hazards come as lasting ground (`BossZones`).
  void _onEnvEvent(EnvEvent e) {
    if (!_inStage) return;
    view.setEnvironmentEvent(
      jsonEncode({'phase': e.phase, 'id': e.id, 'name': e.name, 'startMs': e.startMs, 'durationMs': e.durationMs}),
    );
    view.toast(_envEvents.take(e));
  }

  /// A new instance (town, another stage, a rejoin): every map event the
  /// last one warned of or ran ends on the view at once (the war horn's
  /// side banner, a storm's veil), not only when the next map loads.
  void _endEnvEvents() {
    for (final id in _envEvents.clear()) {
      view.setEnvironmentEvent(jsonEncode({'phase': EnvEvent.ends, 'id': id, 'name': '', 'startMs': 0, 'durationMs': 0}));
    }
  }

  void _onNpcDialog(NpcDialog msg) {
    if (msg.npc != _devNpc) _devNpc = null;
    if (msg.kind == 'refiner') {
      _refiner = (id: msg.npc, name: msg.name);
      _showRefiner();
      return;
    }
    _dropRefiner();
    if (msg.kind == 'stylist') {
      view.closeDialog();
      _stylist = msg.npc;
      _styling = false;
      _showStylist();
      return;
    }
    final missions = msg.missions;
    final isKafra = msg.kind == 'kafra';
    // A Kafra employee sends to battle like the old mission board did.
    final isBoard = msg.kind == 'mission_board' || isKafra;
    final isMaster = msg.kind == 'skill_master' || isKafra;
    final isLog = msg.kind == 'changelog';
    // The Scribe takes feedback; a Kafra does too.
    final isScribe = msg.kind == 'feedback';
    final npc = _npcs.where((n) => n.id == msg.npc).firstOrNull;
    _settingsOpen = false;
    final inLobby = _lobby != null;
    if (isBoard) {
      _offer = missions;
      _offerFrom = msg.name;
    }
    // A plain mission board has nothing else to say: straight to the window.
    if (msg.kind == 'mission_board' && missions.isNotEmpty) {
      _showMissions(null);
      return;
    }
    final cutin = npc?.cutin;
    final cutinUrl = cutin == null
        ? null
        : (cutin.startsWith('/') || cutin.startsWith('http'))
        ? cutin
        : '$assetsBase/client/$cutin';
    _npcDialog = true;
    view.showDialog(
      jsonEncode({
        'id': 'npc:${msg.npc}',
        'title': msg.name,
        if (cutinUrl != null) 'cutin': cutinUrl,
        'text': [
          npc?.text ?? (isBoard ? '' : 'Hello there.'),
          if (isBoard && inLobby && _leading) 'You lead a party: pick its mission and start whenever you like.',
          if (isBoard && inLobby && !_leading) 'You are in a party: its leader picks and starts its missions.',
          if (isBoard && !inLobby && missions.isNotEmpty)
            'Missions: go alone or in parties of up to ${missions.map((m) => m.maxPlayers).reduce(math.max)}.',
        ].where((t) => t.isNotEmpty).join('\n'),
        if (isLog)
          'sections': [
            for (final c in changelog) {'title': c.title, 'date': c.date, 'items': c.items},
          ],
        'options': [
          // Every mission is picked in the expedition window (list, page,
          // modifiers, start); a party member sees the leader's pick there.
          if (isBoard && missions.isNotEmpty)
            {
              'id': 'missions:',
              'label': 'Choose an expedition',
              'detail': switch ((inLobby, _leading)) {
                (false, _) => _expeditionsLine(missions),
                (true, true) => "Pick your party's next expedition and start whenever you like.",
                _ => "See where your party is headed: its leader picks and starts.",
              },
            },
          if (isKafra)
            {
              'id': 'parties:list',
              'label': 'Parties looking for adventurers',
              'detail': 'Public parties in town or already out on an expedition: join one and go.',
            },
          if (isKafra)
            {
              'id': 'menu:guide',
              'label': "Adventurer's Guide",
              'detail': 'How the game works: town, expeditions, the horde, skills, items and parties.',
            },
          if (isKafra)
            {'id': 'restyle', 'label': 'Hair and dyes', 'detail': 'A new hair style, hair dye or clothes dye.'},
          if (isMaster)
            {
              'id': 'skill_reset',
              'label': 'Forget every skill',
              'detail': 'All skill points come back to spend again. Your hotbar is cleared.',
            },
          // The notice board also posts the Hall of Fame.
          if (isLog)
            {
              'id': 'menu:leaderboard',
              'label': 'Hall of Fame',
              'detail': [
                'The top adventurers by level, kills, MVPs and zeny.',
                if (_pulse.last case OnlineStatus(down: false, :final online) when online > 0)
                  '$online adventurer${online == 1 ? ' is' : 's are'} out there right now.',
              ].join(' '),
            },
          if (isScribe)
            for (final k in feedbackKinds) {'id': 'feedback:${k.id}', 'label': k.prompt, 'detail': k.detail},
          if (isKafra)
            {
              'id': 'feedback:',
              'label': 'Leave feedback',
              'detail': 'Report a bug or suggest something: the developers read every report.',
            },
        ],
        if (isBoard && !inLobby) 'input': {'id': 'code', 'label': 'Join', 'placeholder': 'Party code', 'maxLength': 8},
      }),
    );
  }

  /// The smith's refine window: every refinable item worn, then in the
  /// bag, each with its next attempt worked out (`refineRow`); [select]
  /// picks a row (an attempt's item under its new key).
  void _showRefiner({String? select}) {
    final smith = _refiner, inv = _inventory;
    if (smith == null || inv == null) return;
    final npc = _npcs.where((n) => n.id == smith.id).firstOrNull;
    final rows = <Map<String, Object?>>[];
    void offer(String key, int? slot) {
      final def = _itemDefs[splitKey(key).id];
      if (def == null) return;
      final row = refineRow(
        def,
        key,
        slot,
        have: inv.count,
        oreName: (ore) => _itemDefs[ore]?.name ?? ore,
        zeny: inv.zeny,
      );
      if (row != null) rows.add(row);
    }

    for (var s = 0; s < inv.equip.length; s++) {
      if (inv.equip[s].isNotEmpty) offer(inv.equip[s], s);
    }
    for (final st in inv.items) {
      offer(st.id, null);
    }
    _npcDialog = true;
    view.showRefine(
      jsonEncode({
        'npc': smith.id,
        'name': smith.name,
        'text': npc?.text ?? 'What shall I refine?',
        'zeny': inv.zeny,
        'rows': rows,
        'select': ?select,
        'rules': refineRules,
      }),
    );
  }

  /// The smith's window goes (another window takes over, out of reach).
  void _dropRefiner() {
    if (_refiner == null) return;
    _refiner = null;
    _refining = null;
    view.showRefine('');
  }

  /// One line for the Kafra's "Choose an expedition": how many, which levels.
  String _expeditionsLine(List<MissionInfo> offer) {
    final open = _ladder.where((c) => !c.soon).toList();
    if (_ladder.length > 1 && open.isNotEmpty) {
      final lo = open.map((c) => c.band.min).reduce(math.min), hi = open.map((c) => c.band.max).reduce(math.max);
      final soon = _ladder.length - open.length;
      return '${open.length} expedition${open.length == 1 ? '' : 's'} open, Lv $lo–$hi${soon > 0 ? ', $soon more coming soon' : ''}. '
          'Go alone or in parties of up to ${open.map((c) => c.maxPlayers).reduce(math.max)}.';
    }
    return 'Pick where to fight, see its monsters and loot, then go alone or in parties of up to '
        '${offer.map((m) => m.maxPlayers).reduce(math.max)}.';
  }

  /// The expedition window for the last mission giver's offer (the cards
  /// come with the dialog: `MissionList`, then the briefs: `MissionBriefing`),
  /// or for the party's mission (its modifiers) when no giver was asked.
  void _showMissions(String? selected) {
    final lobby = _lobby;
    final offer = _offer.isNotEmpty ? _offer : [?lobby?.mission];
    if (offer.isEmpty) return;
    final cards = {for (final c in _ladder) c.id: c};
    // The ladder in its order (coming-soon ones too) when a giver offered
    // it; else the offer, with its card where there is one.
    final ids = _offer.isNotEmpty && _ladder.isNotEmpty
        ? [for (final c in _ladder) c.id]
        : [for (final m in offer) m.id];
    final infos = {for (final m in offer) m.id: m};
    _missionsShown = true;
    view.showMissions(
      jsonEncode({
        'name': _offer.isNotEmpty ? _offerFrom : 'Party',
        'missions': [
          for (final id in ids)
            if (cards[id] case final c?)
              _cardJson(c)
            else if (infos[id] case final m?)
              {'id': m.id, 'name': m.name, 'description': m.description, 'maxPlayers': m.maxPlayers, 'map': m.map},
        ],
        'inLobby': lobby != null,
        'selected': ?selected,
        'level': _progress?.level ?? _skills?.level ?? _myLevel,
        // Who a run would take: the under-level warning goes by them all
        // (server `LevelBand::fit`: only when everyone is under the band).
        'levels': _goingLevels,
      }),
    );
  }

  /// Levels of who a start takes: the party's members in town, else the player.
  List<int> get _goingLevels {
    final me = _progress?.level ?? _skills?.level ?? _myLevel;
    final lobby = _lobby;
    if (lobby == null) return [me];
    final going = [
      for (final m in lobby.members)
        if (m.online && !m.away) m.entityId == _me ? me : m.level,
    ];
    return going.isEmpty ? [me] : going;
  }

  /// A mission card as the expedition window takes it (`MissionEntry`).
  static Map<String, Object?> _cardJson(MissionCard c) => {
    'id': c.id,
    'name': c.name,
    'description': c.story.isNotEmpty ? c.story : c.description,
    'maxPlayers': c.maxPlayers,
    'map': c.map,
    'art': c.art,
    'levels': [c.band.min, c.band.max],
    'recommended': c.band.recommended,
    'terrain': c.terrain,
    'timeLimit': c.timeLimitSecs,
    'soon': c.soon,
    if (c.reason.isNotEmpty) 'reason': c.reason,
    'bosses': [
      for (final b in c.bosses) {'id': '', 'name': b},
    ],
    'tags': [
      for (final t in c.tags) {'label': t},
    ],
    if (c.rewards.isNotEmpty) 'rewards': c.rewards,
  };

  bool get _leading => _lobby?.members.any((m) => m.entityId == _me && m.leader) ?? false;

  /// A choice made in the mission window before the party existed goes to
  /// the lobby it opened (a public party joined instead keeps its leader's).
  void _sendPendingMods() {
    final pending = _pendingMods;
    _pendingMods = null;
    if (pending != null && _leading) _send(ClientMsg.lobbyModifiers(pending.$1, pending.$2));
  }

  /// The lobby's modifiers for the mission window (editable by the leader).
  void _renderModifiers() {
    final lobby = _lobby;
    view.setMissionModifiers(
      jsonEncode({
        'scope': 'lobby',
        if (lobby != null) ...{
          'mission': lobby.mission.id,
          'leader': _leading,
          'state': ?_lobbyMods?.toJson(),
          // Who a start now leaves behind (offline or on a mission).
          'behind': [
            for (final m in lobby.members)
              if (!m.online || m.away) m.name,
          ],
        },
      }),
    );
  }

  /// Names of the curses in [mask] (from the server's table).
  List<String> _curseNames(int mask) => [
    for (final d in _modDefs?.defs ?? const <ModifierDef>[])
      if (d.bit != ModifierDef.slider && mask & (1 << d.bit) != 0) d.name,
  ];

  /// One line about a modifier choice: `Hard II · Darkness, Swarm · risk 20`.
  String _modsLine(int tier, int mask, int risk) {
    final slider = _modDefs?.defs.where((d) => d.bit == ModifierDef.slider).firstOrNull;
    final tierName = slider != null && tier < slider.steps.length ? slider.steps[tier].$1 : 'Tier $tier';
    return [tierName, ..._curseNames(mask), 'risk $risk'].join(' · ');
  }

  /// The party key: the Social window's Party tab (the engine opens the
  /// window on it, or closes it when it already shows).
  void _toggleParty() {
    _renderParty();
    view.toggleWindow('party');
  }

  /// The Party tab's content for where the player is: the stage's party,
  /// the lobby, or (neither) how to find a party.
  void _renderParty() {
    if (_inStage) {
      _renderStagePanel();
    } else if (_lobby != null) {
      _renderLobby();
    } else {
      _renderSolo();
    }
  }

  /// Not in a party: the mission board's Find a party, or a code. `quiet`:
  /// it fills the tab without opening the Social window.
  void _renderSolo() {
    final board = _npcs.where((n) => n.kind == 'mission_board').firstOrNull?.name;
    view.showPanel(
      jsonEncode({
        'id': 'party',
        'title': 'Party',
        'quiet': true,
        'subtitle': board == null
            ? 'Not in a party. Missions are picked at a mission board in town.'
            : 'Not in a party. Pick a mission at $board, then Find a party, or join friends with their code.',
        'rows': [
          {'text': _myName, 'detail': '(you)', 'tag': 'solo'},
        ],
        'actions': [
          {'id': 'find', 'label': 'Find a party', 'primary': true},
          {'id': 'code', 'label': 'Party code'},
        ],
      }),
    );
  }

  // ---------------------------------------------------------------- friends

  /// The friends list (account level), as the server last sent it.
  List<FriendInfo> _friends = const [];

  void _onFriends(List<FriendInfo> friends) {
    _friends = friends;
    view.setFriends(
      jsonEncode({
        'friends': [
          for (final f in friends)
            {
              'account': f.account,
              'link': f.link.name,
              'name': f.name,
              'online': f.online,
              'cls': _title(f.archetype, f.level),
              'level': f.level,
              'map': f.map,
              'channel': f.channel,
              'chars': f.characters,
            },
        ],
      }),
    );
  }

  /// The player [entityId] (a character id: entity ids of players are
  /// character ids) is a friend, or a request between you is pending: the
  /// player menu leaves out "Add friend".
  bool isFriendEntity(int entityId) => _friends.any((f) => f.characters.contains(entityId));

  /// A light toast for a friends event.
  void _onFriendNote(FriendNote n) {
    final key = _binds.friends?.caption ?? 'Alt+H';
    switch (n.op) {
      case FriendNoteOp.online:
        view.toast('${n.name} is online.');
      case FriendNoteOp.request:
        view.toast('${n.name} wants to be your friend ($key).');
      case FriendNoteOp.accepted:
        view.toast('You and ${n.name} are friends now.');
      case FriendNoteOp.sent:
        view.toast('Friend request sent to ${n.name}.');
      case FriendNoteOp.removed:
        view.toast('${n.name} is off your friends list.');
    }
  }

  void _onFriendAction(Map<String, Object?> e) {
    final account = (e['account'] as num?)?.toInt();
    switch (e['op']) {
      case 'accept' when account != null:
        _send(ClientMsg.friendAccept(account));
      case 'decline' when account != null:
        _send(ClientMsg.friendDecline(account));
      case 'remove' when account != null:
        _send(ClientMsg.friendRemove(account));
      case 'add_name':
        final name = '${e['name'] ?? ''}'.trim();
        if (name.isNotEmpty) _send(ClientMsg.friendAddName(name));
    }
  }

  /// The game menu's Friends line: online friends and requests waiting.
  String _friendsDetail() {
    final online = _friends.where((f) => f.link == FriendLink.friend && f.online).length;
    final asks = _friends.where((f) => f.link == FriendLink.incoming).length;
    final key = _binds.friends?.caption ?? 'Alt+H';
    final parts = [
      'Your friends list',
      if (online > 0) '$online online',
      if (asks > 0) '$asks request${asks == 1 ? '' : 's'}',
    ];
    return '${parts.join(' · ')} ($key)';
  }

  /// Party panel: the members (their current character, or the last one
  /// while offline), where they are, start/leave. `quiet`: update the tab
  /// without opening the window.
  void _renderLobby({bool quiet = true}) {
    final lobby = _lobby;
    _renderMarks();
    if (lobby == null) return;
    final members = lobby.members;
    final mission = lobby.mission;
    final me = members.where((m) => m.entityId == _me).firstOrNull;
    final leader = me?.leader ?? false;
    final behind = members.where((m) => !m.online || m.away).length;
    view.showPanel(
      jsonEncode({
        'id': 'lobby',
        'quiet': quiet,
        'title': 'Party · ${mission.name}',
        'subtitle': [
          '${lobby.public ? 'Public' : 'Private'} party · code ${lobby.code} · ${members.length}/${mission.maxPlayers}',
          if (_lobbyMods case final m? when m.tier > 0 || m.mask != 0)
            'Modifiers: ${_modsLine(m.tier, m.mask, m.risk)} · clear bonus +${m.zenyPct}% zeny, +${m.expPct}% exp',
        ].join('\n'),
        'rows': [
          for (final m in members)
            {
              'id': '${m.entityId}',
              'text': m.name,
              'detail': ['${_title(m.archetype, m.level)} · Lv ${m.level}', if (m.entityId == _me) '(you)'].join(' '),
              'tag': [
                if (m.leader) 'leader',
                if (!m.online)
                  'offline${_ago(m.offlineSecs)}'
                else if (m.away)
                  'on a mission'
                else if (!m.leader)
                  'in town',
              ].join(' · '),
              'tagKind': !m.online ? null : (m.leader ? 'lead' : (m.away ? 'wait' : 'ok')),
              if (leader && m.entityId != _me) 'action': {'id': 'kick', 'label': 'Kick'},
            },
        ],
        'actions': [
          // No ready check: the leader starts at any time with whoever is
          // in town.
          if (leader)
            {
              'id': 'start',
              'label': behind == 0 ? 'Start' : 'Start (${members.length - behind}/${members.length})',
              'primary': true,
            },
          {'id': 'modifiers', 'label': leader ? 'Modifiers…' : 'Modifiers'},
          {'id': 'leave', 'label': 'Leave'},
        ],
      }),
    );
  }

  /// In-stage panel: who is here and the way back.
  void _renderStagePanel() {
    double? hp(int? id) {
      final v = id == null ? null : _vitals[id];
      return v == null || v.maxHp <= 0 ? null : v.hp / v.maxHp;
    }

    List<int>? vital(int? id) {
      final v = id == null ? null : _vitals[id];
      return v == null || v.maxHp <= 0 ? null : [v.hp, v.maxHp];
    }

    _renderMarks();
    view.showPanel(
      jsonEncode({
        'id': 'stage',
        'title': 'Party · ${_missionName.isEmpty ? _mapName : _missionName}',
        'subtitle': '$_channel · $_mapName',
        'rows': [
          {'text': _myName, 'detail': '(you)', 'hp': ?hp(_me), 'vital': ?vital(_me)},
          for (final e in _peers.entries)
            if (e.value.petSprite == null) {'text': e.value.name, 'hp': ?hp(e.key), 'vital': ?vital(e.key)},
        ],
        'actions': [
          {'id': 'return', 'label': 'Return to town'},
        ],
      }),
    );
  }

  /// Party members and the boss on the minimap (only when the set changes).
  void _renderMarks() {
    final party = <int>{
      if (_inStage)
        for (final e in _peers.entries)
          if (e.value.petSprite == null) e.key,
      if (!_inStage)
        for (final m in _lobby?.members ?? const <LobbyMember>[]) m.entityId,
    }..remove(_me);
    final marks = [
      for (final id in party) {'id': id, 'kind': 'party'},
      if (_bossId != null) {'id': _bossId, 'kind': 'boss'},
    ];
    final json = jsonEncode(marks);
    if (json == _marksSent) return;
    _marksSent = json;
    view.setMinimapMarks(json);
  }

  int? _bossId;

  /// The own kill combo as the server last sent it (0 = none).
  int get combo => _combo;
  int _combo = 0;
  String _marksSent = '';
  double _partyDrawn = 0;

  /// `?mission=<id>` (e2e runs): picks a private mission on the first town
  /// join. Only a server with `GAME_DEV=1` accepts it away from the board.
  /// `?shop=<npc>` talks to a merchant the same way.
  void _devMission() {
    if (_devMissionSent) return;
    _devMissionSent = true;
    final mission = Uri.base.queryParameters['mission'];
    if (mission != null && mission.isNotEmpty) _askParty(ClientMsg.missionSelect(mission, public: false));
    // `?shop=<npc>`: opens that merchant (a `GAME_DEV` server opens it from
    // anywhere) once the map has loaded: arriving closes any open dialog.
    final shop = Uri.base.queryParameters['shop'];
    if (shop != null && shop.isNotEmpty) _devShop = shop;
  }

  bool _devMissionSent = false;
  String? _devShop;

  /// The NPC `?shop=` opened from afar (GAME_DEV): its dialog is not closed
  /// for being out of reach.
  String? _devNpc;

  void _onPanelAction(String panel, String action, String? row) {
    switch ((panel, action)) {
      case ('lobby', 'start'):
        // A soft warning, never a lock (server `LevelBand::fit`).
        final id = _lobby?.mission.id;
        final band =
            _ladder.where((c) => c.id == id).firstOrNull?.band ?? (_brief?.mission == id ? _brief?.band : null);
        if (band != null && _goingLevels.every((l) => l < band.min)) {
          view.toast(
            'Heads up: everyone going is under Lv ${band.min}, the floor of this mission. It will be a hard run.',
          );
        }
        _send(ClientMsg.lobbyStart());
      case ('lobby', 'leave'):
        _send(ClientMsg.lobbyLeave());
      case ('lobby', 'modifiers'):
        _showMissions(_lobby?.mission.id);
      case ('lobby', 'kick'):
        final id = int.tryParse(row ?? '');
        if (id != null) _send(ClientMsg.lobbyKick(id));
      case ('stage', 'return'):
        _send(ClientMsg.stageLeave());
      case ('party', 'find'):
        if (_offer.isNotEmpty) {
          _showMissions(null);
        } else {
          final board = _npcs.where((n) => n.kind == 'mission_board').firstOrNull?.name ?? 'a mission board';
          view.toast('Talk to $board to pick a mission.');
        }
      case ('party', 'code'):
        _npcDialog = false;
        _dropRefiner();
        view.showDialog(
          jsonEncode({
            'id': 'party_code',
            'title': 'Join a party',
            'text': 'The party leader sees the code in their party window.',
            'options': const [],
            'input': {'id': 'code', 'label': 'Join', 'placeholder': 'Party code', 'maxLength': 8},
          }),
        );
    }
  }

  void _onUiEvent(String json) {
    final e = jsonDecode(json) as Map<String, Object?>;
    switch (e['type']) {
      case 'chat_send':
        final text = '${e['text'] ?? ''}'.trim();
        if (text.isNotEmpty) _send(ClientMsg.chat(text));
      case 'dialog_choice':
        final option = e['option']! as String;
        if (option.startsWith('menu:')) {
          _onMenu(option.substring(5));
          return;
        }
        if (option.startsWith('missions:')) {
          view.closeDialog();
          final id = option.substring(9);
          _showMissions(id.isEmpty ? null : id);
          return;
        }
        if (option.startsWith('feedback:')) {
          final dialog = '${e['dialog']}';
          view.closeDialog();
          _openFeedback(dialog.startsWith('npc:') ? dialog.substring(4) : dialog, option.substring(9));
          return;
        }
        if (option.startsWith('parties:')) {
          // The Kafra's party list (`parties.dart`).
          final parts = option.split(':');
          switch (parts[1]) {
            case 'list':
              _partiesAsked = true;
              _send(ClientMsg.partyList());
            case 'join':
              final id = int.tryParse(parts.last);
              view.closeDialog();
              if (id != null) _askParty(ClientMsg.partyJoin(id));
            case 'public':
              _partiesAsked = true;
              _send(ClientMsg.partyPublic(public: parts.last == '1'));
              _send(ClientMsg.partyList());
          }
          return;
        }
        if (option.startsWith('resume:')) {
          _resumeOffer = null;
          view.closeDialog();
          _send(ClientMsg.stageResume(accept: option == 'resume:yes'));
          return;
        }
        if (option.startsWith('party:')) {
          final parts = option.split(':');
          final from = int.tryParse(parts.last);
          _inviteFrom = null;
          view.closeDialog();
          if (from != null) _askParty(ClientMsg.partyAnswer(from, accept: parts[1] == 'accept'));
          return;
        }
        if (option == 'code') {
          _askParty(ClientMsg.lobbyJoin('${e['value']}'));
        } else if (option == 'restyle') {
          final dialog = '${e['dialog']}';
          view.closeDialog();
          _stylist = dialog.startsWith('npc:') ? dialog.substring(4) : dialog;
          _styling = false;
          _showStylist();
          return;
        } else if (option == 'skill_reset') {
          _send(ClientMsg.skillReset());
          view.toast('Your skills are forgotten: the points are yours to spend again (K).');
        } else if (option.startsWith('public:') || option.startsWith('private:')) {
          final i = option.indexOf(':');
          _askParty(ClientMsg.missionSelect(option.substring(i + 1), public: option.startsWith('public:')));
        }
        view.closeDialog();
      case 'missions_close':
        _missionsShown = false;
      case 'guide_close':
        break;
      case 'guide_auto':
        _applySetting('guideAuto', e['on'] == true);
      case 'guide_term':
        // "Open in guide" of a glossary tip: the rules page at that term.
        _showGuide(term: e['term'] as String?);
      case 'mission_select':
        final tier = (e['tier'] as num?)?.toInt() ?? 0, mask = (e['mask'] as num?)?.toInt() ?? 0;
        _pendingMods = tier == 0 && mask == 0 ? null : (tier, mask);
        _askParty(ClientMsg.missionSelect(e['mission']! as String, public: e['public'] == true));
      case 'mission_start':
        // Quick start: alone it launches at once, a leader starts the
        // party's run; the server checks the giver, the party and the levels.
        final mission = e['mission']! as String;
        final name = [..._offer, ?_lobby?.mission].where((m) => m.id == mission).firstOrNull?.name;
        if (name != null) _missionName = name;
        _send(
          ClientMsg.missionStart(
            mission,
            tier: (e['tier'] as num?)?.toInt() ?? 0,
            mask: (e['mask'] as num?)?.toInt() ?? 0,
          ),
        );
      case 'mission_modifiers':
        // The leader's change in the mission window (the server checks it).
        _send(ClientMsg.lobbyModifiers((e['tier'] as num?)?.toInt() ?? 0, (e['mask'] as num?)?.toInt() ?? 0));
      case 'party_code':
        final code = '${e['code'] ?? ''}'.trim();
        if (code.isNotEmpty) _askParty(ClientMsg.lobbyJoin(code));
      case 'panel_action':
        _onPanelAction(e['panel']! as String, e['action']! as String, e['row'] as String?);
      case 'setting' when e['id'] == 'account':
        _openAccount();
      case 'setting':
        _applySetting(e['id']! as String, e['value']);
      case 'account_save':
        _saveAccount(e);
      case 'account_close':
        _account = null;
      case 'dialog_close':
        if (e['dialog'] == 'settings') _settingsOpen = false;
        // The character is in no instance until the player answers.
        final offer = _resumeOffer;
        if (e['dialog'] == 'resume' && offer != null) Timer.run(() => _offerResume(offer));
        if (e['dialog'] == 'party_invite') {
          // Closed without an answer: decline.
          final from = _inviteFrom;
          _inviteFrom = null;
          if (from != null) _send(ClientMsg.partyAnswer(from, accept: false));
        }
      case 'refine_strike':
        // `<slot | b>:<key>` (refineRow).
        final row = '${e['row'] ?? ''}', i = row.indexOf(':');
        final smith = _refiner;
        if (smith == null || i < 0 || _refining != null) return;
        _refining = row;
        _send(ClientMsg.refine(smith.id, row.substring(i + 1), int.tryParse(row.substring(0, i))));
      case 'craft':
        final hatter = _hatter;
        if (hatter == null || _crafting) return;
        _crafting = true;
        _send(ClientMsg.craft(hatter, '${e['recipe'] ?? ''}'));
      case 'hom_stance':
        _send(ClientMsg.companionStance((e['stance'] as num?)?.toInt() ?? CompanionStance.defensive));
      case 'hom_rest':
        _send(ClientMsg.companionRest());
      case 'hom_rename':
        final name = '${e['name'] ?? ''}'.trim();
        if (name.isNotEmpty) _send(ClientMsg.companionRename(name));
      case 'hom_evolve':
        _send(ClientMsg.companionEvolve());
      case 'hom_choose':
        _homChooser = false;
        _homChanging = false;
        final t = (e['homType'] as num?)?.toInt() ?? HomType.none;
        if (t != HomType.none) _send(ClientMsg.companionChoose(t));
      case 'hom_choose_close':
        _homChooser = false;
        _homChanging = false;
      case 'hom_change':
        _openHomChange();
      case 'craft_close':
        _hatter = null;
        _crafting = false;
        _npcDialog = false;
      case 'refine_close':
        _refiner = null;
        _refining = null;
        _npcDialog = false;
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
      case 'skill_open':
        _openSkill('${e['skill'] ?? ''}');
      case 'skill_town':
        _useTownSkill('${e['skill'] ?? ''}');
      case 'skill_auto':
        _toggleAutoCast((e['slot']! as num).toInt());
      case 'open_settings':
        _toggleSettings();
      case 'open_menu':
        _toggleMenu();
      case 'friend_action':
        _onFriendAction(e);
      case 'leaderboard_query':
        unawaited(_board.query(e));
      case 'feedback_submit':
        _submitFeedback(e);
      case 'feedback_close':
        _feedback = null;
      case 'return_town':
        if (_inStage) _send(ClientMsg.stageLeave());
      case 'player_action':
        final target = (e['target'] as num?)?.toInt();
        switch (e['action']) {
          case 'party_invite' when target != null:
            _askParty(ClientMsg.partyInvite(target));
            view.toast('Invited ${_peers[target]?.name ?? 'them'} to your party.');
          case 'party_kick' when target != null:
            _send(ClientMsg.lobbyKick(target));
          case 'inspect' when target != null:
            _inspect(target);
          case 'friend' when target != null:
            _send(ClientMsg.friendAdd(target));
          case 'trade' when target != null:
            _tradeRequest(target);
        }
      case 'skill_slot':
        _send(ClientMsg.skillSlot((e['slot']! as num).toInt(), e['skill']! as String));
      case 'item_use':
        if (_inStage && _downedToast()) return;
        _send(ClientMsg.itemUse(e['item']! as String));
      case 'item_equip':
        _send(ClientMsg.equip(e['item']! as String, (e['slot'] as num?)?.toInt()));
      case 'item_unequip':
        _send(ClientMsg.unequip((e['slot']! as num).toInt()));
      case 'card_insert' || 'card_remove':
        final slot = (e['slot'] as num?)?.toInt() ?? 255;
        _send(
          ClientMsg.card(
            insert: e['type'] == 'card_insert',
            item: e['item']! as String,
            slot: slot == 255 ? null : slot,
            card: e['card']! as String,
          ),
        );
      case 'shop_buy' || 'shop_sell':
        final npc = e['npc']! as String, item = e['item']! as String, n = (e['count']! as num).toInt();
        _send(e['type'] == 'shop_buy' ? ClientMsg.shopBuy(npc, item, n) : ClientMsg.shopSell(npc, item, n));
      case 'shop_deal':
        List<({String id, int count})> lines(Object? l) => [
          for (final x in (l as List<Object?>? ?? const []).cast<Map<String, Object?>>())
            (id: x['id']! as String, count: (x['count']! as num).toInt()),
        ];
        _send(ClientMsg.shopDeal(e['npc']! as String, lines(e['buy']), lines(e['sell'])));
      case 'shop_close':
        _shopNpc = null;
      case 'trade_respond':
        _send(ClientMsg.tradeRespond(accept: e['accept'] == true));
      case 'trade_cancel':
        _tradePending = false;
        _send(ClientMsg.tradeCancel());
      case 'trade_item':
        _send(ClientMsg.tradeItem(e['item']! as String, (e['count']! as num).toInt()));
      case 'trade_zeny':
        _send(ClientMsg.tradeZeny((e['zeny']! as num).toInt()));
      case 'trade_lock':
        _send(ClientMsg.tradeLock());
      case 'trade_confirm':
        _send(ClientMsg.tradeConfirm());
    }
  }

  // ---------------------------------------------------------------- trade

  /// A trade window is open ([TradeState] seen, not yet closed).
  bool _trading = false;

  /// The own request is out, unanswered.
  bool _tradePending = false;

  /// Asks the player [target] (entity id) to trade (the player menu).
  void _tradeRequest(int target) {
    if (_inStage) {
      view.toast('Trade with other players in town.');
      return;
    }
    _send(ClientMsg.tradeRequest(target));
    _tradePending = true;
    view.showTrade(
      jsonEncode({
        'pending': {'name': _peers[target]?.name ?? 'them'},
      }),
    );
  }

  void _onTrade(ServerMsg msg) {
    switch (msg) {
      case TradeRequest(:final name, :final level):
        view.showTrade(
          jsonEncode({
            'request': {'name': name, 'level': level},
          }),
        );
      case final TradeState t:
        _trading = true;
        _tradePending = false;
        view.showTrade(
          jsonEncode({
            'partner': t.name,
            'mine': t.mine.toJson(),
            'theirs': t.theirs.toJson(),
            'myLock': t.myLock,
            'theirLock': t.theirLock,
            'myOk': t.myConfirm,
            'theirOk': t.theirConfirm,
          }),
        );
      case TradeClosed(:final reason, :final message):
        // A request prompt closing (empty message) never ends an open table.
        if (_trading && message.isEmpty) return;
        _trading = false;
        _tradePending = false;
        view.showTrade(reason == 'done' ? jsonEncode({'result': 'done'}) : '');
        if (message.isNotEmpty) view.toast(message);
      default:
    }
  }

  // ---------------------------------------------------------------- items

  /// Inventory and Equipment windows (and the orbs' potions).
  void _renderInventory() {
    final inv = _inventory;
    if (inv == null) return;
    view.setInventory(
      jsonEncode({
        'zeny': inv.zeny,
        'items': [
          for (final s in inv.items) {'id': s.id, 'count': s.count},
        ],
        'equip': inv.equip,
        'cls': _myArchetype,
        'combat': _inStage,
      }),
    );
  }

  /// Downed or fallen: items wait for a revive. Says so (at most every
  /// two seconds) and returns `true`.
  bool _downedToast() {
    if (_myState == 0) return false;
    final now = _now();
    if (now - _downedToastAt > 2) {
      _downedToastAt = now;
      view.toast(
        _myState == 1
            ? 'You are down: wait for an ally to revive you.'
            : 'You have fallen: you return with the next wave.',
      );
    }
    return true;
  }

  double _downedToastAt = -10;

  /// Drinks the best potion for [kind] (`hp` / `sp`) on a mission: the
  /// server picks it from what the fighter carries (it knows the running
  /// potion and the bonuses); it says so when there is none or the bar is
  /// full.
  void _quickPotion(String kind) {
    if (!_inStage || _downedToast()) return;
    _send(ClientMsg.itemUse(kind == 'sp' ? ClientMsg.quickSp : ClientMsg.quickHp));
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
        'keys': _binds.slotCaptions,
        // Skills that open a window (in town) rather than being cast.
        'opens': {pharmacySkill: 'Brew'},
      }),
    );
  }

  /// The Alchemist's Pharmacy: a recipe book the server opens in town
  /// (`npc_interact("pharmacy")`, server `hub/quest.rs PHARMACY`).
  static const pharmacySkill = 'pharmacy';

  /// A skill tree node's open button (`skill_open`): the town skill.
  void _openSkill(String skill) {
    if (skill == pharmacySkill) _useTownSkill(skill);
  }

  /// The mission is over: the result screen follows (`RUN_STATS`,
  /// `RUN_REWARDS`) and stays up, back in town too, until closed.
  void _onStageEnd(StageEnd e) {
    view.setBoss('');
    view.setBossBar('');
    view.setMissionClock(_stageMission, e.secs * 1000, false);
    _stageEnd = e;
    _showReturnCall(e);
  }

  /// Won or lost: the call to go home, counting down [StageEnd.leaveMs]
  /// (the server sends everyone back then). Should the move not come, the
  /// client asks for it a little later itself.
  void _showReturnCall(StageEnd e) {
    if (!_inStage) return;
    _returnCall = true;
    view.showReturnCall(jsonEncode({'victory': e.victory, 'leaveMs': e.leaveMs}));
    _returnTimer?.cancel();
    _returnTimer = Timer(Duration(milliseconds: e.leaveMs + returnGraceMs), () {
      if (_inStage && _returnCall) _send(ClientMsg.stageLeave());
    });
  }

  void _hideReturnCall() {
    _returnTimer?.cancel();
    _returnTimer = null;
    if (!_returnCall) return;
    _returnCall = false;
    view.showReturnCall('');
  }

  /// The result screen: the party's numbers with the player's own take.
  void _showResults(RunRewards r) {
    final s = _runStats;
    if (s == null) return;
    final end = _stageEnd;
    _runStats = null;
    _stageEnd = null;
    view.showResults(
      jsonEncode({
        'victory': s.victory,
        'mission': _missionName.isNotEmpty ? _missionName : _stageMission,
        'secs': s.secs,
        'kills': end?.kills ?? s.seats.fold<int>(0, (n, x) => n + x.kills),
        'leaveMs': _inStage ? (end?.leaveMs ?? 0) : 0,
        'bucketSecs': s.bucketSecs,
        'me': r.seat,
        'seats': [for (final x in s.seats) x.toJson()],
        'marks': [for (final m in s.marks) m.toJson()],
        'rewards': r.toJson(),
      }),
    );
  }

  void _onCast(Cast c) {
    final own = c.caster == _me;
    _castPose(c);
    view.castSkill(
      jsonEncode({
        'caster': c.caster,
        'skill': c.skill,
        'x': c.x,
        'y': c.y,
        'castMs': c.castMs,
        'cooldownMs': c.cooldownMs,
        'level': c.level > 0 ? c.level : 1,
        'own': own,
        'areas': [for (final a in c.areas) a.toJson()],
      }),
    );
  }

  /// A cast turns its caster to the aimed point: a character holds the
  /// wind-up pose (at least a moment for instant skills), a monster
  /// strikes toward it. An empty skill (a called-off wind-up) ends the pose.
  void _castPose(Cast c) {
    final now = _now();
    final id = c.caster;
    if (id >= monsterEntityBase) {
      final mob = _mobs[id - monsterEntityBase];
      if (mob == null || c.skill.isEmpty) return;
      final dx = c.x - mob.x / Reckoning.posOne, dy = c.y - mob.y / Reckoning.posOne;
      if (dx * dx + dy * dy > 0.04) mob.attackFacing = facingOf(dx, dy);
      mob.attackUntil = now + 0.5;
      return;
    }
    if (c.skill.isEmpty) {
      _castUntil.remove(id);
      _throws.remove(id);
      _tracks[id]?.resetSent();
      return;
    }
    final p = _predictor;
    final t = _tracks[id];
    final (x, y) = id == _me && p != null
        ? (p.x, p.y)
        : t != null
        ? (t.x1, t.y1)
        : (c.x, c.y);
    final dx = c.x - x, dy = c.y - y;
    if (dx * dx + dy * dy > 0.04) _attackFacing[id] = facingOf(dx, dy);
    // The wind-up pose, then for a thrown skill the toss.
    final pose = CastPose.of(c.skill, now, c.castMs);
    _castUntil[id] = pose.windUpEnd;
    if (pose.throws) {
      _throws[id] = pose;
    } else {
      _throws.remove(id);
    }
    _stanceUntil[id] = pose.end + 4;
    t?.resetSent();
  }

  /// Skill slots held or tapped this tick (bit i = slot i); only on a
  /// stage, standing up, with no window open.
  int _heldSlots() {
    if (!_inStage || _myState != 0 || view.dialogOpen()) return 0;
    var bits = _keySlots();
    bool down(String code) => _keys.contains(code) || _tapped.contains(code);
    final pressed = {..._keys, ..._tapped};
    // The travel key: the server casts the learned travel skill on bit 15.
    final t = _binds.travel;
    if (t != null && down(t.code) && t.heldIn(pressed)) bits |= 1 << travelSlot;
    // Gamepad / touch presses, and auto-cast slots every tick (the server
    // ignores what it cannot honour: cooldown, SP, no target). The mount
    // slot is no cast ([_mountPress]).
    return (bits | _input.state.slots | autoCastSlots(_autoCast, _skills?.hotbar ?? const [])) & ~_mountMask();
  }

  /// Hotbar slots whose keys are held (bit i = slot i), in town too.
  int _keySlots() {
    bool down(String code) => _keys.contains(code) || _tapped.contains(code);
    final pressed = {..._keys, ..._tapped};
    var bits = 0;
    for (var i = 0; i < Bindings.count; i++) {
      final k = _binds.slots[i];
      if (k != null && (down(k.code)) && k.heldIn(pressed)) bits |= 1 << i;
    }
    return bits;
  }

  /// Town slot keys held last tick (bit i = slot i).
  int _townHeld = 0;

  /// In town the bar is the town skill bar (engine `townSkillAt`): a key
  /// pressed (its rising edge) uses the skill on that slot.
  void _townPress() {
    if (_myState != 0 || view.dialogOpen()) {
      _townHeld = 0;
      return;
    }
    final held = _keySlots() | _input.state.slots;
    final rising = held & ~_townHeld;
    _townHeld = held;
    if (rising == 0) return;
    for (var i = 0; i < Bindings.count; i++) {
      if (rising & (1 << i) == 0) continue;
      final skill = view.townSkillAt(i);
      if (skill.isNotEmpty) _useTownSkill(skill);
    }
  }

  /// A town skill (the town bar, its key, the tree's Brew button): the
  /// server does it (`TOWN_SKILL`: mount, Pharmacy, homunculus). Calling a
  /// homunculus without a type opens the chooser at once.
  void _useTownSkill(String skill) {
    if (_inStage) {
      if (skill == pharmacySkill) view.toast('The Pharmacy brews in town.');
      return;
    }
    if (skill == callHomunculusSkill) {
      _callHomAt = _now();
      if (_homState?.homType == HomType.none && !_homChooser) {
        _homChooser = true;
        _loadCompanionsPack();
        view.showHomunculusChooser(true);
      }
    }
    _send(ClientMsg.townSkill(skill));
  }

  /// Hotbar slots holding the mount skill (bit i = slot i).
  int _mountMask() {
    final hotbar = _skills?.hotbar ?? const <String>[];
    var mask = 0;
    for (var i = 0; i < hotbar.length && i < Bindings.count; i++) {
      if (hotbar[i] == mountSkill) mask |= 1 << i;
    }
    return mask;
  }

  /// A press of the mount slot (its key, or a gamepad / touch press) gets
  /// on or off the Peco Peco, in town and on missions (`mount_req`; the
  /// server answers with `player_appearance`).
  void _mountPress() {
    final mask = _mountMask();
    if (mask == 0 || _myState != 0 || view.dialogOpen()) {
      _mountHeld = false;
      return;
    }
    final pressed = {..._keys, ..._tapped};
    var tapped = false;
    for (var i = 0; i < Bindings.count; i++) {
      final k = _binds.slots[i];
      if (mask & (1 << i) != 0 && k != null && _tapped.contains(k.code) && k.heldIn(pressed)) tapped = true;
    }
    final held = _input.state.slots & mask != 0;
    if (tapped || (held && !_mountHeld)) _send(ClientMsg.mount(on: !_riding));
    _mountHeld = held;
  }

  /// What the local prediction walks at: the conditions' pace, riding faster.
  void _applyPace() {
    _predictor?.pace = _statusPace * (_riding ? 1 + _rideSpeed / 100 : 1);
  }

  void _toggleAutoCast(int slot) {
    final skill = _skills?.hotbar.elementAtOrNull(slot) ?? '';
    if (skill.isEmpty) return;
    _autoCast ^= 1 << slot;
    view.setAutoCast(_autoCast);
    _applySetting('autoCast', _autoCast);
    view.toast('Auto-cast ${_autoCast & (1 << slot) != 0 ? 'on' : 'off'}: slot ${slot + 1}');
  }

  /// Pointer on the ground relative to the player, in 1/8 m (0,0 = none).
  (int, int) _aim() {
    // Right stick / touch drag; a gamepad or touch press without one is auto.
    final input = _input.state;
    if (input.hasAim) return (input.aimX, input.aimY);
    if (input.slots != 0 || _input.touchOn) return (0, 0);
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

  /// The open dialog is an NPC's (it closes when the player walks away).
  bool _npcDialog = false;

  /// The server's version (`Connected`): the settings show it.
  String serverVersion = '';

  // ---------------------------------------------------------------- account

  /// Settings → Account while open (the optional email and Telegram).
  AccountForm? _account;

  /// Opens the Account window with what the server has (`/api/auth/me`).
  void _openAccount() {
    if (_account != null) return;
    api
        .me()
        .then((me) {
          final user = (me['user'] as Map?)?.cast<String, Object?>() ?? const {};
          final f = _account = AccountForm(
            '${user['username'] ?? ''}',
            (user['contacts'] as Map?)?.cast<String, Object?>() ?? const {},
          );
          view.showAccount(f.spec());
        })
        .catchError((Object err) => view.toast('Could not load the account: $err'));
  }

  void _saveAccount(Map<String, Object?> e) {
    final f = _account;
    if (f == null || f.busy) return;
    final send = f.submit(e);
    view.showAccount(f.spec());
    if (send == null) return;
    api
        .updateContacts(email: send.email, telegram: send.telegram)
        .then((stored) => f.saved(stored))
        .catchError((Object err) => f.refused('$err', err is ApiException ? err.field : null))
        .whenComplete(() {
          if (identical(_account, f)) view.showAccount(f.spec());
        });
  }

  // ---------------------------------------------------------------- feedback

  /// The open feedback form (the Scribe or a Kafra).
  FeedbackForm? _feedback;

  /// Opens the form at [npc], on report kind [kind] ('' = the default).
  void _openFeedback(String npc, String kind) {
    final f = _feedback = FeedbackForm(npc: npc, kind: kind);
    view.showFeedback(f.spec());
  }

  void _submitFeedback(Map<String, Object?> e) {
    final f = _feedback;
    if (f == null || f.busy) return;
    final frame = f.submit(e, userAgent: web.window.navigator.userAgent);
    if (frame != null) _send(frame);
    view.showFeedback(f.spec());
  }

  void _onFeedbackResult(FeedbackResult r) {
    final f = _feedback;
    if (r.ok || f == null) {
      if (r.ok) {
        _feedback = null;
        view.showFeedback('');
      }
      view.toast(r.message);
      return;
    }
    f.refused(r.message);
    view.showFeedback(f.spec());
  }

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
    _autoCast = (_settings['autoCast'] as num?)?.toInt() ?? 0;
    _manualAim = _settings['manualAim'] == true;
    _clickMove = _settings['clickMove'] != false;
    _tapMove = _settings['tapMove'] == true;
    _binds = Bindings(_settings);
    view
      ..setAutoCast(_autoCast)
      ..setTouchControls((_settings['touchControls'] as num?)?.toInt() ?? -1);
  }

  void _applySetting(String id, Object? value) {
    _settings[id] = value;
    switch (id) {
      case 'frameCap':
        _frameCap = (value! as num).toInt();
      case 'interpBiasMs':
        _clock.bias = (value! as num) / 1000;
      case 'touchControls':
        view.setTouchControls((value! as num).toInt());
      case 'manualAim':
        _manualAim = value == true;
      case 'autoPotion' || 'autoPotionHp' || 'autoPotionSp':
        _syncUsePrefs();
      case 'clickMove':
        _clickMove = value == true;
        if (!_clickMove) _cancelWalk();
        _hoverDirty = true;
      case 'tapMove':
        _tapMove = value == true;
      case final String k when k.startsWith('bind.') || k.startsWith('emote.'):
        _binds = Bindings(_settings);
        _renderSkills();
      default:
        view.configure(jsonEncode({id: value}));
    }
    try {
      web.window.localStorage.setItem(_settingsKey, jsonEncode(_settings));
    } on Object catch (_) {
      // Private mode or blocked storage: settings last for this session.
    }
  }

  /// The game menu (Esc, the gamepad's Start, the touch ☰ button).
  void _toggleMenu() {
    _npcDialog = false;
    _dropRefiner();
    if (view.dialogOpen()) {
      view.closeDialog();
      _settingsOpen = false;
      return;
    }
    // Every entry says what it does (the tiles share one height).
    final away = _inStage ? ' · leaves the mission' : '';
    view.showDialog(
      jsonEncode({
        'id': 'menu',
        'title': 'Menu',
        'text': '$_myName · $_mapName · $_channel',
        'options': [
          {'id': 'menu:resume', 'label': 'Return to game', 'detail': 'Close this menu (Esc)'},
          {'id': 'menu:settings', 'label': 'Settings', 'detail': 'Graphics, sound, keys, interface (F10)'},
          {'id': 'menu:controls', 'label': 'Controls', 'detail': 'Every key and mouse action at a glance'},
          {'id': 'menu:guide', 'label': "Adventurer's Guide", 'detail': 'How the game works, topic by topic'},
          {'id': 'menu:fullscreen', 'label': 'Full screen', 'detail': 'Hide the browser around the game (on / off)'},
          {'id': 'menu:friends', 'label': 'Friends', 'detail': _friendsDetail()},
          {
            'id': 'menu:leaderboard',
            'label': 'Ranking',
            'detail': 'Top adventurers by level, kills, MVPs and zeny (${_binds.leaderboard?.caption ?? 'Alt+L'})',
          },
          {'id': 'menu:chat', 'label': 'Chat', 'detail': 'Say something to the players around (Enter)'},
          {'id': 'menu:debug', 'label': 'Debug overlay', 'detail': 'Frame rate and network numbers (F1)'},
          {'id': 'menu:characters', 'label': 'Character select', 'detail': 'Play another character$away'},
          {'id': 'menu:logout', 'label': 'Log out', 'detail': 'Back to the sign-in screen$away'},
        ],
      }),
    );
  }

  void _onMenu(String id) {
    switch (id) {
      case 'resume':
        view.closeDialog();
      case 'settings':
        view.closeDialog();
        _toggleSettings();
      case 'controls':
        _showControls();
      case 'guide':
        view.closeDialog();
        _showGuide();
      case 'fullscreen':
        view.closeDialog();
        view.toggleFullscreen();
      case 'debug':
        view.closeDialog();
        if (view.cycleDebugMode() == 0) view.toast('Debug overlay off (F1)');
      case 'friends':
        view.closeDialog();
        view.toggleWindow('friends');
      case 'leaderboard':
        view.closeDialog();
        // Open it (a toggle on the tab already showing closes it first).
        if (!view.toggleWindow('leaderboard')) view.toggleWindow('leaderboard');
      case 'chat':
        view.closeDialog();
        view.openChat();
      case 'characters':
      case 'logout':
        view.closeDialog();
        unawaited(_leave(id));
    }
  }

  /// The Adventurer's Guide; [welcome]: opened on its own for a new
  /// character (a greeting and the "open for new characters" box).
  void _showGuide({bool welcome = false, String? term}) {
    view.showGuide(
      jsonEncode({
        'topics': guideTopics(_binds, _ladder, assetsBase),
        if (term != null) ...{'selected': 'rules', 'term': term},
        if (welcome) 'welcome': true,
        if (welcome) 'auto': _settings['guideAuto'] != false,
      }),
    );
  }

  void _showControls() {
    _npcDialog = false;
    _dropRefiner();
    final b = _binds;
    String k(KeyBinding? x, String fallback) => x?.caption ?? fallback;
    view.showDialog(
      jsonEncode({
        'id': 'controls',
        'title': 'Controls',
        'sections': [
          {
            'title': 'Moving and talking',
            'items': [
              'WASD or the arrows: walk (the camera turns with the right mouse button; double right-click faces north)',
              if (_clickMove)
                'Left click: walk there (hold to steer) · on a monster: into weapon reach · on loot or a portal: into it',
              '${k(b.interact, 'F')} or ${k(b.interact2, 'Space')}: talk to the nearest NPC · click an NPC',
              '${k(b.travel, 'Space')} on missions: your travel skill (Back Slide, Blink, Charge…)',
              '${k(b.sit, 'C')}: sit down / stand up in town',
            ],
          },
          {
            'title': 'Skills',
            'items': [
              '${b.slotCaptions.where((c) => c.isNotEmpty).join(' ')}: hotbar slots (aim with the pointer)',
              'K: skill tree · right click a slot: auto-cast',
              '${k(b.potion, 'R')} / ${k(b.potion2, 'T')}: drink a healing / SP potion (Auto-use potions in the settings drinks them for you)',
            ],
          },
          {
            'title': 'Windows',
            'items': [
              '${k(b.equip, 'Alt+Q')}: equipment · ${k(b.items, 'Alt+E')}: inventory · ${k(b.party, 'Alt+Z')}: party · ${k(b.friends, 'Alt+H')}: friends · ${k(b.info, 'Alt+V')}: fold Basic Info',
              'F10: settings · ${k(b.debug, 'F1')}: debug overlay · Esc: close a window, or this menu',
              'Alt+1…0: emotes · Ctrl+1…0: flags',
            ],
          },
        ],
        'options': [
          {'id': 'menu:resume', 'label': 'Back to the game', 'detail': 'Close this list (Esc)'},
        ],
      }),
    );
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
        'note': versionLine(serverVersion),
        'fields': [
          {'id': 'h.account', 'label': 'Account', 'type': 'heading'},
          {
            'id': 'account',
            'label': 'Email & Telegram',
            'type': 'button',
            'value': 'Edit…',
            'hint': 'Optional: for password recovery and notifications.',
          },
          {'id': 'h.gfx', 'label': 'Graphics', 'type': 'heading'},
          {
            'id': 'frameCap',
            'label': 'Frame rate cap',
            'type': 'select',
            'value': _frameCap,
            'hint': 'Most frames drawn per second. Display follows the screen; lower saves battery.',
            'options': [
              {'value': 0, 'label': 'Display'},
              {'value': 30, 'label': '30'},
              {'value': 60, 'label': '60'},
              {'value': 90, 'label': '90'},
            ],
          },
          range(
            'renderScale',
            'Render scale',
            0.25,
            1,
            0.05,
            e['renderScale'],
            'Fraction of screen pixels drawn. Lower is faster and blurrier.',
          ),
          range(
            'maxPixelRatio',
            'Max pixel ratio',
            1,
            3,
            0.25,
            e['maxPixelRatio'],
            'Caps the drawing resolution on high-density (Retina) screens. Lower is faster.',
          ),
          {
            'id': 'pixelated',
            'label': 'Pixelated upscale',
            'type': 'toggle',
            'value': e['pixelated'],
            'hint': 'Below full render scale: sharp square pixels instead of a smooth blur.',
          },
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
          range(
            'highStepHz',
            'Player/boss step rate, Hz',
            0,
            60,
            1,
            e['highStepHz'],
            '0 = smooth. Low values give players and bosses the stepped look too.',
          ),
          range(
            'animLodNear',
            'Full-rate animation radius, m',
            4,
            40,
            1,
            e['animLodNear'],
            'Characters closer than this animate every frame.',
          ),
          range(
            'animLodFar',
            'Reduced animation beyond, m',
            8,
            80,
            1,
            e['animLodFar'],
            'Characters farther than this animate less often: saves time in big crowds.',
          ),
          {
            'id': 'water',
            'label': 'Animated water',
            'type': 'toggle',
            'value': e['water'],
            'hint': 'Moving waves on water.',
          },
          {
            'id': 'props',
            'label': 'Props (buildings, trees)',
            'type': 'toggle',
            'value': e['props'],
            'hint': 'Buildings, trees and decorations. Off is faster on weak devices.',
          },
          {
            'id': 'cutout',
            'label': 'See-through buildings',
            'type': 'toggle',
            'value': e['cutout'],
            'hint': 'Walls and roofs between the camera and your character turn see-through.',
          },
          range(
            'cutoutRadius',
            'See-through radius, m',
            0.5,
            4,
            0.1,
            e['cutoutRadius'],
            'Size of the see-through hole around your character.',
          ),
          {
            'id': 'fog',
            'label': 'Animated fog',
            'type': 'toggle',
            'value': e['fog'],
            'hint': 'Drifting fog beyond the light around you.',
          },
          range('grass', 'Grass density', 0, 1, 0.25, e['grass'], '0 = no grass tufts (the lawn stays painted).'),
          {
            'id': 'trample',
            'label': 'Grass trampling',
            'type': 'select',
            'value': e['trample'],
            'hint': 'Grass bends underfoot: nobody, players only, or everyone (monsters too).',
            'options': [
              {'value': 0, 'label': 'Off'},
              {'value': 1, 'label': 'Players only'},
              {'value': 2, 'label': 'Everyone'},
            ],
          },
          {
            'id': 'foliage',
            'label': 'Swaying trees and wind',
            'type': 'toggle',
            'value': e['foliage'],
            'hint': 'Trees sway and wind gusts sweep through the grass.',
          },
          {
            'id': 'leaves',
            'label': 'Falling leaves',
            'type': 'toggle',
            'value': e['leaves'],
            'hint': 'Leaves drift down from the trees.',
          },
          {
            'id': 'damageNumbers',
            'label': 'Damage numbers',
            'type': 'select',
            'value': e['damageNumbers'],
            'options': [
              {'value': 0, 'label': 'Off'},
              {'value': 1, 'label': 'Reduced'},
              {'value': 2, 'label': 'All'},
            ],
            'hint': 'Reduced: fewer numbers on screen in big fights (crits and hits on you always show).',
          },
          {
            'id': 'lowHealth',
            'label': 'Low health effects',
            'type': 'select',
            'value': e['lowHealth'],
            'options': [
              {'value': 0, 'label': 'Off'},
              {'value': 1, 'label': 'On'},
              {'value': 2, 'label': 'On, with heartbeat'},
            ],
            'hint': 'Below half health the screen greys out and veins creep in from the corners.',
          },
          {
            'id': 'cursorStyle',
            'label': 'Mouse cursor',
            'type': 'select',
            'value': e['cursorStyle'],
            'options': [
              {'value': 0, 'label': 'System'},
              {'value': 1, 'label': 'Classic'},
              {'value': 2, 'label': 'Gilded'},
            ],
            'hint':
                "The game's own pointer: a sword over monsters, a speech bubble over NPCs, a grab over loot. System keeps your usual one.",
          },
          {
            'id': 'cursorSize',
            'label': 'Cursor size',
            'type': 'select',
            'value': e['cursorSize'],
            'options': [
              {'value': 0, 'label': 'Small'},
              {'value': 1, 'label': 'Normal'},
              {'value': 2, 'label': 'Large'},
            ],
            'hint': 'Small is the classic pixel size; Normal and Large scale it up.',
          },
          {'id': 'h.sound', 'label': 'Sound', 'type': 'heading'},
          {
            'id': 'muted',
            'label': 'Mute all audio',
            'type': 'toggle',
            'value': e['muted'] ?? false,
            'hint': 'Silences music, effects and ambience at once.',
          },
          range('masterVolume', 'Master volume', 0, 1, 0.05, e['masterVolume'], 'Overall loudness of the game.'),
          range('bgmVolume', 'Music', 0, 1, 0.05, e['bgmVolume'], 'Town and battle music.'),
          range('sfxVolume', 'Effects', 0, 1, 0.05, e['sfxVolume'], 'Skills, hits, monsters and interface sounds.'),
          range(
            'ambienceVolume',
            'Ambience',
            0,
            1,
            0.05,
            e['ambienceVolume'],
            'Wind, water, birds and town bustle. Fades under big fights.',
          ),
          {
            'id': 'footsteps',
            'label': 'Footsteps',
            'type': 'toggle',
            'value': e['footsteps'],
            'hint': 'Footstep sounds of the characters around you.',
          },
          {
            'id': 'muteInBackground',
            'label': 'Silent in a background tab',
            'type': 'toggle',
            'value': e['muteInBackground'],
            'hint': 'Mutes the game while its tab or window is not in front.',
          },
          ..._controlFields(),
          {
            'id': 'touchControls',
            'label': 'Touch controls',
            'type': 'select',
            'value': (_settings['touchControls'] as num?)?.toInt() ?? -1,
            'hint': 'On-screen joystick and skill buttons. Auto: they turn on with the first touch.',
            'options': [
              {'value': -1, 'label': 'Auto'},
              {'value': 1, 'label': 'On'},
              {'value': 0, 'label': 'Off'},
            ],
          },
          {
            'id': 'manualAim',
            'label': 'Manual aiming',
            'type': 'toggle',
            'value': _manualAim,
            'hint':
                'Skills aimed at a monster take the one nearest the mouse pointer, ground skills land on it. '
                'Off: they pick their own target. Auto-cast slots, gamepad and touch always aim by themselves.',
          },
          ...autoUseFields(_settings),
          {
            'id': 'clickMove',
            'label': 'Click to move',
            'type': 'toggle',
            'value': _clickMove,
            'hint':
                'Left click the ground to walk there around obstacles, hold the button to steer. '
                'A click on a monster walks into weapon reach, on loot or a portal walks into it. '
                'Keys and the gamepad take over at once. Off: clicks only talk to NPCs.',
          },
          {
            'id': 'tapMove',
            'label': 'Tap to move',
            'type': 'toggle',
            'value': _tapMove,
            'hint':
                'Touch: tap the ground, a monster, loot or a portal to walk there, alongside the joystick. '
                'Off: taps only talk to NPCs (a missed skill button never moves you).',
          },
        ],
      }),
    );
  }

  /// Key bindings section of the settings: hotbar, emotes (key and which
  /// emote), flags and actions.
  List<Map<String, Object?>> _controlFields() {
    String bind(String id) => (_settings[id] as String?) ?? Bindings.defaults[id] ?? '';
    Map<String, Object?> key(String id, String label, String hint) => {
      'id': id,
      'label': label,
      'type': 'key',
      'value': bind(id),
      'hint': hint,
    };
    final emotes = <Map<String, Object?>>[
      for (final e in (jsonDecode(view.emoteNames()) as List).cast<Map<String, Object?>>())
        {'value': e['id'], 'label': '${e['name']}'},
    ];
    final flags = (jsonDecode(view.flagNames()) as List).cast<String>();
    return [
      {'id': 'h.keys', 'label': 'Keys: click, then press a key (Backspace clears)', 'type': 'heading'},
      key('bind.interact', 'Talk to an NPC', 'Talks to the nearest NPC.'),
      key('bind.interact2', 'Talk to an NPC (second key)', 'Another key that talks to the nearest NPC.'),
      key('bind.travel', 'Travel skill', 'On missions: your class travel skill (Back Slide, Blink, Charge…).'),
      key('bind.items', 'Inventory window', 'Opens and closes the inventory.'),
      key('bind.potion', 'Healing potion', 'On missions: drinks the best healing potion or food for the HP missing.'),
      key('bind.potion2', 'SP potion', 'On missions: drinks the best SP potion for the SP missing.'),
      key('bind.sit', 'Sit / stand', 'In town: sits down or stands up.'),
      key('bind.equip', 'Equipment window', 'Opens and closes your equipment.'),
      key('bind.party', 'Party window', 'Opens and closes the party window.'),
      key('bind.friends', 'Friends window', 'Opens and closes the friends list.'),
      key('bind.leaderboard', 'Ranking', 'Opens the leaderboard of the top adventurers.'),
      key('bind.info', 'Fold Basic Info', 'Folds the status window (name, HP, SP) to a thin strip.'),
      key('bind.debug', 'Debug overlay', 'Steps through the debug overlay modes.'),
      {'id': 'h.slots', 'label': 'Hotbar', 'type': 'heading'},
      for (var i = 0; i < Bindings.count; i++)
        key('bind.slot.$i', 'Slot ${i + 1}', 'Uses the skill in hotbar slot ${i + 1} (aimed at the pointer).'),
      {'id': 'h.emotes', 'label': 'Emotes', 'type': 'heading'},
      for (var i = 0; i < Bindings.count; i++) ...[
        key('bind.emote.$i', 'Emote ${i + 1}: key', 'Plays emote ${i + 1} (picked on the next line).'),
        if (emotes.isNotEmpty)
          {
            'id': 'emote.$i',
            'label': 'Emote ${i + 1}: plays',
            'type': 'select',
            'value': _binds.emoteIds[i],
            'options': emotes,
            'hint': 'The emote that key ${i + 1} plays over your head.',
          },
      ],
      {'id': 'h.flags', 'label': 'Flags (placed at the pointer)', 'type': 'heading'},
      for (var i = 0; i < Bindings.count; i++)
        key(
          'bind.flag.$i',
          'Flag: ${i < flags.length ? flags[i] : '${i + 1}'}',
          'Plants this flag on the ground at the pointer (under you on touch).',
        ),
      {'id': 'h.video', 'label': 'Other', 'type': 'heading'},
    ];
  }

  /// WASD / arrows → world direction (x east, y north), turned by the camera
  /// yaw so "up" always walks away from the camera.
  (double, double) _inputDir() {
    var sx = 0.0, sy = 0.0;
    if (_keys.contains('KeyW') || _keys.contains('ArrowUp')) sy += 1;
    if (_keys.contains('KeyS') || _keys.contains('ArrowDown')) sy -= 1;
    if (_keys.contains('KeyD') || _keys.contains('ArrowRight')) sx += 1;
    if (_keys.contains('KeyA') || _keys.contains('ArrowLeft')) sx -= 1;
    // Analog sticks (gamepad, touch joystick) when no key is down; their
    // length is the speed. RT holds position.
    if (sx == 0 && sy == 0) (sx, sy) = (_input.state.moveX, _input.state.moveY);
    if (sx == 0 && sy == 0 || _input.state.hold) return (0, 0);
    if (view.dialogOpen()) return (0, 0);
    final len = math.sqrt(sx * sx + sy * sy);
    if (len > 1) {
      sx /= len;
      sy /= len;
    }
    final yaw = view.cameraYaw();
    final c = math.cos(yaw), s = math.sin(yaw);
    // Yaw is clockwise from north: rotate the screen vector clockwise.
    return quantizeDir(sx * c + sy * s, -sx * s + sy * c);
  }

  // ---------------------------------------------------------------- loop

  void _tick() {
    _input.poll(hotbar: _skills?.hotbar ?? const [], inStage: _inStage);
    final p = _predictor;
    // Offline: nothing reaches the server, so nothing is predicted either.
    if (p == null || _loading || _offline) {
      _tapped.clear();
      return;
    }
    // Downed or fallen: the server ignores movement; do not predict any.
    var (dx, dy) = _myState == 0 ? _inputDir() : (0.0, 0.0);
    // Belts carry only a standing player (as the server's drift).
    p.drifts = _myState == 0;
    // Skills are for missions: in town the bar is only set up, never cast
    // (but for the mount, anywhere).
    // In town the slot keys drive the town skill bar instead.
    if (_inStage) {
      _mountPress();
    } else {
      _townPress();
    }
    final slots = _inStage ? _heldSlots() : 0;
    if (_inStage) _callHomPress(slots);
    _tapped.clear();
    // Click to move: steering of our own (keys, stick, joystick) or going
    // down stops it at once. A skill press stands for the cast (casting is
    // rooted): a one-off walk ends, a held button walks on after it. Auto-
    // cast slots and the travel key (it goes where the hero walks) do not.
    if (_walker.active) {
      final cast = slots & ~(autoCastSlots(_autoCast, _skills?.hotbar ?? const []) | 1 << travelSlot);
      if (dx != 0 || dy != 0 || _myState != 0) {
        _cancelWalk();
      } else if (cast != 0) {
        if (!_steerHeld) _cancelWalk();
      } else if (!view.dialogOpen()) {
        (dx, dy) = _walkDir(p);
      }
    }
    final moving = dx != 0 || dy != 0;
    // Walking stands up (the server does the same).
    if (moving && _me != null) _sitting.remove(_me);
    if (moving || slots != 0) _stopSends = 3;
    if (moving || slots != 0 || _stopSends > 0) {
      if (!moving && slots == 0) _stopSends--;
      final (ax, ay) = slots != 0 ? _aim() : (0, 0);
      if (slots != 0) _syncCastPrefs();
      final seq = p.step(dx, dy);
      _ws.push(encodeInput(seq, dx, dy, aimX: ax, aimY: ay, slots: slots)).ignore();
    } else {
      p.idle();
    }
  }

  /// Render-clock seconds for a (fractional) server tick on this map.
  double _sec(num tick) => (tick - (_epochTick ?? 0)) / _tickHz;

  /// Leaves the world: closes the socket, removes the listeners, stops the
  /// loop and clears the view (the next session starts from scratch).
  Future<void> stop() async {
    if (_stopped) return;
    _stopped = true;
    _pingTimer?.cancel();
    _returnTimer?.cancel();
    _inspectTimer?.cancel();
    _pulse.stop();
    for (final (target, type, fn) in _listeners) {
      target.removeEventListener(type, fn);
    }
    _listeners.clear();
    for (final s in _subs) {
      await s.cancel();
    }
    _subs.clear();
    await _ws.close().catchError((Object _) {});
    view.showResults('');
    view.resetSession();
  }

  Future<void> _leave(String why) async {
    await stop();
    if (!_exit.isCompleted) _exit.complete(why);
  }

  void _frame(num _) {
    if (_stopped) return;
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
    _updateHover();

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
      facing: _poseFacing(me, p.moving, p.facing, now),
      sprite: _spriteFor(me, _myArchetype, _myLook, tier: _tierOf(me)),
      action: _charAction(me, p.moving, now),
      flags: Row.flagFocus | Row.flagHigh | statusRowFlags(_vitals[me]?.status ?? 0),
    );

    // Remote players: a row only when the bracketing snapshot pair, facing
    // or action changes (about once per server tick while moving, never
    // while standing). The shader interpolates in between.
    final rt = _clock.renderTick(now);
    final stale = <int>[];
    // Town fog: the server stops sending what leaves the light; it dissolves.
    final fog = !_inStage && _townLight > 0;
    for (final t in _tracks.values) {
      if (now - (_lastSeen[t.id] ?? now) > (fog ? 0.6 : 3)) {
        stale.add(t.id);
        continue;
      }
      t.segment(rt);
      // In the town fog a name shows only while its owner is in the light.
      if (fog) {
        final dx = t.x1 - mx, dy = t.y1 - my;
        final lit = dx * dx + dy * dy <= (_townLight - 1) * (_townLight - 1);
        if (lit == _unlit.contains(t.id)) {
          final peer = _peers[t.id];
          if (lit) {
            _unlit.remove(t.id);
            if (peer != null) _showPeer(t.id, peer);
          } else {
            _unlit.add(t.id);
            view.setLabel(t.id.toDouble(), null, '');
          }
        }
      }
      final peer = _peers[t.id];
      // A homunculus: a creature of the companions pack (never a body).
      final comp = peer == null ? _companions[t.id] : null;
      final action = comp != null ? _companionAction(t.id, t.moving, now) : _charAction(t.id, t.moving, now);
      final facing = comp != null
          ? (now < (_attackUntil[t.id] ?? 0) ? _attackFacing[t.id] ?? t.facing : t.facing)
          : _poseFacing(t.id, t.moving, t.facing, now);
      final pet = peer?.petSprite;
      final sprite = comp != null
          ? _companionSprite(comp)
          : pet != null
          ? (_spriteCache['$_mapId:pet:$pet'] ??= view.spriteIndex(pet))
          : _spriteFor(t.id, peer?.archetype ?? 'novice', peer?.look, tier: _tierOf(t.id));
      final still = t.x0 == t.x1 && t.y0 == t.y1;
      final flags =
          Row.flagHigh |
          (fog ? Row.flagFade : 0) |
          statusRowFlags(_vitals[t.id]?.status ?? 0) |
          (comp != null ? _allyFlags(comp) : 0);
      if (t.sentFacing == facing && t.sentAction == action && t.sentSprite == sprite && _sentFlags[t.id] == flags) {
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
        facing: facing,
        sprite: sprite,
        action: action,
        flags: flags,
      );
      _sentFlags[t.id] = flags;
      t
        ..sentTick0 = t.tick0
        ..sentTick1 = t.tick1
        ..sentFacing = facing
        ..sentAction = action
        ..sentSprite = sprite
        ..sentX = still ? t.x1 : double.nan
        ..sentY = still ? t.y1 : double.nan;
    }
    stale.forEach(_hidePeer);
    _rootedRows(now);

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
      final held = m.status & MobStatus.held != 0;
      final action = dieAt != null
          ? Act.die
          : held
          ? Act.hit
          : now < m.attackUntil
          ? Act.attack
          : now < m.hitUntil
          ? Act.hit
          : m.moving
          ? Act.walk
          : Act.idle;
      // Diving and surfacing are heard; a mound on the move digs.
      final under = m.status & MobStatus.burrowed != 0 && dieAt == null;
      if (under != m.burrowed) {
        m.burrowed = under;
        m.digAt = now + 0.9;
        view.unitSound(m.id, under ? 'burrow' : 'emerge');
      } else if (under && m.moving && now >= m.digAt) {
        m.digAt = now + 1.1;
        view.unitSound(m.id, 'dig');
      }
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
        facing: now < m.attackUntil ? m.attackFacing : m.facing,
        sprite: sprite,
        action: action,
        speed: m.animSpeed,
        flags: Row.flagFade | m.statusFlags,
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
    // An NPC's dialog closes when its NPC is out of reach (menus stay).
    if (_nearNpc == null && _npcDialog && view.dialogOpen() && !_settingsOpen && _devNpc == null) {
      view.closeDialog();
      _npcDialog = false;
      _dropRefiner();
    }
    view.setHint(
      _nearNpc != null && !view.dialogOpen()
          ? '${[_binds.interact?.caption, _binds.interact2?.caption].whereType<String>().join(' / ')}: talk to ${_nearNpc!.name}'
          : null,
    );

    _out.flush(view);
    view.renderFrame(_clock.synced ? _sec(rt) : 0);
    _updateStats(now, p);
  }

  final _spriteCache = <String, int>{};

  /// Class tier of a character's body: from its appearance (the own
  /// character's level until the first one).
  int _tierOf(int id) => id == _me
      ? (_mine?.tier ?? tierOfLevel(_progress?.level ?? _skills?.level ?? _myLevel))
      : (_peers[id]?.tier ?? 1);

  /// Sprite for a character: the dyeable body of its class at [tier] and
  /// its gender (hair and dyes come from `setAppearance`; the clothes dye rows
  /// follow the body); without the customization pack,
  /// `<archetype>_m|_f` when the atlas has it, else the novice placeholder.
  int _spriteFor(int id, String archetype, Look? look, {int tier = 1}) {
    final gender = look?.gender ?? (id.isEven ? 0 : 1);
    final job = riderJob(archetype, tier, riding: id == _me ? _riding : (_peers[id]?.riding ?? false));
    final body = _spriteCache['$_mapId:body:$job:$gender'] ??= view.bodySpriteIndex(gender, job);
    if (body >= 0) return body;
    final g = gender == 0 ? 'm' : 'f';
    final key = '$_mapId:$archetype:$g';
    return _spriteCache[key] ??= () {
      final s = view.spriteIndex('${archetype}_$g');
      return s >= 0 ? s : view.spriteIndex('novice_$g');
    }();
  }

  /// A monster's stars on its plate, auras and boss bar (engine `affixes.ts`).
  void _showAffixes(_Mob m) {
    final a = _affixes[m.slot];
    if (a == null) return;
    final id = m.kind < _monsterKinds.length ? _monsterKinds[m.kind] : '';
    final name = _brief?.monsters.where((b) => b.id == id).firstOrNull?.name ?? id;
    view.setAffixes(m.id, a.bits, a.state, name);
  }

  /// Sprite of a monster kind in the current location atlas (-1 when the
  /// atlas lacks it; the engine skips such rows). A miss asks for the
  /// companions pack: it carries the field plants Plant Cultivation grows
  /// on stages whose atlas has none.
  int _mobSprite(int kind) {
    final name = kind < _monsterKinds.length ? _monsterKinds[kind] : '';
    final i = _spriteCache['$_mapId:mob:$name'] ??= view.spriteIndex(name);
    if (i < 0 && name.isNotEmpty) _loadCompanionsPack();
    return i;
  }

  void _updateStats(double now, LocalPredictor p) {
    if (now - _rateWindowStart >= 1) {
      final span = now - _rateWindowStart;
      _snapshotRate = _snapshots / span;
      final bytesIn = _ws.metrics.bytesReceived;
      _bytesInRate = (bytesIn - _lastBytesIn) / span;
      _lastBytesIn = bytesIn;
      final bytesOut = _ws.metrics.bytesSent;
      _bytesOutRate = (bytesOut - _lastBytesOut) / span;
      _lastBytesOut = bytesOut;
      _snapshots = 0;
      _rateWindowStart = now;
    }
    // The debug overlay reads four times a second.
    if (now - _lastNetStats < 0.25) return;
    _lastNetStats = now;
    view.updateHud('$_mapName · $_channel · ${_peers.length + 1} here');
    view.setNetStats(
      jsonEncode({
        'map': _mapName,
        'channel': _channel,
        'transport': 'WebSocket',
        'players': _peers.length + 1,
        'monsters': _mobs.length,
        'rttMs': _rttMs,
        'jitterMs': _clock.jitter * 1000,
        'interpMs': _clock.delay * 1000,
        'snapshotsPerSec': _snapshotRate,
        'kbInPerSec': _bytesInRate / 1024,
        'kbOutPerSec': _bytesOutRate / 1024,
        'pendingInputs': p.pendingCount,
        'x': p.x,
        'y': p.y,
      }),
    );
  }
}
