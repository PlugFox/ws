// suspend() and resume(): DECISIONS.md D44, docs/review/future_risks.md
// 2.1, 2.5, 3.2 and 3.3.
import 'package:fake_async/fake_async.dart';
import 'package:test/test.dart';
import 'package:ws/testing.dart';
import 'package:ws/ws.dart';

void fake(void Function(FakeAsync async) body) => FakeAsync().run((async) {
  body(async);
  async.flushMicrotasks();
});

const _heartbeat = WebSocketHeartbeat<String>(
  interval: Duration(seconds: 10),
  timeout: Duration(seconds: 5),
);

WebSocketClient<String> _client(
  FakeWebSocketServer server, {
  WebSocketHeartbeat<String>? heartbeat = _heartbeat,
  List<WebSocketState>? states,
  List<Object>? errors,
  WebSocketRpc<String>? rpc,
}) => WebSocketClient.text(
  options: WebSocketOptions(
    transport: server,
    heartbeat: heartbeat,
    onStateChange: states?.add,
    onError: errors == null ? null : (e, _) => errors.add(e),
    rpc: rpc,
  ),
);

void main() {
  group('suspend()', () {
    test('keeps an open connection and stops the heartbeat', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = _client(server);
        client.connect('ws://t');
        async.flushMicrotasks();
        final conn = server.last..autoPong = false;
        client.suspend();
        // A frozen app: the heartbeat would have dropped it twice over.
        async.elapse(const Duration(minutes: 5));
        expect(conn.pings, 0);
        expect(conn.isOpen, isTrue);
        expect(client.state.isOpen, isTrue);
        // Messages still arrive (Android keeps running for a while).
        final got = <String>[];
        client.messages.listen(got.add);
        conn.sendText('hi');
        async.flushMicrotasks();
        expect(got, ['hi']);
        client.close();
      });
    });

    test('stops a reconnect wait; nothing is retried until resume()', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final states = <WebSocketState>[];
        final client = _client(server, states: states);
        var connected = false;
        client.connect('ws://t');
        async.flushMicrotasks();
        server
          ..autoAccept = false
          ..last.drop();
        async.flushMicrotasks();
        expect(client.state, isA<WebSocketState$Reconnecting>());
        client.suspend();
        expect(client.state, isA<WebSocketState$Suspended>());
        expect((client.state as WebSocketState$Suspended).cause, isNull);
        async.elapse(const Duration(hours: 1));
        expect(server.connections, hasLength(1));

        client.connect('ws://t').then((_) => connected = true);
        async.flushMicrotasks();
        // connect() to the same URL is resume(): attempt 0, no delay.
        expect(server.connections, hasLength(2));
        expect(
          states.last,
          isA<WebSocketState$Connecting>().having((s) => s.attempt, 'att', 0),
        );
        server.last.accept();
        async.flushMicrotasks();
        expect(connected, isTrue);
        client.close();
      });
    });

    test('a connection lost while suspended is not retried', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final errors = <Object>[];
        final client = _client(server, errors: errors);
        client.connect('ws://t');
        async.flushMicrotasks();
        client.suspend();
        server.last.drop();
        async.flushMicrotasks();
        expect(
          client.state,
          isA<WebSocketState$Suspended>().having(
            (s) => s.cause,
            'cause',
            isA<WebSocketConnectionException>(),
          ),
        );
        expect(errors, [isA<WebSocketConnectionException>()]);
        async.elapse(const Duration(hours: 1));
        expect(server.connections, hasLength(1));
        expect(client.metrics.reconnectAttempts, 0);

        var resumed = false;
        client.resume().then((_) => resumed = true);
        async.flushMicrotasks();
        expect(server.connections, hasLength(2));
        expect(resumed, isTrue);
        client.close();
      });
    });

    test('a terminal close while suspended still stops the client', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = _client(server);
        client.connect('ws://t');
        async.flushMicrotasks();
        client.suspend();
        server.last.close(1008, 'policy');
        async.flushMicrotasks();
        expect(
          client.state,
          isA<WebSocketState$Disconnected>().having(
            (s) => s.error,
            'error',
            isA<WebSocketClosedException>(),
          ),
        );
        client.resume();
        async.flushMicrotasks();
        expect(server.connections, hasLength(1));
        client.close();
      });
    });

    test('disconnect: true closes with 1000 and fails pending calls', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = _client(server, rpc: const _Rpc());
        client.connect('ws://t');
        async.flushMicrotasks();
        final conn = server.last;
        Object? rpcError;
        client.rpc('x').then<void>((_) {}, onError: (Object e) => rpcError = e);
        async.flushMicrotasks();
        client.suspend(disconnect: true);
        async.flushMicrotasks();
        expect(conn.clientClose, (code: 1000, reason: 'Suspended'));
        expect(conn.isEnded, isTrue);
        expect(client.state, const TypeMatcher<WebSocketState$Suspended>());
        expect(
          rpcError,
          isA<WebSocketRpcException>().having(
            (e) => e.failure,
            'failure',
            WebSocketRpcFailure.disconnected,
          ),
        );
        async.elapse(const Duration(hours: 1));
        expect(server.connections, hasLength(1));
        client.resume();
        async.flushMicrotasks();
        expect(server.connections, hasLength(2));
        expect(client.state.isOpen, isTrue);
        client.close();
      });
    });

    test('an attempt that opens while suspended starts no heartbeat', () {
      fake((async) {
        final server = FakeWebSocketServer(autoAccept: false);
        final client = _client(server);
        client.connect('ws://t');
        async.flushMicrotasks();
        client.suspend();
        server.last
          ..autoPong = false
          ..accept();
        async.flushMicrotasks();
        expect(client.state.isOpen, isTrue);
        async.elapse(const Duration(minutes: 5));
        expect(server.last.pings, 0);
        expect(server.last.isOpen, isTrue);
        client.close();
      });
    });

    test('before connect() and after close() it changes nothing', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final states = <WebSocketState>[];
        final client = _client(server, states: states)..suspend();
        var done = false;
        client.resume().then((_) => done = true);
        async.flushMicrotasks();
        expect(done, isTrue);
        expect(states, isEmpty);
        expect(server.connections, isEmpty);

        // suspend() is forgotten by connect().
        client.suspend();
        client.connect('ws://t');
        async.flushMicrotasks();
        expect(client.state.isOpen, isTrue);
        final conn = server.last..autoPong = false;
        async.elapse(const Duration(seconds: 16));
        expect(conn.pings, 1, reason: 'the heartbeat runs');
        expect(conn.isEnded, isTrue);

        client.close();
        async.flushMicrotasks();
        client.suspend();
        done = false;
        client.resume().then((_) => done = true);
        async.flushMicrotasks();
        expect(done, isTrue);
        expect(client.state.isClosed, isTrue);
      });
    });
  });

  group('resume()', () {
    test('probes an open connection once, then the heartbeat runs', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = _client(server);
        client.connect('ws://t');
        async.flushMicrotasks();
        final conn = server.last;
        client.suspend();
        async.elapse(const Duration(minutes: 5));
        var resumed = false;
        client.resume().then((_) => resumed = true);
        async.flushMicrotasks();
        expect(conn.pings, 1);
        expect(resumed, isTrue, reason: 'the pong arrived');
        expect(server.connections, hasLength(1));
        // The heartbeat is back.
        async.elapse(const Duration(seconds: 10));
        expect(conn.pings, 2);
        client.close();
      });
    });

    test('a dead connection is replaced at once, without backoff', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final states = <WebSocketState>[];
        final client = _client(server, states: states);
        client.connect('ws://t');
        async.flushMicrotasks();
        final dead = server.last..autoPong = false;
        client.suspend();
        async.elapse(const Duration(minutes: 5));
        var resumed = false;
        client.resume().then((_) => resumed = true);
        async.elapse(const Duration(milliseconds: 4999));
        expect(dead.isOpen, isTrue);
        expect(resumed, isFalse);
        async.elapse(const Duration(milliseconds: 1));
        expect(dead.isEnded, isTrue);
        expect(server.connections, hasLength(2));
        expect(resumed, isTrue);
        expect(
          states.whereType<WebSocketState$Reconnecting>().single,
          isA<WebSocketState$Reconnecting>()
              .having((s) => s.delay, 'delay', Duration.zero)
              .having(
                (s) => s.cause,
                'cause',
                isA<WebSocketTimeoutException>(),
              ),
        );
        client.close();
      });
    });

    test('a connection lost during the probe is replaced at once', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final states = <WebSocketState>[];
        final client = _client(server, states: states);
        client.connect('ws://t');
        async.flushMicrotasks();
        server.last.autoPong = false;
        var resumed = false;
        client
          ..suspend()
          ..resume().then((_) => resumed = true);
        async.flushMicrotasks();
        server.last.drop(); // the write hit a reset
        async.elapse(Duration.zero);
        expect(server.connections, hasLength(2));
        expect(resumed, isTrue);
        expect(
          states.whereType<WebSocketState$Reconnecting>().single.delay,
          Duration.zero,
        );
        // Later losses use the backoff again.
        server.last.drop();
        async.flushMicrotasks();
        expect(
          (client.state as WebSocketState$Reconnecting).delay,
          greaterThan(Duration.zero),
        );
        client.close();
      });
    });

    test('any message from the server answers the probe', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = _client(server);
        client.connect('ws://t');
        async.flushMicrotasks();
        final conn = server.last..autoPong = false;
        client.suspend();
        var resumed = false;
        client.resume().then((_) => resumed = true);
        async.elapse(const Duration(seconds: 1));
        expect(resumed, isFalse);
        conn.sendText('backlog');
        async.flushMicrotasks();
        expect(resumed, isTrue);
        async.elapse(const Duration(seconds: 5));
        expect(conn.isOpen, isTrue);
        client.close();
      });
    });

    test('a draining upload extends the probe', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = _client(server);
        client.connect('ws://t');
        async.flushMicrotasks();
        final conn = server.last
          ..autoPong = false
          ..bufferedAmount = 1000;
        client
          ..suspend()
          ..resume();
        for (var i = 0; i < 4; i++) {
          conn.bufferedAmount -= 200;
          async.elapse(const Duration(seconds: 5));
          expect(conn.isOpen, isTrue, reason: 'round $i');
        }
        async.elapse(const Duration(seconds: 5));
        expect(conn.isEnded, isTrue, reason: 'the queue stopped draining');
        client.close();
      });
    });

    test('uses the application ping; without any ping it trusts', () {
      fake((async) {
        final server = FakeWebSocketServer(supportsPing: false);
        final client = _client(
          server,
          heartbeat: WebSocketHeartbeat<String>(
            timeout: const Duration(seconds: 5),
            ping: () => 'ping',
            isPong: (m) => m == 'pong',
          ),
        );
        client.connect('ws://t');
        async.flushMicrotasks();
        final conn = server.last;
        client
          ..suspend()
          ..resume();
        async.flushMicrotasks();
        expect(conn.received, ['ping']);
        conn.sendText('pong');
        async.elapse(const Duration(seconds: 6));
        expect(conn.isOpen, isTrue);
        client.close();

        // A browser without an application ping: nothing to probe with.
        final web = FakeWebSocketServer(supportsPing: false);
        final plain = _client(web);
        plain.connect('ws://t');
        async.flushMicrotasks();
        var resumed = false;
        plain
          ..suspend()
          ..resume().then((_) => resumed = true);
        async.elapse(const Duration(minutes: 1));
        expect(resumed, isTrue);
        expect(web.connections, hasLength(1));
        expect(web.last.received, isEmpty);
        plain.close();
      });
    });

    test('with the heartbeat off, probes with a 10 s timeout', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = _client(server, heartbeat: null);
        client.connect('ws://t');
        async.flushMicrotasks();
        final conn = server.last..autoPong = false;
        client.resume();
        async.elapse(const Duration(milliseconds: 9999));
        expect(conn.pings, 1);
        expect(conn.isOpen, isTrue);
        async.elapse(const Duration(milliseconds: 1));
        expect(conn.isEnded, isTrue);
        expect(server.connections, hasLength(2));
        // No heartbeat on the new connection either.
        server.last.autoPong = false;
        async.elapse(const Duration(minutes: 5));
        expect(server.last.pings, 0);
        client.close();
      });
    });

    test('resets the backoff and skips the reconnect wait', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final states = <WebSocketState>[];
        final client = _client(server, states: states);
        client.connect('ws://t');
        async.flushMicrotasks();
        server.autoAccept = false;
        // Three failed attempts: the wait is now up to 4 s.
        for (var i = 0; i < 3; i++) {
          server.last.drop();
          async.flushMicrotasks();
          final wait = (states.last as WebSocketState$Reconnecting).delay;
          async.elapse(wait);
        }
        server.last.drop();
        async.flushMicrotasks();
        expect(
          client.state,
          isA<WebSocketState$Reconnecting>().having((s) => s.attempt, 'n', 4),
        );
        final before = server.connections.length;
        client.resume(); // the network came back
        async.flushMicrotasks();
        expect(server.connections, hasLength(before + 1));
        expect(
          states.last,
          isA<WebSocketState$Connecting>().having((s) => s.attempt, 'n', 0),
        );
        client.close();
      });
    });

    test('concurrent resumes share one probe', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = _client(server);
        client.connect('ws://t');
        async.flushMicrotasks();
        final conn = server.last..autoPong = false;
        final done = <int>[];
        client
          ..resume().then((_) => done.add(1))
          ..resume().then((_) => done.add(2));
        async.flushMicrotasks();
        expect(conn.pings, 1);
        conn.pong();
        async.flushMicrotasks();
        expect(done, [1, 2]);
        client.close();
      });
    });

    test('suspend() during the probe cancels it; disconnect() fails it', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = _client(server);
        client.connect('ws://t');
        async.flushMicrotasks();
        final conn = server.last..autoPong = false;
        Object? first;
        client.resume().then<void>((_) {}, onError: (Object e) => first = e);
        client.suspend();
        async.elapse(const Duration(minutes: 1));
        expect(conn.isOpen, isTrue);
        expect(first, isNull, reason: 'still waiting for the next open');

        Object? second;
        client.resume().then<void>((_) {}, onError: (Object e) => second = e);
        client.disconnect();
        async.elapse(const Duration(seconds: 5));
        expect(first, isA<WebSocketNotConnectedException>());
        expect(second, isA<WebSocketNotConnectedException>());
        client.close();
      });
    });

    test('while connecting it waits for that attempt', () {
      fake((async) {
        final server = FakeWebSocketServer(autoAccept: false);
        final client = _client(server);
        client.connect('ws://t');
        async.flushMicrotasks();
        var resumed = false;
        client.resume().then((_) => resumed = true);
        async.flushMicrotasks();
        expect(server.connections, hasLength(1));
        server.last.accept();
        async.flushMicrotasks();
        expect(resumed, isTrue);
        client.close();
      });
    });

    test('reconnect() ends the suspension too', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = _client(server);
        client.connect('ws://t');
        async.flushMicrotasks();
        client.suspend();
        client.reconnect();
        async.flushMicrotasks();
        expect(server.connections, hasLength(2));
        final conn = server.last..autoPong = false;
        async.elapse(const Duration(seconds: 16));
        expect(conn.isEnded, isTrue, reason: 'the heartbeat runs');
        client.close();
      });
    });

    test('connect() to another URL while suspended switches', () {
      fake((async) {
        final server = FakeWebSocketServer();
        final client = _client(server);
        client.connect('ws://a');
        async.flushMicrotasks();
        client.suspend(disconnect: true);
        client.connect('ws://b');
        async.flushMicrotasks();
        expect(server.last.request.url, Uri.parse('ws://b'));
        expect(client.state.isOpen, isTrue);
        client.close();
      });
    });
  });
}

final class _Rpc implements WebSocketRpc<String> {
  const _Rpc();

  @override
  (Object, String) encode(int seq, String data) => ('$seq', '$seq:$data');

  @override
  Object? match(String message) => message.split(':').first;

  @override
  String decode(String reply) => reply;
}
