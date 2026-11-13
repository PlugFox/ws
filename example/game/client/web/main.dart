import 'dart:js_interop';

import 'package:game_client/src/api.dart';
import 'package:game_client/src/engine.dart';
import 'package:game_client/src/hub_session.dart';
import 'package:game_client/src/login.dart';
import 'package:game_client/src/protocol.dart' show Look;
import 'package:web/web.dart' as web;

const _assets = '/assets';

Future<void> main() async {
  final origin = web.window.location.origin;
  final api = Api(origin);
  final stage = web.document.getElementById('stage')! as web.HTMLElement;
  // The engine is up before sign-in: it draws the character screens.
  final view = gameEngine.createGameView();
  await view.init(web.document.getElementById('c')! as web.HTMLCanvasElement).toDart;
  final character = await runLogin(api, stage, view, assetsBase: _assets);

  final session = HubSession(
    api,
    view,
    assetsBase: _assets,
    characterName: character['name']! as String,
    archetype: character['archetype'] as String? ?? 'novice',
    level: (character['level'] as num?)?.toInt() ?? 1,
    look: Look.fromJson(character['look']),
  );
  web.console.info('Entering the hub as ${character['name']}'.toJS);
  await session.start(api.token!);
}
