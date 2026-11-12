// Message codecs on their own (TEST_PLAN C16): every branch, including the
// ones only the browser (strings) or the native JSON mode reach.
import 'dart:convert';
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/src/client/codec.dart';
import 'package:ws/src/client/exceptions.dart';
import 'package:ws/src/transport/transport.dart';

final _bad = Uint8List.fromList([0x61, 0xC0, 0xAF]);
Uint8List _u(String s) => utf8.encode(s);

void main() {
  test('binary codec', () {
    const c = BinaryCodec();
    expect(c.textMode, TransportTextMode.raw);
    expect(c.fromTextBytes(_u('a')), _u('a'));
    expect(c.fromTextString('é'), _u('é'));
    expect(c.fromBinary(_u('b')), _u('b'));
    expect(
      () => c.fromJson(const {}),
      throwsA(isA<WebSocketFormatException>()),
    );
  });

  test('text codec', () {
    const c = TextCodec();
    expect(c.fromTextBytes(_u('ü ✓')), 'ü ✓');
    expect(() => c.fromTextBytes(_bad), throwsA(isA<InvalidUtf8>()));
    expect(c.fromTextString('s'), 's');
    expect(c.fromBinary(_u('bin')), 'bin');
    expect(() => c.fromBinary(_bad), throwsA(isA<WebSocketFormatException>()));
  });

  test('json codec', () {
    const c = JsonCodec();
    expect(c.textMode, TransportTextMode.json);
    expect(c.fromTextBytes(_u('{"a":[1]}')), {
      'a': [1],
    });
    expect(() => c.fromTextBytes(_bad), throwsA(isA<InvalidUtf8>()));
    expect(
      () => c.fromTextBytes(_u('{x')),
      throwsA(isA<WebSocketFormatException>()),
    );
    expect(c.fromTextString('{"b":null}'), {'b': null});
    expect(
      () => c.fromTextString('nope'),
      throwsA(isA<WebSocketFormatException>()),
    );
    expect(c.fromJson(<String, Object?>{'c': 1}), {'c': 1});
    for (final notObject in [null, 1, 'x', <Object?>[]]) {
      expect(
        () => c.fromJson(notObject),
        throwsA(isA<WebSocketFormatException>()),
        reason: '$notObject',
      );
    }
    expect(
      () => c.fromBinary(_u('{}')),
      throwsA(isA<WebSocketFormatException>()),
    );
  });

  test('encodeText picks ASCII or UTF-8', () {
    expect(TextCodec.encodeText('plain'), ascii.encode('plain'));
    expect(TextCodec.encodeText('naïve'), utf8.encode('naïve'));
    expect(TextCodec.encodeText(''), isEmpty);
  });

  test('utf8Length matches utf8.encode', () {
    for (final t in ['', 'abc', 'é', '€', '𝄞', 'a\u{D800}b', 'мир ✓ 𝄞 é']) {
      expect(utf8Length(t), utf8.encode(t).length, reason: t);
    }
  });
}
