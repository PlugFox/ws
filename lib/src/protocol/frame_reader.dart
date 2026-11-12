import 'dart:convert' show utf8;
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:ws/src/protocol/frame.dart';
import 'package:ws/src/protocol/utf8_validator.dart';

/// Receives complete messages and control frames from a [FrameReader].
abstract interface class FrameHandler {
  /// A complete text message as UTF-8 bytes. Fragmented messages are always
  /// validated (incrementally, failing fast); single-frame messages only
  /// with `validateUtf8: true`, because decoders validate anyway
  /// (DECISIONS.md D5).
  ///
  /// [payload] may be a view into a larger buffer; it is never reused by the
  /// reader, so it is safe to keep.
  void onText(Uint8List payload);

  /// A complete binary message. Same buffer rules as [onText].
  void onBinary(Uint8List payload);

  /// A ping frame. The caller answers with a pong.
  void onPing(Uint8List payload);

  /// A pong frame.
  void onPong(Uint8List payload);

  /// A close frame. [code] is `null` when the frame had no status code.
  void onClose(int? code, String reason);

  /// The peer violated the protocol or a limit. The caller must send a close
  /// frame with [code] and fail the connection. No more calls follow.
  void onFailure(int code, String reason);
}

/// Incremental RFC 6455 frame parser.
///
/// Feed it whatever the socket returns with [add]. Frames that fit in one
/// read are delivered as zero-copy views; larger frames are collected into a
/// buffer allocated once with the exact size. Limits are checked before any
/// allocation.
final class FrameReader {
  /// Creates a reader.
  ///
  /// [expectMasked] is `false` for clients (servers must not mask) and
  /// `true` for servers. [maxMessageSize] limits a frame and a whole
  /// fragmented message, in bytes.
  FrameReader(
    this._handler, {
    bool expectMasked = false,
    int maxMessageSize = 64 << 20,
    bool validateUtf8 = true,
  }) : _expectMasked = expectMasked,
       _maxMessageSize = maxMessageSize,
       _validateUtf8 = validateUtf8;

  final FrameHandler _handler;
  final bool _expectMasked;
  final int _maxMessageSize;
  final bool _validateUtf8;

  bool _stopped = false;

  /// Bytes of a frame header split across reads (at most 14 bytes).
  Uint8List? _partialHeader;

  // The frame whose payload is being filled across reads.
  Uint8List? _payload;
  int _filled = 0;
  int _opcode = 0;
  bool _fin = false;
  int _maskKey = 0;
  bool _masked = false;

  // The fragmented message being assembled.
  int _messageOpcode = 0;
  List<Uint8List>? _fragments;
  int _messageLength = 0;
  final Utf8Validator _utf8 = Utf8Validator();

  /// Whether the reader stopped after a close frame or a failure.
  bool get isStopped => _stopped;

  /// Stops parsing; later input is ignored.
  void stop() => _stopped = true;

  /// Parses [data]. Calls the handler synchronously for every complete
  /// message and control frame.
  void add(Uint8List data) {
    if (_stopped) return;
    var buf = data;
    var off = 0;

    final payload = _payload;
    if (payload != null) {
      final n = math.min(payload.length - _filled, buf.length);
      payload.setRange(_filled, _filled + n, buf);
      _filled += n;
      off = n;
      if (_filled < payload.length) return;
      _payload = null;
      if (_masked) _unmask(payload, 0, payload.length, _maskKey);
      _frame(_opcode, _fin, payload);
      if (_stopped) return;
    }

    final partial = _partialHeader;
    if (partial != null) {
      _partialHeader = null;
      buf = Uint8List(partial.length + buf.length - off)
        ..setAll(0, partial)
        ..setRange(partial.length, partial.length + buf.length - off, buf, off);
      off = 0;
    }

    final len = buf.length;
    while (off < len && !_stopped) {
      final available = len - off;
      if (available < 2) return _keepHeader(buf, off);
      final b0 = buf[off];
      final b1 = buf[off + 1];
      final fin = b0 & 0x80 != 0;
      final opcode = b0 & 0x0F;
      final masked = b1 & 0x80 != 0;
      if (b0 & 0x70 != 0) {
        return _fail(CloseCode.protocolError, 'Reserved bits are set');
      }
      if (masked != _expectMasked) {
        return _fail(
          CloseCode.protocolError,
          masked ? 'Masked frame from server' : 'Unmasked frame from client',
        );
      }
      var length = b1 & 0x7F;
      var header = 2;
      if (length == 126) {
        header = 4;
        if (available < header) return _keepHeader(buf, off);
        length = (buf[off + 2] << 8) | buf[off + 3];
      } else if (length == 127) {
        header = 10;
        if (available < header) return _keepHeader(buf, off);
        if (buf[off + 2] & 0x80 != 0) {
          return _fail(CloseCode.protocolError, 'Invalid payload length');
        }
        length = 0;
        for (var i = 2; i < 10; i++) {
          // Multiplication instead of shifts keeps this exact on the web.
          length = length * 256 + buf[off + i];
        }
      }
      if (masked) header += 4;
      if (available < header) return _keepHeader(buf, off);

      if (opcode >= 0x8) {
        if (!fin) {
          return _fail(CloseCode.protocolError, 'Fragmented control frame');
        }
        if (length > 125) {
          return _fail(CloseCode.protocolError, 'Control frame too long');
        }
      } else if (length > _maxMessageSize ||
          _messageLength + length > _maxMessageSize) {
        return _fail(CloseCode.messageTooBig, 'Message too big');
      }

      final maskKey = masked
          ? (buf[off + header - 4] << 24) |
                (buf[off + header - 3] << 16) |
                (buf[off + header - 2] << 8) |
                buf[off + header - 1]
          : 0;
      off += header;
      if (len - off >= length) {
        final view = Uint8List.sublistView(buf, off, off + length);
        off += length;
        if (masked) _unmask(view, 0, length, maskKey);
        _frame(opcode, fin, view);
      } else {
        final target = Uint8List(length)..setRange(0, len - off, buf, off);
        _payload = target;
        _filled = len - off;
        _opcode = opcode;
        _fin = fin;
        _masked = masked;
        _maskKey = maskKey;
        return;
      }
    }
  }

  void _keepHeader(Uint8List buf, int off) =>
      _partialHeader = Uint8List.fromList(Uint8List.sublistView(buf, off));

  void _frame(int opcode, bool fin, Uint8List payload) {
    switch (opcode) {
      case Opcode.text || Opcode.binary:
        if (_messageOpcode != 0) {
          return _fail(CloseCode.protocolError, 'Expected a continuation');
        }
        if (fin) {
          if (opcode == Opcode.text &&
              _validateUtf8 &&
              !Utf8Validator.validate(payload)) {
            return _fail(CloseCode.invalidPayload, 'Invalid UTF-8');
          }
          return opcode == Opcode.text
              ? _handler.onText(payload)
              : _handler.onBinary(payload);
        }
        _messageOpcode = opcode;
        _fragments = <Uint8List>[payload];
        _messageLength = payload.length;
        if (opcode == Opcode.text) {
          _utf8.reset();
          if (!_utf8.add(payload)) {
            return _fail(CloseCode.invalidPayload, 'Invalid UTF-8');
          }
        }
      case Opcode.continuation:
        final fragments = _fragments;
        if (fragments == null) {
          return _fail(CloseCode.protocolError, 'Unexpected continuation');
        }
        fragments.add(payload);
        _messageLength += payload.length;
        final text = _messageOpcode == Opcode.text;
        if (text && !_utf8.add(payload)) {
          return _fail(CloseCode.invalidPayload, 'Invalid UTF-8');
        }
        if (!fin) return;
        if (text && !_utf8.isComplete) {
          return _fail(CloseCode.invalidPayload, 'Invalid UTF-8');
        }
        final message = _join(fragments, _messageLength);
        _messageOpcode = 0;
        _fragments = null;
        _messageLength = 0;
        text ? _handler.onText(message) : _handler.onBinary(message);
      case Opcode.ping:
        _handler.onPing(payload);
      case Opcode.pong:
        _handler.onPong(payload);
      case Opcode.close:
        _onCloseFrame(payload);
      default:
        _fail(CloseCode.protocolError, 'Unknown opcode $opcode');
    }
  }

  void _onCloseFrame(Uint8List payload) {
    if (payload.length == 1) {
      return _fail(CloseCode.protocolError, 'Invalid close frame');
    }
    if (payload.isEmpty) {
      _stopped = true;
      return _handler.onClose(null, '');
    }
    final code = (payload[0] << 8) | payload[1];
    if (!CloseCode.isValidOnWire(code)) {
      return _fail(CloseCode.protocolError, 'Invalid close code $code');
    }
    if (!Utf8Validator.validate(payload, 2)) {
      return _fail(CloseCode.invalidPayload, 'Invalid close reason');
    }
    _stopped = true;
    _handler.onClose(code, utf8.decode(Uint8List.sublistView(payload, 2)));
  }

  void _fail(int code, String reason) {
    _stopped = true;
    _payload = null;
    _fragments = null;
    _handler.onFailure(code, reason);
  }

  static Uint8List _join(List<Uint8List> parts, int length) {
    if (parts.length == 1) return parts.first;
    final out = Uint8List(length);
    var offset = 0;
    for (final p in parts) {
      out.setRange(offset, offset += p.length, p);
    }
    return out;
  }

  static void _unmask(Uint8List data, int start, int end, int key) {
    for (var i = start; i < end; i++) {
      data[i] ^= (key >> (24 - ((i - start) & 3) * 8)) & 0xFF;
    }
  }
}
