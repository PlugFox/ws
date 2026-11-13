import 'dart:async';
import 'dart:convert';
import 'dart:js_interop';

import 'package:game_client/src/api.dart';
import 'package:game_client/src/classes.dart';
import 'package:game_client/src/engine.dart';
import 'package:game_client/src/protocol.dart' show Look;
import 'package:web/web.dart' as web;

/// Classes offered at creation (the server validates the id). Each line
/// grows through three titles with level; see example/docs/COMBAT.md.
const _classes = [
  {'id': 'swordsman', 'name': 'Swordsman', 'description': 'Crusader → Paladin. Front line: taunts the horde, shield and holy blade.'},
  {'id': 'mage', 'name': 'Mage', 'description': 'Wizard → High Wizard. Storms of fire, frost and lightning; fragile.'},
  {'id': 'archer', 'name': 'Archer', 'description': 'Hunter → Sniper. Extreme range, traps and a falcon; bursts the biggest target.'},
  {'id': 'acolyte', 'name': 'Acolyte', 'description': 'Priest → High Priest. Heals the most wounded, blesses the party, smites undead.'},
  {'id': 'thief', 'name': 'Thief', 'description': 'Assassin → Assassin Cross.', 'locked': true},
  {'id': 'merchant', 'name': 'Merchant', 'description': 'Blacksmith → Whitesmith.', 'locked': true},
];

/// Character slots per account (the server enforces the same limit).
const maxCharacters = 6;

/// Friendly place names for the slot cards.
const _places = {
  'izlude': 'Izlude',
  'izlude_in': 'Izlude',
  'hub_central': 'Izlude',
  'prt_fild08': 'Poring Island',
};

/// Sign-in form (HTML), then the engine's character slots and creation
/// screens. Completes with the selected character once the server has it
/// loaded into the session.
Future<Map<String, Object?>> runLogin(Api api, web.HTMLElement host, GameView view, {required String assetsBase}) async {
  await _signIn(api, host);
  await view.loadCustomization('$assetsBase/client/customization/').toDart;
  return _chooseCharacter(api, view);
}

Future<void> _signIn(Api api, web.HTMLElement host) {
  final done = Completer<void>();
  final panel = web.document.createElement('div') as web.HTMLDivElement..className = 'login';
  host.appendChild(panel);

  web.HTMLElement el(String tag, {String? text, String? cls}) {
    final e = web.document.createElement(tag) as web.HTMLElement;
    if (text != null) e.textContent = text;
    if (cls != null) e.className = cls;
    return e;
  }

  web.HTMLInputElement input(String placeholder, {String type = 'text'}) =>
      (web.document.createElement('input') as web.HTMLInputElement)
        ..placeholder = placeholder
        ..type = type;

  final error = el('div', cls: 'error');
  final user = input('username')..autocomplete = 'username';
  final pass = input('password', type: 'password')..autocomplete = 'current-password';
  final login = el('button', text: 'Log in');
  final register = el('button', text: 'Create account', cls: 'secondary');
  var busy = false;
  Future<void> go(bool reg) async {
    if (busy) return;
    busy = true;
    error.textContent = '';
    try {
      await api.login(user.value.trim(), pass.value, register: reg);
      panel.remove();
      done.complete();
    } on Object catch (e) {
      error.textContent = '$e';
    } finally {
      busy = false;
    }
  }

  login.addEventListener(
    'click',
    ((web.Event _) {
      go(false);
    }).toJS,
  );
  register.addEventListener(
    'click',
    ((web.Event _) {
      go(true);
    }).toJS,
  );
  pass.addEventListener(
    'keydown',
    ((web.KeyboardEvent e) {
      if (e.key == 'Enter') go(false);
    }).toJS,
  );
  panel
    ..appendChild(el('h1', text: 'Izlude'))
    ..appendChild(el('p', text: 'A survivors-like over WebSockets', cls: 'sub'))
    ..appendChild(user)
    ..appendChild(pass)
    ..appendChild(login)
    ..appendChild(register)
    ..appendChild(error);
  user.focus();
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
        // Creation shows each class's tier-1 body.
        'classes': [
          for (final c in _classes) {...c, 'job': bodyJob('${c['id']}')},
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
    chars = await api.characters();
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
