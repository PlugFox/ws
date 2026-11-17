import 'dart:typed_data';

/// Incremental UTF-8 validator (Björn Höhrmann's DFA).
///
/// Used for fragmented text messages, so an invalid sequence fails the
/// connection as soon as the fragment that contains it arrives, and for
/// close reasons. Also accepts a message split in arbitrary places.
final class Utf8Validator {
  int _state = _accept;

  static const int _accept = 0;

  static const bool _isWeb = bool.fromEnvironment('dart.library.js_interop');

  // 0x8080808080808080 does not fit a positive 64-bit literal; built from
  // halves so the web compiler never sees a 64-bit constant.
  static const int _high64 = _isWeb ? 0 : (0x80808080 << 32) | 0x80808080;
  static const int _reject = 12;

  /// Whether everything fed so far is a valid prefix of UTF-8.
  bool get isValidPrefix => _state != _reject;

  /// Whether everything fed so far is complete, valid UTF-8.
  bool get isComplete => _state == _accept;

  /// Feeds `bytes[start, end)`. Returns `false` as soon as the input can no
  /// longer be valid UTF-8.
  bool add(Uint8List bytes, [int start = 0, int? end]) {
    final stop = end ?? bytes.length;
    var state = _state;
    var i = start;
    while (i < stop) {
      // The word scan pays off only for a run of ASCII (two bytes in a
      // row, so the space between non-Latin words does not count).
      // Entering it after every multi-byte character halved the speed on
      // non-Latin text (docs/review/protocol.md F10).
      if (state == _accept &&
          stop - i >= 16 &&
          bytes[i] < 0x80 &&
          bytes[i + 1] < 0x80) {
        i = _skipAscii(bytes, i, stop);
        if (i == stop) break;
      }
      state = _transitions[state + _classes[bytes[i++]]];
      if (state == _reject) {
        _state = _reject;
        return false;
      }
    }
    _state = state;
    return true;
  }

  /// Returns the index of the first non-ASCII byte in `bytes[i, stop)`, or
  /// [stop]. On the VM it checks 32 bytes per step with 64-bit words; the
  /// web has no 64-bit integers, so it uses 32-bit words there.
  static int _skipAscii(Uint8List bytes, int i, int stop) =>
      _isWeb ? _skipAscii32(bytes, i, stop) : _skipAscii64(bytes, i, stop);

  static int _skipAscii64(Uint8List bytes, int from, int stop) {
    var i = from;
    final base = bytes.offsetInBytes;
    while (i < stop && ((base + i) & 7) != 0) {
      if (bytes[i] >= 0x80) return i;
      i++;
    }
    final words = (stop - i) >> 3;
    if (words > 0) {
      final v = bytes.buffer.asUint64List(base + i, words);
      var w = 0;
      for (; w + 4 <= words; w += 4) {
        if ((v[w] | v[w + 1] | v[w + 2] | v[w + 3]) & _high64 != 0) break;
      }
      while (w < words && v[w] & _high64 == 0) {
        w++;
      }
      i += w << 3;
    }
    while (i < stop && bytes[i] < 0x80) {
      i++;
    }
    return i;
  }

  static int _skipAscii32(Uint8List bytes, int from, int stop) {
    var i = from;
    final base = bytes.offsetInBytes;
    while (i < stop && ((base + i) & 3) != 0) {
      if (bytes[i] >= 0x80) return i;
      i++;
    }
    final words = (stop - i) >> 2;
    if (words > 0) {
      final v = bytes.buffer.asUint32List(base + i, words);
      var w = 0;
      while (w < words && v[w] & 0x80808080 == 0) {
        w++;
      }
      i += w << 2;
    }
    while (i < stop && bytes[i] < 0x80) {
      i++;
    }
    return i;
  }

  /// Whether `bytes[start, end)` is pure ASCII.
  static bool isAscii(Uint8List bytes, [int start = 0, int? end]) {
    final stop = end ?? bytes.length;
    return _skipAscii(bytes, start, stop) == stop;
  }

  /// Resets the validator for a new message.
  void reset() => _state = _accept;

  /// Whether [bytes] is complete, valid UTF-8.
  static bool validate(Uint8List bytes, [int start = 0, int? end]) {
    final v = Utf8Validator();
    return v.add(bytes, start, end) && v.isComplete;
  }

  // Character classes for every byte value.
  static final Uint8List _classes = Uint8List.fromList(const <int>[
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, //
    9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 9, //
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, //
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, //
    8, 8, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, //
    2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, //
    10, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 4, 3, 3, //
    11, 6, 6, 6, 5, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, 8, //
  ]);

  // State transitions; states are multiples of 12.
  static final Uint8List _transitions = Uint8List.fromList(const <int>[
    0, 12, 24, 36, 60, 96, 84, 12, 12, 12, 48, 72, //
    12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, //
    12, 0, 12, 12, 12, 12, 12, 0, 12, 0, 12, 12, //
    12, 24, 12, 12, 12, 12, 12, 24, 12, 24, 12, 12, //
    12, 12, 12, 12, 12, 12, 12, 24, 12, 12, 12, 12, //
    12, 24, 12, 12, 12, 12, 12, 12, 12, 24, 12, 12, //
    12, 12, 12, 12, 12, 12, 12, 36, 12, 36, 12, 12, //
    12, 36, 12, 12, 12, 12, 12, 36, 12, 36, 12, 12, //
    12, 36, 12, 12, 12, 12, 12, 12, 12, 12, 12, 12, //
  ]);
}
