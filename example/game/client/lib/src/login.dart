import 'dart:async';
import 'dart:convert';
import 'dart:js_interop';

import 'package:game_client/src/api.dart';
import 'package:game_client/src/classes.dart';
import 'package:game_client/src/contacts.dart';
import 'package:game_client/src/engine.dart';
import 'package:game_client/src/online.dart';
import 'package:game_client/src/protocol.dart' show Look;
import 'package:game_client/src/version.dart';
import 'package:web/web.dart' as web;

/// Character slots per account (the server enforces the same limit).
const maxCharacters = 6;

/// Friendly place names for the slot cards.
const _places = {
  'izlude': 'Izlude',
  'izlude_in': 'Izlude',
  'hub_central': 'Izlude',
  'prt_fild08': 'Poring Island',
};

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
    // Back from the world with a session: straight to the characters.
    if (api.token == null) await _signIn(api, view, notice: notice);
    if (!_customization) {
      await view.loadCustomization('$assetsBase/client/customization/').toDart;
      _customization = true;
    }
    return await _chooseCharacter(api, view);
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

Future<Map<String, Object?>> _chooseCharacter(Api api, GameView view) {
  final done = Completer<Map<String, Object?>>();
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
              'location': _places[c['last_location_id']] ?? c['last_location_id'],
              'look': {
                ...Look.fromJson(c['look']).toUi(),
                // The class body at the character's tier.
                'job': bodyJob('${c['archetype']}', tierOfLevel((c['level'] as num?)?.toInt() ?? 1)),
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

  view.onUiEvent(
    ((String json) {
      if (done.isCompleted) return;
      final e = jsonDecode(json) as Map<String, Object?>;
      switch (e['type']) {
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
              .catchError((Object err) => show(error: '$err'));
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
              .catchError((Object err) => show(error: '$err'));
      }
    }).toJS,
  );

  refresh().then((_) => show(create: chars.isEmpty)).catchError((Object err) => show(error: '$err'));
  return done.future;
}
