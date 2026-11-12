// Regression tests for docs/review/client.md (F1–F31). Each test names the
// finding it pins.
import 'dart:async';
import 'dart:typed_data';

import 'package:fake_async/fake_async.dart';
import 'package:test/test.dart';
import 'package:ws/testing.dart';
import 'package:ws/transport.dart';
import 'package:ws/ws.dart';

void fake(void Function(FakeAsync async) body) => FakeAsync().run((async) {
  body(async);
  async.flushMicrotasks();
});

WebSocketClient<String> _text(
  FakeWebSocketServer server, {
  WebSocketReconnect reconnect = const WebSocketReconnect(),
  WebSocketHeartbeat<String>? heartbeat,
  void Function(Object, StackTrace)? onError,
  void Function(WebSocketState)? onStateChange,
  void Function(String)? onMessage,
  FutureOr<void> Function(WebSocketClientInterface<String>)? onOpen,
  FutureOr<WebSocketRequest> Function(Uri, int)? request,
  Duration connectTimeout = const Duration(seconds: 10),
}) => WebSocketClient.text(
  options: WebSocketOptions(
    transport: server,
    reconnect: reconnect,
    heartbeat: heartbeat,
    onError: onError,
    onStateChange: onStateChange,
    onMessage: onMessage,
    onOpen: onOpen,
    request: request,
    connectTimeout: connectTimeout,
  ),
);

int _open(FakeWebSocketServer s) => s.connections.where((c) => c.isOpen).length;

void main() {
  test('F2: close() with an invalid code throws and changes nothing', () async {
    final server = FakeWebSocketServer();
    final client = _text(server);
    await client.connect('ws://t');
    expect(() => client.close(1001), throwsArgumentError);
    expect(client.state.isOpen, isTrue);
    await client.close();
    expect(client.state.isClosed, isTrue);
    expect(_open(server), 0);
  });

  test('F22: a second close() waits for the first', () {
    fake((async) {
      final server = FakeWebSocketServer();
      final client = _text(server);
      client.connect('ws://t');
      async.flushMicrotasks();
      server.last.echoClose = false;
      var first = false;
      var second = false;
      client.close().then((_) => first = true);
      client.close().then((_) => second = true);
      async.flushMicrotasks();
      expect(second, isFalse);
      async.elapse(const Duration(seconds: 5));
      expect((first, second), (true, true));
    });
  });

  test('F3: reconnect() from onError leaves exactly one connection', () {
    fake((async) {
      final server = FakeWebSocketServer();
      late WebSocketClient<String> client;
      var once = true;
      client = _text(
        server,
        onError: (_, _) {
          if (once) {
            once = false;
            client.reconnect().ignore();
          }
        },
      );
      client.connect('ws://t');
      async.flushMicrotasks();
      server.last.drop();
      async.elapse(const Duration(minutes: 1));
      expect(_open(server), 1);
      expect(client.state.isOpen, isTrue);
      client.close();
    });
  });

  test('F3: disconnect() from onError ends in disconnected', () {
    fake((async) {
      final server = FakeWebSocketServer();
      late WebSocketClient<String> client;
      client = _text(server, onError: (_, _) => client.disconnect());
      client.connect('ws://t');
      async.flushMicrotasks();
      server.last.drop();
      async.elapse(const Duration(minutes: 1));
      expect(client.state, isA<WebSocketState$Disconnected>());
      expect(server.connections, hasLength(1));
      client.close();
    });
  });

  test('F4: close() completes with a paused listener', () async {
    final server = FakeWebSocketServer();
    final client = _text(server);
    final sub = client.states.listen((_) {})..pause();
    final messages = client.messages.listen((_) {})..pause();
    await client.connect('ws://t');
    await client.close().timeout(const Duration(seconds: 2));
    await sub.cancel();
    await messages.cancel();
  });

  test('F4: close() inside an await-for over messages', () async {
    final server = FakeWebSocketServer();
    final client = _text(server);
    await client.connect('ws://t');
    server.last.sendText('stop');
    await for (final m in client.messages) {
      if (m == 'stop') await client.close().timeout(const Duration(seconds: 2));
    }
  });

  test('F5: disconnect() during onOpen wins', () {
    fake((async) {
      final server = FakeWebSocketServer();
      final gate = Completer<void>();
      final states = <String>[];
      final client = _text(
        server,
        onOpen: (_) => gate.future,
        onStateChange: (s) => states.add(s.runtimeType.toString()),
      );
      client.connect('ws://t').ignore();
      async.flushMicrotasks();
      client.disconnect();
      gate.complete();
      async.elapse(const Duration(seconds: 5));
      expect(states, isNot(contains(r'WebSocketState$Open')));
      expect(client.state, isA<WebSocketState$Disconnected>());
      client.close();
    });
  });

  test('F6: timeout >= interval does not kill a healthy connection', () {
    fake((async) {
      final server = FakeWebSocketServer();
      final client = _text(
        server,
        heartbeat: const WebSocketHeartbeat(
          interval: Duration(seconds: 5),
          timeout: Duration(seconds: 10),
        ),
      );
      client.connect('ws://t');
      async
        ..flushMicrotasks()
        ..elapse(const Duration(minutes: 2));
      expect(server.connections, hasLength(1));
      expect(server.last.pings, greaterThan(3));
      expect(client.state.isOpen, isTrue);
      client.close();
    });
  });

  test('F7: greet-then-close servers see a growing backoff', () {
    fake((async) {
      final server = FakeWebSocketServer();
      server.connected.listen((c) {
        scheduleMicrotask(() {
          c
            ..sendText('{"error":"auth"}')
            ..close(4401, 'unauthorized');
        });
      });
      final client = _text(
        server,
        reconnect: const WebSocketReconnect(
          minDelay: Duration(milliseconds: 500),
          maxDelay: Duration(seconds: 30),
          maxAttempts: 5,
        ),
      );
      client.connect('ws://t').ignore();
      async.elapse(const Duration(minutes: 5));
      expect(server.connections.length, lessThanOrEqualTo(6));
      expect(client.state, isA<WebSocketState$Disconnected>());
      client.close();
    });
  });

  test('F8: minDelay zero still backs off', () {
    fake((async) {
      final server = FakeWebSocketServer(autoAccept: false);
      server.connected.listen((c) => c.reject(503));
      final client = _text(
        server,
        reconnect: const WebSocketReconnect(
          minDelay: Duration.zero,
          maxDelay: Duration(seconds: 10),
        ),
      );
      client.connect('ws://t').ignore();
      async.elapse(const Duration(seconds: 10));
      expect(server.connections.length, lessThan(40));
      client.close();
    });
  });

  test('F9: disconnect() then connect() completes the disconnect future', () {
    fake((async) {
      final server = FakeWebSocketServer();
      final client = _text(server);
      client.connect('ws://t');
      async.flushMicrotasks();
      server.last.echoClose = false;
      var disconnected = false;
      client.disconnect().then((_) => disconnected = true);
      client.connect('ws://t');
      async.elapse(const Duration(seconds: 10));
      expect(disconnected, isTrue);
      expect(client.state.isOpen, isTrue);
      client.close();
      async.elapse(const Duration(seconds: 10));
    });
  });

  test(
    'F10: connect() from a terminal-failure listener gets its own result',
    () {
      fake((async) {
        final server = FakeWebSocketServer(autoAccept: false);
        late WebSocketClient<String> client;
        Object? inner = 'pending';
        var retried = false;
        client = _text(
          server,
          reconnect: const WebSocketReconnect.disabled(),
          onStateChange: (s) {
            if (s is WebSocketState$Disconnected &&
                s.error != null &&
                !retried) {
              retried = true;
              server.autoAccept = true;
              client
                  .connect('ws://t')
                  .then<void>(
                    (_) => inner = null,
                    onError: (Object e) => inner = e,
                  );
            }
          },
        );
        client.connect('ws://t').ignore();
        async.flushMicrotasks();
        server.last.reject(503);
        async.elapse(const Duration(seconds: 1));
        expect(inner, isNull);
        expect(client.state.isOpen, isTrue);
        client.close();
      });
    },
  );

  group('rpc', () {
    test('F11: a duplicate id is refused, the first call survives', () async {
      final server = FakeWebSocketServer();
      final client = WebSocketClient.json(
        options: WebSocketOptions(transport: server, rpc: const _FixedIdRpc()),
      );
      await client.connect('ws://t');
      final first = client.rpc({'a': 1});
      await expectLater(client.rpc({'a': 2}), throwsArgumentError);
      server.last.sendJson({'id': 'same', 'ok': true});
      expect((await first)['ok'], isTrue);
      await client.close();
    });

    test('F12: server messages with an unknown id are delivered', () async {
      final server = FakeWebSocketServer();
      final client = WebSocketClient.json(
        options: WebSocketOptions(
          transport: server,
          rpc: const WebSocketJsonRpc(),
          rpcTimeout: const Duration(milliseconds: 50),
        ),
      );
      final got = <Object?>[];
      client.messages.listen(got.add);
      await client.connect('ws://t');
      server.last.sendJson({'type': 'user.updated', 'id': 42});
      await pumpEventQueue();
      expect(got, hasLength(1));
      // A late reply to a call that timed out is dropped and counted.
      await expectLater(
        client.rpc(const {}),
        throwsA(isA<WebSocketRpcException>()),
      );
      server.last.sendJson({'id': 1});
      await pumpEventQueue();
      expect(got, hasLength(1));
      expect(client.metrics.droppedReplies, 1);
      await client.close();
    });

    test(
      'F14: pending calls fail at disconnect(), not after the handshake',
      () {
        fake((async) {
          final server = FakeWebSocketServer();
          final client = WebSocketClient.json(
            options: WebSocketOptions(
              transport: server,
              rpc: const WebSocketJsonRpc(),
            ),
          );
          client.connect('ws://t');
          async.flushMicrotasks();
          server.last.echoClose = false;
          Object? error;
          client
              .rpc(const {})
              .then<void>((_) {}, onError: (Object e) => error = e);
          client.disconnect();
          async.flushMicrotasks();
          expect(
            error,
            isA<WebSocketRpcException>().having(
              (e) => e.failure,
              'failure',
              WebSocketRpcFailure.disconnected,
            ),
          );
          client.close();
          async.elapse(const Duration(seconds: 5));
        });
      },
    );
  });

  test('F13: invalid UTF-8 ends the connection even if the server stalls', () {
    fake((async) {
      final server = FakeWebSocketServer();
      final client = _text(
        server,
        reconnect: const WebSocketReconnect.disabled(),
      );
      client.connect('ws://t').ignore();
      async.flushMicrotasks();
      server.last
        ..echoClose = false
        ..sendTextBytes(Uint8List.fromList([0xFF]));
      async.elapse(const Duration(seconds: 5));
      final state = client.state as WebSocketState$Disconnected;
      expect(
        state.error,
        isA<WebSocketClosedException>().having((e) => e.code, 'code', 1007),
      );
      client.close();
    });
  });

  test('F17: a transport throwing from connect() is reported, not leaked', () {
    fake((async) {
      final zone = <Object>[];
      runZonedGuarded(() {
        final client = WebSocketClient.text(
          options: const WebSocketOptions(transport: _ThrowingTransport()),
        );
        Object? error;
        client
            .connect('ws://t')
            .then<void>((_) {}, onError: (Object e) => error = e);
        async.elapse(const Duration(seconds: 30));
        expect(error, isA<WebSocketConnectionException>());
        expect(client.state, isA<WebSocketState$Disconnected>());
        client.close();
      }, (e, _) => zone.add(e));
      expect(zone, isEmpty);
    });
  });

  test('F18: a request provider that never answers hits connectTimeout', () {
    fake((async) {
      final server = FakeWebSocketServer();
      final client = _text(
        server,
        reconnect: const WebSocketReconnect.disabled(),
        connectTimeout: const Duration(seconds: 2),
        request: (url, attempt) => Completer<WebSocketRequest>().future,
      );
      Object? error;
      client
          .connect('ws://t')
          .then<void>((_) {}, onError: (Object e) => error = e);
      async.elapse(const Duration(seconds: 3));
      expect(error, isA<WebSocketTimeoutException>());
      client.close();
    });
  });

  test('F18: an onOpen hook that never ends hits connectTimeout', () {
    fake((async) {
      final server = FakeWebSocketServer();
      final client = _text(
        server,
        reconnect: const WebSocketReconnect.disabled(),
        connectTimeout: const Duration(seconds: 2),
        onOpen: (_) => Completer<void>().future,
      );
      Object? error;
      client
          .connect('ws://t')
          .then<void>((_) {}, onError: (Object e) => error = e);
      async.elapse(const Duration(seconds: 3));
      expect(error, isA<WebSocketTimeoutException>());
      expect(server.last.isEnded, isTrue);
      client.close();
    });
  });

  test('F19: bytesSent counts encoded payloads', () async {
    final server = FakeWebSocketServer();
    final client = _text(server);
    await client.connect('ws://t');
    await client.push('abc');
    await client.push('é');
    expect(client.metrics.bytesSent, 5);
    await client.close();
  });

  test('F20: the fake delivers messages sent right before close', () async {
    final server = FakeWebSocketServer();
    final client = _text(server);
    final got = <String>[];
    client.messages.listen(got.add);
    await client.connect('ws://t');
    server.last
      ..sendText('bye')
      ..close();
    await pumpEventQueue();
    expect(got, ['bye']);
    await client.close();
  });

  test('F25: no timers are left after reconnect() then close()', () {
    fake((async) {
      final server = FakeWebSocketServer();
      final client = _text(server);
      client.connect('ws://t');
      async.flushMicrotasks();
      server.last.echoClose = false;
      client.reconnect().ignore();
      async.flushMicrotasks();
      client.close();
      async.flushMicrotasks();
      expect(async.pendingTimers, isEmpty);
    });
  });

  test('F26: a message is not streamed after onMessage disconnected', () async {
    final server = FakeWebSocketServer();
    late WebSocketClient<String> client;
    client = _text(server, onMessage: (_) => client.disconnect());
    final got = <String>[];
    client.messages.listen(got.add);
    await client.connect('ws://t');
    server.last.sendText('last');
    await pumpEventQueue();
    expect(got, isEmpty);
    await client.close();
  });

  test('Retry-After is honoured for 429/503', () {
    fake((async) {
      final server = FakeWebSocketServer(autoAccept: false);
      final client = _text(server);
      client.connect('ws://t').ignore();
      async.flushMicrotasks();
      server.last.reject(503, headers: const {'retry-after': '30'});
      async.flushMicrotasks();
      final state = client.state as WebSocketState$Reconnecting;
      expect(state.delay, greaterThanOrEqualTo(const Duration(seconds: 30)));
      client.close();
    });
  });

  test('invalid request headers are terminal, not retried', () {
    fake((async) {
      final server = FakeWebSocketServer();
      final client = WebSocketClient.text(
        options: WebSocketOptions(
          transport: server,
          vm: const WebSocketVmOptions(headers: {'Host': 'evil'}),
        ),
      );
      Object? error;
      client
          .connect('ws://t')
          .then<void>((_) {}, onError: (Object e) => error = e);
      async.elapse(const Duration(minutes: 1));
      expect(error, isA<WebSocketHandshakeException>());
      expect(server.connections, isEmpty);
      client.close();
    });
  });
}

final class _FixedIdRpc implements WebSocketRpc<Map<String, Object?>> {
  const _FixedIdRpc();
  @override
  (Object, Map<String, Object?>) encode(int seq, Map<String, Object?> data) =>
      ('same', {...data, 'id': 'same'});
  @override
  Object? match(Map<String, Object?> message) => message['id'];
  @override
  Map<String, Object?> decode(Map<String, Object?> reply) => reply;
}

final class _ThrowingTransport implements WebSocketTransport {
  const _ThrowingTransport();
  @override
  String get name => 'throwing';
  @override
  bool get supportsPing => true;
  @override
  bool get decodesJson => false;
  @override
  TransportConnection connect(TransportRequest r, TransportListener l) =>
      throw UnsupportedError('no sockets here');
}
