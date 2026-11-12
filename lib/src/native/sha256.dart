import 'dart:typed_data';

/// Incremental SHA-256 (FIPS 180-4), to verify downloaded libraries in
/// `dart run ws:install` without a dependency on `package:crypto`
/// (DECISIONS.md D3, D32). Dart VM only (64-bit integers).
final class Sha256 {
  /// Starts a new digest.
  Sha256();

  static const List<int> _k = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, //
    0x923f82a4, 0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3,
    0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786,
    0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147,
    0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13,
    0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
    0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a,
    0x5b9cca4f, 0x682e6ff3, 0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208,
    0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
  ];

  final Uint32List _h = Uint32List.fromList([
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, //
    0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
  ]);
  final Uint8List _block = Uint8List(64);
  final Uint32List _w = Uint32List(64);
  int _blockLength = 0;
  int _length = 0;
  bool _closed = false;

  /// Adds [bytes] to the digest.
  void add(List<int> bytes) {
    if (_closed) throw StateError('Sha256 is closed');
    _length += bytes.length;
    var i = 0;
    while (i < bytes.length) {
      final n = (64 - _blockLength).clamp(0, bytes.length - i);
      _block.setRange(_blockLength, _blockLength + n, bytes, i);
      _blockLength += n;
      i += n;
      if (_blockLength == 64) {
        _compress();
        _blockLength = 0;
      }
    }
  }

  /// Finishes the digest and returns it as lowercase hex.
  String close() {
    if (_closed) throw StateError('Sha256 is closed');
    final bits = _length * 8;
    final tail = BytesBuilder(copy: false)
      ..addByte(0x80)
      ..add(Uint8List((55 - _length) % 64))
      ..add(Uint8List(8)..buffer.asByteData().setUint64(0, bits));
    add(tail.takeBytes());
    _closed = true;
    final buffer = StringBuffer();
    for (final word in _h) {
      buffer.write(word.toRadixString(16).padLeft(8, '0'));
    }
    return buffer.toString();
  }

  static int _rotr(int x, int n) => ((x >>> n) | (x << (32 - n))) & 0xFFFFFFFF;

  void _compress() {
    final w = _w;
    final view = ByteData.sublistView(_block);
    for (var i = 0; i < 16; i++) {
      w[i] = view.getUint32(i * 4);
    }
    for (var i = 16; i < 64; i++) {
      final a = w[i - 15], b = w[i - 2];
      final s0 = _rotr(a, 7) ^ _rotr(a, 18) ^ (a >>> 3);
      final s1 = _rotr(b, 17) ^ _rotr(b, 19) ^ (b >>> 10);
      w[i] = w[i - 16] + s0 + w[i - 7] + s1;
    }
    var a = _h[0], b = _h[1], c = _h[2], d = _h[3];
    var e = _h[4], f = _h[5], g = _h[6], h = _h[7];
    for (var i = 0; i < 64; i++) {
      final s1 = _rotr(e, 6) ^ _rotr(e, 11) ^ _rotr(e, 25);
      final ch = (e & f) ^ (~e & g);
      final t1 = (h + s1 + ch + _k[i] + w[i]) & 0xFFFFFFFF;
      final s0 = _rotr(a, 2) ^ _rotr(a, 13) ^ _rotr(a, 22);
      final maj = (a & b) ^ (a & c) ^ (b & c);
      final t2 = (s0 + maj) & 0xFFFFFFFF;
      h = g;
      g = f;
      f = e;
      e = (d + t1) & 0xFFFFFFFF;
      d = c;
      c = b;
      b = a;
      a = (t1 + t2) & 0xFFFFFFFF;
    }
    _h[0] += a;
    _h[1] += b;
    _h[2] += c;
    _h[3] += d;
    _h[4] += e;
    _h[5] += f;
    _h[6] += g;
    _h[7] += h;
  }
}
