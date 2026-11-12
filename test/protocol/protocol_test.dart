import 'dart:convert';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/src/protocol/frame.dart';
import 'package:ws/src/protocol/frame_reader.dart';
import 'package:ws/src/protocol/frame_writer.dart';
import 'package:ws/src/protocol/handshake.dart';
import 'package:ws/src/protocol/sha1.dart';
import 'package:ws/src/protocol/utf8_text.dart';
import 'package:ws/src/protocol/utf8_validator.dart';

final class _Events implements FrameHandler {
  final List<Object> events = <Object>[];

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

/// Server-side (unmasked) frame for client tests.
Uint8List _frame(int opcode, List<int> payload, {bool fin = true}) =>
    FrameWriter(
      mask: false,
    ).frame(opcode, Uint8List.fromList(payload), fin: fin);

Uint8List _cat(List<Uint8List> parts) =>
    Uint8List.fromList([for (final p in parts) ...p]);

void main() {
  group('sha1', () {
    String hex(List<int> b) =>
        b.map((x) => x.toRadixString(16).padLeft(2, '0')).join();
    test('known vectors', () {
      expect(hex(sha1(const [])), 'da39a3ee5e6b4b0d3255bfef95601890afd80709');
      expect(
        hex(sha1(ascii.encode('abc'))),
        'a9993e364706816aba3e25717850c26c9cd0d89d',
      );
      expect(
        hex(
          sha1(
            ascii.encode(
              'abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq',
            ),
          ),
        ),
        '84983e441c3bd26ebaae4aa1f95129e5e54670f1',
      );
    });
    test('RFC 6455 accept key', () {
      expect(
        acceptKey('dGhlIHNhbXBsZSBub25jZQ=='),
        's3pPLMBiTxaQ9kYGzzhZRbK+xOo=',
      );
    });
  });

  group('utf8', () {
    test('valid and invalid', () {
      expect(Utf8Validator.validate(utf8.encode('héllo 𝄞 мир')), isTrue);
      expect(Utf8Validator.validate(Uint8List.fromList([0xC0, 0x80])), isFalse);
      expect(
        Utf8Validator.validate(Uint8List.fromList([0xED, 0xA0, 0x80])),
        isFalse,
      ); // surrogate
      expect(
        Utf8Validator.validate(Uint8List.fromList([0xF4, 0x90, 0x80, 0x80])),
        isFalse,
      ); // > U+10FFFF
      expect(Utf8Validator.validate(Uint8List.fromList([0xE2, 0x82])), isFalse);
    });
    test('ASCII scan on unaligned views of every length', () {
      final base = Uint8List(200)..fillRange(0, 200, 0x61);
      for (var start = 0; start < 9; start++) {
        for (var len = 0; len < 120; len++) {
          final view = Uint8List.sublistView(base, start, start + len);
          expect(Utf8Validator.isAscii(view), isTrue);
          if (len == 0) continue;
          for (final at in {0, len ~/ 2, len - 1}) {
            final copy = Uint8List.fromList(base);
            copy[start + at] = 0xC3;
            expect(
              Utf8Validator.isAscii(
                Uint8List.sublistView(copy, start, start + len),
              ),
              isFalse,
              reason: 'start $start len $len at $at',
            );
          }
        }
      }
    });
    test('decodeUtf8', () {
      expect(decodeUtf8(utf8.encode('plain ascii')), 'plain ascii');
      expect(decodeUtf8(utf8.encode('мир ✓')), 'мир ✓');
      expect(
        () => decodeUtf8(Uint8List.fromList([0x61, 0xFF])),
        throwsFormatException,
      );
    });
    test('split anywhere', () {
      final bytes = utf8.encode('a𝄞b€c');
      for (var i = 0; i <= bytes.length; i++) {
        final v = Utf8Validator()
          ..add(bytes, 0, i)
          ..add(bytes, i);
        expect(v.isComplete, isTrue, reason: 'split at $i');
      }
    });
  });

  group('reader', () {
    test('single frames, every split point', () {
      final data = _cat([
        _frame(Opcode.text, utf8.encode('hello')),
        _frame(Opcode.binary, List<int>.filled(300, 7)),
        _frame(Opcode.ping, const [1, 2]),
        _frame(Opcode.binary, List<int>.filled(70000, 1)),
        _frame(Opcode.close, [0x03, 0xE8, ...utf8.encode('bye')]),
      ]);
      for (final step in [1, 2, 3, 7, 13, 100, 4096, data.length]) {
        final h = _Events();
        final r = FrameReader(h);
        for (var i = 0; i < data.length; i += step) {
          r.add(
            Uint8List.sublistView(data, i, math.min(i + step, data.length)),
          );
        }
        expect(h.events, [
          'text:hello',
          'binary:300',
          'ping:2',
          'binary:70000',
          'close:1000:bye',
        ], reason: 'step $step');
      }
    });

    test('fragmented text with interleaved ping', () {
      final h = _Events();
      FrameReader(h).add(
        _cat([
          _frame(Opcode.text, utf8.encode('he'), fin: false),
          _frame(Opcode.ping, const []),
          _frame(Opcode.continuation, utf8.encode('ll'), fin: false),
          _frame(Opcode.continuation, utf8.encode('o'), fin: true),
        ]),
      );
      expect(h.events, ['ping:0', 'text:hello']);
    });

    test('UTF-8 split across fragments is valid', () {
      final bytes = utf8.encode('𝄞');
      final h = _Events();
      FrameReader(h).add(
        _cat([
          _frame(Opcode.text, bytes.sublist(0, 2), fin: false),
          _frame(Opcode.continuation, bytes.sublist(2)),
        ]),
      );
      expect(h.events, ['text:𝄞']);
    });

    test('invalid UTF-8 fails fast in the first fragment', () {
      final h = _Events();
      FrameReader(h).add(_frame(Opcode.text, const [0x61, 0xFF], fin: false));
      expect(h.events, ['fail:1007']);
    });

    test('protocol violations', () {
      Object failure(List<int> bytes, {int max = 1 << 20}) {
        final h = _Events();
        FrameReader(h, maxMessageSize: max).add(Uint8List.fromList(bytes));
        return h.events.single;
      }

      expect(failure([0xC1, 0x00]), 'fail:1002'); // RSV1
      expect(failure([0x81, 0x80, 0, 0, 0, 0]), 'fail:1002'); // masked
      expect(failure([0x09, 0x00]), 'fail:1002'); // fragmented ping
      expect(
        failure([0x89, 126, 0, 126, ...List.filled(126, 0)]),
        'fail:1002',
      ); // long ping
      expect(failure([0x80, 0x00]), 'fail:1002'); // stray continuation
      expect(failure([0x83, 0x00]), 'fail:1002'); // reserved opcode
      expect(failure([0x88, 0x01, 0x03]), 'fail:1002'); // 1-byte close
      expect(failure([0x88, 0x02, 0x03, 0xED]), 'fail:1002'); // code 1005
      expect(
        failure([0x88, 0x04, 0x03, 0xE8, 0xC0, 0x80]),
        'fail:1007',
      ); // bad reason
      expect(
        failure([0x82, 0x7F, 0x80, 0, 0, 0, 0, 0, 0, 0]),
        'fail:1002',
      ); // 64-bit length MSB
    });

    test('size limit is checked before allocation', () {
      final h = _Events();
      FrameReader(
        h,
        maxMessageSize: 1000,
      ).add(Uint8List.fromList([0x82, 0x7F, 0, 0, 0, 1, 0, 0, 0, 0]));
      expect(h.events, ['fail:1009']);
    });

    test('fragmented message limit', () {
      final h = _Events();
      FrameReader(h, maxMessageSize: 10).add(
        _cat([
          _frame(Opcode.binary, List<int>.filled(8, 0), fin: false),
          _frame(Opcode.continuation, List<int>.filled(8, 0)),
        ]),
      );
      expect(h.events, ['fail:1009']);
    });

    test('stops after close', () {
      final h = _Events();
      FrameReader(h).add(
        _cat([
          _frame(Opcode.close, const []),
          _frame(Opcode.text, utf8.encode('late')),
        ]),
      );
      expect(h.events, ['close:null:']);
    });
  });

  group('writer', () {
    test('masked frames round-trip through a server reader', () {
      final writer = FrameWriter(mask: true);
      final rnd = math.Random(1);
      for (final n in [0, 1, 3, 4, 5, 125, 126, 127, 65535, 65536, 100003]) {
        final payload = Uint8List.fromList(
          List<int>.generate(n, (_) => rnd.nextInt(256)),
        );
        final h = _Recorder();
        FrameReader(
          h,
          expectMasked: true,
        ).add(writer.frame(Opcode.binary, payload));
        expect(h.binary.single, payload, reason: 'length $n');
      }
    });

    test('close frame', () {
      final h = _Events();
      FrameReader(
        h,
        expectMasked: true,
      ).add(FrameWriter(mask: true).close(4000, 'done'));
      expect(h.events, ['close:4000:done']);
    });
  });

  group('handshake', () {
    test('request and response', () {
      final request = HandshakeRequest(
        Uri.parse('wss://example.com:8443/socket?x=1'),
        headers: {'Authorization': 'Bearer t'},
        protocols: ['v2.json', 'v1.json'],
      );
      final text = latin1.decode(request.encode());
      expect(text, startsWith('GET /socket?x=1 HTTP/1.1\r\n'));
      expect(text, contains('Host: example.com:8443\r\n'));
      expect(text, contains('Sec-WebSocket-Protocol: v2.json, v1.json\r\n'));
      expect(text, contains('Authorization: Bearer t\r\n'));
      expect(text, endsWith('\r\n\r\n'));

      final response = HandshakeResponse.parse(
        latin1.encode(
          'HTTP/1.1 101 Switching Protocols\r\n'
          'Upgrade: websocket\r\nConnection: Upgrade\r\n'
          'Sec-WebSocket-Accept: ${acceptKey(request.key)}\r\n'
          'Sec-WebSocket-Protocol: v1.json',
        ),
      );
      expect(response.validate(request), 'v1.json');
    });

    test('errors', () {
      final request = HandshakeRequest(Uri.parse('ws://h/'));
      HandshakeError error(String head) {
        try {
          HandshakeResponse.parse(latin1.encode(head)).validate(request);
        } on HandshakeError catch (e) {
          return e;
        }
        fail('no error');
      }

      expect(error('HTTP/1.1 401 Unauthorized\r\nX: y').statusCode, 401);
      expect(
        error(
          'HTTP/1.1 101 OK\r\nUpgrade: websocket\r\n'
          'Connection: Upgrade\r\nSec-WebSocket-Accept: nope',
        ).message,
        contains('Accept'),
      );
      expect(
        () => HandshakeRequest(
          Uri.parse('ws://h/'),
          headers: {'Upgrade': 'x'},
        ).encode(),
        throwsArgumentError,
      );
      expect(
        () => HandshakeRequest(
          Uri.parse('ws://h/'),
          headers: {'X': 'a\r\nInjected: 1'},
        ).encode(),
        throwsArgumentError,
      );
    });
  });
}

final class _Recorder implements FrameHandler {
  final List<Uint8List> binary = <Uint8List>[];
  @override
  void onBinary(Uint8List payload) => binary.add(Uint8List.fromList(payload));
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
