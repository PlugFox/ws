import 'dart:js_interop';

import 'package:game_client/src/api.dart';
import 'package:game_client/src/engine.dart';
import 'package:game_client/src/hub_session.dart';
import 'package:game_client/src/login.dart';
import 'package:game_client/src/page_guard.dart';
import 'package:game_client/src/protocol.dart' show Look;
import 'package:web/web.dart' as web;

// The baked pack. The image build sets `/assets/v/<build>` (cached as
// immutable by the server, `api/static_files.rs`); `tool/build.sh` passes
// `ASSETS_BASE` through.
const _assets = String.fromEnvironment('ASSETS_BASE', defaultValue: '/assets');

Future<void> main() async {
  // Browser zoom, context menu, selection and drag stay out of the game.
  guardPage();
  final origin = web.window.location.origin;
  final api = Api(origin);
  final stage = web.document.getElementById('stage')! as web.HTMLElement;
  // The engine is up before sign-in: it draws the character screens.
  final view = gameEngine.createGameView();
  await view.init(web.document.getElementById('c')! as web.HTMLCanvasElement).toDart;
  try {
    final raw = web.window.localStorage.getItem('game.settings');
    if (raw != null) view.configure(raw);
  } catch (_) {}
  // Sign in → characters → the world; the game menu leads back to the
  // character slots or out to the sign-in, and so does a session that
  // cannot reconnect (with why on the sign-in form).
  String? notice;
  for (;;) {
    final character = await runLogin(api, stage, view, assetsBase: _assets, notice: notice);
    final session = HubSession(
      api,
      view,
      assetsBase: _assets,
      characterId: (character['id']! as num).toInt(),
      characterName: character['name']! as String,
      archetype: character['archetype'] as String? ?? 'novice',
      level: (character['level'] as num?)?.toInt() ?? 1,
      look: Look.fromJson(character['look']),
      stats: (character['stats'] as Map<String, Object?>?) ?? const {},
      exp: (character['base_exp'] as num?)?.toInt() ?? 0,
    );
    web.console.info('Entering the hub as ${character['name']}'.toJS);
    await session.start();
    if (await session.exited != 'characters') api.forget();
    notice = session.exitNotice;
  }
}
