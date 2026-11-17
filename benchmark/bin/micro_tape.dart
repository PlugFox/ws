// Micro benchmark: Order.fromJson over a decoded Map vs over tape views.
import 'dart:convert';
import 'dart:io';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:ws_benchmark/ws_benchmark.dart';

Map<String, Object?> payload(int size) {
  final items = <Object?>[];
  for (var i = 0, length = 1; length + 48 < size; i++) {
    final item = {
      'id': 100000 + i, 'name': 'item-$i', 'price': i * 1.37 + 0.99, 'qty': i % 17,
      'active': i.isEven, 'tags': ['alpha', 'beta', 'gamma'],
      'meta': {'score': 1.5, 'rank': i % 100, 'note': null},
    };
    items.add(item);
    length += jsonEncode(item).length + 1;
  }
  return {'seq': 1, 'ts': 2, 'items': items};
}

double time(String name, int iterations, void Function() body) {
  for (var i = 0; i < iterations ~/ 10; i++) {
    body();
  }
  final sw = Stopwatch()..start();
  for (var i = 0; i < iterations; i++) {
    body();
  }
  final us = sw.elapsedMicroseconds / iterations;
  stdout.writeln('${name.padRight(36)} ${us.toStringAsFixed(1).padLeft(8)} µs');
  return us;
}

void main() {
  final bytes = utf8.encode(jsonEncode(payload(61440)));
  final map = jsonDecode(utf8.decode(bytes)) as Map<String, Object?>;
  final tape = buildTape(bytes);
  final transferable = TransferableTypedData.fromList([tape]).materialize().asUint8List();
  const n = 2000;
  var sink = 0;
  stdout.writeln('payload: ${bytes.length} bytes, tape: ${tape.length} bytes');
  time('jsonDecode(fused bytes)', n, () => sink += (decodeJsonBytes(bytes)! as Map).length);
  time('buildTape', n, () => sink += buildTape(bytes).length);
  time('Order.fromJson(Map)', n, () => sink += Order.fromJson(map).items.length);
  time('Order.fromJson(TapeMap)', n,
      () => sink += Order.fromJson(TapeDocument(tape).root! as Map<String, Object?>).items.length);
  time('Order.fromJson(TapeMap, transferred)', n,
      () => sink += Order.fromJson(TapeDocument(transferable).root! as Map<String, Object?>).items.length);
  time('decode + fromJson (fused)', n,
      () => sink += Order.fromJson(decodeJsonBytes(bytes)! as Map<String, Object?>).items.length);
  final troot = TapeDocument(tape).root! as Map<String, Object?>;
  final mitems = map['items']! as List<Object?>;
  final titems = troot['items']! as List<Object?>;
  for (final (name, items) in [('Map', mitems), ('Tape', titems)]) {
    time('$name: iterate items', n, () { for (final it in items) { sink += identityHashCode(it) & 1; } });
    time('$name: [id]', n, () { for (final it in items) { sink += (it! as Map)['id']! as int; } });
    time('$name: [meta] only', n, () { for (final it in items) { sink += ((it! as Map)['meta']! as Map).length; } });
    time('$name: [name] only', n, () { for (final it in items) { sink += ((it! as Map)['name']! as String).length; } });
    time('$name: [price] only', n, () { for (final it in items) { sink += ((it! as Map)['price']! as double).toInt(); } });
    time('$name: tags iterate', n, () { for (final it in items) { for (final t in (it! as Map)['tags']! as List) { sink += (t as String).length; } } });
  }
  // String creation alternatives for an ASCII slice.
  final s = Uint8List.fromList(utf8.encode('item-12345'));
  time('1e3 × String.fromCharCodes(u8,s,e)', n, () { for (var i = 0; i < 1000; i++) { sink += String.fromCharCodes(s, 0, s.length).length; } });
  time('1e3 × latin1.decode(view)', n, () { for (var i = 0; i < 1000; i++) { sink += latin1.decode(Uint8List.sublistView(s, 0, s.length)).length; } });
  time('1e3 × utf8.decode(view)', n, () { for (var i = 0; i < 1000; i++) { sink += utf8.decode(Uint8List.sublistView(s, 0, s.length)).length; } });
  stdout.writeln(sink);
}
