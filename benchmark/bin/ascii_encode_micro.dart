/// String → bytes alternatives for (mostly) ASCII strings.
library;

import 'dart:convert';
import 'dart:typed_data';

double _measure(void Function() body, {int ms = 300}) {
  final warm = Stopwatch()..start();
  while (warm.elapsedMilliseconds < 100) {
    body();
  }
  var n = 0;
  final sw = Stopwatch()..start();
  while (sw.elapsedMilliseconds < ms) {
    for (var i = 0; i < 16; i++) {
      body();
    }
    n += 16;
  }
  return sw.elapsedMicroseconds * 1000 / n;
}

void main() {
  String us(double ns) => (ns / 1000).toStringAsFixed(2).padLeft(7);
  print('| text | utf8.encode | latin1.encode | ascii.encode | fromList(codeUnits) | setRange(codeUnits) | JsonUtf8Encoder(map) | jsonEncode+utf8.encode |');
  print('|---|---:|---:|---:|---:|---:|---:|---:|');
  for (final kb in [1, 14, 60]) {
    final list = <Object?>[];
    var s = '';
    for (var i = 0; ; i++) {
      list.add({'id': i, 'name': 'item-$i', 'price': i * 1.5, 'tags': ['a', 'b']});
      if (list.length % 10 == 0) {
        s = jsonEncode({'items': list});
        if (s.length >= kb * 1024) break;
      }
    }
    final map = {'items': list};
    var sink = 0;
    final a = _measure(() => sink += utf8.encode(s).length);
    final b = _measure(() => sink += latin1.encode(s).length);
    final c = _measure(() => sink += ascii.encode(s).length);
    final d = _measure(() => sink += Uint8List.fromList(s.codeUnits).length);
    final e = _measure(() {
      final out = Uint8List(s.length)..setRange(0, s.length, s.codeUnits);
      sink += out.length;
    });
    final enc = JsonUtf8Encoder();
    final f = _measure(() => sink += enc.convert(map).length);
    final g = _measure(() => sink += utf8.encode(jsonEncode(map)).length);
    print('| $kb KB json ascii | ${us(a)} | ${us(b)} | ${us(c)} | ${us(d)} | ${us(e)} | ${us(f)} | ${us(g)} |');
    if (sink == 0) print('');
  }
}
