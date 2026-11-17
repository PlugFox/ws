// RFC 6455 edge cases ported from other implementations (TEST_PLAN P1–P17):
// Node `ws` receiver tests, tungstenite, gorilla/websocket, OkHttp, Python
// `websockets` and the Autobahn TestSuite.
import 'dart:convert';
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/src/protocol/frame.dart';
import 'package:ws/src/protocol/frame_reader.dart';
import 'package:ws/src/protocol/frame_writer.dart';
import 'package:ws/src/protocol/handshake.dart';

final class _Events implements FrameHandler {
  final List<String> events = <String>[];

  @override
  void onText(Uint8List payload) => events.add('text:${utf8.decode(payload)}');

  @override
  void onBinary(Uint8List payload) => events.add('binary:${payload.length}');

  @override
  void onPing(Uint8List payload) => events.add('ping:${payload.length}');

  @override
  void onPong(Uint8List payload) => events.add('pong:${payload.length}');

  @override
  void onClose(int? code, String reason) => events.add('close:$code:$reason');

  @override
  void onFailure(int code, String reason) => events.add('fail:$code');
}

Uint8List _frame(int opcode, List<int> payload, {bool fin = true}) =>
    FrameWriter(
      mask: false,
    ).frame(opcode, Uint8List.fromList(payload), fin: fin);

Uint8List _cat(List<Uint8List> parts) =>
    Uint8List.fromList([for (final p in parts) ...p]);

List<String> _read(
  List<int> bytes, {
  int max = 1 << 20,
  bool validateUtf8 = true,
}) {
  final h = _Events();
  FrameReader(
    h,
    maxMessageSize: max,
    validateUtf8: validateUtf8,
  ).add(Uint8List.fromList(bytes));
  return h.events;
}

Uint8List _close(int code, [String reason = '']) =>
    _frame(Opcode.close, [code >> 8, code & 0xFF, ...utf8.encode(reason)]);

void main() {
  group('header bits (P1, P2)', () {
    test('RSV2 and RSV3 fail with 1002', () {
      expect(_read([0xA1, 0x00]), ['fail:1002']);
      expect(_read([0x91, 0x00]), ['fail:1002']);
    });

    test('RSV1 on control and continuation frames fails', () {
      expect(_read([0xC9, 0x00]), ['fail:1002']);
      expect(
        _read([
          ..._frame(Opcode.text, [0x61], fin: false),
          0xC0,
          0x00,
        ]),
        ['fail:1002'],
      );
    });

    test('opcodes 3–7 and 0xB–0xF are reserved', () {
      for (final op in [3, 4, 5, 6, 7, 0xB, 0xC, 0xD, 0xE, 0xF]) {
        expect(_read([0x80 | op, 0x00]), ['fail:1002'], reason: 'opcode $op');
      }
    });
  });

  group('fragmentation (P3, P4, P9, P13)', () {
    test('a new data frame inside a fragmented message fails', () {
      for (final op in [Opcode.text, Opcode.binary]) {
        expect(
          _read(
            _cat([
              _frame(Opcode.binary, [1], fin: false),
              _frame(op, [2]),
            ]),
          ),
          ['fail:1002'],
          reason: 'opcode $op',
        );
      }
    });

    test('close between fragments is handled', () {
      expect(
        _read(
          _cat([
            _frame(Opcode.text, utf8.encode('par'), fin: false),
            _close(1000, 'bye'),
          ]),
        ),
        ['close:1000:bye'],
      );
    });

    test('text whose last fragment ends mid-codepoint fails with 1007', () {
      final euro = utf8.encode('€'); // E2 82 AC
      expect(
        _read(
          _cat([
            _frame(Opcode.text, [0x61], fin: false),
            _frame(Opcode.continuation, euro.sublist(0, 2)),
          ]),
        ),
        ['fail:1007'],
      );
    });

    test('invalid byte only in a later fragment fails with 1007', () {
      expect(
        _read(
          _cat([
            _frame(Opcode.text, utf8.encode('ok'), fin: false),
            _frame(Opcode.continuation, [0xC0, 0xAF]),
          ]),
        ),
        ['fail:1007'],
      );
    });

    test('single-frame text ending mid-codepoint fails with 1007', () {
      expect(_read([0x81, 0x01, 0xE2]), ['fail:1007']);
    });

    test('without validation the decoder sees the raw bytes', () {
      final h = _Events();
      final got = <int>[];
      FrameReader(
        _Raw(h, got),
        validateUtf8: false,
      ).add(Uint8List.fromList([0x81, 0x01, 0xE2]));
      expect(got, [0xE2]);
    });

    test('many empty continuation frames still make one message', () {
      final parts = <Uint8List>[
        _frame(Opcode.binary, const [], fin: false),
        for (var i = 0; i < 10000; i++)
          _frame(Opcode.continuation, const [], fin: false),
        _frame(Opcode.continuation, const [7]),
      ];
      expect(_read(_cat(parts)), ['binary:1']);
    });

    test('control frames keep their position between data frames', () {
      expect(
        _read(
          _cat([
            _frame(Opcode.text, utf8.encode('a')),
            _frame(Opcode.pong, const [1, 2, 3]),
            _frame(Opcode.ping, const []),
            _frame(Opcode.binary, const [1]),
          ]),
        ),
        ['text:a', 'pong:3', 'ping:0', 'binary:1'],
      );
    });
  });

  group('memory and fail-fast (review protocol.md F1, F2, F13)', () {
    test('tiny fragments do not pin the read chunks they came in', () {
      // Each read chunk: a 1-byte continuation and ~64 KiB of pings.
      final pings = _cat([
        for (var i = 0; i < 500; i++) _frame(Opcode.ping, List.filled(125, 0)),
      ]);
      Uint8List? message;
      final reader = FrameReader(_Message((m) => message = m));
      reader.add(_frame(Opcode.binary, const [1], fin: false));
      for (var i = 0; i < 200; i++) {
        reader.add(
          _cat([
            _frame(Opcode.continuation, const [2], fin: false),
            pings,
          ]),
        );
      }
      reader.add(_frame(Opcode.continuation, const [3]));
      expect(message!.length, 202);
      expect(message!.buffer.lengthInBytes, lessThan(4096));
    });

    test('a bad header fails before its payload arrives', () {
      // Reserved opcode / stray continuation / new message mid-fragment,
      // each announcing 1 MiB that never comes.
      List<int> header(int b0) => [b0, 0x7F, 0, 0, 0, 0, 0, 0x10, 0, 0];
      expect(_read(header(0x83)), ['fail:1002']);
      expect(_read(header(0x80)), ['fail:1002']);
      expect(
        _read([
          ..._frame(Opcode.text, [0x61], fin: false),
          ...header(0x82),
        ]),
        ['fail:1002'],
      );
    });
  });

  group('close frames (P6, P12)', () {
    const valid = [
      1000, 1001, 1002, 1003, 1007, 1008, 1009, 1010, 1011, 1012, 1013, //
      1014, 3000, 3999, 4000, 4999,
    ];
    const invalid = [
      0, 1, 999, 1004, 1005, 1006, 1015, 1016, 1100, 2000, 2999, 5000, //
      65535,
    ];

    test('valid codes on the wire are accepted', () {
      for (final code in valid) {
        expect(_read(_close(code)), ['close:$code:'], reason: '$code');
      }
    });

    test('invalid codes on the wire fail with 1002', () {
      for (final code in invalid) {
        expect(_read(_close(code)), ['fail:1002'], reason: '$code');
      }
    });

    test('empty close, and control payloads of exactly 125 bytes', () {
      expect(_read([0x88, 0x00]), ['close:null:']);
      expect(_read(_frame(Opcode.ping, List.filled(125, 1))), ['ping:125']);
      expect(_read(_frame(Opcode.pong, List.filled(125, 1))), ['pong:125']);
      expect(_read(_close(1000, 'x' * 123)), ['close:1000:${'x' * 123}']);
    });
  });

  group('lengths (P7, P8, P10, P11)', () {
    test('64-bit lengths above 2^32 are not truncated', () {
      // 2^32 + 5: on a 32-bit wrap this would look like a 5-byte frame.
      expect(_read([0x82, 0x7F, 0, 0, 0, 1, 0, 0, 0, 5, 1, 2, 3, 4, 5]), [
        'fail:1009',
      ]);
      expect(
        _read([0x82, 0x7F, 0x7F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]),
        ['fail:1009'],
      );
    });

    test('non-minimal length encodings are accepted (as ws, gorilla do)', () {
      expect(_read([0x82, 0x7E, 0x00, 0x02, 9, 9]), ['binary:2']);
      expect(_read([0x82, 0x7F, 0, 0, 0, 0, 0, 0, 0, 0x02, 9, 9]), [
        'binary:2',
      ]);
    });

    test('boundary sizes, in one chunk', () {
      final sizes = [0, 1, 125, 126, 127, 65535, 65536];
      expect(
        _read(
          _cat([
            for (final n in sizes) _frame(Opcode.binary, List.filled(n, 1)),
          ]),
        ),
        [for (final n in sizes) 'binary:$n'],
      );
    });

    test('thousands of tiny frames in one chunk', () {
      final events = _read(
        _cat([
          for (var i = 0; i < 5000; i++) _frame(Opcode.text, [0x61]),
        ]),
      );
      expect(events, hasLength(5000));
    });

    test('a message exactly at the limit passes, one more byte fails', () {
      expect(
        _read(
          _cat([
            _frame(Opcode.binary, List.filled(60, 0), fin: false),
            _frame(Opcode.ping, const []),
            _frame(Opcode.continuation, List.filled(40, 0)),
          ]),
          max: 100,
        ),
        ['ping:0', 'binary:100'],
      );
      expect(
        _read(
          _cat([
            _frame(Opcode.binary, List.filled(60, 0), fin: false),
            _frame(Opcode.continuation, List.filled(41, 0)),
          ]),
          max: 100,
        ),
        ['fail:1009'],
      );
    });
  });

  group('writer (P17)', () {
    test('close reason of 123 bytes, multibyte at the edge', () {
      final h = _Events();
      FrameReader(
        h,
        expectMasked: true,
      ).add(FrameWriter(mask: true).close(1000, '${'é' * 61}x'));
      expect(h.events, ['close:1000:${'é' * 61}x']);
    });

    test('close without a code has an empty payload', () {
      final frame = FrameWriter(mask: false).close(null);
      expect(frame, [0x88, 0x00]);
    });
  });

  group('handshake (P14–P16)', () {
    final request = HandshakeRequest(
      Uri.parse('ws://h/'),
      protocols: const ['v1'],
    );
    String head(Map<String, String?> override) {
      final headers = <String, String?>{
        'Upgrade': 'websocket',
        'Connection': 'Upgrade',
        'Sec-WebSocket-Accept': acceptKey(request.key),
        ...override,
      };
      return 'HTTP/1.1 101 Switching Protocols\r\n'
          '${[for (final MapEntry(:key, :value) in headers.entries)
            if (value != null) '$key: $value'].join('\r\n')}';
    }

    String? validate(String text) =>
        HandshakeResponse.parse(latin1.encode(text)).validate(request);

    Matcher failsWith(String message) => throwsA(
      isA<HandshakeError>().having(
        (e) => e.message,
        'message',
        contains(message),
      ),
    );

    test('token lists and header case', () {
      expect(validate(head({'Upgrade': 'foo, WebSocket'})), isNull);
      expect(validate(head({'Connection': 'keep-alive, upgrade'})), isNull);
      expect(
        validate(
          'HTTP/1.1 101 OK\r\nUPGRADE: websocket\r\nconnection: Upgrade\r\n'
          'sec-websocket-accept: ${acceptKey(request.key)}',
        ),
        isNull,
      );
    });

    test('missing or wrong headers', () {
      expect(() => validate(head({'Upgrade': null})), failsWith('Upgrade'));
      expect(() => validate(head({'Upgrade': 'h2c'})), failsWith('Upgrade'));
      expect(
        () => validate(head({'Connection': null})),
        failsWith('Connection'),
      );
      expect(
        () => validate(head({'Sec-WebSocket-Accept': null})),
        failsWith('Accept'),
      );
      expect(
        () => validate(
          '${head({})}\r\nSec-WebSocket-Accept: ${acceptKey(request.key)}',
        ),
        failsWith('Accept'),
        reason: 'a duplicated Accept is not the expected value',
      );
    });

    test('extensions and subprotocols', () {
      expect(
        () =>
            validate(head({'Sec-WebSocket-Extensions': 'permessage-deflate'})),
        failsWith('extension'),
      );
      expect(validate(head({'Sec-WebSocket-Extensions': ''})), isNull);
      expect(validate(head({'Sec-WebSocket-Protocol': 'v1'})), 'v1');
      expect(
        () => validate(head({'Sec-WebSocket-Protocol': 'v2'})),
        failsWith('subprotocol'),
      );
      final bare = HandshakeRequest(Uri.parse('ws://h/'));
      expect(
        () => HandshakeResponse.parse(
          latin1.encode(
            head({
              'Sec-WebSocket-Accept': acceptKey(bare.key),
              'Sec-WebSocket-Protocol': 'v1',
            }),
          ),
        ).validate(bare),
        failsWith('subprotocol'),
        reason: 'a protocol when none was offered',
      );
    });

    test('status errors keep status and headers', () {
      final error = () {
        try {
          validate('HTTP/1.1 429 Too Many Requests\r\nRetry-After: 5');
        } on HandshakeError catch (e) {
          return e;
        }
        return null;
      }()!;
      expect(error.statusCode, 429);
      expect(error.headers['retry-after'], '5');
      expect(
        error.toString(),
        'HandshakeError: Unexpected HTTP status 429 '
        'Too Many Requests',
      );
    });

    test('junk and partial heads', () {
      expect(HandshakeResponse.headEnd(latin1.encode('HTTP/1.1 101 Sw')), -1);
      expect(HandshakeResponse.headEnd(latin1.encode('a\r\n\r\nb')), 1);
      expect(
        () => HandshakeResponse.parse(latin1.encode('220 smtp ESMTP')),
        throwsFormatException,
      );
      expect(
        () => HandshakeResponse.parse(latin1.encode('HTTP/1.1 abc OK')),
        throwsFormatException,
      );
      expect(
        () => HandshakeResponse.parse(latin1.encode('HTTP/1.1')),
        throwsFormatException,
      );
    });

    test('user info becomes Basic authorization (UTF-8)', () {
      String authorization(String url) {
        final text = latin1.decode(HandshakeRequest(Uri.parse(url)).encode());
        return RegExp(
          r'Authorization: Basic (\S+)\r\n',
        ).firstMatch(text)!.group(1)!;
      }

      expect(
        utf8.decode(base64.decode(authorization('ws://us%40er:p%3Ass@h/'))),
        'us@er:p:ss',
      );
      expect(
        utf8.decode(base64.decode(authorization('ws://юзер:пароль@h/'))),
        'юзер:пароль',
      );
    });

    test('IPv6 hosts are bracketed, default ports omitted', () {
      String host(String url) => RegExp(r'Host: (\S+)\r\n')
          .firstMatch(latin1.decode(HandshakeRequest(Uri.parse(url)).encode()))!
          .group(1)!;
      expect(host('ws://[::1]:8080/'), '[::1]:8080');
      expect(host('wss://[::1]/'), '[::1]');
      expect(host('ws://h:80/'), 'h');
      expect(host('wss://h:443/'), 'h');
      expect(host('ws://h:443/'), 'h:443');
    });
  });
}

final class _Raw implements FrameHandler {
  _Raw(this._events, this._text);
  final _Events _events;
  final List<int> _text;
  @override
  void onText(Uint8List payload) => _text.addAll(payload);
  @override
  void onBinary(Uint8List payload) => _events.onBinary(payload);
  @override
  void onPing(Uint8List payload) {}
  @override
  void onPong(Uint8List payload) {}
  @override
  void onClose(int? code, String reason) {}
  @override
  void onFailure(int code, String reason) => _events.onFailure(code, reason);
}

final class _Message implements FrameHandler {
  _Message(this._onBinary);
  final void Function(Uint8List) _onBinary;
  @override
  void onBinary(Uint8List payload) => _onBinary(payload);
  @override
  void onText(Uint8List payload) {}
  @override
  void onPing(Uint8List payload) {}
  @override
  void onPong(Uint8List payload) {}
  @override
  void onClose(int? code, String reason) {}
  @override
  void onFailure(int code, String reason) => fail('failure $code $reason');
}
