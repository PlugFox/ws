/// End-to-end tests against the Rust fault server (server/, TEST_PLAN §3):
/// TLS trust, proxies, IPv6, handshake faults, protocol violations and
/// close semantics. Run on every native-platform transport.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io' show sleep;
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/transport.dart';
import 'package:ws/ws.dart';

import '../support/test_server.dart';

/// Registers the server-backed tests for the transport made by [create].
void serverConformance(WebSocketTransport Function() create) {
  group('against the Rust test server', () {
    late TestServer server;
    setUpAll(() async => server = await TestServer.start());
    tearDownAll(() => server.stop());
    setUp(() => WebSocketTransports.use(create()));
    tearDown(() => WebSocketTransports.use(null));

    var ids = 0;
    String id() => 'c${ids++}-${DateTime.now().microsecondsSinceEpoch}';

    WebSocketClient<String> client({
      WebSocketVmOptions vm = const WebSocketVmOptions(),
      Duration connectTimeout = const Duration(seconds: 5),
      Duration disconnectTimeout = const Duration(seconds: 2),
      List<String> protocols = const [],
    }) => WebSocketClient.text(
      options: WebSocketOptions(
        reconnect: const WebSocketReconnect.disabled(),
        vm: vm,
        connectTimeout: connectTimeout,
        disconnectTimeout: disconnectTimeout,
        protocols: protocols,
      ),
    );

    Future<void> roundTrip(
      WebSocketClient<String> c, [
      String text = 'hi',
    ]) async {
      final got = c.messages.first;
      await c.push(text);
      expect(await got.timeout(const Duration(seconds: 5)), text);
    }

    Future<Object> connectError(
      String url, {
      WebSocketVmOptions vm = const WebSocketVmOptions(),
      Duration connectTimeout = const Duration(seconds: 5),
    }) async {
      final c = client(vm: vm, connectTimeout: connectTimeout);
      try {
        await c.connect(url);
      } on Object catch (e) {
        return e;
      } finally {
        await c.close();
      }
      fail('connected to $url');
    }

    /// Connects, then waits for the connection to end on its own.
    Future<WebSocketState$Disconnected> serverEnds(String url) async {
      final c = client();
      final ended = c.states
          .where((s) => s is WebSocketState$Disconnected)
          .cast<WebSocketState$Disconnected>()
          .first;
      await c.connect(url);
      final state = await ended.timeout(const Duration(seconds: 10));
      await c.close();
      return state;
    }

    group('TLS', () {
      test('trusted through trustedRootsPem', () async {
        final c = client(
          vm: WebSocketVmOptions(
            tls: WebSocketTls(trustedRootsPem: server.caPem),
          ),
        );
        await c.connect(server.tls('/echo'));
        await roundTrip(c, 'secure ✓');
        await c.close();
        // IP address as the server name (SAN 127.0.0.1).
        final ip = client(
          vm: WebSocketVmOptions(
            tls: WebSocketTls(trustedRootsPem: server.caPem),
          ),
        );
        await ip.connect(server.tls('/echo', host: '127.0.0.1'));
        await roundTrip(ip);
        await ip.close();
      });

      test('an unknown CA is rejected', () async {
        expect(
          await connectError(server.tls('/echo')),
          isA<WebSocketConnectionException>(),
        );
      });

      test('a certificate for another host is rejected', () async {
        expect(
          await connectError(
            'wss://localhost:${server.wssBadHost}/echo',
            vm: WebSocketVmOptions(
              tls: WebSocketTls(trustedRootsPem: server.caPem),
            ),
          ),
          isA<WebSocketConnectionException>(),
        );
      });

      test('allowInsecure accepts it (development only)', () async {
        final c = client(
          vm: const WebSocketVmOptions(tls: WebSocketTls(allowInsecure: true)),
        );
        await c.connect('wss://localhost:${server.wssBadHost}/echo');
        await roundTrip(c);
        await c.close();
      });

      test('wss to a plain port and ws to a TLS port fail cleanly', () async {
        expect(
          await connectError(
            'wss://127.0.0.1:${server.ws}/echo',
            vm: const WebSocketVmOptions(
              tls: WebSocketTls(allowInsecure: true),
            ),
          ),
          isA<WebSocketException>(),
        );
        expect(
          await connectError('ws://127.0.0.1:${server.wss}/echo'),
          isA<WebSocketException>(),
        );
      });
    });

    group('proxy', () {
      test('ws:// and wss:// through a CONNECT tunnel', () async {
        final proxy = WebSocketProxy(host: '127.0.0.1', port: server.proxy);
        final plain = client(vm: WebSocketVmOptions(proxy: proxy));
        await plain.connect(server.url('/echo'));
        await roundTrip(plain, 'tunnelled');
        await plain.close();

        final secure = client(
          vm: WebSocketVmOptions(
            proxy: proxy,
            tls: WebSocketTls(trustedRootsPem: server.caPem),
          ),
        );
        await secure.connect(server.tls('/echo'));
        await roundTrip(secure, 'tls in a tunnel');
        await secure.close();
        expect(
          await server.proxyLog(),
          containsAll(['127.0.0.1:${server.ws}', 'localhost:${server.wss}']),
        );
      });

      test('proxy authentication', () async {
        final c = client(
          vm: WebSocketVmOptions(
            proxy: WebSocketProxy.parse(
              'http://test:secret@127.0.0.1:${server.proxyAuth}',
            ),
          ),
        );
        await c.connect(server.url('/echo'));
        await roundTrip(c);
        await c.close();

        final refused = await connectError(
          server.url('/echo'),
          vm: WebSocketVmOptions(
            proxy: WebSocketProxy.parse(
              'http://test:wrong@127.0.0.1:${server.proxyAuth}',
            ),
          ),
        );
        expect(refused, isA<WebSocketConnectionException>());
        expect(refused.toString(), contains('407'));
      });

      test('upstream failure (502) and a dead proxy', () async {
        final proxy = WebSocketProxy(host: '127.0.0.1', port: server.proxy);
        final bad = await connectError(
          'ws://127.0.0.1:1/echo',
          vm: WebSocketVmOptions(proxy: proxy),
        );
        expect(bad.toString(), contains('502'));
        expect(
          await connectError(
            server.url('/echo'),
            vm: const WebSocketVmOptions(
              proxy: WebSocketProxy(host: '127.0.0.1', port: 1),
            ),
          ),
          isA<WebSocketConnectionException>(),
        );
      });
    });

    group('addressing', () {
      test('IPv6 literal and a name resolving to ::1 and 127.0.0.1', () async {
        final port = server.ws6;
        if (port == null) return markTestSkipped('no IPv6 loopback');
        final literal = client();
        await literal.connect('ws://[::1]:$port/echo');
        await roundTrip(literal);
        await literal.close();
        // Only [::1] listens on this port: if 127.0.0.1 is tried first it
        // is refused and the next address must still be tried.
        final name = client();
        await name.connect('ws://localhost:$port/echo');
        await roundTrip(name);
        await name.close();
      });
    });

    group('handshake faults', () {
      test('no answer: connect timeout', () async {
        expect(
          await connectError(
            server.url('/hang'),
            connectTimeout: const Duration(milliseconds: 300),
          ),
          isA<WebSocketTimeoutException>(),
        );
      });

      test(
        'HTTP status, headers and redirects are reported, not followed',
        () async {
          final e = await connectError(
            server.url('/status?code=503&header=Retry-After:7'),
          );
          expect(
            e,
            isA<WebSocketHandshakeException>()
                .having((e) => e.statusCode, 'status', 503)
                .having((e) => e.headers['retry-after'], 'retry-after', '7'),
          );
          expect(
            await connectError(server.url('/redirect?code=302&to=/echo')),
            isA<WebSocketHandshakeException>().having(
              (e) => e.statusCode,
              'status',
              302,
            ),
          );
          expect(
            await connectError(server.url('/status?code=200')),
            isA<WebSocketHandshakeException>(),
          );
        },
      );

      test('broken 101 responses', () async {
        for (final query in [
          'accept=wrong',
          'accept=dup',
          'omit=upgrade',
          'omit=connection',
          'omit=accept',
          'extensions=permessage-deflate',
          'protocol=surprise',
        ]) {
          expect(
            await connectError(server.url('/bad-101?$query')),
            isA<WebSocketHandshakeException>(),
            reason: query,
          );
        }
      });

      test('EOF, junk and a huge head', () async {
        for (final path in [
          '/eof-handshake',
          '/eof-handshake?partial=1',
          '/junk',
          '/big-head?kb=80',
        ]) {
          expect(
            await connectError(server.url(path)),
            isA<WebSocketException>(),
            reason: path,
          );
        }
        final ok = client();
        await ok.connect(server.url('/big-head?kb=8'));
        await roundTrip(ok);
        await ok.close();
      });

      test('a server may select none of the offered subprotocols', () async {
        final c = client(protocols: const ['v1', 'v2']);
        await c.connect(server.url('/echo?protocol='));
        expect((c.state as WebSocketState$Open).protocol, isNull);
        await roundTrip(c);
        await c.close();
      });

      test('handshake headers, subprotocol and user info', () async {
        final c = client(
          vm: const WebSocketVmOptions(headers: {'X-Token': 'secret'}),
          protocols: const ['v1', 'v2'],
        );
        final first = c.messages.first;
        await c.connect('ws://юзер:p%40ss@127.0.0.1:${server.ws}/headers');
        expect((c.state as WebSocketState$Open).protocol, 'v1');
        final request = jsonDecode(await first) as Map<String, Object?>;
        final headers = {
          for (final h in request['headers']! as List<Object?>)
            ((h! as List)[0] as String).toLowerCase(): (h as List)[1] as String,
        };
        expect(request['target'], '/headers');
        expect(headers['sec-websocket-protocol'], 'v1, v2');
        expect(headers['host'], '127.0.0.1:${server.ws}');
        expect(
          utf8.decode(base64.decode(headers['authorization']!.substring(6))),
          'юзер:p@ss',
        );
        await c.close();
      });
    });

    test('invalid subprotocols and user info fail the attempt', () async {
      final c = WebSocketClient.text(
        options: const WebSocketOptions(
          reconnect: WebSocketReconnect.disabled(),
          protocols: ['chat\r\nX-Injected: yes'],
        ),
      );
      await expectLater(
        c.connect(server.url('/echo')),
        throwsA(isA<WebSocketHandshakeException>()),
      );
      await c.close();
      expect(
        await connectError('ws://user:%ff@127.0.0.1:${server.ws}/echo'),
        isA<WebSocketHandshakeException>(),
      );
    });

    test('invalid header values fail the attempt, never hang', () async {
      for (final headers in [
        {'X-Token': 't ✓'},
        {'X-Token': 'a\r\nInjected: 1'},
        {'Bad Name': 'v'},
      ]) {
        expect(
          await connectError(
            server.url('/echo'),
            vm: WebSocketVmOptions(headers: headers),
            connectTimeout: const Duration(seconds: 3),
          ),
          isA<WebSocketException>().having(
            (e) => e,
            'not a timeout',
            isNot(isA<WebSocketTimeoutException>()),
          ),
          reason: '$headers',
        );
      }
    });

    test('a URL with a query but no path requests "/?query"', () async {
      final c = client();
      // The server answers an upgrade for an unknown path with 101 then
      // close 1008, so opening at all proves the request line was valid.
      final ended = c.states
          .where((s) => s is WebSocketState$Disconnected)
          .cast<WebSocketState$Disconnected>()
          .first;
      await c.connect('ws://127.0.0.1:${server.ws}?token=x');
      expect((await ended).closeCode, 1008);
      await c.close();
    });

    test('transport sends before open are refused', () async {
      final events = _Recorder();
      final connection = WebSocketTransports.current.connect(
        TransportRequest(url: Uri.parse(server.url('/hang'))),
        events,
      );
      expect(
        () => connection.sendText('too early'),
        throwsA(isA<WebSocketException>()),
      );
      connection.abort();
      await events.closed.future.timeout(const Duration(seconds: 5));
    });

    group('protocol violations', () {
      Future<void> expectClose(String preset, int code) async {
        final state = await serverEnds(server.url('/raw?preset=$preset'));
        final error = state.error;
        expect(error, isA<WebSocketClosedException>(), reason: preset);
        expect(
          (error! as WebSocketClosedException).code,
          code == -1 ? anyOf(1002, 1009) : code,
          reason: preset,
        );
      }

      for (final (preset, code) in [
        ('rsv1', 1002),
        ('rsv2', 1002),
        ('rsv3', 1002),
        ('opcode3', 1002),
        ('opcodeB', 1002),
        ('masked', 1002),
        ('ping-nofin', 1002),
        ('ping126', 1002),
        ('cont-first', 1002),
        ('text-mid-frag', 1002),
        ('close1b', 1002),
        ('close-code-1005', 1002),
        ('close-code-999', 1002),
        ('close-bad-utf8', 1007),
        ('utf8-bad', 1007),
        ('utf8-trunc', 1007),
        ('utf8-frag-bad', 1007),
        // tungstenite reports a length with the MSB set as too big (1009).
        ('len64-msb', -1),
      ]) {
        test(
          '$preset closes with ${code == -1 ? '1002/1009' : code}',
          () => expectClose(preset, code),
        );
      }

      test('frames over maxMessageSize close with 1009', () async {
        final state = await serverEnds(server.url('/raw?preset=len64-max'));
        expect(
          (state.error! as WebSocketClosedException).code,
          anyOf(1009, 1002),
        );
      });

      test('connection lost mid-frame is a connection error', () async {
        for (final preset in [
          'eof-in-header',
          'eof-in-payload',
          'eof-between-frags',
        ]) {
          final state = await serverEnds(server.url('/raw?preset=$preset'));
          expect(
            state.error,
            isA<WebSocketConnectionException>(),
            reason: preset,
          );
        }
      });

      test('valid edge cases are accepted', () async {
        Future<List<String>> messages(String path, int n) async {
          final c = client();
          final got = c.messages.take(n).toList();
          await c.connect(server.url(path));
          final result = await got.timeout(const Duration(seconds: 5));
          await c.close();
          return result;
        }

        expect(await messages('/raw?preset=len-nonmin', 1), hasLength(1));
        expect(await messages('/raw?preset=zero-cont-x10000', 1), ['']);
        expect(
          (await messages(
            '/fragment?size=3000&parts=7&split_utf8=1',
            1,
          )).single,
          '€' * 1000,
        );
        expect(
          (await messages(
            '/fragment?size=100&parts=10&ping_between=1',
            1,
          )).single.length,
          100,
        );
      });
    });

    group('close semantics', () {
      test('server close code and reason; empty close is 1005', () async {
        final state = await serverEnds(
          server.url('/close?code=4001&reason=bye'),
        );
        expect((state.closeCode, state.closeReason), (4001, 'bye'));
        final empty = await serverEnds(server.url('/close-empty'));
        expect(empty.closeCode, isNull);
      });

      test('TCP drop without a close frame is a connection error', () async {
        for (final mode in ['fin', 'rst']) {
          final state = await serverEnds(server.url('/drop?mode=$mode'));
          expect(
            state.error,
            isA<WebSocketConnectionException>(),
            reason: mode,
          );
        }
      });

      test('close then RST still reports the close code', () async {
        final state = await serverEnds(
          server.url('/close?code=4002&reason=r&then=rst'),
        );
        expect(state.closeCode, 4002);
      });

      test(
        'disconnect() sends 1000 masked, and the client closes TCP',
        () async {
          final tag = id();
          final c = client();
          await c.connect(server.url('/echo?id=$tag'));
          await roundTrip(c);
          await c.disconnect(4000, 'done');
          final log = await server.log(tag);
          final close = log['close']! as Map<String, Object?>;
          expect((close['code'], close['reason']), (4000, 'done'));
          final frames = (log['frames']! as List).cast<Map<String, Object?>>();
          expect(frames.every((f) => f['masked'] == true), isTrue);
          expect(log['tcp'], 'fin');
          await c.close();
        },
      );

      test(
        'a server ignoring our close is aborted after disconnectTimeout',
        () async {
          final c = client(
            disconnectTimeout: const Duration(milliseconds: 300),
          );
          await c.connect(server.url('/ignore-close'));
          final watch = Stopwatch()..start();
          await c.disconnect();
          expect(watch.elapsed, lessThan(const Duration(seconds: 3)));
          expect(c.state, isA<WebSocketState$Disconnected>());
          await c.close();
        },
      );
    });

    group('traffic', () {
      test('a flood of 20000 messages arrives complete and in order', () async {
        final c = client();
        var n = 0;
        final done = Completer<void>();
        c.messages.listen((m) {
          if (++n == 20000) done.complete();
        });
        await c.connect(server.url('/flood?count=20000&size=32&then=stay'));
        await done.future.timeout(const Duration(seconds: 20));
        await c.close();
      });

      test('protocol ping and pong', () async {
        final c = client();
        final msgs = c.messages.first;
        await c.connect(server.url('/ping?payload=0102'));
        expect(await msgs.timeout(const Duration(seconds: 5)), 'pong-ok');
        await c.ping();
        await c.close();
      });

      test('a slow reader fills the queue; push waits, then drains', () async {
        final c = WebSocketClient.binary(
          options: const WebSocketOptions(
            reconnect: WebSocketReconnect.disabled(),
            sendHighWaterMark: 64 << 10,
          ),
        );
        await c.connect(server.url('/slow-read?pause_ms=300&bps=0'));
        final chunk = Uint8List(256 << 10);
        var waited = false;
        for (var i = 0; i < 40 && !waited; i++) {
          final f = c.push(chunk);
          final quick = await Future.any([
            f.then((_) => true),
            Future<bool>.delayed(const Duration(milliseconds: 5), () => false),
          ]);
          if (!quick) {
            waited = true;
            await f.timeout(const Duration(seconds: 10));
          }
        }
        expect(waited, isTrue, reason: 'push never applied backpressure');
        await c.close();
      });
    });

    group('liveness', () {
      test('a blocked isolate does not drop a healthy connection', () async {
        final c = WebSocketClient.text(
          options: const WebSocketOptions(
            reconnect: WebSocketReconnect.disabled(),
            heartbeat: WebSocketHeartbeat(
              interval: Duration(milliseconds: 100),
              timeout: Duration(milliseconds: 200),
            ),
          ),
        );
        final states = <WebSocketState>[];
        c.states.listen(states.add);
        await c.connect(server.url('/echo'));
        // Block longer than interval + timeout, wake up briefly (a ping goes
        // out), block again while the pong arrives.
        for (var i = 0; i < 6; i++) {
          sleep(const Duration(milliseconds: 500));
          await Future<void>.delayed(Duration(milliseconds: 20 + 15 * i));
        }
        await Future<void>.delayed(const Duration(milliseconds: 400));
        expect(c.state.isOpen, isTrue, reason: '$states');
        expect(c.metrics.lastRoundTrip, isNotNull);
        await roundTrip(c);
        await c.close();
      });

      test('a slow upload delays the pong without dropping', () async {
        final c = WebSocketClient.binary(
          options: const WebSocketOptions(
            reconnect: WebSocketReconnect.disabled(),
            disconnectTimeout: Duration(milliseconds: 500),
            heartbeat: WebSocketHeartbeat(
              interval: Duration(milliseconds: 300),
              timeout: Duration(milliseconds: 1500),
            ),
          ),
        );
        final states = <WebSocketState>[];
        c.states.listen(states.add);
        await c.connect(server.url('/slow-read?bps=2097152'));
        final chunk = Uint8List(256 << 10);
        final pushes = [for (var i = 0; i < 32; i++) c.push(chunk)];
        await Future.wait(pushes).timeout(const Duration(seconds: 20));
        // 8 MiB at 2 MiB/s: pongs wait behind the data for seconds.
        await Future<void>.delayed(const Duration(seconds: 1));
        expect(c.state.isOpen, isTrue, reason: '$states');
        await c.close();
      });

      test('a server that stops reading and answering is dropped', () async {
        final c = WebSocketClient.binary(
          options: const WebSocketOptions(
            reconnect: WebSocketReconnect.disabled(),
            heartbeat: WebSocketHeartbeat(
              interval: Duration(milliseconds: 200),
              timeout: Duration(milliseconds: 500),
            ),
          ),
        );
        final ended = c.states
            .where((s) => s is WebSocketState$Disconnected)
            .cast<WebSocketState$Disconnected>()
            .first;
        await c.connect(server.url('/silent?read=0'));
        final chunk = Uint8List(256 << 10);
        for (var i = 0; i < 64; i++) {
          c.push(chunk).ignore();
        }
        final state = await ended.timeout(const Duration(seconds: 10));
        expect(state.error, isA<WebSocketTimeoutException>());
        await c.close();
      });
    });
  });
}

final class _Recorder implements TransportListener {
  final Completer<void> closed = Completer<void>();
  @override
  void onOpen(String? protocol) {}
  @override
  void onTextBytes(Uint8List bytes) {}
  @override
  void onTextString(String text) {}
  @override
  void onJson(Object? value) {}
  @override
  void onBinary(Uint8List bytes) {}
  @override
  void onPong() {}
  @override
  void onDrain() {}
  @override
  void onClosed(int? code, String reason, WebSocketException? error) {
    if (!closed.isCompleted) closed.complete();
  }
}
