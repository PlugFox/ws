// Headless hub bots on the Dart VM (same package:ws client as the web app):
// each registers, creates and selects a character, joins the hub and walks
// in a circle around the spawn. For load tests and for watching remote
// interpolation from a browser.
//
//   dart run tool/bot.dart [--server http://127.0.0.1:8088] [--count 5]
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:game_client/src/protocol.dart';
import 'package:ws/ws.dart';

Future<void> main(List<String> args) async {
  String opt(String name, String def) {
    final i = args.indexOf('--$name');
    return i >= 0 && i + 1 < args.length ? args[i + 1] : def;
  }

  final server = Uri.parse(opt('server', 'http://127.0.0.1:8088'));
  final count = int.parse(opt('count', '5'));
  final run = DateTime.now().millisecondsSinceEpoch % 100000;
  for (var i = 0; i < count; i++) {
    final name = 'bot${run}_$i';
    // Staggered, and one failing bot must not take the others down.
    unawaited(_bot(server, name, i, count).catchError((Object e) => stderr.writeln('$name: $e')));
    await Future<void>.delayed(const Duration(milliseconds: 150));
  }
}

Future<Map<String, Object?>> _post(HttpClient http, Uri uri, Object body, [String? token]) async {
  final req = await http.postUrl(uri);
  req.headers.contentType = ContentType.json;
  if (token != null) req.headers.set('Authorization', 'Bearer $token');
  req.write(jsonEncode(body));
  final resp = await req.close();
  final text = await resp.transform(utf8.decoder).join();
  if (resp.statusCode >= 300) throw StateError('${uri.path}: ${resp.statusCode} $text');
  return text.isEmpty ? {} : jsonDecode(text) as Map<String, Object?>;
}

Future<void> _bot(Uri server, String name, int index, int count) async {
  final http = HttpClient();
  // Throwaway credentials for a local dev server.
  final reg = await _post(http, server.resolve('/api/auth/register'), {
    'username': name,
    'password': 'Bot-${name.hashCode.abs()}-pass!',
  });
  final token = reg['access_token']! as String;
  final ch = await _post(http, server.resolve('/api/characters'), {'name': name, 'archetype': 'novice'}, token);
  await _post(http, server.resolve('/api/characters/${ch['id']}/select'), {}, token);

  final ws = WebSocketClient.binary();
  double? x, y;
  double cx = 0, cy = 0;
  ws.messages.listen((m) {
    if (m.isNotEmpty && m[0] == 0x7B) {
      final msg = jsonDecode(utf8.decode(m)) as Map<String, Object?>;
      if (msg['type'] == 'map_enter') {
        x = (msg['x']! as num).toDouble();
        y = (msg['y']! as num).toDouble();
        cx = x!;
        cy = y! - 4;
      }
    }
  });
  final wsUri = server.replace(scheme: server.scheme == 'https' ? 'wss' : 'ws', path: '/api/ws', query: 'token=$token');
  await ws.connect(wsUri.toString());
  await ws.push(Uint8List.fromList(utf8.encode('{"type":"hub_join"}')));
  stdout.writeln('$name joined');

  var seq = 0;
  final phase = index * 2 * math.pi / count;
  final sw = Stopwatch()..start();
  Timer.periodic(const Duration(milliseconds: 50), (_) {
    if (x == null) return;
    // Head for a point moving on a circle; the server does the walking.
    final t = sw.elapsedMilliseconds / 1000 * 0.6 + phase;
    final (tx, ty) = (cx + math.cos(t) * 4, cy + math.sin(t) * 4);
    final (dx, dy) = quantizeDir((tx - x!).clamp(-1, 1).toDouble(), (ty - y!).clamp(-1, 1).toDouble());
    ws.push(encodeInput(++seq, dx, dy)).ignore();
    // Dead reckoning is enough for steering a bot.
    x = x! + dx * 0.3;
    y = y! + dy * 0.3;
  });
}
