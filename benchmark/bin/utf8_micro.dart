/// UTF-8 micro-experiments: 4- vs 8-byte ASCII scan, and decoding ASCII
/// text with String.fromCharCodes vs utf8.decode. Build with `dart build cli`.
library;

import 'dart:convert';
import 'dart:typed_data';

double _measure(void Function() body, {int ms = 400}) {
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

int scan4(Uint8List b) {
  final words = b.length >> 2;
  final v = b.buffer.asUint32List(b.offsetInBytes, words);
  for (var w = 0; w < words; w++) {
    if (v[w] & 0x80808080 != 0) return w << 2;
  }
  return words << 2;
}

int scan8(Uint8List b) {
  final words = b.length >> 3;
  final v = b.buffer.asUint64List(b.offsetInBytes, words);
  for (var w = 0; w < words; w++) {
    if (v[w] & 0x8080808080808080 != 0) return w << 3;
  }
  return words << 3;
}

int scan8unrolled(Uint8List b) {
  final words = b.length >> 3;
  final v = b.buffer.asUint64List(b.offsetInBytes, words);
  var w = 0;
  for (; w + 4 <= words; w += 4) {
    if ((v[w] | v[w + 1] | v[w + 2] | v[w + 3]) & 0x8080808080808080 != 0) {
      break;
    }
  }
  for (; w < words; w++) {
    if (v[w] & 0x8080808080808080 != 0) return w << 3;
  }
  return words << 3;
}

void main() {
  String us(double ns) => (ns / 1000).toStringAsFixed(2).padLeft(7);
  print('| size | scan4 | scan8 | scan8x4 | utf8.decode | fromCharCodes | latin1.decode |');
  print('|---|---:|---:|---:|---:|---:|---:|');
  for (final kb in [1, 14, 60]) {
    final text = List.filled(kb * 1024, 'a').join();
    final b = Uint8List.fromList(utf8.encode(text));
    var sink = 0;
    final s4 = _measure(() => sink += scan4(b));
    final s8 = _measure(() => sink += scan8(b));
    final s8u = _measure(() => sink += scan8unrolled(b));
    final d = _measure(() => sink += utf8.decode(b).length);
    final f = _measure(() => sink += String.fromCharCodes(b).length);
    final l = _measure(() => sink += latin1.decode(b).length);
    print('| $kb KB | ${us(s4)} | ${us(s8)} | ${us(s8u)} | ${us(d)} | ${us(f)} | ${us(l)} |  $sink'.replaceAll(RegExp(r'\|  \d+$'), '|'));
  }
}
