import 'dart:convert';
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws_benchmark/ws_benchmark.dart';

/// Deep-converts tape views to plain collections for comparison.
Object? plain(Object? v) => switch (v) {
      Map<String, Object?> m => {for (final e in m.entries) e.key: plain(e.value)},
      List<Object?> l => [for (final e in l) plain(e)],
      _ => v,
    };

Object? viaTape(String json) =>
    TapeDocument(buildTape(utf8.encode(json))).root;

void main() {
  final cases = <String>[
    'null', 'true', 'false', '0', '-0', '42', '-17', '3.14', '-2.5e-3', '1E10',
    '123456789012345678', '1234567890123456789', '-9223372036854775808',
    '18446744073709551616', '""', '"plain"', r'"esc \" \\ \/ \b \f \n \r \t"',
    r'"Aé中😀"', '"Товар №1 — ünïcødé ✓"', '[]', '{}',
    '[1, [2, [3, {}]], {"a": []}]',
    '{"a":1,"b":{"c":[true,false,null],"d":"x"},"e":-1.5,"Ключ":"значение"}',
    ' { "ws" : [ 1 , 2 ] , "k\\"q" : "v" } ',
  ];
  for (final json in cases) {
    test('matches jsonDecode: $json', () {
      expect(plain(viaTape(json)), equals(jsonDecode(json)));
    });
  }

  test('lookups and iteration on a large document', () {
    final items = [
      for (var i = 0; i < 500; i++)
        {'id': i, 'name': 'item-$i', 'price': i * 1.37 + 0.5, 'qty': i % 17,
         'active': i.isEven, 'tags': ['a', 'b'],
         'meta': {'score': 1.5, 'rank': i % 7, 'note': null}},
    ];
    final json = jsonEncode({'seq': 7, 'ts': 123, 'items': items});
    final root = viaTape(json)! as Map<String, Object?>;
    expect(root['seq'], 7);
    expect(root['missing'], isNull);
    expect(root.containsKey('items'), isTrue);
    expect(root.keys.toList(), ['seq', 'ts', 'items']);
    final list = root['items']! as List<Object?>;
    expect(list.length, 500);
    expect((list[321]! as Map)['name'], 'item-321');
    expect(((list[499]! as Map)['meta']! as Map)['rank'], 499 % 7);
    expect(plain(root), jsonDecode(json));
    final order = Order.fromJson(root);
    expect(order.items.length, 500);
    expect(order.items[42].name, 'item-42');
    expect(order.items[42].price, 42 * 1.37 + 0.5);
  });

  test('rejects invalid JSON', () {
    for (final bad in ['', '{', '[1,]', '{"a"}', '"abc', 'tru', '1 2', '{"a":1,}']) {
      expect(() => buildTape(Uint8List.fromList(utf8.encode(bad))),
          throwsFormatException, reason: bad);
    }
  });
}
