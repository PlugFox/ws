import 'dart:convert' show utf8;
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:ws/src/protocol/frame.dart';

/// Encodes RFC 6455 frames.
///
/// Clients mask every frame (`mask: true`). Masking keys come from a pool
/// filled by `Random.secure()`, as RFC 6455 §5.3 asks for keys that cannot
/// be predicted.
final class FrameWriter {
  /// Creates a writer. [mask] is `true` for clients.
  FrameWriter({required bool mask, math.Random? random})
    : _mask = mask,
      _random = random ?? math.Random.secure();

  final bool _mask;
  final math.Random _random;

  final Uint32List _keys = Uint32List(64);
  int _nextKey = 64;

  int _maskKey() {
    if (_nextKey == _keys.length) {
      for (var i = 0; i < _keys.length; i++) {
        // Two 16-bit halves: nextInt is limited to 2^32 and is exact on web.
        _keys[i] = (_random.nextInt(0x10000) << 16) | _random.nextInt(0x10000);
      }
      _nextKey = 0;
    }
    return _keys[_nextKey++];
  }

  /// Encodes one frame: header, then the (masked) payload, in one buffer.
  Uint8List frame(int opcode, Uint8List payload, {bool fin = true}) {
    final n = payload.length;
    final lengthBytes = n < 126 ? 0 : (n < 65536 ? 2 : 8);
    final header = 2 + lengthBytes + (_mask ? 4 : 0);
    final out = Uint8List(header + n);
    out[0] = (fin ? 0x80 : 0) | opcode;
    final maskBit = _mask ? 0x80 : 0;
    if (n < 126) {
      out[1] = maskBit | n;
    } else if (n < 65536) {
      out
        ..[1] = maskBit | 126
        ..[2] = n >> 8
        ..[3] = n & 0xFF;
    } else {
      out[1] = maskBit | 127;
      var rest = n;
      for (var i = 9; i >= 2; i--) {
        out[i] = rest % 256;
        rest = rest ~/ 256;
      }
    }
    if (!_mask) {
      out.setRange(header, header + n, payload);
      return out;
    }
    final key = _maskKey();
    out
      ..[header - 4] = (key >> 24) & 0xFF
      ..[header - 3] = (key >> 16) & 0xFF
      ..[header - 2] = (key >> 8) & 0xFF
      ..[header - 1] = key & 0xFF;
    _applyMask(out, header, payload, key);
    return out;
  }

  /// Encodes a close frame. A `null` [code] sends an empty close frame.
  Uint8List close(int? code, [String reason = '']) {
    if (code == null) return frame(Opcode.close, Uint8List(0));
    final reasonBytes = utf8.encode(reason);
    final payload = Uint8List(2 + math.min(reasonBytes.length, 123))
      ..[0] = code >> 8
      ..[1] = code & 0xFF
      ..setRange(2, 2 + math.min(reasonBytes.length, 123), reasonBytes);
    return frame(Opcode.close, payload);
  }

  /// Copies [payload] into `out[offset..]` XORed with [key].
  ///
  /// Works on 32-bit words when the destination is 4-byte aligned, which
  /// is several times faster than byte-by-byte on the VM.
  static void _applyMask(
    Uint8List out,
    int offset,
    Uint8List payload,
    int key,
  ) {
    final n = payload.length;
    final k0 = (key >> 24) & 0xFF;
    final k1 = (key >> 16) & 0xFF;
    final k2 = (key >> 8) & 0xFF;
    final k3 = key & 0xFF;
    var i = 0;
    // Align the destination to 4 bytes.
    final start = out.offsetInBytes + offset;
    while (i < n && ((start + i) & 3) != 0) {
      out[offset + i] = payload[i] ^ _byte(i, k0, k1, k2, k3);
      i++;
    }
    final words = (n - i) >> 2;
    if (words > 0) {
      // The key as it lies in memory starting at mask phase `i & 3`,
      // read as a host-endian 32-bit word.
      final phased = Uint8List(4);
      for (var j = 0; j < 4; j++) {
        phased[j] = _byte(i + j, k0, k1, k2, k3);
      }
      final key32 = phased.buffer.asUint32List()[0];
      out.setRange(offset + i, offset + i + words * 4, payload, i);
      final dst = out.buffer.asUint32List(start + i, words);
      for (var w = 0; w < words; w++) {
        dst[w] ^= key32;
      }
      i += words * 4;
    }
    for (; i < n; i++) {
      out[offset + i] = payload[i] ^ _byte(i, k0, k1, k2, k3);
    }
  }

  static int _byte(int i, int k0, int k1, int k2, int k3) => switch (i & 3) {
    0 => k0,
    1 => k1,
    2 => k2,
    _ => k3,
  };
}
