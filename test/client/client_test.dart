import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:fake_async/fake_async.dart';
import 'package:test/test.dart';
import 'package:ws/testing.dart';
import 'package:ws/ws.dart';

/// Runs [body] in fake time and flushes everything at the end.
void fake(void Function(FakeAsync async) body) => FakeAsync().run((async) {
  body(async);
  async.flushMicrotasks();
});

void main() {
  group('basics', () {
    test('connect, push, receive, close', () async {
      final server = FakeWebSocketServer();
      final client = WebSocketClient.text(
        options: WebSocketOptions(transport: server),
      );
      final states = <String>[];
      client.states.listen((s) => states.add(s.runtimeType.toString()));
      final received = <String>[];
      client.messages.listen(received.add);

      await client.connect('ws://test/socket');
      expect(client.state, isA<WebSocketState$Open>());
      expect(server.last.request.url, Uri.parse('ws://test/socket'));

      await client.push('héllo');
      expect(server.last.received, ['héllo']);

      server.last.sendText('back ✓');
      await pumpEventQueue();
      expect(received, ['back ✓']);

      await client.close();
      expect(client.state, isA<WebSocketState$Closed>());
      expect(server.last.clientClose?.code, 1000);
      expect(states, [
        'WebSocketState\$Connecting',
        'WebSocketState\$Open',
        'WebSocketState\$Disconnecting',
        'WebSocketState\$Disconnected',
        'WebSocketState\$Closed',
      ]);
      await expectLater(
        client.push('late'),
        throwsA(isA<WebSocketNotConnectedException>()),
      );
      await expectLater(client.connect('ws://test'), throwsStateError);
    });

    test('push while not connected fails fast', () async {
      final client = WebSocketClient.text(
        options: WebSocketOptions(transport: FakeWebSocketServer()),
      );
      await expectLater(
        client.push('x'),
        throwsA(isA<WebSocketNotConnectedException>()),
      );
    });

    test('http(s) URLs map to ws(s); other schemes are rejected', () async {
      final server = FakeWebSocketServer();
      final client = WebSocketClient.text(
        options: WebSocketOptions(transport: server),
      );
      await client.connect('https://h/p');
      expect(server.last.request.url.scheme, 'wss');
      expect(() => client.connect('ftp://h'), throwsArgumentError);
      await client.close();
    });

    test('binary client gets text as bytes', () async {
      final server = FakeWebSocketServer();
      final client = WebSocketClient.binary(
        options: WebSocketOptions(transport: server),
      );
      final got = <Uint8List>[];
      client.messages.listen(got.add);
      await client.connect('ws://t');
      server.last
        ..sendBinary(Uint8List.fromList([1, 2, 3]))
        ..sendText('ab');
      await pumpEventQueue();
      expect(got, [
        [1, 2, 3],
        [97, 98],
      ]);
      await client.push(Uint8List.fromList([9]));
      expect(server.last.received.single, [9]);
      await client.close();
    });

    test('close code and reason are validated like in browsers', () async {
      final client = WebSocketClient.text(
        options: WebSocketOptions(transport: FakeWebSocketServer()),
      );
      expect(() => client.disconnect(1001), throwsArgumentError);
      expect(() => client.disconnect(1000, 'x' * 124), throwsArgumentError);
      await client.disconnect(4000, 'ok');
    });
  });

  group('json', () {
    test('decode errors are reported, connection stays open', () async {
      final server = FakeWebSocketServer();
      final errors = <Object>[];
      final client = WebSocketClient.json(
        options: WebSocketOptions(
          transport: server,
          onError: (e, _) => errors.add(e),
        ),
      );
      final got = <Map<String, Object?>>[];
      client.messages.listen(got.add);
      await client.connect('ws://t');
      server.last
        ..sendText('{bad json')
        ..sendText('[1, 2]')
        ..sendJson({'ok': true});
      await pumpEventQueue();
      expect(got, [
        {'ok': true},
      ]);
      expect(errors, [
        isA<WebSocketFormatException>(),
        isA<WebSocketFormatException>(),
      ]);
      expect(client.metrics.decodeErrors, 2);
      expect(client.state.isOpen, isTrue);
      await client.push({'a': 1});
      expect(server.last.receivedJson.single, {'a': 1});
      await client.close();
    });

    test('invalid UTF-8 closes with 1007 and is not retried', () async {
      final server = FakeWebSocketServer();
      final client = WebSocketClient.json(
        options: WebSocketOptions(transport: server),
      );
      await client.connect('ws://t');
      server.last.sendTextBytes(Uint8List.fromList([0x7B, 0xFF, 0x7D]));
      await pumpEventQueue();
      expect(server.last.clientClose?.code, 1007);
      final state = client.state;
      expect(state, isA<WebSocketState$Disconnected>());
      expect(
        (state as WebSocketState$Disconnected).error,
        isA<WebSocketClosedException>().having((e) => e.code, 'code', 1007),
      );
      expect(server.connections, hasLength(1));
    });
  });

  group('rpc', () {
    WebSocketClient<Map<String, Object?>> rpcClient(
      FakeWebSocketServer server, {
      Duration timeout = const Duration(seconds: 5),
    }) => WebSocketClient.json(
      options: WebSocketOptions(
        transport: server,
        rpc: const WebSocketJsonRpc(),
        rpcTimeout: timeout,
      ),
    );

    test('replies are matched by id, pushes still flow', () async {
      final server = FakeWebSocketServer();
      final client = rpcClient(server);
      final pushes = <Object?>[];
      client.messages.listen(pushes.add);
      await client.connect('ws://t');
      server.connected.listen((_) {});
      server.last.messages.listen((m) {
        final request = jsonDecode(m as String) as Map<String, Object?>;
        server.last
          ..sendJson({'event': 'unrelated'})
          ..sendJson({
            'id': request['id'],
            'result': (request['n']! as int) * 2,
          });
      });
      final a = client.rpc({'n': 2});
      final b = client.rpc({'n': 5});
      expect((await a)['result'], 4);
      expect((await b)['result'], 10);
      expect(pushes, hasLength(2));
      await client.close();
    });

    test('deadlines: each call expires at its own time', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = rpcClient(server, timeout: const Duration(seconds: 5));
        client.connect('ws://t');
        async.flushMicrotasks();
        final failed = <String>[];
        void track(String name, Future<Object?> call) =>
            call.then((_) {}, onError: (Object _) => failed.add(name));
        track('a 5 s', client.rpc({'n': 1}));
        // An earlier deadline moves the timer forward.
        track(
          'b 1 s',
          client.rpc({'n': 2}, timeout: const Duration(seconds: 1)),
        );
        track(
          'c 3 s',
          client.rpc({'n': 3}, timeout: const Duration(seconds: 3)),
        );
        async.elapse(const Duration(milliseconds: 999));
        expect(failed, isEmpty);
        async.elapse(const Duration(milliseconds: 2));
        expect(failed, ['b 1 s']);
        // A reply does not touch the timer; it fires at 3 s for c only.
        final c = jsonDecode(server.last.received[2] as String) as Map;
        server.last.sendJson({'id': c['id'], 'result': 0});
        async.elapse(const Duration(seconds: 2));
        expect(failed, ['b 1 s']);
        async.elapse(const Duration(seconds: 2));
        expect(failed, ['b 1 s', 'a 5 s']);
        expect(
          async.pendingTimers.where(
            (t) =>
                t.duration > Duration.zero &&
                t.duration <= const Duration(seconds: 5),
          ),
          isEmpty,
          reason: 'no RPC timer once no call waits',
        );
        client.close();
      });
    });

    test('a burst of 20000 calls is linear (F23)', () async {
      // Real time, real Stopwatch: 32000 calls took 1.9 s to issue and
      // 2.4 s to answer when the timer was recomputed per call and reply.
      final server = FakeWebSocketServer();
      final client = rpcClient(server);
      await client.connect('ws://t');
      final watch = Stopwatch()..start();
      final calls = [
        for (var i = 0; i < 20000; i++) client.rpc({'n': i}),
      ];
      await Future<void>.delayed(Duration.zero);
      for (final m in server.last.received) {
        final id = (jsonDecode(m as String) as Map)['id'];
        server.last.sendJson({'id': id, 'result': 0});
      }
      await Future.wait(calls);
      expect(watch.elapsed, lessThan(const Duration(seconds: 1)));
      await client.close();
    });

    test('a real-time timeout expires on the monotonic clock', () async {
      final server = FakeWebSocketServer();
      final client = rpcClient(
        server,
        timeout: const Duration(milliseconds: 100),
      );
      await client.connect('ws://t');
      final watch = Stopwatch()..start();
      await expectLater(
        client.rpc({'n': 1}),
        throwsA(isA<WebSocketRpcException>()),
      );
      expect(
        watch.elapsed,
        greaterThanOrEqualTo(const Duration(milliseconds: 95)),
      );
      expect(watch.elapsed, lessThan(const Duration(seconds: 1)));
      await client.close();
    });

    test('error replies, timeouts, unknown replies, disconnect', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = rpcClient(server, timeout: const Duration(seconds: 2));
        client.connect('ws://t');
        async.flushMicrotasks();

        Object? error;
        client
            .rpc({'m': 1})
            .then<void>((_) {}, onError: (Object e) => error = e);
        async.flushMicrotasks();
        server.last.sendJson({'id': 1, 'error': 'nope'});
        async.flushMicrotasks();
        expect(
          error,
          isA<WebSocketRpcException>()
              .having((e) => e.failure, 'failure', WebSocketRpcFailure.remote)
              .having((e) => e.error, 'error', 'nope'),
        );

        Object? timeout;
        client
            .rpc({'m': 2})
            .then<void>((_) {}, onError: (Object e) => timeout = e);
        async.elapse(const Duration(seconds: 3));
        expect(
          timeout,
          isA<WebSocketRpcException>().having(
            (e) => e.failure,
            'failure',
            WebSocketRpcFailure.timeout,
          ),
        );
        server.last.sendJson({'id': 2, 'late': true});
        async.flushMicrotasks();
        expect(client.metrics.droppedReplies, 1);

        Object? lost;
        client
            .rpc({'m': 3})
            .then<void>((_) {}, onError: (Object e) => lost = e);
        async.flushMicrotasks();
        server.last.drop();
        async.flushMicrotasks();
        expect(
          lost,
          isA<WebSocketRpcException>().having(
            (e) => e.failure,
            'failure',
            WebSocketRpcFailure.disconnected,
          ),
        );
        client.close();
      });
    });

    test('rpc without a handler throws', () async {
      final client = WebSocketClient.text(
        options: WebSocketOptions(transport: FakeWebSocketServer()),
      );
      await client.connect('ws://t');
      expect(() => client.rpc('x'), throwsA(isA<AssertionError>()));
      await client.close();
    });
  });

  group('reconnect', () {
    test('reconnects with backoff after the connection drops', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            reconnect: const WebSocketReconnect(
              minDelay: Duration(milliseconds: 100),
              maxDelay: Duration(seconds: 1),
            ),
          ),
        );
        final states = <WebSocketState>[];
        client.states.listen(states.add);
        client.connect('ws://t');
        async.flushMicrotasks();
        expect(client.state.isOpen, isTrue);

        server.last.drop();
        async.flushMicrotasks();
        final reconnecting = client.state as WebSocketState$Reconnecting;
        expect(reconnecting.attempt, 1);
        expect(reconnecting.cause, isA<WebSocketConnectionException>());
        expect(reconnecting.delay.inMilliseconds, inInclusiveRange(100, 200));
        async.elapse(reconnecting.delay);
        expect(client.state.isOpen, isTrue);
        expect(server.connections, hasLength(2));
        expect(client.metrics.reconnectAttempts, 1);
        client.close();
      });
    });

    test('delays grow while attempts keep failing, reset when stable', () {
      fake((async) {
        final server = FakeWebSocketServer(autoAccept: false);
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            reconnect: const WebSocketReconnect(
              minDelay: Duration(milliseconds: 100),
              maxDelay: Duration(seconds: 10),
              stableAfter: Duration(seconds: 1),
            ),
          ),
        );
        client.connect('ws://t');
        for (var i = 1; i <= 4; i++) {
          async.flushMicrotasks();
          server.last.reject(503);
          async.flushMicrotasks();
          final s = client.state as WebSocketState$Reconnecting;
          expect(s.attempt, i);
          expect(
            s.delay.inMilliseconds,
            lessThanOrEqualTo(100 + 100 * (1 << (i - 1))),
          );
          async.elapse(s.delay);
        }
        server.last.accept();
        async.flushMicrotasks();
        expect(client.state.isOpen, isTrue);
        async.elapse(const Duration(seconds: 2)); // stable
        server.last.drop();
        async.flushMicrotasks();
        expect((client.state as WebSocketState$Reconnecting).attempt, 1);
        client.close();
      });
    });

    test('4xx handshake is terminal (except 408/425/429)', () {
      fake((async) {
        final server = FakeWebSocketServer(autoAccept: false);
        final client = WebSocketClient.text(
          options: WebSocketOptions(transport: server),
        );
        Object? error;
        client
            .connect('ws://t')
            .then<void>((_) {}, onError: (Object e) => error = e);
        async.flushMicrotasks();
        server.last.reject(429);
        async.flushMicrotasks();
        expect(client.state, isA<WebSocketState$Reconnecting>());
        async.elapse(const Duration(seconds: 2));
        server.last.reject(401);
        async.flushMicrotasks();
        expect(client.state, isA<WebSocketState$Disconnected>());
        expect(
          error,
          isA<WebSocketHandshakeException>().having(
            (e) => e.statusCode,
            'status',
            401,
          ),
        );
        async.elapse(const Duration(minutes: 1));
        expect(server.connections, hasLength(2));
      });
    });

    test('terminal close codes stop, others reconnect', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(transport: server),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.close(1001, 'restart');
        async.flushMicrotasks();
        expect(client.state, isA<WebSocketState$Reconnecting>());
        async.elapse(const Duration(seconds: 2));
        expect(client.state.isOpen, isTrue);
        server.last.close(1008, 'banned');
        async.flushMicrotasks();
        expect(client.state, isA<WebSocketState$Disconnected>());
        async.elapse(const Duration(minutes: 1));
        expect(server.connections, hasLength(2));
      });
    });

    test('maxAttempts and shouldReconnect', () {
      fake((async) {
        final server = FakeWebSocketServer(autoAccept: false);
        final causes = <int>[];
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            reconnect: WebSocketReconnect(
              minDelay: const Duration(milliseconds: 10),
              maxAttempts: 2,
              shouldReconnect: (cause, attempt) {
                causes.add(attempt);
                return true;
              },
            ),
          ),
        );
        client.connect('ws://t').ignore();
        for (var i = 0; i < 3; i++) {
          async.flushMicrotasks();
          server.last.reject(500);
          async.elapse(const Duration(seconds: 1));
        }
        expect(client.state, isA<WebSocketState$Disconnected>());
        expect(server.connections, hasLength(3));
        expect(causes, [1, 2]);
      });
    });

    test('request provider runs per attempt (token refresh)', () {
      fake((async) {
        final server = FakeWebSocketServer();
        var token = 0;
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            request: (url, attempt) async => WebSocketRequest(
              url.replace(queryParameters: {'token': '${++token}'}),
              headers: {'X-Attempt': '$attempt'},
            ),
          ),
        );
        client.connect('ws://t/s');
        async.flushMicrotasks();
        server.last.drop();
        async.elapse(const Duration(seconds: 2));
        expect(server.connections.map((c) => c.request.url.query), [
          'token=1',
          'token=2',
        ]);
        expect(server.last.request.headers['X-Attempt'], '1');
        client.close();
      });
    });

    test('reconnect() skips the wait', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            reconnect: const WebSocketReconnect(
              minDelay: Duration(seconds: 30),
              maxDelay: Duration(seconds: 60),
            ),
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.drop();
        async.flushMicrotasks();
        expect(client.state, isA<WebSocketState$Reconnecting>());
        client.reconnect();
        async.flushMicrotasks();
        expect(client.state.isOpen, isTrue);
        client.close();
      });
    });

    test('connect timeout is retried', () {
      fake((async) {
        final server = FakeWebSocketServer(autoAccept: false);
        final errors = <Object>[];
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            connectTimeout: const Duration(seconds: 3),
            onError: (e, _) => errors.add(e),
          ),
        );
        client.connect('ws://t');
        async.elapse(const Duration(seconds: 3));
        expect(errors.single, isA<WebSocketTimeoutException>());
        expect(client.state, isA<WebSocketState$Reconnecting>());
        client.close();
      });
    });
  });

  group('lifecycle', () {
    test('onOpen runs before open and can push', () async {
      final server = FakeWebSocketServer();
      final seen = <String>[];
      final client = WebSocketClient.text(
        options: WebSocketOptions(
          transport: server,
          onOpen: (c) async {
            await c.push('auth');
            seen.add('onOpen');
          },
          onStateChange: (s) {
            if (s.isOpen) seen.add('open');
          },
        ),
      );
      await client.connect('ws://t');
      expect(seen, ['onOpen', 'open']);
      expect(server.last.received, ['auth']);
      await client.close();
    });

    test('disconnect aborts when the server ignores the close', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            disconnectTimeout: const Duration(seconds: 3),
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.echoClose = false;
        var done = false;
        client.disconnect().then((_) => done = true);
        async.elapse(const Duration(seconds: 2));
        expect(done, isFalse);
        expect(client.state, isA<WebSocketState$Disconnecting>());
        async.elapse(const Duration(seconds: 2));
        expect(done, isTrue);
        expect(client.state, isA<WebSocketState$Disconnected>());
        async.elapse(const Duration(minutes: 1));
        expect(server.connections, hasLength(1));
      });
    });

    test('disconnect during reconnect wait and while connecting', () {
      fake((async) {
        final server = FakeWebSocketServer(autoAccept: false);
        final client = WebSocketClient.text(
          options: WebSocketOptions(transport: server),
        );
        Object? error;
        client
            .connect('ws://t')
            .then<void>((_) {}, onError: (Object e) => error = e);
        async.flushMicrotasks();
        client.disconnect();
        async.flushMicrotasks();
        expect(error, isA<WebSocketNotConnectedException>());
        expect(client.state, isA<WebSocketState$Disconnected>());
        expect(server.last.isEnded, isTrue);

        client.connect('ws://t').ignore();
        async.flushMicrotasks();
        server.last.reject(500);
        async.flushMicrotasks();
        expect(client.state, isA<WebSocketState$Reconnecting>());
        client.disconnect();
        async.elapse(const Duration(minutes: 1));
        expect(client.state, isA<WebSocketState$Disconnected>());
        expect(server.connections, hasLength(2));
      });
    });

    test('connect to another URL switches', () async {
      final server = FakeWebSocketServer();
      final client = WebSocketClient.text(
        options: WebSocketOptions(transport: server),
      );
      await client.connect('ws://a');
      await client.connect('ws://b');
      expect(server.connections.first.clientClose?.code, 1000);
      expect(server.last.request.url.host, 'b');
      expect(client.state.isOpen, isTrue);
      await client.close();
    });

    test('a throwing listener does not break the client', () async {
      final server = FakeWebSocketServer();
      final errors = <Object>[];
      final client = WebSocketClient.text(
        options: WebSocketOptions(
          transport: server,
          onMessage: (_) => throw StateError('boom'),
          onError: (e, _) => errors.add(e),
        ),
      );
      final got = <String>[];
      client.messages.listen(got.add);
      await client.connect('ws://t');
      server.last.sendText('x');
      await pumpEventQueue();
      expect(got, ['x']);
      expect(errors.single, isA<StateError>());
      await client.close();
    });
  });

  group('backpressure', () {
    test('push waits for the queue to drain above the high-water mark', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(transport: server, sendHighWaterMark: 100),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.bufferedAmount = 50;
        var first = false;
        client.push('a').then((_) => first = true);
        async.flushMicrotasks();
        expect(first, isTrue);

        server.last.bufferedAmount = 500;
        var second = false;
        client.push('b').then((_) => second = true);
        async.flushMicrotasks();
        expect(second, isFalse);
        server.last.drain();
        async.flushMicrotasks();
        expect(second, isTrue);

        server.last.bufferedAmount = 500;
        Object? lost;
        client.push('c').then<void>((_) {}, onError: (Object e) => lost = e);
        server.last.drop();
        async.flushMicrotasks();
        expect(lost, isA<WebSocketNotConnectedException>());
        client.close();
      });
    });
  });

  group('heartbeat', () {
    test('protocol ping when idle, reconnect when no answer', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            heartbeat: const WebSocketHeartbeat(
              interval: Duration(seconds: 10),
              timeout: Duration(seconds: 5),
            ),
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        final first = server.last;
        async.elapse(const Duration(seconds: 10));
        expect(first.pings, 1);
        async.elapse(const Duration(seconds: 6));
        expect(client.state.isOpen, isTrue); // pong arrived
        expect(client.metrics.lastRoundTrip, isNotNull);

        // Traffic postpones the ping.
        async.elapse(const Duration(seconds: 8));
        first.sendText('activity');
        async.elapse(const Duration(seconds: 8));
        expect(first.pings, 1);

        first.autoPong = false;
        async.elapse(const Duration(seconds: 20));
        expect(first.isEnded, isTrue);
        expect(server.connections.length, greaterThan(1));
        client.close();
      });
    });

    test('is on by default and can be disabled with null', () {
      expect(const WebSocketOptions<String>().heartbeat, isNotNull);
      expect(
        const WebSocketOptions<String>().heartbeat!.interval,
        const Duration(seconds: 30),
      );
      expect(const WebSocketOptions<String>(heartbeat: null).heartbeat, isNull);
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(transport: server),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.autoPong = false;
        async.elapse(const Duration(seconds: 41));
        expect(server.connections.first.pings, 1);
        expect(server.connections.first.isEnded, isTrue);
        client.close();
      });
    });

    test('a draining send queue extends the wait (slow upload)', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            heartbeat: const WebSocketHeartbeat(
              interval: Duration(seconds: 10),
              timeout: Duration(seconds: 5),
            ),
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        final conn = server.last..autoPong = false;
        // The ping is stuck behind a large queue that drains slowly.
        conn.bufferedAmount = 1000;
        async.elapse(const Duration(seconds: 10));
        expect(conn.pings, 1);
        for (var i = 0; i < 6; i++) {
          conn.bufferedAmount -= 100;
          async.elapse(const Duration(seconds: 5));
          expect(conn.isEnded, isFalse, reason: 'round $i');
        }
        // Pushing more is not progress by itself (the fake does not count
        // pushed bytes, a real queue does).
        client.push('x' * 50);
        conn.bufferedAmount += 50;
        async.elapse(const Duration(seconds: 5));
        expect(conn.isEnded, isTrue, reason: 'the queue stopped draining');
        client.close();
      });
    });

    test('a stalled send queue is a dead connection', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            heartbeat: const WebSocketHeartbeat(
              interval: Duration(seconds: 10),
              timeout: Duration(seconds: 5),
            ),
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        final conn = server.last
          ..autoPong = false
          ..bufferedAmount = 1000;
        async.elapse(const Duration(seconds: 16));
        expect(conn.isEnded, isTrue);
        client.close();
      });
    });

    test('application-level ping and pong', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            transport: server,
            heartbeat: WebSocketHeartbeat(
              interval: const Duration(seconds: 10),
              ping: () => 'ping',
              isPong: (m) => m == 'pong',
            ),
          ),
        );
        final got = <String>[];
        client.messages.listen(got.add);
        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.messages.listen((m) {
          if (m == 'ping') server.last.sendText('pong');
        });
        var ponged = false;
        client.ping().then((_) => ponged = true);
        async.flushMicrotasks();
        expect(ponged, isTrue);
        expect(got, isEmpty);
        expect(server.last.pings, 0);
        client.close();
      });
    });
  });
}
