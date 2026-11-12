import 'dart:async';
import 'dart:js_interop';

import 'package:game_client/src/api.dart';
import 'package:web/web.dart' as web;

/// Minimal auth + character screen (HTML). Completes with the selected
/// character once the server has it loaded into the session.
Future<Map<String, Object?>> runLogin(Api api, web.HTMLElement host) {
  final done = Completer<Map<String, Object?>>();
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

  Future<void> guarded(Future<void> Function() f) async {
    error.textContent = '';
    try {
      await f();
    } on Object catch (e) {
      error.textContent = '$e';
    }
  }

  late void Function() showCharacters;

  void showAuth() {
    panel.textContent = '';
    final user = input('username');
    final pass = input('password', type: 'password');
    final login = el('button', text: 'Log in');
    final register = el('button', text: 'Register', cls: 'secondary');
    Future<void> go(bool reg) => guarded(() async {
      await api.login(user.value.trim(), pass.value, register: reg);
      showCharacters();
    });
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
      ..appendChild(el('h1', text: 'ws · Izlude'))
      ..appendChild(user)
      ..appendChild(pass)
      ..appendChild(login)
      ..appendChild(register)
      ..appendChild(error);
    user.focus();
  }

  showCharacters = () {
    guarded(() async {
      final chars = await api.characters();
      panel.textContent = '';
      panel.appendChild(el('h1', text: 'Characters'));
      for (final c in chars) {
        final b = el('button', text: '${c['name']} · ${c['archetype']} · lv ${c['level']}');
        b.addEventListener(
          'click',
          ((web.Event _) {
            guarded(() async {
              await api.select((c['id']! as num).toInt());
              panel.remove();
              done.complete(c);
            });
          }).toJS,
        );
        panel.appendChild(b);
      }
      final name = input('new character name');
      final arch = web.document.createElement('select') as web.HTMLSelectElement;
      for (final a in ['novice', 'warrior', 'ranger', 'mage', 'rogue']) {
        arch.appendChild(
          (web.document.createElement('option') as web.HTMLOptionElement)
            ..value = a
            ..textContent = a,
        );
      }
      final create = el('button', text: 'Create', cls: 'secondary');
      create.addEventListener(
        'click',
        ((web.Event _) {
          guarded(() async {
            await api.createCharacter(name.value.trim(), arch.value);
            showCharacters();
          });
        }).toJS,
      );
      panel
        ..appendChild(name)
        ..appendChild(arch)
        ..appendChild(create)
        ..appendChild(error);
    });
  };

  showAuth();
  return done.future;
}
