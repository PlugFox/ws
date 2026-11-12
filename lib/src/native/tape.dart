// ignore_for_file: public_member_api_docs, lines_longer_than_80_chars, avoid_js_rounded_ints, parameter_assignments, use_raw_strings, omit_local_variable_types
/// JSON "tape": a flat, zero-copy-friendly representation of a parsed JSON
/// document, plus lazy `Map` / `List` views over it.
///
/// The tape can be produced anywhere — a Dart worker isolate or native code
/// (Rust) — and handed to the main isolate as a single byte buffer
/// (`TransferableTypedData` / external typed data) in O(1). The main isolate
/// then pays only for the fields it actually reads: no tokenizing, no number
/// parsing, no hashing of keys, no eager allocation of the whole tree.
///
/// ## Format (version 1, little-endian)
///
/// ```text
/// [u64 header][u64 word × N][string region]
/// header: bits 0..31 = N (number of tape words), bits 32..63 = version (1)
/// string region: the original JSON bytes, followed by unescaped copies of
///                strings that contained escape sequences.
/// ```
///
/// The root value starts at word 0. Each value starts with a word whose top
/// byte is a tag and bits 0..55 are the payload:
///
/// | tag | value  | words | payload / next word                                   |
/// |-----|--------|-------|-------------------------------------------------------|
/// | `n` | null   | 1     | —                                                     |
/// | `t` | true   | 1     | —                                                     |
/// | `f` | false  | 1     | —                                                     |
/// | `l` | int    | 2     | next word: int64                                      |
/// | `d` | double | 2     | next word: float64 bits                               |
/// | `s` | string | 2     | payload: byte offset in string region; next word: bits 0..61 byte length, bit 62 = ASCII only |
/// | `{` | object | 2+    | payload: index of the word after the object; next word: field count; then key (`s`) / value pairs |
/// | `[` | array  | 2+    | payload: index of the word after the array; next word: element count; then elements |
library;

import 'dart:collection';
import 'dart:convert';
import 'dart:typed_data';

const int tapeVersion = 1;

const int tagNull = 0x6E; // n
const int tagTrue = 0x74; // t
const int tagFalse = 0x66; // f
const int tagInt = 0x6C; // l
const int tagDouble = 0x64; // d
const int tagString = 0x73; // s
const int tagObject = 0x7B; // {
const int tagArray = 0x5B; // [

const int _payloadMask = 0x00FFFFFFFFFFFFFF;
const int _lengthMask = 0x3FFFFFFFFFFFFFFF;
const int _asciiFlag = 0x4000000000000000;

// --- reading ----------------------------------------------------------------

/// A parsed JSON document backed by a tape buffer.
final class TapeDocument {
  factory TapeDocument(Uint8List buffer) {
    if (buffer.offsetInBytes & 7 != 0) {
      // Word access needs 8-byte alignment; copy if the view is unaligned.
      buffer = Uint8List.fromList(buffer);
    }
    final header = buffer.buffer.asInt64List(buffer.offsetInBytes, 1)[0];
    final version = header >> 32;
    if (version != tapeVersion) {
      throw FormatException('Unsupported tape version $version');
    }
    final n = header & 0xFFFFFFFF;
    final words = buffer.buffer.asInt64List(buffer.offsetInBytes + 8, n);
    final strings = Uint8List.sublistView(buffer, 8 + n * 8);
    return TapeDocument._(words, strings);
  }

  TapeDocument._(this._words, this._strings);

  final Int64List _words;
  final Uint8List _strings;

  /// The root value: `TapeMap`, `TapeList`, `String`, `num`, `bool` or null.
  Object? get root => _value(0);

  Object? _value(int i) {
    final w = _words[i];
    switch (w >>> 56) {
      case tagObject:
        return TapeMap._(this, i);
      case tagArray:
        return TapeList._(this, i);
      case tagString:
        return _string(i);
      case tagInt:
        return _words[i + 1];
      case tagDouble:
        return _doubleAt(i + 1);
      case tagTrue:
        return true;
      case tagFalse:
        return false;
      case tagNull:
        return null;
      default:
        throw FormatException('Bad tape tag ${w >>> 56} at $i');
    }
  }

  double _doubleAt(int i) =>
      _words.buffer.asFloat64List(_words.offsetInBytes + i * 8, 1)[0];

  String _string(int i) {
    final offset = _words[i] & _payloadMask;
    final meta = _words[i + 1];
    final length = meta & _lengthMask;
    if (meta & _asciiFlag != 0) {
      return String.fromCharCodes(_strings, offset, offset + length);
    }
    return utf8.decode(
      Uint8List.sublistView(_strings, offset, offset + length),
    );
  }

  /// Index of the word right after the value starting at [i].
  int _skip(int i) {
    final w = _words[i];
    switch (w >>> 56) {
      case tagObject || tagArray:
        return w & _payloadMask;
      case tagInt || tagDouble || tagString:
        return i + 2;
      default:
        return i + 1;
    }
  }

  /// Compares the tape string at [i] with [key] without allocating.
  bool _keyEquals(int i, String key) {
    final offset = _words[i] & _payloadMask;
    final meta = _words[i + 1];
    final length = meta & _lengthMask;
    if (meta & _asciiFlag != 0) {
      if (length != key.length) return false;
      final s = _strings;
      for (var j = 0; j < length; j++) {
        if (s[offset + j] != key.codeUnitAt(j)) return false;
      }
      return true;
    }
    // Non-ASCII key: UTF-8 length is at least the UTF-16 length.
    if (length < key.length) return false;
    return _string(i) == key;
  }
}

/// Unmodifiable `Map` view over a tape object. Lookups scan the keys
/// linearly — faster than hashing for typical small JSON objects.
final class TapeMap extends UnmodifiableMapBase<String, Object?> {
  TapeMap._(this._doc, this._index);

  final TapeDocument _doc;
  final int _index;

  @override
  int get length => _doc._words[_index + 1];

  @override
  bool get isEmpty => length == 0;

  @override
  bool get isNotEmpty => length != 0;

  /// Word index of the key to try first. `fromJson` code usually reads
  /// fields in document order, so the next lookup typically hits the key
  /// right after the previous one in O(1).
  int _hint = 0;

  int _find(Object? key) {
    if (key is! String) return -1;
    final doc = _doc;
    final first = _index + 2;
    final end = doc._words[_index] & _payloadMask;
    final hint = _hint < first || _hint >= end ? first : _hint;
    // Scan [hint, end), then wrap around to [first, hint).
    var i = hint;
    while (i < end) {
      final next = doc._skip(i + 2);
      if (doc._keyEquals(i, key)) {
        _hint = next;
        return i + 2;
      }
      i = next;
    }
    i = first;
    while (i < hint) {
      final next = doc._skip(i + 2);
      if (doc._keyEquals(i, key)) {
        _hint = next;
        return i + 2;
      }
      i = next;
    }
    return -1;
  }

  @override
  Object? operator [](Object? key) {
    final i = _find(key);
    return i < 0 ? null : _doc._value(i);
  }

  @override
  bool containsKey(Object? key) => _find(key) >= 0;

  @override
  Iterable<String> get keys sync* {
    final doc = _doc;
    final end = doc._words[_index] & _payloadMask;
    var i = _index + 2;
    while (i < end) {
      yield doc._string(i);
      i = doc._skip(i + 2);
    }
  }

  @override
  void forEach(void Function(String key, Object? value) action) {
    final doc = _doc;
    final end = doc._words[_index] & _payloadMask;
    var i = _index + 2;
    while (i < end) {
      action(doc._string(i), doc._value(i + 2));
      i = doc._skip(i + 2);
    }
  }

  @override
  Iterable<MapEntry<String, Object?>> get entries sync* {
    final doc = _doc;
    final end = doc._words[_index] & _payloadMask;
    var i = _index + 2;
    while (i < end) {
      yield MapEntry(doc._string(i), doc._value(i + 2));
      i = doc._skip(i + 2);
    }
  }
}

/// Unmodifiable `List` view over a tape array. Element offsets are resolved
/// lazily, once, on the first indexed access.
final class TapeList extends ListBase<Object?> {
  TapeList._(this._doc, this._index);

  final TapeDocument _doc;
  final int _index;
  Int32List? _offsets;

  @override
  int get length => _doc._words[_index + 1];

  Int32List _resolve() {
    final offsets = Int32List(length);
    var i = _index + 2;
    for (var j = 0; j < offsets.length; j++) {
      offsets[j] = i;
      i = _doc._skip(i);
    }
    return _offsets = offsets;
  }

  @override
  Object? operator [](int index) {
    RangeError.checkValidIndex(index, this);
    return _doc._value((_offsets ?? _resolve())[index]);
  }

  @override
  Iterator<Object?> get iterator => _TapeListIterator(_doc, _index + 2, length);

  @override
  void operator []=(int index, Object? value) =>
      throw UnsupportedError('Cannot modify an unmodifiable list');

  @override
  set length(int newLength) => throw UnsupportedError(
    'Cannot change the length of an unmodifiable list',
  );
}

final class _TapeListIterator implements Iterator<Object?> {
  _TapeListIterator(this._doc, this._next, this._remaining);

  final TapeDocument _doc;
  int _next;
  int _remaining;
  Object? _current;

  @override
  Object? get current => _current;

  @override
  bool moveNext() {
    if (_remaining == 0) return false;
    _remaining--;
    _current = _doc._value(_next);
    _next = _doc._skip(_next);
    return true;
  }
}

// --- building ---------------------------------------------------------------

/// Parses UTF-8 JSON [input] into a tape buffer (see the library docs).
///
/// Strings without escapes are referenced in place; non-ASCII strings are
/// validated lazily, when read.
Uint8List buildTape(Uint8List input) => _TapeBuilder(input).build();

final class _TapeBuilder {
  _TapeBuilder(this._in);

  final Uint8List _in;
  int _pos = 0;

  Int64List _words = Int64List(256);
  int _n = 0;

  /// Unescaped strings, stored after the input in the string region.
  Uint8List _extra = Uint8List(0);
  int _extraLength = 0;

  Uint8List build() {
    _skipWs();
    _parseValue();
    _skipWs();
    if (_pos != _in.length) _fail('Unexpected trailing data');

    final wordsBytes = 8 + _n * 8;
    final out = Uint8List(wordsBytes + _in.length + _extraLength);
    final header = out.buffer.asInt64List(0, 1 + _n);
    header[0] = (tapeVersion << 32) | _n;
    header.setRange(1, 1 + _n, _words);
    out
      ..setRange(wordsBytes, wordsBytes + _in.length, _in)
      ..setRange(wordsBytes + _in.length, out.length, _extra);
    return out;
  }

  Never _fail(String message) => throw FormatException(message, _in, _pos);

  void _push(int word) {
    if (_n == _words.length) {
      _words = Int64List(_words.length * 2)..setAll(0, _words);
    }
    _words[_n++] = word;
  }

  void _skipWs() {
    final b = _in;
    var p = _pos;
    while (p < b.length) {
      final c = b[p];
      if (c != 0x20 && c != 0x0A && c != 0x0D && c != 0x09) break;
      p++;
    }
    _pos = p;
  }

  void _parseValue() {
    if (_pos >= _in.length) _fail('Unexpected end of input');
    switch (_in[_pos]) {
      case 0x7B: // {
        _parseContainer(tagObject, 0x7D);
      case 0x5B: // [
        _parseContainer(tagArray, 0x5D);
      case 0x22: // "
        _parseString();
      case 0x74: // t
        _literal('true');
        _push(tagTrue << 56);
      case 0x66: // f
        _literal('false');
        _push(tagFalse << 56);
      case 0x6E: // n
        _literal('null');
        _push(tagNull << 56);
      default:
        _parseNumber();
    }
  }

  void _literal(String word) {
    if (_pos + word.length > _in.length) _fail('Unexpected end of input');
    for (var i = 0; i < word.length; i++) {
      if (_in[_pos + i] != word.codeUnitAt(i)) _fail('Invalid literal');
    }
    _pos += word.length;
  }

  void _parseContainer(int tag, int close) {
    final start = _n;
    _push(0);
    _push(0);
    _pos++; // opening bracket
    var count = 0;
    _skipWs();
    if (_pos < _in.length && _in[_pos] == close) {
      _pos++;
    } else {
      while (true) {
        _skipWs();
        if (tag == tagObject) {
          if (_pos >= _in.length || _in[_pos] != 0x22) _fail('Expected key');
          _parseString();
          _skipWs();
          if (_pos >= _in.length || _in[_pos] != 0x3A) _fail('Expected ":"');
          _pos++;
          _skipWs();
        }
        _parseValue();
        count++;
        _skipWs();
        if (_pos >= _in.length) _fail('Unexpected end of input');
        final c = _in[_pos++];
        if (c == close) break;
        if (c != 0x2C) _fail('Expected "," or closing bracket');
      }
    }
    _words[start] = (tag << 56) | _n;
    _words[start + 1] = count;
  }

  void _parseString() {
    final b = _in;
    var p = ++_pos; // opening quote
    final start = p;
    var ascii = true;
    while (true) {
      if (p >= b.length) _fail('Unterminated string');
      final c = b[p];
      if (c == 0x22) break;
      if (c == 0x5C) {
        _pos = p;
        return _parseEscapedString(start);
      }
      if (c < 0x20) _fail('Control character in string');
      if (c >= 0x80) ascii = false;
      p++;
    }
    _pos = p + 1;
    _push((tagString << 56) | start);
    _push((p - start) | (ascii ? _asciiFlag : 0));
  }

  void _ensureExtra(int more) {
    if (_extraLength + more <= _extra.length) return;
    var capacity = _extra.isEmpty ? 64 : _extra.length * 2;
    while (capacity < _extraLength + more) {
      capacity *= 2;
    }
    _extra = Uint8List(capacity)..setRange(0, _extraLength, _extra);
  }

  void _addExtra(int byte) {
    _ensureExtra(1);
    _extra[_extraLength++] = byte;
  }

  /// Slow path: string with escapes, copied unescaped into the extra region.
  void _parseEscapedString(int start) {
    final b = _in;
    final outStart = _extraLength;
    var ascii = true;
    // Copy the prefix before the first escape.
    _ensureExtra(_pos - start);
    _extra.setRange(_extraLength, _extraLength + _pos - start, b, start);
    _extraLength += _pos - start;
    for (var i = start; i < _pos; i++) {
      if (b[i] >= 0x80) ascii = false;
    }
    var p = _pos;
    while (true) {
      if (p >= b.length) _fail('Unterminated string');
      final c = b[p];
      if (c == 0x22) break;
      if (c == 0x5C) {
        if (p + 1 >= b.length) _fail('Unterminated string');
        final e = b[p + 1];
        p += 2;
        switch (e) {
          case 0x22 || 0x5C || 0x2F:
            _addExtra(e);
          case 0x62:
            _addExtra(0x08);
          case 0x66:
            _addExtra(0x0C);
          case 0x6E:
            _addExtra(0x0A);
          case 0x72:
            _addExtra(0x0D);
          case 0x74:
            _addExtra(0x09);
          case 0x75: // \uXXXX
            var cp = _hex4(p);
            p += 4;
            if (cp >= 0xD800 &&
                cp <= 0xDBFF &&
                p + 6 <= b.length &&
                b[p] == 0x5C &&
                b[p + 1] == 0x75) {
              final low = _hex4(p + 2);
              if (low >= 0xDC00 && low <= 0xDFFF) {
                cp = 0x10000 + ((cp - 0xD800) << 10) + (low - 0xDC00);
                p += 6;
              }
            }
            if (cp >= 0x80) ascii = false;
            _addCodePoint(cp);
          default:
            _fail('Invalid escape');
        }
        continue;
      }
      if (c < 0x20) _fail('Control character in string');
      if (c >= 0x80) ascii = false;
      _addExtra(c);
      p++;
    }
    _pos = p + 1;
    _push((tagString << 56) | (_in.length + outStart));
    _push((_extraLength - outStart) | (ascii ? _asciiFlag : 0));
  }

  int _hex4(int p) {
    if (p + 4 > _in.length) _fail('Invalid \\u escape');
    var v = 0;
    for (var i = 0; i < 4; i++) {
      final c = _in[p + i];
      final d = switch (c) {
        >= 0x30 && <= 0x39 => c - 0x30,
        >= 0x41 && <= 0x46 => c - 0x37,
        >= 0x61 && <= 0x66 => c - 0x57,
        _ => _fail('Invalid \\u escape'),
      };
      v = (v << 4) | d;
    }
    return v;
  }

  void _addCodePoint(int cp) {
    if (cp < 0x80) {
      _addExtra(cp);
    } else if (cp < 0x800) {
      _addExtra(0xC0 | (cp >> 6));
      _addExtra(0x80 | (cp & 0x3F));
    } else if (cp < 0x10000) {
      _addExtra(0xE0 | (cp >> 12));
      _addExtra(0x80 | ((cp >> 6) & 0x3F));
      _addExtra(0x80 | (cp & 0x3F));
    } else {
      _addExtra(0xF0 | (cp >> 18));
      _addExtra(0x80 | ((cp >> 12) & 0x3F));
      _addExtra(0x80 | ((cp >> 6) & 0x3F));
      _addExtra(0x80 | (cp & 0x3F));
    }
  }

  void _parseNumber() {
    final b = _in;
    final start = _pos;
    var p = start;
    var negative = false;
    if (p < b.length && b[p] == 0x2D) {
      negative = true;
      p++;
    }
    // RFC 8259 grammar: int = "0" / [1-9]*DIGIT; frac = "." 1*DIGIT;
    // exp = ("e" / "E") ["+" / "-"] 1*DIGIT. Same strictness as simd-json.
    bool isDigit(int i) => i < b.length && b[i] >= 0x30 && b[i] <= 0x39;
    var value = 0;
    var digits = 0;
    if (!isDigit(p)) _fail('Unexpected character');
    if (b[p] == 0x30 && isDigit(p + 1)) _fail('Leading zero in a number');
    while (isDigit(p)) {
      value = value * 10 + (b[p] - 0x30);
      digits++;
      p++;
    }
    var isDouble = false;
    if (p < b.length && b[p] == 0x2E) {
      isDouble = true;
      p++;
      if (!isDigit(p)) _fail('Expected a digit after the decimal point');
      while (isDigit(p)) {
        p++;
      }
    }
    if (p < b.length && (b[p] == 0x65 || b[p] == 0x45)) {
      isDouble = true;
      p++;
      if (p < b.length && (b[p] == 0x2B || b[p] == 0x2D)) p++;
      if (!isDigit(p)) _fail('Expected a digit in the exponent');
      while (isDigit(p)) {
        p++;
      }
    }
    _pos = p;
    if (!isDouble && digits <= 18) {
      _push(tagInt << 56);
      _push(negative ? -value : value);
      return;
    }
    final text = String.fromCharCodes(b, start, p);
    final num parsed = isDouble
        ? double.parse(text)
        : (int.tryParse(text) ?? double.parse(text));
    if (parsed is int) {
      _push(tagInt << 56);
      _push(parsed);
    } else {
      _push(tagDouble << 56);
      _push(_doubleBits(parsed as double));
    }
  }

  static final ByteData _scratch = ByteData(8);
  static int _doubleBits(double v) =>
      (_scratch..setFloat64(0, v, Endian.little)).getInt64(0, Endian.little);
}
