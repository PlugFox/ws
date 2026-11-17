/// Prototype RFC 6455 client on top of `RawSocket` / `RawSecureSocket`.
///
/// Candidate for the new VM transport of the `ws` package. Goals:
/// * whole-buffer frame parsing, zero-copy payload views when a frame fits
///   into a single socket read;
/// * raw payload bytes for text frames, so the consumer can run a fused
///   UTF-8 + JSON decoder without an intermediate `String`;
/// * word-wise masking on send;
/// * no `dart:async` streams on the hot path — plain callbacks.
///
/// Not supported yet: permessage-deflate, proxies, redirects.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:crypto/crypto.dart' show sha1;

const int opContinuation = 0x0;
const int opText = 0x1;
const int opBinary = 0x2;
const int opClose = 0x8;
const int opPing = 0x9;
const int opPong = 0xA;

/// Called for every complete data message. [payload] may be a view into a
/// socket buffer: it is valid only during the call unless copied.
typedef MessageCallback = void Function(int opcode, Uint8List payload);

/// Called for every chunk of data read from the socket — after all messages
/// in it were dispatched. Useful to batch work per socket read.
typedef ReadEndCallback = void Function();

final class WsCodecClient {
  WsCodecClient._(this._socket, this.onMessage, this.onReadEnd);

  final RawSocket _socket;
  final MessageCallback onMessage;
  final ReadEndCallback? onReadEnd;

  final Completer<void> _done = Completer<void>();

  /// Completes when the connection is closed (by either side).
  Future<void> get done => _done.future;

  int? closeCode;
  String? closeReason;

  // --- connect / handshake ------------------------------------------------

  static Future<WsCodecClient> connect(
    Uri uri, {
    required MessageCallback onMessage,
    ReadEndCallback? onReadEnd,
    bool allowBadCertificates = false,
    Duration timeout = const Duration(seconds: 10),
  }) async {
    final secure = uri.scheme == 'wss' || uri.scheme == 'https';
    final port = uri.hasPort ? uri.port : (secure ? 443 : 80);
    final RawSocket socket = secure
        ? await RawSecureSocket.connect(
            uri.host,
            port,
            timeout: timeout,
            onBadCertificate: allowBadCertificates ? (_) => true : null,
          )
        : await RawSocket.connect(uri.host, port, timeout: timeout);
    socket.setOption(SocketOption.tcpNoDelay, true);

    final client = WsCodecClient._(socket, onMessage, onReadEnd);
    await client._handshake(uri, port, secure).timeout(timeout);
    return client;
  }

  static const _guid = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

  Future<void> _handshake(Uri uri, int port, bool secure) {
    final rnd = math.Random.secure();
    final key = base64.encode(List<int>.generate(16, (_) => rnd.nextInt(256)));
    final defaultPort = port == (secure ? 443 : 80);
    final host = defaultPort ? uri.host : '${uri.host}:$port';
    final path = uri.hasQuery ? '${uri.path}?${uri.query}' : uri.path;
    final request = 'GET ${path.isEmpty ? '/' : path} HTTP/1.1\r\n'
        'Host: $host\r\n'
        'Upgrade: websocket\r\n'
        'Connection: Upgrade\r\n'
        'Sec-WebSocket-Key: $key\r\n'
        'Sec-WebSocket-Version: 13\r\n'
        '\r\n';
    final expectedAccept =
        base64.encode(sha1.convert(ascii.encode(key + _guid)).bytes);

    final handshake = Completer<void>();
    final header = BytesBuilder(copy: false);
    var upgraded = false;

    _socket.listen(
      (event) {
        switch (event) {
          case RawSocketEvent.read:
            final data = _socket.read();
            if (data == null) return;
            if (upgraded) {
              _onData(data);
              onReadEnd?.call();
              return;
            }
            header.add(data);
            final bytes = header.toBytes();
            final end = _indexOfHeaderEnd(bytes);
            if (end < 0) return;
            try {
              _checkResponse(latin1.decode(bytes.sublist(0, end)), expectedAccept);
            } on Object catch (e, s) {
              handshake.completeError(e, s);
              _socket.close();
              return;
            }
            upgraded = true;
            handshake.complete();
            if (end + 4 < bytes.length) {
              _onData(Uint8List.sublistView(bytes, end + 4));
              onReadEnd?.call();
            }
          case RawSocketEvent.write:
            _flushWrites();
          case RawSocketEvent.readClosed:
          case RawSocketEvent.closed:
            _finish();
        }
      },
      onError: (Object e, StackTrace s) {
        if (!handshake.isCompleted) handshake.completeError(e, s);
        _finish();
      },
      onDone: _finish,
    );
    _write(ascii.encode(request));
    return handshake.future;
  }

  static int _indexOfHeaderEnd(Uint8List b) {
    for (var i = 0; i + 3 < b.length; i++) {
      if (b[i] == 13 && b[i + 1] == 10 && b[i + 2] == 13 && b[i + 3] == 10) {
        return i;
      }
    }
    return -1;
  }

  static void _checkResponse(String response, String expectedAccept) {
    final lines = response.split('\r\n');
    if (lines.isEmpty || !lines.first.startsWith('HTTP/1.1 101')) {
      throw WebSocketException('Unexpected handshake response: ${lines.first}');
    }
    String? accept;
    for (final line in lines.skip(1)) {
      final i = line.indexOf(':');
      if (i < 0) continue;
      if (line.substring(0, i).trim().toLowerCase() == 'sec-websocket-accept') {
        accept = line.substring(i + 1).trim();
      }
    }
    if (accept != expectedAccept) {
      throw const WebSocketException('Invalid Sec-WebSocket-Accept');
    }
  }

  // --- receive ------------------------------------------------------------

  /// Leftover bytes of an incomplete frame header (< 14 bytes).
  Uint8List? _partialHeader;

  /// Payload of the current frame being filled across reads.
  Uint8List? _framePayload;
  int _frameFilled = 0;
  int _frameOpcode = 0;
  bool _frameFin = false;

  /// Opcode of the fragmented message being assembled (0 if none).
  int _messageOpcode = 0;
  BytesBuilder? _fragments;

  void _onData(Uint8List data) {
    var buf = data;
    var off = 0;

    // Continue filling a frame payload from previous reads.
    final payload = _framePayload;
    if (payload != null) {
      final n = math.min(payload.length - _frameFilled, buf.length);
      payload.setRange(_frameFilled, _frameFilled + n, buf);
      _frameFilled += n;
      off = n;
      if (_frameFilled < payload.length) return;
      _framePayload = null;
      _onFrame(_frameOpcode, _frameFin, payload);
    }

    // Glue a partial header with the new data (rare, tiny copy).
    final partial = _partialHeader;
    if (partial != null) {
      _partialHeader = null;
      final glued = Uint8List(partial.length + buf.length - off)
        ..setAll(0, partial)
        ..setRange(partial.length, partial.length + buf.length - off, buf, off);
      buf = glued;
      off = 0;
    }

    final len = buf.length;
    while (off < len) {
      if (len - off < 2) {
        _partialHeader = Uint8List.fromList(buf.sublist(off));
        return;
      }
      final b0 = buf[off];
      final b1 = buf[off + 1];
      final fin = b0 & 0x80 != 0;
      final opcode = b0 & 0x0F;
      if (b0 & 0x70 != 0) return _protocolError('RSV bits set');
      if (b1 & 0x80 != 0) return _protocolError('Masked server frame');
      var payloadLen = b1 & 0x7F;
      var headerLen = 2;
      if (payloadLen == 126) {
        headerLen = 4;
        if (len - off < headerLen) {
          _partialHeader = Uint8List.fromList(buf.sublist(off));
          return;
        }
        payloadLen = (buf[off + 2] << 8) | buf[off + 3];
      } else if (payloadLen == 127) {
        headerLen = 10;
        if (len - off < headerLen) {
          _partialHeader = Uint8List.fromList(buf.sublist(off));
          return;
        }
        payloadLen = 0;
        for (var i = 2; i < 10; i++) {
          payloadLen = (payloadLen << 8) | buf[off + i];
        }
      }
      off += headerLen;
      final available = len - off;
      if (available >= payloadLen) {
        // Fast path: whole frame in this buffer — zero-copy view.
        final view = Uint8List.sublistView(buf, off, off + payloadLen);
        off += payloadLen;
        _onFrame(opcode, fin, view);
      } else {
        // Slow path: allocate the exact payload and keep filling it.
        final target = Uint8List(payloadLen)..setRange(0, available, buf, off);
        _framePayload = target;
        _frameFilled = available;
        _frameOpcode = opcode;
        _frameFin = fin;
        return;
      }
    }
  }

  void _onFrame(int opcode, bool fin, Uint8List payload) {
    switch (opcode) {
      case opText || opBinary:
        if (_messageOpcode != 0) return _protocolError('Expected continuation');
        if (fin) return onMessage(opcode, payload);
        _messageOpcode = opcode;
        _fragments = BytesBuilder()..add(payload);
      case opContinuation:
        final fragments = _fragments;
        if (fragments == null) return _protocolError('Unexpected continuation');
        fragments.add(payload);
        if (!fin) return;
        final op = _messageOpcode;
        _messageOpcode = 0;
        _fragments = null;
        onMessage(op, fragments.takeBytes());
      case opPing:
        _sendFrame(opPong, payload);
      case opPong:
        break;
      case opClose:
        if (payload.length >= 2) {
          closeCode = (payload[0] << 8) | payload[1];
          closeReason = utf8.decode(
            Uint8List.sublistView(payload, 2),
            allowMalformed: true,
          );
        } else {
          closeCode = 1005;
        }
        if (!_closeSent) {
          _closeSent = true;
          _sendFrame(opClose, payload.length >= 2 ? payload.sublist(0, 2) : payload);
        }
        _closeWhenFlushed = true;
        _flushWrites();
      default:
        _protocolError('Unknown opcode $opcode');
    }
  }

  void _protocolError(String reason) {
    closeCode = 1002;
    closeReason = reason;
    close(1002, reason);
  }

  // --- send ---------------------------------------------------------------

  bool _closeSent = false;
  bool _closeWhenFlushed = false;
  final List<Uint8List> _writeQueue = <Uint8List>[];
  int _writeOffset = 0;

  // Fast non-cryptographic mask generator (xorshift32), seeded securely.
  int _maskState = math.Random.secure().nextInt(0x7FFFFFFF) | 1;
  int _nextMask() {
    var x = _maskState;
    x ^= (x << 13) & 0xFFFFFFFF;
    x ^= x >> 17;
    x ^= (x << 5) & 0xFFFFFFFF;
    return _maskState = x;
  }

  void sendText(String text) => _sendFrame(opText, utf8.encode(text));

  /// Sends already UTF-8 encoded text.
  void sendUtf8(Uint8List bytes) => _sendFrame(opText, bytes);

  void sendBinary(Uint8List bytes) => _sendFrame(opBinary, bytes);

  void _sendFrame(int opcode, Uint8List payload) {
    if (_closeSent && opcode != opClose) {
      throw const WebSocketException('Connection is closing');
    }
    final n = payload.length;
    final headerLen = n < 126 ? 6 : (n < 65536 ? 8 : 14);
    // Place the payload at an 8-byte aligned offset to mask it word-wise.
    const payloadOffset = 16;
    final buffer = Uint8List(payloadOffset + n);
    final start = payloadOffset - headerLen;
    buffer[start] = 0x80 | opcode;
    if (n < 126) {
      buffer[start + 1] = 0x80 | n;
    } else if (n < 65536) {
      buffer[start + 1] = 0x80 | 126;
      buffer[start + 2] = n >> 8;
      buffer[start + 3] = n & 0xFF;
    } else {
      buffer[start + 1] = 0x80 | 127;
      for (var i = 0; i < 8; i++) {
        buffer[start + 2 + i] = (n >> (56 - 8 * i)) & 0xFF;
      }
    }
    final mask = _nextMask();
    buffer
      ..[payloadOffset - 4] = mask & 0xFF
      ..[payloadOffset - 3] = (mask >> 8) & 0xFF
      ..[payloadOffset - 2] = (mask >> 16) & 0xFF
      ..[payloadOffset - 1] = (mask >> 24) & 0xFF
      ..setRange(payloadOffset, payloadOffset + n, payload);
    _mask(buffer, payloadOffset, n, mask);
    _write(Uint8List.sublistView(buffer, start));
  }

  /// XORs `buffer[offset, offset + length)` with the 4-byte [mask]
  /// (little-endian byte order), 8 bytes at a time. [offset] must be 8-aligned.
  static void _mask(Uint8List buffer, int offset, int length, int mask) {
    final words = length >> 3;
    if (words > 0) {
      final mask64 = mask | (mask << 32);
      final view = buffer.buffer.asUint64List(buffer.offsetInBytes + offset, words);
      for (var i = 0; i < words; i++) {
        view[i] ^= mask64;
      }
    }
    for (var i = words << 3; i < length; i++) {
      buffer[offset + i] ^= (mask >> ((i & 3) << 3)) & 0xFF;
    }
  }

  void _write(Uint8List bytes) {
    if (_writeQueue.isEmpty) {
      final written = _socket.write(bytes);
      if (written == bytes.length) return;
      _writeQueue.add(bytes);
      _writeOffset = written;
      _socket.writeEventsEnabled = true;
    } else {
      _writeQueue.add(bytes);
    }
  }

  void _flushWrites() {
    while (_writeQueue.isNotEmpty) {
      final head = _writeQueue.first;
      final written = _socket.write(head, _writeOffset);
      _writeOffset += written;
      if (_writeOffset < head.length) {
        _socket.writeEventsEnabled = true;
        return;
      }
      _writeQueue.removeAt(0);
      _writeOffset = 0;
    }
    if (_closeWhenFlushed) {
      _socket.close();
      _finish();
    }
  }

  // --- close --------------------------------------------------------------

  Future<void> close([int code = 1000, String reason = '']) {
    if (!_closeSent) {
      _closeSent = true;
      final reasonBytes = utf8.encode(reason);
      final payload = Uint8List(2 + reasonBytes.length)
        ..[0] = code >> 8
        ..[1] = code & 0xFF
        ..setAll(2, reasonBytes);
      _sendFrame(opClose, payload);
    }
    // Wait for the server's close frame, but not forever.
    _closeTimer ??= Timer(const Duration(seconds: 2), () {
      _socket.close();
      _finish();
    });
    return done;
  }

  Timer? _closeTimer;

  void _finish() {
    _closeTimer?.cancel();
    if (_done.isCompleted) return;
    closeCode ??= 1006;
    _done.complete();
  }
}
