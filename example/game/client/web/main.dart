import 'dart:js_interop';

import 'package:game_client/src/api.dart';
import 'package:game_client/src/engine.dart';
import 'package:game_client/src/hub_session.dart';
import 'package:game_client/src/login.dart';
import 'package:web/web.dart' as web;

Future<void> main() async {
  final origin = web.window.location.origin;
  final api = Api(origin);
  final stage = web.document.getElementById('stage')! as web.HTMLElement;
  final character = await runLogin(api, stage);

  final view = gameEngine.createGameView();
  await view.init(web.document.getElementById('c')! as web.HTMLCanvasElement).toDart;
  final session = HubSession(api, view, assetsBase: '/assets', characterName: character['name']! as String);
  web.console.info('Entering the hub as ${character['name']}'.toJS);
  await session.start(api.token!);
}
