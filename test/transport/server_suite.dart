/// End-to-end tests against the Rust fault server (server/, TEST_PLAN §3)
/// on the VM transports: TLS trust, proxies, IPv6, handshake faults and
/// liveness under a blocked isolate, plus the platform-neutral half in
/// server_shared.dart (also run in the browser).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io' show sleep;
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/transport.dart';
import 'package:ws/ws.dart';

import '../support/checked_transport.dart';
import '../support/test_server.dart';
import 'server_shared.dart';

/// Registers the server-backed tests for the transport made by [create].
void serverConformance(WebSocketTransport Function() create) {
  group('against the Rust test server', () {
    late TestServer server;
    setUpAll(() async => server = await TestServer.start());
    tearDownAll(() => server.stop());
    useCheckedTransport(create);

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

    Matcher failed(
      WebSocketConnectionFailure failure, {
      WebSocketTlsFailure? tls,
      int? proxyStatus,
    }) => isA<WebSocketConnectionException>()
        .having((e) => e.failure, 'failure', failure)
        .having((e) => e.tls, 'tls', tls)
        .having((e) => e.proxyStatus, 'proxyStatus', proxyStatus);

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
          failed(
            WebSocketConnectionFailure.tls,
            tls: WebSocketTlsFailure.untrusted,
          ),
        );
      });

      test('an expired certificate is rejected as expired', () async {
        expect(
          await connectError(
            'wss://localhost:${server.wssExpired}/echo',
            vm: WebSocketVmOptions(
              tls: WebSocketTls(trustedRootsPem: server.caPem),
            ),
          ),
          failed(
            WebSocketConnectionFailure.tls,
            tls: WebSocketTlsFailure.expired,
          ),
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
          failed(
            WebSocketConnectionFailure.tls,
            tls: WebSocketTlsFailure.hostname,
          ),
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

      group('pins (D45)', () {
        const wrong = 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=';
        WebSocketVmOptions pinned(
          List<String> pins, {
          bool roots = true,
          bool insecure = false,
          WebSocketProxy? proxy,
        }) => WebSocketVmOptions(
          tls: WebSocketTls(
            trustedRootsPem: roots ? server.caPem : null,
            pins: pins,
            allowInsecure: insecure,
          ),
          proxy: proxy,
        );

        test('the right key (with a backup pin) connects', () async {
          final c = client(vm: pinned([wrong, 'sha256/${server.wssPin}']));
          await c.connect(server.tls('/echo'));
          await roundTrip(c);
          await c.close();
        });

        test('another key fails as pinMismatch', () async {
          expect(
            await connectError(server.tls('/echo'), vm: pinned([wrong])),
            failed(
              WebSocketConnectionFailure.tls,
              tls: WebSocketTlsFailure.pinMismatch,
            ),
          );
        });

        test('the chain is checked first', () async {
          expect(
            await connectError(
              server.tls('/echo'),
              vm: pinned([server.wssPin], roots: false),
            ),
            failed(
              WebSocketConnectionFailure.tls,
              tls: WebSocketTlsFailure.untrusted,
            ),
          );
        });

        test('allowInsecure skips the chain, never the pins', () async {
          final c = client(
            vm: pinned([server.wssPin], roots: false, insecure: true),
          );
          await c.connect(server.tls('/echo'));
          await roundTrip(c);
          await c.close();
          // Same CA, another key.
          expect(
            await connectError(
              'wss://localhost:${server.wssBadHost}/echo',
              vm: pinned([server.wssPin], roots: false, insecure: true),
            ),
            failed(
              WebSocketConnectionFailure.tls,
              tls: WebSocketTlsFailure.pinMismatch,
            ),
          );
        });

        test('through a CONNECT tunnel', () async {
          final proxy = WebSocketProxy(host: '127.0.0.1', port: server.proxy);
          final c = client(vm: pinned([server.wssPin], proxy: proxy));
          await c.connect(server.tls('/echo'));
          await roundTrip(c);
          await c.close();
          expect(
            await connectError(
              server.tls('/echo'),
              vm: pinned([wrong], proxy: proxy),
            ),
            failed(
              WebSocketConnectionFailure.tls,
              tls: WebSocketTlsFailure.pinMismatch,
            ),
          );
        });

        test('an invalid pin fails without connecting or retrying', () async {
          final c = WebSocketClient.text(
            options: WebSocketOptions(vm: pinned(['not a pin'])),
          );
          final states = <WebSocketState>[];
          c.states.listen(states.add);
          await expectLater(
            c.connect(server.tls('/echo')),
            throwsA(
              allOf(
                failed(
                  WebSocketConnectionFailure.tls,
                  tls: WebSocketTlsFailure.other,
                ),
                isA<WebSocketException>().having(
                  (e) => e.cause,
                  'cause',
                  isA<FormatException>(),
                ),
              ),
            ),
          );
          await Future<void>.delayed(const Duration(milliseconds: 50));
          expect(states.last, isA<WebSocketState$Disconnected>());
          expect(states.whereType<WebSocketState$Reconnecting>(), isEmpty);
          await c.close();
        });
      });

      test('wss to a plain port and ws to a TLS port fail cleanly', () async {
        expect(
          await connectError(
            'wss://127.0.0.1:${server.ws}/echo',
            vm: const WebSocketVmOptions(
              tls: WebSocketTls(allowInsecure: true),
            ),
          ),
          // The plain server waits for an HTTP head that never comes.
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
        expect(
          refused,
          failed(WebSocketConnectionFailure.proxy, proxyStatus: 407),
        );
        expect(refused.toString(), contains('407'));
      });

      test('upstream failure (502) and a dead proxy', () async {
        final proxy = WebSocketProxy(host: '127.0.0.1', port: server.proxy);
        final bad = await connectError(
          'ws://127.0.0.1:1/echo',
          vm: WebSocketVmOptions(proxy: proxy),
        );
        expect(bad.toString(), contains('502'));
        expect(bad, failed(WebSocketConnectionFailure.proxy, proxyStatus: 502));
        expect(
          await connectError(
            server.url('/echo'),
            vm: const WebSocketVmOptions(
              proxy: WebSocketProxy(host: '127.0.0.1', port: 1),
            ),
          ),
          failed(WebSocketConnectionFailure.proxy),
        );
      });
    });

    group('addressing', () {
      test('DNS failure and a refused port are told apart', () async {
        expect(
          await connectError('ws://nonexistent.invalid/echo'),
          failed(WebSocketConnectionFailure.dns),
        );
        expect(
          await connectError('ws://127.0.0.1:1/echo'),
          failed(WebSocketConnectionFailure.refused),
        );
      });

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

      test('an Authorization header wins over URL user info', () async {
        final c = client(
          vm: const WebSocketVmOptions(headers: {'authorization': 'Bearer t'}),
        );
        final first = c.messages.first;
        await c.connect('ws://u:p@127.0.0.1:${server.ws}/headers');
        final request = jsonDecode(await first) as Map<String, Object?>;
        final values = [
          for (final h in request['headers']! as List<Object?>)
            if (((h! as List)[0] as String).toLowerCase() == 'authorization')
              (h as List)[1],
        ];
        expect(values, ['Bearer t']);
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
    });

    group('reconnect policy on connect failures', () {
      const maxDelay = Duration(seconds: 2);

      Future<WebSocketState> decision(
        String url, {
        WebSocketVmOptions vm = const WebSocketVmOptions(),
      }) async {
        final c = WebSocketClient.text(
          options: WebSocketOptions(
            reconnect: const WebSocketReconnect(
              minDelay: Duration(milliseconds: 10),
              maxDelay: maxDelay,
            ),
            vm: vm,
          ),
        );
        final decided = c.states.firstWhere(
          (s) =>
              s is WebSocketState$Reconnecting ||
              s is WebSocketState$Disconnected,
        );
        c.connect(url).ignore();
        final state = await decided.timeout(const Duration(seconds: 10));
        await c.close();
        return state;
      }

      Matcher handshake(int status) => isA<WebSocketHandshakeException>()
          .having((e) => e.statusCode, 'statusCode', status);

      Matcher retried(Matcher cause, {Matcher? delay}) =>
          isA<WebSocketState$Reconnecting>()
              .having((s) => s.cause, 'cause', cause)
              .having(
                (s) => s.delay,
                'delay',
                delay ?? lessThan(const Duration(milliseconds: 500)),
              );

      Matcher stopped(Matcher error) => isA<WebSocketState$Disconnected>()
          .having((s) => s.error, 'error', error);

      for (final status in [400, 401, 403, 404, 410]) {
        test('HTTP $status stops reconnecting', () async {
          expect(
            await decision(server.url('/status?code=$status')),
            stopped(handshake(status)),
          );
        });
      }

      for (final status in [408, 425, 429, 500, 502, 503]) {
        test('HTTP $status is retried', () async {
          expect(
            await decision(server.url('/status?code=$status')),
            retried(handshake(status)),
          );
        });
      }

      test('Retry-After (429, 503) sets the least delay', () async {
        for (final status in [429, 503]) {
          expect(
            await decision(
              server.url('/status?code=$status&header=Retry-After:3'),
            ),
            retried(
              handshake(status),
              delay: greaterThanOrEqualTo(const Duration(seconds: 3)),
            ),
            reason: '$status',
          );
        }
      });

      test('a redirect is reported and retried, not followed', () async {
        expect(
          await decision(server.url('/redirect?code=302&to=/echo')),
          retried(handshake(302)),
        );
      });

      test('a broken 101 is retried', () async {
        expect(
          await decision(server.url('/bad-101?accept=wrong')),
          retried(isA<WebSocketHandshakeException>()),
        );
      });

      test('a rejected certificate is retried after maxDelay', () async {
        for (final (url, tls) in [
          (server.tls('/echo'), WebSocketTlsFailure.untrusted),
          (
            'wss://localhost:${server.wssExpired}/echo',
            WebSocketTlsFailure.expired,
          ),
          (
            'wss://localhost:${server.wssBadHost}/echo',
            WebSocketTlsFailure.hostname,
          ),
        ]) {
          expect(
            await decision(
              url,
              vm: tls == WebSocketTlsFailure.untrusted
                  ? const WebSocketVmOptions()
                  : WebSocketVmOptions(
                      tls: WebSocketTls(trustedRootsPem: server.caPem),
                    ),
            ),
            retried(
              failed(WebSocketConnectionFailure.tls, tls: tls),
              delay: greaterThanOrEqualTo(maxDelay),
            ),
            reason: '$tls',
          );
        }
      });

      test('a pin mismatch is retried after maxDelay', () async {
        expect(
          await decision(
            server.tls('/echo'),
            vm: WebSocketVmOptions(
              tls: WebSocketTls(
                trustedRootsPem: server.caPem,
                pins: [base64.encode(List<int>.filled(32, 7))],
              ),
            ),
          ),
          retried(
            failed(
              WebSocketConnectionFailure.tls,
              tls: WebSocketTlsFailure.pinMismatch,
            ),
            delay: greaterThanOrEqualTo(maxDelay),
          ),
        );
      });

      test('proxy 407 stops, proxy 502 is retried', () async {
        expect(
          await decision(
            server.url('/echo'),
            vm: WebSocketVmOptions(
              proxy: WebSocketProxy.parse(
                'http://test:wrong@127.0.0.1:${server.proxyAuth}',
              ),
            ),
          ),
          stopped(failed(WebSocketConnectionFailure.proxy, proxyStatus: 407)),
        );
        expect(
          await decision(
            'ws://127.0.0.1:1/echo',
            vm: WebSocketVmOptions(
              proxy: WebSocketProxy(host: '127.0.0.1', port: server.proxy),
            ),
          ),
          retried(failed(WebSocketConnectionFailure.proxy, proxyStatus: 502)),
        );
      });

      test('a refused port and an unknown host are retried', () async {
        expect(
          await decision('ws://127.0.0.1:1/echo'),
          retried(failed(WebSocketConnectionFailure.refused)),
        );
        expect(
          await decision('ws://no-such-host.invalid/echo'),
          retried(failed(WebSocketConnectionFailure.dns)),
        );
      });
    });

    sharedServerConformance(() => server, (
      protocolPing: true,
      protocolCloseCodes: true,
    ));
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
