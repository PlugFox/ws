/// Behaviour every native-platform transport must share: run against the
/// pure-Dart transport here and against the Rust transport in `native/`.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/src/protocol/frame.dart';
import 'package:ws/src/protocol/frame_writer.dart';
import 'package:ws/src/protocol/handshake.dart';
import 'package:ws/transport.dart';
import 'package:ws/ws.dart';

/// A dart:io WebSocket server; [onSocket] handles each connection.
/// [onRequest] sees the request before the 101 is sent (the client may
/// finish connecting before [onSocket] runs).
Future<HttpServer> ioServer(
  void Function(WebSocket socket, HttpRequest request) onSocket, {
  int? rejectStatus,
  void Function(HttpRequest request)? onRequest,
}) async {
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  server.listen((request) async {
    onRequest?.call(request);
    if (rejectStatus != null) {
      request.response.statusCode = rejectStatus;
      await request.response.close();
      return;
    }
    final socket = await WebSocketTransformer.upgrade(
      request,
      protocolSelector: (protocols) => protocols.last,
    );
    onSocket(socket, request);
  });
  return server;
}

/// A connection accepted by [rawServer]: write with [socket], read what the
/// client sent after the handshake from [received].
typedef RawConnection = ({Socket socket, BytesBuilder received});

/// A raw TCP server that completes the handshake and then lets the test
/// write arbitrary bytes.
Future<(ServerSocket, Stream<RawConnection>)> rawServer() async {
  final server = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
  final sockets = StreamController<RawConnection>();
  server.listen((socket) {
    final head = BytesBuilder();
    late StreamSubscription<Uint8List> sub;
    sub = socket.listen((data) {
      head.add(data);
      final bytes = head.toBytes();
      final end = HandshakeResponse.headEnd(bytes);
      if (end < 0) return;
      final text = latin1.decode(bytes.sublist(0, end));
      final key = RegExp(
        r'Sec-WebSocket-Key: (\S+)',
      ).firstMatch(text)!.group(1)!;
      socket.add(
        latin1.encode(
          'HTTP/1.1 101 Switching Protocols\r\n'
          'Upgrade: websocket\r\nConnection: Upgrade\r\n'
          'Sec-WebSocket-Accept: ${acceptKey(key)}\r\n\r\n',
        ),
      );
      final received = BytesBuilder();
      sub.onData(received.add);
      sockets.add((socket: socket, received: received));
    });
  });
  return (server, sockets.stream);
}

Uint8List frame(int opcode, List<int> payload, {bool fin = true}) =>
    FrameWriter(
      mask: false,
    ).frame(opcode, Uint8List.fromList(payload), fin: fin);

/// Registers the conformance tests for the transport made by [create].
void transportConformance(WebSocketTransport Function() create) {
  late WebSocketTransport transport;
  setUp(() => WebSocketTransports.use(transport = create()));
  tearDown(() => WebSocketTransports.use(null));

  group('against dart:io server', () {
    late HttpServer server;
    final headers = <String, String?>{};

    setUp(() async {
      server = await ioServer(
        (socket, request) {
          socket.listen((m) {
            if (m == 'close-4001') return socket.close(4001, 'bye').ignore();
            socket.add(m);
          });
        },
        onRequest: (request) =>
            headers['x-token'] = request.headers.value('x-token'),
      );
    });
    tearDown(() => server.close(force: true));

    String url() => 'ws://127.0.0.1:${server.port}/echo';

    test('echo text (ascii, unicode, 1 MB) and binary', () async {
      final client = WebSocketClient.text(
        options: const WebSocketOptions(
          protocols: ['v1', 'v2'],
          vm: WebSocketVmOptions(headers: {'X-Token': 'secret'}),
        ),
      );
      final got = <String>[];
      final all = Completer<void>();
      client.messages.listen((m) {
        got.add(m);
        if (got.length == 3) all.complete();
      });
      await client.connect(url());
      expect((client.state as WebSocketState$Open).protocol, 'v2');
      expect(headers['x-token'], 'secret');
      final big = 'x' * (1 << 20);
      await client.push('hello');
      await client.push('мир ✓ 𝄞');
      await client.push(big);
      await all.future;
      expect(got, ['hello', 'мир ✓ 𝄞', big]);
      expect(client.metrics.transport, transport.name);
      await client.close();

      final binary = WebSocketClient.binary();
      final echoed = Completer<Uint8List>();
      binary.messages.listen(echoed.complete);
      await binary.connect(url());
      await binary.push(
        Uint8List.fromList(List<int>.generate(70000, (i) => i)),
      );
      expect((await echoed.future).length, 70000);
      await binary.close();
    });

    test('json client end to end', () async {
      final client = WebSocketClient.json();
      final got = Completer<Map<String, Object?>>();
      client.messages.listen(got.complete);
      await client.connect(url());
      await client.push({
        'a': [1, 2.5, 'ü', null],
      });
      expect(await got.future, {
        'a': [1, 2.5, 'ü', null],
      });
      await client.close();
    });

    test('json corpus decodes like jsonDecode', () async {
      // Raw texts, so escapes and number forms reach the transport's own
      // decoder (simd-json tape on native, dart:convert on dart:io).
      const corpus = [
        r'{"esc":"\" \\ \/ \b \f \n \r \t","u":"\u0041\u00e9\ud834\udd1e"}',
        '{"n":[0,-0,1,-1,123456789012345678,9007199254740993,1.5,-0.0]}',
        '{"e":[1e3,1E-3,2.5e+10,1.7976931348623157e308,5e-324]}',
        '{"deep":{"a":[{"b":[{"c":[]}]},{}],"t":true,"f":false,"z":null}}',
        r'{"k\u00e9y":1,"":"empty","ünï":"cødé ✓"}',
        '{"dup":1,"dup":2}',
        ' { "ws" : [ 1 , 2 ] } ',
      ];
      final server = await ioServer((socket, _) => corpus.forEach(socket.add));
      final client = WebSocketClient.json();
      final got = <Map<String, Object?>>[];
      final done = Completer<void>();
      client.messages.listen((m) {
        got.add(m);
        if (got.length == corpus.length) done.complete();
      });
      await client.connect('ws://127.0.0.1:${server.port}');
      await done.future.timeout(const Duration(seconds: 5));
      for (var i = 0; i < corpus.length; i++) {
        expect(_plainJson(got[i]), jsonDecode(corpus[i]), reason: corpus[i]);
      }
      await client.close();
      await server.close(force: true);
    });

    test('protocol ping', () async {
      final client = WebSocketClient.text();
      await client.connect(url());
      await client.ping();
      expect(client.metrics.lastRoundTrip, isNotNull);
      await client.close();
    });

    test('server close code is reported and policy applied', () async {
      final client = WebSocketClient.text(
        options: WebSocketOptions(
          reconnect: WebSocketReconnect(
            shouldReconnect: (cause, attempt) => false,
          ),
        ),
      );
      await client.connect(url());
      await client.push('close-4001');
      final state = await client.states.firstWhere(
        (s) => s is WebSocketState$Disconnected,
      );
      final error = (state as WebSocketState$Disconnected).error;
      expect(
        error,
        isA<WebSocketClosedException>()
            .having((e) => e.code, 'code', 4001)
            .having((e) => e.reason, 'reason', 'bye'),
      );
      await client.close();
    });

    test('disconnect is a clean close', () async {
      final closed = Completer<int?>();
      final s = await ioServer((socket, _) {
        socket.listen(null, onDone: () => closed.complete(socket.closeCode));
      });
      final client = WebSocketClient.text();
      await client.connect('ws://127.0.0.1:${s.port}');
      await client.disconnect(4000, 'done');
      expect(await closed.future, 4000);
      expect(client.state, isA<WebSocketState$Disconnected>());
      await s.close(force: true);
    });
  });

  test('handshake rejection is terminal for 403', () async {
    final server = await ioServer((_, _) {}, rejectStatus: 403);
    final client = WebSocketClient.text();
    await expectLater(
      client.connect('ws://127.0.0.1:${server.port}'),
      throwsA(
        isA<WebSocketHandshakeException>().having(
          (e) => e.statusCode,
          'status',
          403,
        ),
      ),
    );
    await client.close();
    await server.close(force: true);
  });

  test('connection refused is retried', () async {
    final probe = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    final port = probe.port;
    await probe.close();
    final client = WebSocketClient.text(
      options: const WebSocketOptions(
        reconnect: WebSocketReconnect(minDelay: Duration(milliseconds: 10)),
      ),
    );
    unawaited(client.connect('ws://127.0.0.1:$port'));
    final state = await client.states.firstWhere(
      (s) => s is WebSocketState$Reconnecting,
    );
    expect(
      (state as WebSocketState$Reconnecting).cause,
      isA<WebSocketConnectionException>(),
    );
    await client.close();
  });

  group('against a raw server', () {
    test('fragments, split writes, ping in between', () async {
      final (server, sockets) = await rawServer();
      final client = WebSocketClient.text();
      final got = <String>[];
      final done = Completer<void>();
      client.messages.listen((m) {
        got.add(m);
        if (got.length == 2) done.complete();
      });
      unawaited(client.connect('ws://127.0.0.1:${server.port}'));
      final (:socket, received: _) = await sockets.first;
      final bytes = <int>[
        ...frame(Opcode.text, utf8.encode('frag'), fin: false),
        ...frame(Opcode.ping, const [1]),
        ...frame(Opcode.continuation, utf8.encode('mented ✓')),
        ...frame(Opcode.text, utf8.encode('next')),
      ];
      // One byte at a time.
      for (final b in bytes) {
        socket.add([b]);
        await socket.flush();
      }
      await done.future;
      expect(got, ['fragmented ✓', 'next']);
      await client.close();
      await server.close();
    });

    test('handshake response and a large frame in one write', () async {
      final server = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
      server.listen((socket) {
        socket.done.ignore(); // the client may reset when closing
        final head = BytesBuilder();
        socket.listen((data) {
          head.add(data);
          final bytes = head.toBytes();
          final end = HandshakeResponse.headEnd(bytes);
          if (end < 0) return;
          final key = RegExp(
            r'Sec-WebSocket-Key: (\S+)',
          ).firstMatch(latin1.decode(bytes.sublist(0, end)))!.group(1)!;
          socket.add([
            ...latin1.encode(
              'HTTP/1.1 101 Switching Protocols\r\n'
              'Upgrade: websocket\r\nConnection: Upgrade\r\n'
              'Sec-WebSocket-Accept: ${acceptKey(key)}\r\n\r\n',
            ),
            ...frame(Opcode.binary, List<int>.filled(100000, 7)),
          ]);
        }, onError: (Object _) {}); // the client may reset when closing
      });
      final client = WebSocketClient.binary();
      final got = Completer<Uint8List>();
      client.messages.listen(got.complete);
      await client.connect('ws://127.0.0.1:${server.port}');
      expect((await got.future).length, 100000);
      await client.close();
      await server.close();
    });

    test('masked server frame fails the connection with 1002', () async {
      final (server, sockets) = await rawServer();
      final client = WebSocketClient.text();
      unawaited(client.connect('ws://127.0.0.1:${server.port}'));
      final (:socket, :received) = await sockets.first;
      socket.add(FrameWriter(mask: true).frame(Opcode.text, utf8.encode('x')));
      final state = await client.states.firstWhere(
        (s) => s is WebSocketState$Disconnected,
      );
      expect(
        (state as WebSocketState$Disconnected).error,
        isA<WebSocketClosedException>().having((e) => e.code, 'code', 1002),
      );
      // The client sent a (masked) close frame with 1002.
      for (var i = 0; i < 100 && received.isEmpty; i++) {
        await Future<void>.delayed(const Duration(milliseconds: 10));
      }
      final out = received.toBytes();
      expect(out[0], 0x88);
      expect(out[1] & 0x80, 0x80);
      final key = out.sublist(2, 6);
      expect([out[6] ^ key[0], out[7] ^ key[1]], [0x03, 0xEA]); // 1002
      await client.close();
      await server.close();
    });

    test('silent server: heartbeat timeout reconnects', () async {
      final (server, sockets) = await rawServer();
      final client = WebSocketClient.text(
        options: const WebSocketOptions(
          heartbeat: WebSocketHeartbeat(
            interval: Duration(milliseconds: 100),
            timeout: Duration(milliseconds: 100),
          ),
          reconnect: WebSocketReconnect(minDelay: Duration(milliseconds: 10)),
        ),
      );
      unawaited(client.connect('ws://127.0.0.1:${server.port}'));
      await sockets.first; // never answers pings
      final state = await client.states.firstWhere(
        (s) => s is WebSocketState$Reconnecting,
      );
      expect(
        (state as WebSocketState$Reconnecting).cause,
        isA<WebSocketTimeoutException>(),
      );
      await client.close();
      await server.close();
    });

    test('oversized message closes with 1009', () async {
      final (server, sockets) = await rawServer();
      final client = WebSocketClient.binary(
        options: const WebSocketOptions(maxMessageSize: 1000),
      );
      unawaited(client.connect('ws://127.0.0.1:${server.port}'));
      final (:socket, received: _) = await sockets.first;
      socket.add(frame(Opcode.binary, List<int>.filled(2000, 0)));
      final state = await client.states.firstWhere(
        (s) => s is WebSocketState$Disconnected,
      );
      expect(
        (state as WebSocketState$Disconnected).error,
        isA<WebSocketClosedException>().having((e) => e.code, 'code', 1009),
      );
      await client.close();
      await server.close();
    });
  });
}

/// Deep copy of a decoded message so lazy views compare with `equals`.
Object? _plainJson(Object? v) => switch (v) {
  final Map<String, Object?> m => {
    for (final e in m.entries) e.key: _plainJson(e.value),
  },
  final List<Object?> l => [for (final e in l) _plainJson(e)],
  _ => v,
};
