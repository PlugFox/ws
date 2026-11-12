// JSON tape (DECISIONS.md D10): the Dart builder and the lazy Map/List
// views, checked against dart:convert on a corpus in the spirit of
// JSONTestSuite (escapes, surrogates, number edges, nesting).
@TestOn('vm')
library;

import 'dart:convert';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/src/native/tape.dart';

Object? _tape(String json) => TapeDocument(buildTape(utf8.encode(json))).root;

/// Deep, order-sensitive conversion so views compare with `equals`.
Object? _plain(Object? v) => switch (v) {
  final Map<String, Object?> m => {
    for (final e in m.entries) e.key: _plain(e.value),
  },
  final List<Object?> l => [for (final e in l) _plain(e)],
  _ => v,
};

const _corpus = <String>[
  'null',
  'true',
  'false',
  '0',
  '-0',
  '1',
  '-1',
  '123456789012345678',
  '-123456789012345678',
  '9007199254740993',
  '1.5',
  '-0.0',
  '1e3',
  '1E-3',
  '2.5e+10',
  '1.7976931348623157e308',
  '5e-324',
  '""',
  '"ascii"',
  r'"esc \" \\ \/ \b \f \n \r \t"',
  r'"\u0041\u00e9\u4e2d"',
  r'"\ud834\udd1e"',
  '"ünïcødé ✓ 𝄞"',
  '[]',
  '{}',
  '[[]]',
  '[{}]',
  '{"a":{}}',
  ' { "a" : [ 1 , 2 ] , "b" : null } ',
  '{"":"empty key"}',
  r'{"k\u00e9y":1,"k\"q":2}',
  '[1,"two",3.0,true,false,null,{"x":[{"y":[]}]}]',
  '{"dup":1,"dup":2}',
];

void main() {
  group('builder + views match jsonDecode', () {
    for (final json in _corpus) {
      test(json, () {
        expect(_plain(_tape(json)), jsonDecode(json));
      });
    }

    test('random documents', () {
      final rnd = math.Random(42);
      Object? gen(int depth) {
        switch (depth > 4 ? rnd.nextInt(5) : rnd.nextInt(8)) {
          case 0:
            return null;
          case 1:
            return rnd.nextBool();
          case 2:
            return rnd.nextInt(1 << 31) - (1 << 30);
          case 3:
            return rnd.nextDouble() * 1e6;
          case 4:
            return String.fromCharCodes([
              for (var i = 0; i < rnd.nextInt(12); i++)
                [0x22, 0x5C, 0x0A, 0x41, 0xE9, 0x4E2D, 0x7F][rnd.nextInt(7)],
            ]);
          case 5 || 6:
            return [for (var i = 0; i < rnd.nextInt(6); i++) gen(depth + 1)];
          default:
            return {
              for (var i = 0; i < rnd.nextInt(6); i++) 'k$i': gen(depth + 1),
            };
        }
      }

      for (var i = 0; i < 300; i++) {
        final doc = gen(0);
        final json = jsonEncode(doc);
        expect(_plain(_tape(json)), jsonDecode(json), reason: json);
      }
    });
  });

  group('invalid JSON is rejected', () {
    for (final bad in [
      '',
      '[',
      '{"a"}',
      '{"a":1,}',
      '[1,]',
      'tru',
      '"open',
      r'"\x"',
      r'"\u12"',
      '01',
      '1.',
      '-',
      '[1] 2',
      'nul',
      '+1',
      '.5',
      '-01',
      '1e+',
      '1e',
    ]) {
      test(bad.isEmpty ? '(empty)' : bad, () {
        expect(() => buildTape(utf8.encode(bad)), throwsFormatException);
      });
    }
  });

  group('map view', () {
    final map =
        _tape('{"a":1,"b":[2,3],"c":{"d":null},"é":"x"}')!
            as Map<String, Object?>;

    test('lookups in any order use and wrap the hint', () {
      expect(map['c'], {'d': null});
      expect(map['a'], 1); // wraps around
      expect(map['b'], [2, 3]);
      expect(map['é'], 'x');
      expect(map['missing'], isNull);
      expect(map[42 as Object?], isNull);
      expect(map.containsKey('c'), isTrue);
      expect(map.containsKey('d'), isFalse);
      expect(map.containsKey(null as Object?), isFalse);
    });

    test('length, keys, values, entries, forEach', () {
      expect(map.length, 4);
      expect(map.isEmpty, isFalse);
      expect(map.isNotEmpty, isTrue);
      expect(map.keys, ['a', 'b', 'c', 'é']);
      expect(map.values.first, 1);
      expect([for (final e in map.entries) e.key], ['a', 'b', 'c', 'é']);
      final seen = <String>[];
      map.forEach((k, _) => seen.add(k));
      expect(seen, ['a', 'b', 'c', 'é']);
      final empty = _tape('{}')! as Map<String, Object?>;
      expect((empty.isEmpty, empty.isNotEmpty, empty.length), (true, false, 0));
    });

    test('is unmodifiable', () {
      expect(() => map['a'] = 2, throwsUnsupportedError);
      expect(() => map.remove('a'), throwsUnsupportedError);
      expect(map.clear, throwsUnsupportedError);
    });
  });

  group('list view', () {
    final list = _tape('[10,"s",[1],{"k":true},null]')! as List<Object?>;

    test('indexing, iteration, range checks', () {
      expect(list.length, 5);
      expect(list[3], {'k': true});
      expect(list[0], 10);
      expect(list.last, isNull);
      expect(list.toList(), [
        10,
        's',
        [1],
        {'k': true},
        null,
      ]);
      expect(() => list[5], throwsRangeError);
      expect(() => list[-1], throwsRangeError);
      final it = list.iterator;
      var n = 0;
      while (it.moveNext()) {
        n++;
      }
      expect(n, 5);
      expect(it.moveNext(), isFalse);
    });

    test('is unmodifiable', () {
      expect(() => list[0] = 1, throwsUnsupportedError);
      expect(() => list.length = 0, throwsUnsupportedError);
      expect(() => list.add(1), throwsUnsupportedError);
    });
  });

  group('document', () {
    test('copies unaligned buffers, rejects other versions', () {
      final tape = buildTape(utf8.encode('[1,{"a":"b"}]'));
      final shifted = Uint8List(tape.length + 1)
        ..setRange(1, tape.length + 1, tape);
      expect(_plain(TapeDocument(Uint8List.sublistView(shifted, 1)).root), [
        1,
        {'a': 'b'},
      ]);
      final other = Uint8List.fromList(tape);
      ByteData.sublistView(other).setUint32(4, tapeVersion + 1, Endian.little);
      expect(() => TapeDocument(other), throwsFormatException);
    });

    test('large documents grow the builder buffers', () {
      final big = jsonEncode([
        for (var i = 0; i < 5000; i++) {'i': i, 's': 'line\n$i'},
      ]);
      final root = _tape(big)! as List<Object?>;
      expect(root.length, 5000);
      expect((root[4999]! as Map)['s'], 'line\n4999');
    });
  });
}
