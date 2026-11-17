// ignore_for_file: avoid_print
import 'dart:async';

import 'package:ws/ws.dart';

/// Connects to an echo server, sends a few messages, measures a round trip
/// and closes. Run against the repository's server: `mise run server`, then
/// `dart run example/ws_example.dart ws://127.0.0.1:9090/echo`.
Future<void> main(List<String> args) async {
  final url = args.isNotEmpty ? args.first : 'ws://127.0.0.1:9090/echo';
  final client = WebSocketClient.json(
    options: WebSocketOptions(
      heartbeat: const WebSocketHeartbeat(interval: Duration(seconds: 15)),
      onStateChange: (state) => print('state: $state'),
      onError: (error, _) => print('error: $error'),
    ),
  );
  client.messages.listen((message) => print('< $message'));

  await client.connect(url).timeout(const Duration(seconds: 10));
  for (var i = 0; i < 3; i++) {
    final message = {'n': i, 'text': 'hello ✓'};
    print('> $message');
    await client.push(message);
  }
  await client.ping();
  print('round trip: ${client.metrics.lastRoundTrip}');
  await Future<void>.delayed(const Duration(milliseconds: 200));
  await client.close();
  print(client.metrics);
}
