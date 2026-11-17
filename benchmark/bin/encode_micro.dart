/// Send-path experiments: String → masked text frame.
/// A: utf8.encode + FrameWriter.frame (copy into frame buffer)
/// B: exact length pass + encode straight into the frame buffer + mask
/// C: like B, with vm:unsafe:no-bounds-checks
/// D: ASCII string → codeUnits copy (setRange from String.codeUnits)
library;

import 'dart:convert';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:ws/src/protocol/frame.dart';
import 'package:ws/src/protocol/frame_writer.dart';

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

int utf8Length(String s) {
  var n = s.length;
  for (var i = 0; i < s.length; i++) {
    final c = s.codeUnitAt(i);
    if (c < 0x80) continue;
    if (c < 0x800) {
      n += 1;
    } else if (c >= 0xD800 && c <= 0xDBFF && i + 1 < s.length) {
      final lo = s.codeUnitAt(i + 1);
      if (lo >= 0xDC00 && lo <= 0xDFFF) {
        n += 2; // 2 units → 4 bytes
        i++;
      } else {
        n += 2;
      }
    } else {
      n += 2;
    }
  }
  return n;
}

@pragma('vm:prefer-inline')
int encodeInto(Uint8List b, int p, String v) {
  for (var i = 0; i < v.length; i++) {
    var c = v.codeUnitAt(i);
    if (c <= 0x7F) {
      b[p++] = c;
    } else if (c <= 0x7FF) {
      b[p++] = 0xC0 | (c >> 6);
      b[p++] = 0x80 | (c & 0x3F);
    } else if (c >= 0xD800 && c <= 0xDBFF && i + 1 < v.length &&
        (v.codeUnitAt(i + 1) & 0xFC00) == 0xDC00) {
      c = 0x10000 + ((c - 0xD800) << 10) + (v.codeUnitAt(++i) - 0xDC00);
      b[p++] = 0xF0 | (c >> 18);
      b[p++] = 0x80 | ((c >> 12) & 0x3F);
      b[p++] = 0x80 | ((c >> 6) & 0x3F);
      b[p++] = 0x80 | (c & 0x3F);
    } else if (c >= 0xD800 && c <= 0xDFFF) {
      b[p++] = 0xEF;
      b[p++] = 0xBF;
      b[p++] = 0xBD;
    } else {
      b[p++] = 0xE0 | (c >> 12);
      b[p++] = 0x80 | ((c >> 6) & 0x3F);
      b[p++] = 0x80 | (c & 0x3F);
    }
  }
  return p;
}

@pragma('vm:unsafe:no-bounds-checks')
int encodeIntoUnsafe(Uint8List b, int p, String v) => encodeInto(b, p, v);

void mask(Uint8List out, int offset, int n, int key) {
  final start = out.offsetInBytes + offset;
  var i = 0;
  while (i < n && ((start + i) & 3) != 0) {
    out[offset + i] ^= (key >> (24 - (i & 3) * 8)) & 0xFF;
    i++;
  }
  final words = (n - i) >> 2;
  if (words > 0) {
    final phased = Uint8List(4);
    for (var j = 0; j < 4; j++) {
      phased[j] = (key >> (24 - ((i + j) & 3) * 8)) & 0xFF;
    }
    final k = phased.buffer.asUint32List()[0];
    final dst = out.buffer.asUint32List(start + i, words);
    for (var w = 0; w < words; w++) {
      dst[w] ^= k;
    }
    i += words * 4;
  }
  for (; i < n; i++) {
    out[offset + i] ^= (key >> (24 - (i & 3) * 8)) & 0xFF;
  }
}

Uint8List frameDirect(String s, bool unsafe) {
  final n = utf8Length(s);
  final header = n < 126 ? 6 : (n < 65536 ? 8 : 14);
  final out = Uint8List(header + n);
  final end = unsafe ? encodeIntoUnsafe(out, header, s) : encodeInto(out, header, s);
  assert(end == header + n);
  out[0] = 0x81;
  // (length bytes + key omitted from timing differences; same cost as A)
  mask(out, header, n, 0x12345678);
  return out;
}

void main() {
  final writer = FrameWriter(mask: true);
  String us(double ns) => (ns / 1000).toStringAsFixed(2).padLeft(7);
  print('| text | utf8.encode + frame | direct | direct, no bounds checks | utf8.encode only |');
  print('|---|---:|---:|---:|---:|');
  final rnd = math.Random(1);
  for (final unicode in [false, true]) {
    for (final kb in [1, 14, 60]) {
      final sb = StringBuffer();
      while (sb.length < kb * 1024) {
        sb.write(unicode ? 'Товар №${rnd.nextInt(99)} — ünïcødé ✓ ' : '{"id":${rnd.nextInt(99999)},"name":"item"},');
      }
      final s = sb.toString();
      // correctness
      final ref = utf8.encode(s);
      final d = frameDirect(s, false);
      mask(d, d.length - ref.length, ref.length, 0x12345678);
      if (!_eq(Uint8List.sublistView(d, d.length - ref.length), ref)) throw 'mismatch';
      final a = _measure(() => writer.frame(Opcode.text, utf8.encode(s)));
      final b = _measure(() => frameDirect(s, false));
      final c = _measure(() => frameDirect(s, true));
      final e = _measure(() => utf8.encode(s));
      print('| $kb KB ${unicode ? 'unicode' : 'ascii'} | ${us(a)} | ${us(b)} | ${us(c)} | ${us(e)} |');
    }
  }
}

bool _eq(Uint8List a, Uint8List b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}
