import 'dart:async';
import 'dart:convert';
import 'dart:js_interop';

import 'package:game_client/src/api.dart';
import 'package:game_client/src/classes.dart';
import 'package:game_client/src/contacts.dart';
import 'package:game_client/src/engine.dart';
import 'package:game_client/src/online.dart';
import 'package:game_client/src/protocol.dart' show Look, weaponFamilyOf;
import 'package:game_client/src/version.dart';
import 'package:web/web.dart' as web;

/// Character slots per account (the server enforces the same limit).
const maxCharacters = 6;

/// The engine's sign-in window, then its character slots and creation
/// screens. Completes with the selected character once the server has it
/// loaded into the session. [notice] is why the player is back at the
/// sign-in (an expired session, signed in elsewhere), shown on its form.
Future<Map<String, Object?>> runLogin(
  Api api,
  web.HTMLElement host,
  GameView view, {
  required String assetsBase,
  String? notice,
}) async {
  // Start the menu BGM (Ragnarok Online 2 theme).
  view.playBgm('$assetsBase/client/audio/bgm/menu_ro2.ogg');

  // The interface atlas and the title backdrop first; a failure keeps the
  // plain CSS look.
  try {
    await view.loadUi('$assetsBase/client/ui/').toDart;
  } on Object catch (e) {
    web.console.warn('interface atlas: $e'.toJS);
  }
  // Versions in the title screen corner: the client's now, the server's
  // once `/health` answers.
  view.setVersion(versionLine(null));
  api
      .json('/health')
      .then((h) => view.setVersion(versionLine('${h['version'] ?? ''}')))
      .catchError((Object e) => web.console.warn('health: $e'.toJS));
  // The server's pulse under the panels while on these screens only.
  final pulse = OnlinePoller(() => api.json('/api/online'), (s) => view.setServerStatus(jsonEncode(s.toJson())))..start();
  try {
    for (;;) {
      // Back from the world with a session: straight to the characters.
      if (api.token == null) await _signIn(api, view, notice: notice);
      notice = null;
      if (!_customization) {
        await view.loadCustomization('$assetsBase/client/customization/').toDart;
        _customization = true;
      }
      try {
        final chosen = await _chooseCharacter(api, view);
        if (chosen != null) return chosen;
        // Log out: the server revoked the session; sign in again.
        view.closeScreen();
      } on SessionExpired catch (e) {
        // The session could not be renewed: sign in again, saying why.
        api.forget();
        view.closeScreen();
        notice = e.message;
      }
    }
  } finally {
    pulse.stop();
    view.setServerStatus('');
  }
}

/// The customization pack is loaded once per page.
var _customization = false;

/// The class lines of `GET /api/classes`, fetched once per page.
List<Map<String, Object?>>? _classLines;

/// The account name the player asked to remember.
const _rememberKey = 'ge.login.user';

Future<void> _signIn(Api api, GameView view, {String? notice}) {
  final done = Completer<void>();
  final saved = web.window.localStorage.getItem(_rememberKey);
  var remember = saved != null;
  var user = saved ?? '';

  // [mode] keeps the "Create account" form up after its errors; [field]
  // puts the error under that field (the server's `field`, or ours).
  void show({bool busy = false, String? error, String? field, String? mode}) => view.showLogin(
    jsonEncode({'user': user, 'remember': remember, 'busy': busy, 'error': ?error, 'field': ?field, 'mode': ?mode}),
  );

  view.onUiEvent(
    ((String json) {
      if (done.isCompleted) return;
      final e = jsonDecode(json) as Map<String, Object?>;
      if (e['type'] != 'login') return;
      user = '${e['user']}';
      remember = e['remember'] == true;
      final create = e['create'] == true;
      final mode = create ? 'create' : null;
      final email = '${e['email'] ?? ''}'.trim(), telegram = '${e['telegram'] ?? ''}'.trim();
      if (create) {
        final problem = contactProblem(email: email, telegram: telegram);
        if (problem != null) return show(error: problem.message, field: problem.field, mode: mode);
      }
      show(busy: true, mode: mode);
      api
          .login(user, '${e['password']}', register: create, email: email, telegram: telegram)
          .then((_) {
            if (remember) {
              web.window.localStorage.setItem(_rememberKey, user);
            } else {
              web.window.localStorage.removeItem(_rememberKey);
            }
            done.complete();
          })
          .catchError((Object err) => show(error: '$err', field: err is ApiException ? err.field : null, mode: mode));
    }).toJS,
  );
  show(error: notice);
  return done.future;
}

/// The character slots: completes with the chosen character, or null when
/// the player logs out (the session is revoked by then). Throws
/// [SessionExpired] when the session ended under it.
/// What a stored character wears that the paper doll draws: the held
/// weapon's family and the headgear (equipment slots 0..2 and 4 of its
/// `inventory.equip`; refines and cards trimmed off the ids).
Map<String, Object> _wornLook(Map<String, Object?> c) {
  final equip = ((c['inventory'] as Map?)?['equip'] as List?)?.map((e) => '$e'.split(RegExp(r'[+\[]')).first).toList() ?? const [];
  final hats = [for (var i = 0; i < 3 && i < equip.length; i++) if (equip[i].isNotEmpty) equip[i]];
  final weapon = equip.length > 4 ? weaponFamilyOf(equip[4]) : null;
  return {'headgear': ?(hats.isEmpty ? null : hats), 'weapon': ?weapon};
}

Future<Map<String, Object?>?> _chooseCharacter(Api api, GameView view) {
  final done = Completer<Map<String, Object?>?>();
  var chars = <Map<String, Object?>>[];
  int? selected;

  void show({bool busy = false, String? error, bool create = false}) {
    view.showCharacterSelect(
      jsonEncode({
        'slots': [
          for (final c in chars)
            {
              'id': (c['id']! as num).toInt(),
              'name': c['name'],
              'archetype': c['archetype'],
              'title': ?((c['stats'] as Map<String, Object?>?)?['title']),
              'level': c['level'],
              'location': placeName(c),
              'look': {
                ...Look.fromJson(c['look']).toUi(),
                // The class body at the character's tier.
                'job': bodyJob('${c['archetype']}', tierOfLevel((c['level'] as num?)?.toInt() ?? 1)),
                ..._wornLook(c),
              },
            },
        ],
        'maxSlots': maxCharacters,
        // The server's class lines (`GET /api/classes`: dice, weapons,
        // skills, ratings); creation shows each one's tier-1 body, locked
        // lines a body of the all-bodies pack.
        'classes': [
          for (final c in _classLines ?? const <Map<String, Object?>>[])
            {...c, 'job': bodyJob('${c['id']}'), 'body': ?lockedBodies['${c['id']}']},
        ],
        'selected': ?selected,
        'busy': busy,
        'error': ?error,
        'create': create,
        'nameMin': 2,
        'nameMax': 24,
      }),
    );
  }

  Future<void> refresh() async {
    final lines = _classLines == null ? api.json('/api/classes') : null;
    chars = await api.characters();
    if (lines != null) _classLines = ((await lines)['classes']! as List).cast<Map<String, Object?>>();
  }

  // A request failed: an ended session goes back to the sign-in, anything
  // else shows on the screen.
  void failed(Object err) {
    if (done.isCompleted) return;
    if (err is SessionExpired) return done.completeError(err);
    show(error: '$err');
  }

  view.onUiEvent(
    ((String json) {
      if (done.isCompleted) return;
      final e = jsonDecode(json) as Map<String, Object?>;
      switch (e['type']) {
        case 'char_logout':
          show(busy: true);
          api.logout().whenComplete(() {
            if (!done.isCompleted) done.complete(null);
          });
        case 'char_select':
          final id = (e['id']! as num).toInt();
          selected = id;
          show(busy: true);
          api
              .select(id)
              .then((_) {
                view.closeScreen();
                done.complete(chars.firstWhere((c) => (c['id']! as num).toInt() == id));
              })
              .catchError(failed);
        case 'char_create':
          show(busy: true);
          api
              .createCharacter(
                '${e['name']}',
                '${e['archetype']}',
                look: {
                  'gender': (e['gender']! as num).toInt(),
                  'hair_style': (e['hair']! as num).toInt(),
                  'hair_color': (e['hairColor']! as num).toInt(),
                  'body_color': (e['bodyColor']! as num).toInt(),
                },
              )
              .then((created) async {
                selected = (created['id']! as num).toInt();
                await refresh();
                show();
              })
              .catchError(failed);
      }
    }).toJS,
  );

  refresh().then((_) => show(create: chars.isEmpty)).catchError(failed);
  return done.future;
}

/// The slot card's place: the server's display name of the character's
/// last location (`last_location_name`, from the map's manifest), else the
/// map id.
String placeName(Map<String, Object?> character) {
  final name = character['last_location_name'];
  return name is String && name.isNotEmpty ? name : '${character['last_location_id'] ?? ''}';
}
