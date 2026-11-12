/// Micro-benchmarks for the pure-Dart protocol layer of `package:ws`
/// (`lib/src/protocol/`). Compile AOT for meaningful numbers:
///
///   dart compile exe bin/micro.dart -o build/micro && build/micro
library;

import 'dart:convert';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:ws/src/protocol/frame.dart';
import 'package:ws/src/protocol/frame_reader.dart';
import 'package:ws/src/protocol/frame_writer.dart';
import 'package:ws/src/protocol/utf8_text.dart';
import 'package:ws/src/protocol/utf8_validator.dart';

Map<String, Object?> _payload(int size, bool unicode) {
  final items = <Object?>[];
  var length = 1;
  for (var i = 0; length + 48 < size; i++) {
    final item = <String, Object?>{
      'id': 100000 + i,
      'name': unicode ? 'Товар №$i — ünïcødé ✓' : 'item-$i',
      'price': i * 1.37 + 0.99,
      'qty': i % 17,
      'active': i.isEven,
      'tags': ['alpha', 'beta', 'gamma'],
      'meta': {'score': 1.5, 'rank': i % 100, 'note': null},
    };
    items.add(item);
    length += utf8.encode(jsonEncode(item)).length + 1;
  }
  return {'seq': 0, 'ts': 0, 'items': items};
}

/// Runs [body] for ~[ms] after a warm-up; returns ns per call.
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

final class _Sink implements FrameHandler {
  int n = 0;
  @override
  void onText(Uint8List p) => n += p.length;
  @override
  void onBinary(Uint8List p) => n += p.length;
  @override
  void onPing(Uint8List p) {}
  @override
  void onPong(Uint8List p) {}
  @override
  void onClose(int? c, String r) {}
  @override
  void onFailure(int c, String r) => throw StateError(r);
}

void _maskBytewise(Uint8List out, int offset, Uint8List payload, int key) {
  for (var i = 0; i < payload.length; i++) {
    out[offset + i] = payload[i] ^ ((key >> (24 - (i & 3) * 8)) & 0xFF);
  }
}

void main() {
  final fused = utf8.decoder.fuse(json.decoder);
  String us(double ns) => (ns / 1000).toStringAsFixed(2).padLeft(8);

  print('## UTF-8 validation vs decoding (µs per message)\n');
  print('| payload | validate | utf8.decode | decodeUtf8 | fused utf8+json | validate / fused |');
  print('|---|---:|---:|---:|---:|---:|');
  for (final unicode in [false, true]) {
    for (final kb in [1, 14, 60]) {
      final bytes = utf8.encode(jsonEncode(_payload(kb * 1024, unicode)));
      final v = _measure(() => Utf8Validator.validate(bytes));
      final d = _measure(() => utf8.decode(bytes));
      final t = _measure(() => decodeUtf8(bytes));
      final f = _measure(() => fused.convert(bytes));
      print('| $kb KB ${unicode ? 'unicode' : 'ascii'} | ${us(v)} | ${us(d)} | ${us(t)} '
          '| ${us(f)} | ${(v / f * 100).toStringAsFixed(1)}% |');
    }
  }

  print('\n## Masking (µs per frame)\n');
  print('| payload | byte-wise | FrameWriter.frame (word-wise + header) |');
  print('|---|---:|---:|');
  final writer = FrameWriter(mask: true);
  final rnd = math.Random(1);
  for (final kb in [1, 14, 60]) {
    final p = Uint8List.fromList(
        List<int>.generate(kb * 1024, (_) => rnd.nextInt(256)));
    final out = Uint8List(p.length + 14);
    final b = _measure(() => _maskBytewise(out, 14, p, 0x12345678));
    final w = _measure(() => writer.frame(Opcode.binary, p));
    print('| $kb KB | ${us(b)} | ${us(w)} |');
  }

  print('\n## Frame parsing (µs per message, 64 KB socket reads)\n');
  print('| payload | validateUtf8: false | validateUtf8: true |');
  print('|---|---:|---:|');
  final server = FrameWriter(mask: false);
  for (final kb in [1, 14, 60]) {
    final msg = utf8.encode(jsonEncode(_payload(kb * 1024, true)));
    final frame = server.frame(Opcode.text, msg);
    final count = math.max(1, (1 << 20) ~/ frame.length);
    final stream = Uint8List(frame.length * count);
    for (var i = 0; i < count; i++) {
      stream.setRange(i * frame.length, (i + 1) * frame.length, frame);
    }
    double run(bool validate) => _measure(() {
          final r = FrameReader(_Sink(), validateUtf8: validate);
          for (var o = 0; o < stream.length; o += 65536) {
            r.add(Uint8List.sublistView(
                stream, o, math.min(o + 65536, stream.length)));
          }
        }) / count;
    print('| $kb KB unicode | ${us(run(false))} | ${us(run(true))} |');
  }
}
