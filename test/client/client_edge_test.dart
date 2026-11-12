// Edge cases of WebSocketClient over the fake transport (TEST_PLAN C4–C25).
import 'dart:async';
import 'dart:convert';

import 'package:fake_async/fake_async.dart';
import 'package:test/test.dart';
import 'package:ws/testing.dart';
import 'package:ws/ws.dart';

void fake(void Function(FakeAsync async) body) => FakeAsync().run((async) {
  body(async);
  async.flushMicrotasks();
});

void main() {
  group('idempotence and closed client (C4, C5)', () {
    test('disconnect and close repeat safely', () async {
      final server = FakeWebSocketServer();
      final client = WebSocketClient.text(
        options: WebSocketOptions(transport: server),
      );
      for (var i = 0; i < 10; i++) {
        await client.disconnect();
      }
      expect(client.state, isA<WebSocketState$Disconnected>());
      await client.connect('ws://t');
      await Future.wait([client.disconnect(), client.disconnect()]);
      expect(server.connections, hasLength(1));
      await client.close();
      await client.close();
      await client.disconnect();
      expect(client.state.isClosed, isTrue);
    });

    test(
      'every operation on a closed client fails with a typed error',
      () async {
        final client = WebSocketClient.json(
          options: WebSocketOptions(
            transport: FakeWebSocketServer(),
            rpc: const WebSocketJsonRpc(),
          ),
        );
        await client.close();
        await expectLater(client.connect('ws://t'), throwsStateError);
        await expectLater(client.reconnect(), throwsStateError);
        await expectLater(
          client.push(const {}),
          throwsA(isA<WebSocketNotConnectedException>()),
        );
        await expectLater(
          client.rpc(const {}),
          throwsA(
            isA<WebSocketRpcException>().having(
              (e) => e.failure,
              'failure',
              WebSocketRpcFailure.closed,
            ),
          ),
        );
        await expectLater(
          client.ping(),
          throwsA(isA<WebSocketNotConnectedException>()),
        );
        expect(await client.states.isEmpty, isTrue);
        expect(await client.messages.isEmpty, isTrue);
      },
    );

    test('reconnect() before connect() fails', () async {
      final client = WebSocketClient.text(
        options: WebSocketOptions(transport: FakeWebSocketServer()),
      );
      await expectLater(
        client.reconnect(),
        throwsA(isA<WebSocketNotConnectedException>()),
      );
      await client.close();
    });

    test('close validation (C26)', () async {
      final client = WebSocketClient.text(
        options: WebSocketOptions(transport: FakeWebSocketServer()),
      );
      for (final code in [0, 999, 1001, 1004, 1005, 1006, 2999, 5000]) {
        expect(
          () => client.disconnect(code),
          throwsArgumentError,
          reason: '$code',
        );
      }
      await client.disconnect(3000);
      await client.disconnect(4999, 'é' * 61 + 'x'); // 123 bytes
      expect(() => client.disconnect(1000, 'é' * 62), throwsArgumentError);
      await client.close();
    });
  });

  group('connect while connecting (C6, C7)', () {
    test('same URL joins the attempt; close while connecting', () {
      fake((async) {
        final server = FakeWebSocketServer(autoAccept: false);
        final client = WebSocketClient.text(
          options: WebSocketOptions(transport: server),
        );
        Object? first;
        Object? second;
        client
            .connect('ws://t')
            .then<void>((_) {}, onError: (Object e) => first = e);
        async.flushMicrotasks();
        client
            .connect('ws://t')
            .then<void>((_) {}, onError: (Object e) => second = e);
        async.flushMicrotasks();
        expect(server.connections, hasLength(1));
        client.close();
        async.flushMicrotasks();
        expect(first, isA<WebSocketNotConnectedException>());
        expect(second, isA<WebSocketNotConnectedException>());
        expect(server.last.isEnded, isTrue);
        expect(client.state.isClosed, isTrue);
      });
    });

    test('close during the reconnect wait cancels the timer', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(transport: server),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.drop();
        async.flushMicrotasks();
        expect(client.state, isA<WebSocketState$Reconnecting>());
        client.close();
        async.elapse(const Duration(minutes: 1));
        expect(server.connections, hasLength(1));
        expect(async.pendingTimers, isEmpty);
      });
    });
  });

  group('reconnect policy edges (C8–C11)', () {
    test('close 1013 waits maxDelay', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            reconnect: const WebSocketReconnect(
              minDelay: Duration(milliseconds: 100),
              maxDelay: Duration(seconds: 7),
            ),
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.close(1013, 'try again later');
        async.flushMicrotasks();
        final state = client.state as WebSocketState$Reconnecting;
        // maxDelay plus jitter, so shed clients do not return together.
        expect(state.delay, greaterThanOrEqualTo(const Duration(seconds: 7)));
        expect(state.delay, lessThan(const Duration(milliseconds: 10500)));
        client.close();
      });
    });

    test('a throwing request provider is reported and retried', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final errors = <Object>[];
        var calls = 0;
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            onError: (e, _) => errors.add(e),
            request: (url, attempt) {
              if (calls++ == 0) throw StateError('token service down');
              return WebSocketRequest(url, headers: const {'auth': 'ok'});
            },
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        expect(server.connections, isEmpty);
        expect(
          errors.single,
          isA<WebSocketConnectionException>().having(
            (e) => e.cause,
            'cause',
            isStateError,
          ),
        );
        expect(client.state, isA<WebSocketState$Reconnecting>());
        async.elapse(const Duration(seconds: 2));
        expect(client.state.isOpen, isTrue);
        expect(server.last.request.headers['auth'], 'ok');
        client.close();
      });
    });

    test('a throwing onOpen drops the connection and reports the cause', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final errors = <Object>[];
        var opens = 0;
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            onError: (e, _) => errors.add(e),
            onOpen: (_) {
              if (opens++ == 0) throw StateError('auth failed');
            },
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        // Reported once, wrapped, with the cause.
        expect(
          errors.single,
          isA<WebSocketConnectionException>().having(
            (e) => e.cause,
            'cause',
            isStateError,
          ),
        );
        expect(server.connections.first.isEnded, isTrue);
        async.elapse(const Duration(seconds: 2));
        expect(client.state.isOpen, isTrue);
        expect(server.connections, hasLength(2));
        client.close();
      });
    });

    test('a throwing shouldReconnect means no', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final errors = <Object>[];
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            onError: (e, _) => errors.add(e),
            reconnect: WebSocketReconnect(
              shouldReconnect: (_, _) => throw StateError('policy bug'),
            ),
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.drop();
        async.flushMicrotasks();
        expect(errors.whereType<StateError>(), hasLength(1));
        final state = client.state as WebSocketState$Disconnected;
        expect(state.error, isA<WebSocketConnectionException>());
        expect(client.metrics.lastError, same(state.error));
        client.close();
      });
    });

    test('disabled reconnect stops after the first failure', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            reconnect: const WebSocketReconnect.disabled(),
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.drop();
        async.elapse(const Duration(minutes: 1));
        expect(server.connections, hasLength(1));
        expect(client.state, isA<WebSocketState$Disconnected>());
        client.close();
      });
    });
  });

  group('session identity (C13, C14)', () {
    test('events of a replaced connection are ignored', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(transport: server),
        );
        final got = <String>[];
        client.messages.listen(got.add);
        client.connect('ws://a');
        async.flushMicrotasks();
        final old = server.last;
        client.connect('ws://b');
        async.flushMicrotasks();
        expect(server.connections, hasLength(2));
        // The old connection is closing; whatever it still says is dropped.
        if (old.isOpen) old.sendText('stale');
        server.last.sendText('fresh');
        async.flushMicrotasks();
        expect(got, ['fresh']);
        expect(client.state.isOpen, isTrue);
        client.close();
      });
    });

    test('nothing reaches messages after disconnect()', () {
      fake((async) {
        final server = FakeWebSocketServer()..autoAccept = true;
        final client = WebSocketClient.text(
          options: WebSocketOptions(transport: server),
        );
        final got = <String>[];
        client.messages.listen(got.add);
        client.connect('ws://t');
        async.flushMicrotasks();
        final connection = server.last..echoClose = false;
        connection.sendText('before');
        async.flushMicrotasks();
        client.disconnect();
        if (connection.isOpen) connection.sendText('after');
        async.elapse(const Duration(seconds: 5));
        expect(got, ['before']);
        client.close();
      });
    });
  });

  group('send path (C15, sendBufferLimit)', () {
    test(
      'an unencodable value fails push, the connection stays open',
      () async {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.json(
          options: WebSocketOptions(transport: server),
        );
        await client.connect('ws://t');
        await expectLater(
          client.push({'x': Object()}),
          throwsA(isA<WebSocketFormatException>()),
        );
        expect(client.state.isOpen, isTrue);
        await client.push({'ok': true});
        expect(server.last.receivedJson.single, {'ok': true});
        await client.close();
      },
    );

    test('push fails fast once the queue is over sendBufferLimit', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            sendHighWaterMark: 10,
            sendBufferLimit: 100,
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.bufferedAmount = 100; // at the limit: still queued
        var queued = false;
        client.push('a').then((_) => queued = true);
        server.last.bufferedAmount = 101;
        Object? refused;
        client.push('b').then<void>((_) {}, onError: (Object e) => refused = e);
        async.flushMicrotasks();
        expect(refused, isA<WebSocketBufferFullException>());
        expect((refused! as WebSocketBufferFullException).buffered, 101);
        expect(server.last.received, ['a']);
        expect(client.state.isOpen, isTrue);
        server.last.drain();
        async.flushMicrotasks();
        expect(queued, isTrue);
        client.close();
      });
    });

    test('proxy and TLS settings reach the transport', () async {
      final server = FakeWebSocketServer();
      final proxy = WebSocketProxy.parse('http://u:p@proxy:8080');
      final client = WebSocketClient.text(
        options: WebSocketOptions(
          transport: server,
          vm: WebSocketVmOptions(
            proxy: proxy,
            tls: const WebSocketTls(allowInsecure: true),
          ),
        ),
      );
      await client.connect('wss://t');
      expect(server.last.request.proxy, same(proxy));
      expect(server.last.request.tls.allowInsecure, isTrue);
      await client.close();
    });
  });

  group('rpc edges (C17)', () {
    WebSocketClient<Map<String, Object?>> rpcClient(
      FakeWebSocketServer server,
      WebSocketRpc<Map<String, Object?>> rpc, {
      void Function(Object, StackTrace)? onError,
    }) => WebSocketClient.json(
      options: WebSocketOptions(transport: server, rpc: rpc, onError: onError),
    );

    test('100 concurrent calls answered in reverse order', () async {
      final server = FakeWebSocketServer();
      final client = rpcClient(server, const WebSocketJsonRpc());
      await client.connect('ws://t');
      final calls = [
        for (var i = 0; i < 100; i++) client.rpc({'n': i}),
      ];
      final requests = server.last.receivedJson
          .cast<Map<String, Object?>>()
          .toList()
          .reversed;
      for (final r in requests) {
        server.last.sendJson({'id': r['id'], 'n': r['n']});
      }
      final replies = await Future.wait(calls);
      expect([for (final r in replies) r['n']], List.generate(100, (i) => i));
      expect(client.metrics.pendingRpc, 0);
      await client.close();
    });

    test('rpc while not connected', () async {
      final client = rpcClient(FakeWebSocketServer(), const WebSocketJsonRpc());
      await expectLater(
        client.rpc(const {}),
        throwsA(
          isA<WebSocketRpcException>().having(
            (e) => e.failure,
            'failure',
            WebSocketRpcFailure.disconnected,
          ),
        ),
      );
      await client.close();
    });

    test('encode, match and decode throwing', () async {
      final server = FakeWebSocketServer();
      final errors = <Object>[];
      final rpc = _ThrowingRpc();
      final client = rpcClient(server, rpc, onError: (e, _) => errors.add(e));
      final pushes = <Object?>[];
      client.messages.listen(pushes.add);
      await client.connect('ws://t');

      rpc.failEncode = true;
      await expectLater(client.rpc(const {}), throwsStateError);
      rpc.failEncode = false;

      final call = client.rpc({'x': 1});
      rpc.failMatch = true;
      server.last.sendJson({'id': 2});
      await pumpEventQueue();
      expect(errors.single, isA<FormatException>());
      expect(pushes, hasLength(1), reason: 'unmatched: delivered as a push');
      rpc.failMatch = false;

      rpc.failDecode = true;
      // encode() threw after taking id 1, so this call is id 2.
      server.last.sendJson({'id': 2});
      await expectLater(
        call,
        throwsA(
          isA<WebSocketRpcException>().having(
            (e) => e.failure,
            'failure',
            WebSocketRpcFailure.remote,
          ),
        ),
      );
      await client.close();
    });

    test('a send refused by the buffer limit fails with that error', () async {
      final server = FakeWebSocketServer();
      final client = WebSocketClient.json(
        options: WebSocketOptions(
          transport: server,
          rpc: const WebSocketJsonRpc(),
          sendBufferLimit: 0,
        ),
      );
      await client.connect('ws://t');
      server.last.bufferedAmount = 1;
      await expectLater(
        client.rpc(const {}),
        throwsA(isA<WebSocketBufferFullException>()),
      );
      expect(client.metrics.pendingRpc, 0);
      await client.close();
    });

    test('pending calls fail with closed on close()', () async {
      final server = FakeWebSocketServer();
      final client = rpcClient(server, const WebSocketJsonRpc());
      await client.connect('ws://t');
      final call = client.rpc(const {});
      unawaited(client.close());
      await expectLater(
        call,
        throwsA(
          isA<WebSocketRpcException>().having(
            (e) => e.failure,
            'failure',
            WebSocketRpcFailure.closed,
          ),
        ),
      );
    });
  });

  group('ping and heartbeat (C18–C21)', () {
    test('manual ping: not connected, timeout, shared pong', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(transport: server),
        );
        Object? early;
        client.ping().then<void>((_) {}, onError: (Object e) => early = e);
        async.flushMicrotasks();
        expect(early, isA<WebSocketNotConnectedException>());

        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.autoPong = false;
        Object? timedOut;
        client
            .ping(timeout: const Duration(seconds: 1))
            .then<void>((_) {}, onError: (Object e) => timedOut = e);
        async.elapse(const Duration(seconds: 2));
        expect(timedOut, isA<WebSocketTimeoutException>());

        var done = 0;
        client.ping().then((_) => done++);
        client.ping().then((_) => done++);
        async.flushMicrotasks();
        server.last.pong();
        async.flushMicrotasks();
        expect(done, 2);
        expect(client.metrics.lastRoundTrip, isNotNull);
        client.close();
      });
    });

    test('throwing ping builder and isPong are reported', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final errors = <Object>[];
        final got = <String>[];
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            onError: (e, _) => errors.add(e),
            heartbeat: WebSocketHeartbeat(
              interval: const Duration(seconds: 1),
              timeout: const Duration(seconds: 30),
              ping: () => throw StateError('ping'),
              isPong: (m) => m == 'boom' ? throw StateError('isPong') : false,
            ),
          ),
        );
        client.messages.listen(got.add);
        client.connect('ws://t');
        async.flushMicrotasks();
        async.elapse(const Duration(milliseconds: 1500));
        server.last.sendText('boom');
        async.flushMicrotasks();
        expect(errors.whereType<StateError>().map((e) => e.message), [
          'ping',
          'isPong',
        ]);
        expect(got, ['boom'], reason: 'treated as a normal message');
        client.close();
      });
    });

    test('no timers left after disconnect and close', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.json(
          options: WebSocketOptions(
            transport: server,
            rpc: const WebSocketJsonRpc(),
            heartbeat: const WebSocketHeartbeat(interval: Duration(seconds: 1)),
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        client.rpc(const {}).ignore();
        async.elapse(const Duration(seconds: 3));
        client.close();
        async.elapse(const Duration(seconds: 10));
        expect(async.pendingTimers, isEmpty);
      });
    });
  });

  test('without onError, user-code errors go to the zone (C25)', () async {
    final zoneErrors = <Object>[];
    await runZonedGuarded(() async {
      final server = FakeWebSocketServer();
      final client = WebSocketClient.text(
        options: WebSocketOptions(
          transport: server,
          onMessage: (m) => m == 'bad' ? throw StateError('listener') : null,
        ),
      );
      final got = <String>[];
      client.messages.listen(got.add);
      await client.connect('ws://t');
      server.last
        ..sendText('bad')
        ..sendText('good');
      await pumpEventQueue();
      expect(got, ['bad', 'good']);
      expect(client.state.isOpen, isTrue);
      await client.close();
    }, (e, _) => zoneErrors.add(e));
    expect(zoneErrors.whereType<StateError>(), hasLength(1));
  });

  test('throwing onError goes to the zone', () async {
    final zoneErrors = <Object>[];
    await runZonedGuarded(() async {
      final server = FakeWebSocketServer();
      final client = WebSocketClient.json(
        options: WebSocketOptions(
          transport: server,
          onError: (e, _) => throw StateError('onError'),
        ),
      );
      await client.connect('ws://t');
      server.last.sendText('{not json');
      await pumpEventQueue();
      await client.close();
    }, (e, _) => zoneErrors.add(e));
    expect(
      zoneErrors.whereType<StateError>().map((e) => e.message),
      contains('onError'),
    );
  });

  test(
    'json codec: binary and non-object messages are decode errors (C16)',
    () async {
      final server = FakeWebSocketServer();
      final errors = <Object>[];
      final client = WebSocketClient.json(
        options: WebSocketOptions(
          transport: server,
          onError: (e, _) => errors.add(e),
        ),
      );
      final got = <Object?>[];
      client.messages.listen(got.add);
      await client.connect('ws://t');
      server.last
        ..sendBinary(utf8.encode('{"a":1}'))
        ..sendText('[1,2]')
        ..sendText('{oops')
        ..sendText('{"ok":true}');
      await pumpEventQueue();
      expect(got, [
        {'ok': true},
      ]);
      expect(errors, everyElement(isA<WebSocketFormatException>()));
      expect(errors, hasLength(3));
      expect(client.metrics.decodeErrors, 3);
      expect(client.state.isOpen, isTrue);
      await client.close();
    },
  );
}

final class _ThrowingRpc implements WebSocketRpc<Map<String, Object?>> {
  bool failEncode = false;
  bool failMatch = false;
  bool failDecode = false;

  @override
  (Object, Map<String, Object?>) encode(int seq, Map<String, Object?> data) {
    if (failEncode) throw StateError('encode');
    return (seq, {...data, 'id': seq});
  }

  @override
  Object? match(Map<String, Object?> message) {
    if (failMatch) throw const FormatException('match');
    return message['id'];
  }

  @override
  Map<String, Object?> decode(Map<String, Object?> reply) {
    if (failDecode) throw StateError('decode');
    return reply;
  }
}
