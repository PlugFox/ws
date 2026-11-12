@TestOn('browser')
library;

import 'dart:async';
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/ws.dart';

void main() {
  late int port;

  setUpAll(() async {
    final channel = spawnHybridUri('/test/support/echo_server.dart');
    port = (await channel.stream.first as num).toInt();
  });

  test('echo text, unicode, binary and json through the browser', () async {
    final client = WebSocketClient.text(
      options: const WebSocketOptions(protocols: ['v1', 'v2']),
    );
    final got = <String>[];
    final done = Completer<void>();
    client.messages.listen((m) {
      got.add(m);
      if (got.length == 2) done.complete();
    });
    await client.connect('ws://127.0.0.1:$port/echo');
    expect(client.metrics.transport, 'web');
    expect((client.state as WebSocketState$Open).protocol, 'v2');
    await client.push('hello');
    await client.push('мир ✓ 𝄞');
    await done.future;
    expect(got, ['hello', 'мир ✓ 𝄞']);
    await client.close();

    final binary = WebSocketClient.binary();
    final echoed = Completer<Uint8List>();
    binary.messages.listen(echoed.complete);
    await binary.connect('ws://127.0.0.1:$port/echo');
    final sent = Uint8List.fromList(
      List<int>.generate(70000, (i) => (i * 7 + i ~/ 251) & 0xFF),
    );
    await binary.push(sent);
    expect(await echoed.future, orderedEquals(sent));
    await binary.close();

    final json = WebSocketClient.json();
    final reply = Completer<Map<String, Object?>>();
    json.messages.listen(reply.complete);
    await json.connect('ws://127.0.0.1:$port/echo');
    await json.push({
      'a': [1, 'ü'],
    });
    expect(await reply.future, {
      'a': [1, 'ü'],
    });
    await json.close();
  });

  test('protocol ping is unsupported without an application ping', () async {
    final client = WebSocketClient.text();
    await client.connect('ws://127.0.0.1:$port/echo');
    await expectLater(
      client.ping(),
      throwsA(isA<WebSocketUnsupportedException>()),
    );
    await client.close();
  });

  test('server close code reaches the client', () async {
    final client = WebSocketClient.text(
      options: WebSocketOptions(
        reconnect: WebSocketReconnect(shouldReconnect: (_, _) => false),
      ),
    );
    await client.connect('ws://127.0.0.1:$port/echo');
    await client.push('close-4001');
    final state = await client.states.firstWhere(
      (s) => s is WebSocketState$Disconnected,
    );
    expect(
      (state as WebSocketState$Disconnected).error,
      isA<WebSocketClosedException>()
          .having((e) => e.code, 'code', 4001)
          .having((e) => e.reason, 'reason', 'bye'),
    );
    await client.close();
  });

  test('rejected handshake is a retried connection error on the web', () async {
    final client = WebSocketClient.text(
      options: const WebSocketOptions(
        reconnect: WebSocketReconnect(minDelay: Duration(milliseconds: 10)),
      ),
    );
    unawaited(client.connect('ws://127.0.0.1:$port/reject'));
    final state = await client.states.firstWhere(
      (s) => s is WebSocketState$Reconnecting,
    );
    expect(
      (state as WebSocketState$Reconnecting).cause,
      isA<WebSocketConnectionException>(),
    );
    await client.close();
  });
}
