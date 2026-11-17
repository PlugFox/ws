/// The platform-neutral half of the Rust test server suite: close
/// semantics, traffic, protocol violations, liveness and suspend/resume.
/// Runs on the pure-Dart and native transports (server_suite.dart) and in
/// the browser (web_server_test.dart), so all three are held to one
/// behaviour (docs/review/tests_docs.md, T-3). No dart:io here.
library;

import 'dart:async';
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/transport.dart';
import 'package:ws/ws.dart';

import '../support/server_handle.dart';

const bool _isWasm = bool.fromEnvironment('dart.tool.dart2wasm');

/// Registers the shared tests; [server] is valid once `setUpAll` ran.
/// When [unavailable] returns a reason, each test is skipped with it (the
/// browser learns whether the server was built only at run time).
void sharedServerConformance(
  ServerHandle Function() server,
  ServerCapabilities caps, {
  String? Function()? unavailable,
}) {
  void serverTest(String name, Future<void> Function() body, {Object? skip}) =>
      test(name, () async {
        if (unavailable?.call() case final reason?) {
          markTestSkipped(reason);
          return;
        }
        await body();
      }, skip: skip);

  var ids = 0;
  String id() => 's${ids++}-${DateTime.now().microsecondsSinceEpoch}';
  String url(String path) => server().url(path);

  /// A heartbeat that works on this platform: protocol pings, or an
  /// application ping that `/echo` sends back.
  WebSocketHeartbeat<T> heartbeat<T>(
    Duration interval,
    Duration timeout,
    T ping,
    bool Function(T) isPong,
  ) => caps.protocolPing
      ? WebSocketHeartbeat(interval: interval, timeout: timeout)
      : WebSocketHeartbeat(
          interval: interval,
          timeout: timeout,
          ping: () => ping,
          isPong: isPong,
        );

  WebSocketHeartbeat<String> textHeartbeat(
    Duration interval,
    Duration timeout,
  ) => heartbeat(interval, timeout, '♥', (m) => m == '♥');

  WebSocketHeartbeat<Uint8List> binaryHeartbeat(
    Duration interval,
    Duration timeout,
  ) => heartbeat(
    interval,
    timeout,
    Uint8List.fromList(const [0xFE]),
    (m) => m.length == 1 && m[0] == 0xFE,
  );

  WebSocketClient<String> client({
    Duration disconnectTimeout = const Duration(seconds: 2),
    int maxMessageSize = 64 << 20,
  }) => WebSocketClient.text(
    options: WebSocketOptions(
      reconnect: const WebSocketReconnect.disabled(),
      disconnectTimeout: disconnectTimeout,
      maxMessageSize: maxMessageSize,
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

  /// Connects, then waits for the connection to end on its own.
  Future<WebSocketState$Disconnected> serverEnds(
    String path, {
    int maxMessageSize = 64 << 20,
  }) async {
    final c = client(maxMessageSize: maxMessageSize);
    final ended = c.states
        .where((s) => s is WebSocketState$Disconnected)
        .cast<WebSocketState$Disconnected>()
        .first;
    await c.connect(url(path));
    final state = await ended.timeout(const Duration(seconds: 10));
    await c.close();
    return state;
  }

  final lost = isA<WebSocketConnectionException>().having(
    (e) => e.failure,
    'failure',
    WebSocketConnectionFailure.lost,
  );

  group('protocol violations', () {
    /// [code] is what a client that sees the frames sends; a browser hides
    /// it behind 1006 (connection lost).
    Future<void> expectClose(String preset, Matcher code) async {
      final state = await serverEnds('/raw?preset=$preset');
      if (caps.protocolCloseCodes) {
        expect(
          state.error,
          isA<WebSocketClosedException>()
              .having((e) => e.code, 'code', code)
              .having((e) => e.byServer, 'byServer', isFalse),
          reason: preset,
        );
      } else {
        expect(state.error, lost, reason: preset);
      }
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
      ('close-bad-utf8', 1007),
      ('utf8-bad', 1007),
      ('utf8-trunc', 1007),
      ('utf8-frag-bad', 1007),
    ]) {
      serverTest(
        '$preset closes with $code',
        () => expectClose(preset, equals(code)),
      );
    }

    serverTest('an invalid close code (999) closes with 1002', () async {
      // Chrome passes it through; the web transport checks it.
      final state = await serverEnds('/raw?preset=close-code-999');
      expect(
        state.error,
        isA<WebSocketClosedException>()
            .having((e) => e.code, 'code', 1002)
            .having((e) => e.byServer, 'byServer', isFalse),
      );
    });

    // tungstenite reports a length with the MSB set as too big (1009).
    serverTest(
      'len64-msb closes with 1002/1009',
      () => expectClose('len64-msb', anyOf(1002, 1009)),
    );

    serverTest(
      'a frame over maxMessageSize (len64-max) closes with 1009',
      () => expectClose('len64-max', anyOf(1009, 1002)),
    );

    serverTest('connection lost mid-frame is a connection error', () async {
      for (final preset in [
        'eof-in-header',
        'eof-in-payload',
        'eof-between-frags',
      ]) {
        final state = await serverEnds('/raw?preset=$preset');
        expect(state.error, lost, reason: preset);
      }
    });

    group('valid edge cases are accepted, content exact', () {
      Future<List<T>> messages<T extends Object>(
        String path,
        int n,
        WebSocketClient<T> c,
      ) async {
        final got = c.messages.take(n).toList();
        await c.connect(url(path));
        final result = await got.timeout(const Duration(seconds: 5));
        await c.close();
        return result;
      }

      WebSocketClient<Uint8List> binary() => WebSocketClient.binary(
        options: const WebSocketOptions(
          reconnect: WebSocketReconnect.disabled(),
        ),
      );

      serverTest('a non-minimal 16-bit length', () async {
        if (caps.protocolCloseCodes) {
          expect(await messages('/raw?preset=len-nonmin', 1, binary()), [
            Uint8List.fromList('hello'.codeUnits),
          ]);
        } else {
          // Chrome enforces the sender's MUST (RFC 6455 §5.2: the minimal
          // number of bytes) and fails the connection.
          expect((await serverEnds('/raw?preset=len-nonmin')).error, lost);
        }
      });

      serverTest('10000 empty continuation frames', () async {
        expect(await messages('/raw?preset=zero-cont-x10000', 1, client()), [
          '',
        ]);
      });

      serverTest('fragments cut inside UTF-8 code points', () async {
        expect(
          await messages(
            '/fragment?size=3000&parts=7&split_utf8=1',
            1,
            client(),
          ),
          ['€' * 1000],
        );
      });

      serverTest('pings between fragments', () async {
        final alphabet = String.fromCharCodes([
          for (var i = 0; i < 100; i++) 0x61 + i % 26,
        ]);
        expect(
          await messages(
            '/fragment?size=100&parts=10&ping_between=1',
            1,
            client(),
          ),
          [alphabet],
        );
      });

      serverTest('a fragmented binary message', () async {
        expect(
          await messages(
            '/fragment?size=70000&parts=5&kind=binary',
            1,
            binary(),
          ),
          [
            Uint8List.fromList([for (var i = 0; i < 70000; i++) i % 256]),
          ],
        );
      });
    });
  });

  serverTest(
    'informational 103 responses before the 101 are skipped',
    () async {
      // CDNs send 103 Early Hints (docs/review/protocol.md F16).
      if (!caps.skipsInterim) {
        final c = client();
        await expectLater(
          c.connect(url('/early-hints')),
          throwsA(isA<WebSocketConnectionException>()),
        );
        await c.close();
        return;
      }
      for (final count in [1, 3]) {
        final c = client();
        await c.connect(url('/early-hints?count=$count'));
        await roundTrip(c, 'after $count');
        await c.close();
      }
    },
  );

  group('maxMessageSize', () {
    serverTest(
      'a larger message closes with 1009 and is not delivered',
      () async {
        final c = client(maxMessageSize: 1000);
        final got = <String>[];
        c.messages.listen(got.add);
        final ended = c.states
            .where((s) => s is WebSocketState$Disconnected)
            .cast<WebSocketState$Disconnected>()
            .first;
        await c.connect(url('/flood?count=2&size=1001&then=stay'));
        final state = await ended.timeout(const Duration(seconds: 10));
        expect(
          state.error,
          isA<WebSocketClosedException>()
              .having((e) => e.code, 'code', 1009)
              .having((e) => e.byServer, 'byServer', isFalse),
        );
        expect(got, isEmpty);
        await c.close();
      },
    );

    serverTest(
      'a message of exactly maxMessageSize bytes is delivered',
      () async {
        final c = client(maxMessageSize: 1000);
        final got = c.messages.first;
        await c.connect(url('/flood?count=1&size=1000&then=stay'));
        expect((await got.timeout(const Duration(seconds: 5))).length, 1000);
        expect(c.state.isOpen, isTrue);
        await c.close();
      },
    );

    serverTest('the limit counts UTF-8 bytes, not characters', () async {
      // 334 '€' are 1002 bytes but 334 UTF-16 code units.
      final c = client(maxMessageSize: 1000);
      final ended = c.states
          .where((s) => s is WebSocketState$Disconnected)
          .cast<WebSocketState$Disconnected>()
          .first;
      await c.connect(url('/fragment?size=1002&parts=2&split_utf8=1'));
      final state = await ended.timeout(const Duration(seconds: 10));
      expect(
        state.error,
        isA<WebSocketClosedException>().having((e) => e.code, 'code', 1009),
      );
      await c.close();
    });

    serverTest('a surrogate pair counts as 4 bytes', () async {
      final c = client(maxMessageSize: 1000);
      final got = c.messages.first;
      await c.connect(url('/echo'));
      // 250 × U+1D11E: 1000 bytes, 500 UTF-16 code units.
      await c.push('𝄞' * 250);
      expect(await got.timeout(const Duration(seconds: 5)), '𝄞' * 250);
      final ended = c.states
          .where((s) => s is WebSocketState$Disconnected)
          .cast<WebSocketState$Disconnected>()
          .first;
      await c.push('𝄞' * 251);
      final state = await ended.timeout(const Duration(seconds: 10));
      expect(
        state.error,
        isA<WebSocketClosedException>().having((e) => e.code, 'code', 1009),
      );
      await c.close();
    });
  });

  serverTest('a long close reason is cut at a character boundary', () async {
    final tag = id();
    final events = _Events();
    final c = WebSocketTransports.current.connect(
      TransportRequest(url: Uri.parse(url('/echo?id=$tag'))),
      events,
    );
    await events.opened.future.timeout(const Duration(seconds: 5));
    // 70 'é' are 140 bytes; 61 (122 bytes) fit in a close frame.
    c.close(4000, 'é' * 70);
    await events.closed.future.timeout(const Duration(seconds: 5));
    final log = await server().log(tag);
    final close = log['close']! as Map<String, Object?>;
    expect((close['code'], close['reason']), (4000, 'é' * 61));
    expect(close['reason_utf8'], isTrue);
  });

  group('calls after close() (T-12)', () {
    // The transport itself, without the client: after close() a send is
    // refused with WebSocketNotConnectedException on every transport, and
    // ping, close and abort do nothing more than end the connection once.
    serverTest('send is refused, the rest is harmless', () async {
      final events = _Events();
      final c = WebSocketTransports.current.connect(
        TransportRequest(url: Uri.parse(url('/echo'))),
        events,
      );
      await events.opened.future.timeout(const Duration(seconds: 5));
      c
        ..sendText('before')
        ..close(1000, 'bye');
      for (final send in <void Function()>[
        () => c.sendText('after'),
        () => c.sendTextBytes(Uint8List.fromList('after'.codeUnits)),
        () => c.sendBinary(Uint8List(1)),
      ]) {
        expect(send, throwsA(isA<WebSocketNotConnectedException>()));
      }
      expect(
        c.ping,
        throwsA(
          caps.protocolPing
              ? isA<WebSocketNotConnectedException>()
              : isA<WebSocketUnsupportedException>(),
        ),
      );
      c
        ..close(1000, 'again')
        ..requestDrain();
      await events.closed.future.timeout(const Duration(seconds: 5));
      c
        ..abort()
        ..close(1000, 'late');
      expect(
        () => c.sendText('late'),
        throwsA(isA<WebSocketNotConnectedException>()),
      );
      await Future<void>.delayed(const Duration(milliseconds: 50));
      expect(events.closes, 1);
      // Data that precedes the server's close frame is delivered; browsers
      // drop messages once close() was called (the HTML spec fires message
      // events only while OPEN). The client drops them anyway.
      expect(events.texts, caps.protocolCloseCodes ? ['before'] : isEmpty);
    });
  });

  serverTest('a throwing listener loses no message and no error', () async {
    final events = _Events()..throwOnFirst = true;
    final errors = <Object>[];
    final done = Completer<void>();
    late TransportConnection c;
    runZonedGuarded(() {
      c = WebSocketTransports.current.connect(
        TransportRequest(url: Uri.parse(url('/flood?count=50&size=8'))),
        events,
      );
    }, (e, _) => errors.add(e));
    events.onText = () {
      if (events.texts.length == 49 && !done.isCompleted) done.complete();
    };
    await done.future.timeout(const Duration(seconds: 5));
    expect(errors, [isA<StateError>()]);
    c.abort();
    await events.closed.future.timeout(const Duration(seconds: 5));
  });

  group('reconnect policy on server closes', () {
    const maxDelay = Duration(seconds: 2);

    /// The client's decision after the first failure: reconnecting (and
    /// how soon) or disconnected for good.
    Future<WebSocketState> decision(String path) async {
      final c = WebSocketClient.text(
        options: const WebSocketOptions(
          reconnect: WebSocketReconnect(
            minDelay: Duration(milliseconds: 10),
            maxDelay: maxDelay,
          ),
        ),
      );
      final decided = c.states.firstWhere(
        (s) =>
            s is WebSocketState$Reconnecting ||
            s is WebSocketState$Disconnected,
      );
      c.connect(url(path)).ignore();
      final state = await decided.timeout(const Duration(seconds: 10));
      await c.close();
      return state;
    }

    Matcher retried(int? code, {bool late = false}) =>
        isA<WebSocketState$Reconnecting>()
            .having(
              (s) => s.cause,
              'cause',
              code == null
                  ? lost
                  : isA<WebSocketClosedException>().having(
                      (e) => e.code,
                      'code',
                      code,
                    ),
            )
            .having(
              (s) => s.delay,
              'delay',
              late
                  ? greaterThanOrEqualTo(maxDelay)
                  : lessThan(const Duration(milliseconds: 500)),
            );

    Matcher stopped(int code) => isA<WebSocketState$Disconnected>().having(
      (s) => s.error,
      'error',
      isA<WebSocketClosedException>().having((e) => e.code, 'code', code),
    );

    for (final code in [1000, 1001, 1011, 1012, 4000]) {
      serverTest('$code is retried soon', () async {
        expect(await decision('/close?code=$code&reason=r'), retried(code));
      });
    }

    serverTest('1013 (try again later) waits maxDelay', () async {
      expect(
        await decision('/close?code=1013&reason=busy'),
        retried(1013, late: true),
      );
    });

    for (final code in [1003, 1008]) {
      serverTest('$code stops reconnecting', () async {
        expect(await decision('/close?code=$code&reason=r'), stopped(code));
      });
    }

    serverTest('an empty close (1005) is retried', () async {
      expect(
        await decision('/close-empty'),
        isA<WebSocketState$Reconnecting>(),
      );
    });

    serverTest('a dropped connection is retried', () async {
      expect(await decision('/drop?mode=rst'), retried(null));
    });

    serverTest('a protocol violation by the server', () async {
      final state = await decision('/raw?preset=utf8-bad');
      if (caps.protocolCloseCodes) {
        // We closed with 1007: repeating it would fail the same way.
        expect(state, stopped(1007));
      } else {
        // The browser hides it as 1006: indistinguishable from a drop.
        expect(state, retried(null));
      }
    });
  });

  group('close semantics', () {
    serverTest('server close code and reason; empty close is 1005', () async {
      final state = await serverEnds('/close?code=4001&reason=bye');
      expect((state.closeCode, state.closeReason), (4001, 'bye'));
      expect(
        state.error,
        isA<WebSocketClosedException>()
            .having((e) => e.code, 'code', 4001)
            .having((e) => e.byServer, 'byServer', isTrue),
      );
      final empty = await serverEnds('/close-empty');
      expect((empty.closeCode, empty.closeReason), (null, ''));
    });

    serverTest('a server close 1000 is not an error', () async {
      final state = await serverEnds('/close?code=1000&reason=done');
      expect((state.closeCode, state.closeReason), (1000, 'done'));
    });

    serverTest(
      'TCP drop without a close frame is a connection error',
      () async {
        for (final mode in ['fin', 'rst']) {
          final state = await serverEnds('/drop?mode=$mode');
          expect(state.error, lost, reason: mode);
          expect(state.closeCode, isNull, reason: mode);
        }
      },
    );

    serverTest(
      'a server still sending after its close frame gets our close',
      () async {
        // Close 1000, then 40 text frames written 8 bytes every 5 ms:
        // closing TCP both ways at once would answer them with RST, and
        // the server would lose our close frame (protocol.md F14).
        final tag = id();
        final frames = [
          0x88, 0x02, 0x03, 0xE8, //
          for (var i = 0; i < 40; i++) ...[0x81, 0x04, ...'late'.codeUnits],
        ];
        final hex = [
          for (final b in frames) b.toRadixString(16).padLeft(2, '0'),
        ].join();
        final state = await serverEnds(
          '/raw?id=$tag&hex=$hex&chunk=8&delay_ms=5&end=wait',
        );
        if (!caps.protocolCloseCodes) {
          // Browsers treat data after a close frame as a failure (1006).
          expect(state.error, lost);
          return;
        }
        expect(state.closeCode, 1000);
        final log = await server().log(tag);
        expect((log['close']! as Map<String, Object?>)['code'], 1000);
        expect(log['tcp'], 'fin');
      },
    );

    serverTest('close then RST still reports the close code', () async {
      final state = await serverEnds('/close?code=4002&reason=r&then=rst');
      expect(state.closeCode, 4002);
    });

    serverTest(
      'disconnect() sends the code masked, and the client closes TCP',
      () async {
        final tag = id();
        final c = client();
        await c.connect(url('/echo?id=$tag'));
        await roundTrip(c);
        await c.disconnect(4000, 'done');
        expect(c.state, isA<WebSocketState$Disconnected>());
        final log = await server().log(tag);
        final close = log['close']! as Map<String, Object?>;
        expect((close['code'], close['reason']), (4000, 'done'));
        final frames = (log['frames']! as List).cast<Map<String, Object?>>();
        expect(frames.every((f) => f['masked'] == true), isTrue);
        // The text frame of the round trip, then the close frame.
        expect([for (final f in frames) f['op']], [1, 8]);
        expect(log['tcp'], 'fin');
        await c.close();
      },
    );

    serverTest('disconnect() without a code sends 1000', () async {
      final tag = id();
      final c = client();
      await c.connect(url('/echo?id=$tag'));
      await c.disconnect();
      final log = await server().log(tag);
      final close = log['close']! as Map<String, Object?>;
      expect(close['code'], 1000);
      await c.close();
    });

    serverTest(
      'a server ignoring our close is aborted after disconnectTimeout',
      () async {
        final c = client(disconnectTimeout: const Duration(milliseconds: 300));
        await c.connect(url('/ignore-close'));
        final watch = Stopwatch()..start();
        await c.disconnect();
        expect(watch.elapsed, lessThan(const Duration(seconds: 3)));
        expect(c.state, isA<WebSocketState$Disconnected>());
        await c.close();
      },
    );
  });

  group('traffic', () {
    serverTest(
      'a flood of 20000 messages arrives complete and in order',
      () async {
        final c = client();
        final got = <String>[];
        final done = Completer<void>();
        c.messages.listen((m) {
          got.add(m);
          if (got.length == 20000) done.complete();
        });
        await c.connect(url('/flood?count=20000&size=32&then=stay'));
        await done.future.timeout(const Duration(seconds: 30));
        // Every message is a..z repeated: the same 32 bytes.
        final expected = String.fromCharCodes([
          for (var i = 0; i < 32; i++) 0x61 + i % 26,
        ]);
        expect(got.every((m) => m == expected), isTrue);
        await roundTrip(c, 'after the flood');
        await c.close();
      },
    );

    serverTest('a numbered stream arrives in order, nothing more', () async {
      // /stream numbers its messages (u64 LE at offset 0): order and
      // completeness across batch and credit boundaries (T-6).
      const count = 20000;
      final c = WebSocketClient.binary(
        options: const WebSocketOptions(
          reconnect: WebSocketReconnect.disabled(),
        ),
      );
      final seqs = <int>[];
      c.messages.listen((m) {
        final data = ByteData.sublistView(m);
        // Two u32 halves: dart2js has no 64-bit getUint64.
        expect(data.getUint32(4, Endian.little), 0);
        seqs.add(data.getUint32(0, Endian.little));
      });
      final ended = c.states
          .where((s) => s is WebSocketState$Disconnected)
          .cast<WebSocketState$Disconnected>()
          .first;
      await c.connect(url('/stream?format=binary&size=64&rate=0&count=$count'));
      final state = await ended.timeout(const Duration(seconds: 30));
      expect((state.closeCode, state.closeReason), (1000, 'done'));
      expect(seqs, hasLength(count));
      expect(
        [
          for (var i = 0; i < seqs.length; i++)
            if (seqs[i] != i) i,
        ].take(3),
        isEmpty,
        reason: 'out of order at these indexes',
      );
      await c.close();
    });

    serverTest('a numbered JSON stream arrives in order', () async {
      const count = 5000;
      final c = WebSocketClient.json(
        options: const WebSocketOptions(
          reconnect: WebSocketReconnect.disabled(),
        ),
      );
      final seqs = <int>[];
      c.messages.listen((m) => seqs.add((m['seq']! as num).toInt()));
      final ended = c.states
          .where((s) => s is WebSocketState$Disconnected)
          .cast<WebSocketState$Disconnected>()
          .first;
      await c.connect(
        url('/stream?format=json&size=256&rate=0&count=$count&unicode=1'),
      );
      final state = await ended.timeout(const Duration(seconds: 30));
      expect(state.closeCode, 1000);
      expect(seqs, [for (var i = 0; i < count; i++) i]);
      await c.close();
    });

    serverTest('JSON documents of 1 MiB and 17 MiB decode', () async {
      // The native transport parses up to 16 MiB into a tape off the Dart
      // thread and hands larger texts to Dart (DECISIONS.md D50). Not
      // 17 MiB under dart2wasm: with asserts on, as in tests, its
      // json.decode is quadratic in nested containers (an O(n) assert per
      // container start), about 25 s here; release builds decode 13 MiB
      // in 0.1 s (DECISIONS.md D55).
      for (final size in [1 << 20, if (!_isWasm) 17 << 20]) {
        final c = WebSocketClient.json(
          options: const WebSocketOptions(
            reconnect: WebSocketReconnect.disabled(),
          ),
        );
        final got = c.messages.first;
        await c.connect(
          url('/stream?format=json&size=$size&rate=0&count=1&unicode=1'),
        );
        final doc = await got.timeout(const Duration(seconds: 30));
        expect(doc['seq'], 0, reason: '$size');
        expect((doc['items']! as List).length, greaterThan(1000));
        await c.close();
      }
    });

    serverTest('binary flood with then=close ends with 1000 done', () async {
      final c = WebSocketClient.binary(
        options: const WebSocketOptions(
          reconnect: WebSocketReconnect.disabled(),
        ),
      );
      var n = 0;
      c.messages.listen((m) {
        expect(m, hasLength(64));
        n++;
      });
      final ended = c.states
          .where((s) => s is WebSocketState$Disconnected)
          .cast<WebSocketState$Disconnected>()
          .first;
      await c.connect(url('/flood?count=5000&size=64&kind=binary&then=close'));
      final state = await ended.timeout(const Duration(seconds: 30));
      expect(n, 5000);
      expect((state.closeCode, state.closeReason), (1000, 'done'));
      await c.close();
    });

    // `null`, not `false`: `false` would override a skipped group (T-1).
    serverTest('protocol ping and pong', () async {
      final c = client();
      final msgs = c.messages.first;
      await c.connect(url('/ping?payload=0102'));
      expect(await msgs.timeout(const Duration(seconds: 5)), 'pong-ok');
      await c.ping();
      await c.close();
    }, skip: caps.protocolPing ? null : 'no protocol pings on this platform');

    serverTest('the server pings, the platform answers', () async {
      // Browsers answer pings themselves; the server reports the pong.
      final c = client();
      final msgs = c.messages.take(3).toList();
      await c.connect(url('/ping?payload=0a0b0c&count=3&interval_ms=20'));
      expect(await msgs.timeout(const Duration(seconds: 5)), [
        'pong-ok',
        'pong-ok',
        'pong-ok',
      ]);
      await c.close();
    });

    serverTest(
      'a slow reader fills the queue; push waits, then drains',
      () async {
        final c = WebSocketClient.binary(
          options: const WebSocketOptions(
            reconnect: WebSocketReconnect.disabled(),
            sendHighWaterMark: 64 << 10,
          ),
        );
        final tag = id();
        await c.connect(url('/slow-read?pause_ms=300&bps=0&id=$tag'));
        final chunk = Uint8List(256 << 10);
        var waited = false;
        var sent = 0;
        for (var i = 0; i < 40 && !waited; i++) {
          final f = c.push(chunk);
          sent++;
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
        expect(
          c.metrics.bufferedAmount,
          lessThanOrEqualTo(64 << 10),
          reason: 'push completed above sendHighWaterMark',
        );
        // Everything pushed reaches the server, as `sent` binary messages
        // (Chrome fragments large ones). Polled: /slow-read never answers
        // a close and a browser cannot drop the TCP connection, so the
        // entry would not be finished before the browser gives up.
        var (messages, bytes) = (0, 0);
        for (var i = 0; i < 100 && bytes < sent * chunk.length; i++) {
          if (i > 0) {
            await Future<void>.delayed(const Duration(milliseconds: 50));
          }
          final log = await server().log(tag, done: false);
          (messages, bytes) = (0, 0);
          for (final f
              in (log['frames']! as List).cast<Map<String, Object?>>()) {
            if (f['op'] == 2) messages++;
            if (f['op'] case 0 || 2) bytes += f['len']! as int;
          }
        }
        expect((messages, bytes), (sent, sent * chunk.length));
        await c.close();
      },
    );
  });

  group('liveness', () {
    serverTest(
      'a server that stops reading and answering is dropped',
      () async {
        final c = WebSocketClient.binary(
          options: WebSocketOptions(
            reconnect: const WebSocketReconnect.disabled(),
            heartbeat: binaryHeartbeat(
              const Duration(milliseconds: 200),
              const Duration(milliseconds: 500),
            ),
          ),
        );
        final ended = c.states
            .where((s) => s is WebSocketState$Disconnected)
            .cast<WebSocketState$Disconnected>()
            .first;
        await c.connect(url('/silent?read=0'));
        final chunk = Uint8List(256 << 10);
        for (var i = 0; i < 64; i++) {
          c.push(chunk).ignore();
        }
        final state = await ended.timeout(const Duration(seconds: 10));
        expect(state.error, isA<WebSocketTimeoutException>());
        await c.close();
      },
    );

    serverTest('a silent idle server is dropped and reconnected', () async {
      final c = WebSocketClient.text(
        options: WebSocketOptions(
          reconnect: const WebSocketReconnect(
            minDelay: Duration(milliseconds: 10),
            maxDelay: Duration(milliseconds: 50),
          ),
          heartbeat: textHeartbeat(
            const Duration(milliseconds: 150),
            const Duration(milliseconds: 300),
          ),
        ),
      );
      final reconnecting = c.states
          .where((s) => s is WebSocketState$Reconnecting)
          .cast<WebSocketState$Reconnecting>()
          .first;
      await c.connect(url('/silent'));
      final state = await reconnecting.timeout(const Duration(seconds: 5));
      expect(state.cause, isA<WebSocketTimeoutException>());
      await c.close();
    });

    serverTest('an answering idle server stays connected', () async {
      final c = WebSocketClient.text(
        options: WebSocketOptions(
          reconnect: const WebSocketReconnect.disabled(),
          heartbeat: textHeartbeat(
            const Duration(milliseconds: 100),
            const Duration(milliseconds: 300),
          ),
        ),
      );
      final got = <String>[];
      c.messages.listen(got.add);
      await c.connect(url('/echo'));
      await Future<void>.delayed(const Duration(milliseconds: 800));
      expect(c.state.isOpen, isTrue);
      expect(c.metrics.connects, 1);
      expect(c.metrics.lastRoundTrip, isNotNull);
      // Application pongs are not delivered as messages.
      expect(got, isEmpty);
      await c.close();
    });
  });

  group('suspend and resume (D44)', () {
    WebSocketClient<String> lifecycle() => WebSocketClient.text(
      options: WebSocketOptions(
        disconnectTimeout: const Duration(milliseconds: 500),
        heartbeat: textHeartbeat(
          const Duration(milliseconds: 200),
          const Duration(milliseconds: 600),
        ),
      ),
    );

    serverTest('a live connection survives: one ping, same socket', () async {
      final c = lifecycle();
      await c.connect(url('/echo'));
      c.suspend();
      // Longer than interval + timeout: a running heartbeat would ping.
      await Future<void>.delayed(const Duration(seconds: 1));
      final watch = Stopwatch()..start();
      await c.resume().timeout(const Duration(seconds: 5));
      expect(watch.elapsed, lessThan(const Duration(milliseconds: 600)));
      expect(c.metrics.connects, 1);
      await roundTrip(c, 'after resume');
      await c.close();
    });

    serverTest('a silent peer is replaced at once after the timeout', () async {
      final c = lifecycle();
      final states = <WebSocketState>[];
      c.states.listen(states.add);
      // /silent never answers pings: the stand-in for a socket the OS
      // killed while the app was frozen.
      await c.connect(url('/silent'));
      c.suspend();
      await Future<void>.delayed(const Duration(seconds: 1));
      expect(c.state.isOpen, isTrue);
      final watch = Stopwatch()..start();
      await c.resume().timeout(const Duration(seconds: 5));
      expect(
        watch.elapsed,
        greaterThanOrEqualTo(const Duration(milliseconds: 600)),
      );
      expect(watch.elapsed, lessThan(const Duration(milliseconds: 1500)));
      expect(c.metrics.connects, 2);
      expect(
        states.whereType<WebSocketState$Reconnecting>().single.delay,
        Duration.zero,
      );
      await c.close();
    });

    serverTest('disconnect: true closes; resume() connects again', () async {
      final c = lifecycle();
      await c.connect(url('/echo'));
      c.suspend(disconnect: true);
      expect(c.state, isA<WebSocketState$Suspended>());
      await Future<void>.delayed(const Duration(milliseconds: 300));
      expect(c.metrics.connects, 1);
      await c.resume().timeout(const Duration(seconds: 5));
      await roundTrip(c);
      expect(c.metrics.connects, 2);
      await c.close();
    });
  });
}

final class _Events implements TransportListener {
  final Completer<void> opened = Completer<void>();
  final Completer<void> closed = Completer<void>();
  final List<String> texts = [];
  int closes = 0;
  bool throwOnFirst = false;
  void Function()? onText;
  void _text(String text) {
    if (throwOnFirst) {
      throwOnFirst = false;
      throw StateError('listener failed');
    }
    texts.add(text);
    onText?.call();
  }

  @override
  void onOpen(String? protocol) => opened.complete();
  @override
  void onTextBytes(Uint8List bytes) => _text(String.fromCharCodes(bytes));
  @override
  void onTextString(String text) => _text(text);
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
    closes++;
    if (!closed.isCompleted) closed.complete();
  }
}
