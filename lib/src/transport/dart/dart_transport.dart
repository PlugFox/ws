import 'dart:async';
import 'dart:collection';
import 'dart:convert' show utf8;
import 'dart:io' as io;
import 'dart:typed_data';

import 'package:ws/src/client/exceptions.dart';
import 'package:ws/src/protocol/frame.dart';
import 'package:ws/src/protocol/frame_reader.dart';
import 'package:ws/src/protocol/frame_writer.dart';
import 'package:ws/src/protocol/handshake.dart';
import 'package:ws/src/transport/transport.dart';

/// The platform transport on native platforms: pure Dart.
WebSocketTransport createPlatformTransport() => const DartTransport();

/// RFC 6455 client in pure Dart on `RawSocket` / `RawSecureSocket`.
///
/// Callbacks instead of streams on the hot path, zero-copy frame views,
/// word-wise masking (DECISIONS.md D4–D7). The fallback when the native
/// transport is not installed or cannot load.
final class DartTransport implements WebSocketTransport {
  /// Creates the transport.
  const DartTransport();

  @override
  String get name => 'dart';

  @override
  bool get supportsPing => true;

  @override
  bool get decodesJson => false;

  @override
  TransportConnection connect(
    TransportRequest request,
    TransportListener listener,
  ) => _DartConnection(request, listener).._start();
}

/// Response heads larger than this are rejected (no legitimate handshake
/// response comes close).
const int _maxHeadSize = 64 * 1024;

final class _DartConnection implements TransportConnection, FrameHandler {
  _DartConnection(this._request, this._listener)
    : _writer = FrameWriter(mask: true);

  final TransportRequest _request;
  final TransportListener _listener;
  final FrameWriter _writer;
  late final FrameReader _reader = FrameReader(
    this,
    maxMessageSize: _request.maxMessageSize,
    validateUtf8: false,
  );

  io.ConnectionTask<io.RawSocket>? _task;
  io.RawSocket? _socket;
  StreamSubscription<io.RawSocketEvent>? _subscription;

  bool _open = false;
  bool _finished = false;
  bool _closeSent = false;
  bool _closeAfterFlush = false;

  int? _closeCode;
  String _closeReason = '';
  WebSocketException? _error;

  // --- connect ---------------------------------------------------------------

  Future<void> _start() async {
    final uri = _request.url;
    final handshake = HandshakeRequest(
      uri,
      headers: _request.headers,
      protocols: _request.protocols,
    );
    final host = uri.host;
    try {
      final io.ConnectionTask<io.RawSocket> task;
      if (handshake.secure) {
        final tls = _request.tls;
        final context = tls.trustedRootsPem == null
            ? null
            : (io.SecurityContext(withTrustedRoots: true)
                ..setTrustedCertificatesBytes(
                  Uint8List.fromList(tls.trustedRootsPem!.codeUnits),
                ));
        task = await io.RawSecureSocket.startConnect(
          host,
          handshake.port,
          context: context,
          onBadCertificate: tls.allowInsecure ? (_) => true : null,
        );
      } else {
        task = await io.RawSocket.startConnect(host, handshake.port);
      }
      if (_finished) return task.cancel();
      _task = task;
      final socket = await task.socket;
      _task = null;
      if (_finished) return socket.close().ignore();
      _socket = socket;
      socket.setOption(io.SocketOption.tcpNoDelay, true);
    } on Object catch (e) {
      return _finish(
        null,
        '',
        e is io.TlsException || e is io.HandshakeException
            ? WebSocketConnectionException('TLS handshake failed', cause: e)
            : WebSocketConnectionException(
                'Could not connect to $host',
                cause: e,
              ),
      );
    }

    final head = BytesBuilder(copy: false);
    _subscription = _socket!.listen(
      (event) {
        switch (event) {
          case io.RawSocketEvent.read:
            final data = _socket?.read();
            if (data == null || data.isEmpty) return;
            if (_open) return _reader.add(data);
            head.add(data);
            final bytes = head.takeBytes();
            // Search before checking the limit: the 101 response and the
            // first (large) frame often arrive in one read (found by
            // Autobahn 1.1.6 / 9.x).
            final end = HandshakeResponse.headEnd(bytes);
            if (end >= 0) return _onHandshake(handshake, bytes, end);
            if (bytes.length > _maxHeadSize) {
              return _fail(
                const WebSocketHandshakeException(
                  'Handshake response is too large',
                ),
              );
            }
            head.add(bytes);
          case io.RawSocketEvent.write:
            _flush();
          case io.RawSocketEvent.readClosed:
          case io.RawSocketEvent.closed:
            _finish(
              null,
              '',
              _open
                  ? const WebSocketConnectionException(
                      'Connection closed without a close frame',
                    )
                  : const WebSocketHandshakeException(
                      'Connection closed during the handshake',
                    ),
            );
        }
      },
      onError: (Object e) => _finish(
        null,
        '',
        WebSocketConnectionException('Socket error', cause: e),
      ),
      onDone: () => _finish(
        null,
        '',
        const WebSocketConnectionException('Connection closed'),
      ),
      cancelOnError: true,
    );
    _write(handshake.encode());
  }

  void _onHandshake(HandshakeRequest request, Uint8List bytes, int end) {
    final String? protocol;
    try {
      protocol = HandshakeResponse.parse(
        Uint8List.sublistView(bytes, 0, end),
      ).validate(request);
    } on HandshakeError catch (e) {
      return _fail(
        WebSocketHandshakeException(
          e.message,
          statusCode: e.statusCode,
          headers: e.headers,
          cause: e,
        ),
      );
    } on FormatException catch (e) {
      return _fail(
        WebSocketHandshakeException('Invalid handshake response', cause: e),
      );
    }
    _open = true;
    _listener.onOpen(protocol);
    if (_finished) return;
    if (end + 4 < bytes.length) {
      _reader.add(Uint8List.sublistView(bytes, end + 4));
    }
  }

  // --- FrameHandler ---------------------------------------------------------

  @override
  void onText(Uint8List payload) => _listener.onTextBytes(payload);

  @override
  void onBinary(Uint8List payload) => _listener.onBinary(payload);

  @override
  void onPing(Uint8List payload) {
    if (!_closeSent) _write(_writer.frame(Opcode.pong, payload));
  }

  @override
  void onPong(Uint8List payload) => _listener.onPong();

  @override
  void onClose(int? code, String reason) {
    _closeCode = code;
    _closeReason = reason;
    if (!_closeSent) {
      _closeSent = true;
      // Echo the status code (RFC 6455 §5.5.1).
      _write(_writer.close(code));
    }
    _closeAfterFlush = true;
    _flush();
  }

  @override
  void onFailure(int code, String reason) {
    _error = WebSocketClosedException(
      'Protocol violation by the server: $reason',
      code: code,
      reason: reason,
      byServer: false,
    );
    _closeCode = code;
    _closeReason = reason;
    if (!_closeSent) {
      _closeSent = true;
      _write(_writer.close(code, reason));
    }
    _closeAfterFlush = true;
    _flush();
  }

  // --- send -----------------------------------------------------------------

  final Queue<Uint8List> _queue = Queue<Uint8List>();
  int _queueOffset = 0;
  int _buffered = 0;

  @override
  int get bufferedAmount => _buffered;

  bool _drainRequested = false;

  @override
  void requestDrain() {
    if (_queue.isNotEmpty) {
      _drainRequested = true;
    } else {
      scheduleMicrotask(() {
        if (!_finished) _listener.onDrain();
      });
    }
  }

  void _send(int opcode, Uint8List payload) {
    if (_closeSent || _finished) {
      throw const WebSocketNotConnectedException('The connection is closing');
    }
    _write(_writer.frame(opcode, payload));
  }

  @override
  void sendText(String text) => _send(Opcode.text, utf8.encode(text));

  @override
  void sendTextBytes(Uint8List bytes) => _send(Opcode.text, bytes);

  @override
  void sendBinary(Uint8List bytes) => _send(Opcode.binary, bytes);

  @override
  void ping() => _send(Opcode.ping, Uint8List(0));

  void _write(Uint8List bytes) {
    final socket = _socket;
    if (socket == null || _finished) return;
    if (_queue.isEmpty) {
      final written = socket.write(bytes);
      if (written == bytes.length) return;
      _queue.add(bytes);
      _queueOffset = written;
      _buffered += bytes.length - written;
      socket.writeEventsEnabled = true;
    } else {
      _queue.add(bytes);
      _buffered += bytes.length;
    }
  }

  void _flush() {
    final socket = _socket;
    if (socket == null) return;
    while (_queue.isNotEmpty) {
      final head = _queue.first;
      final written = socket.write(head, _queueOffset);
      _queueOffset += written;
      _buffered -= written;
      if (_queueOffset < head.length) {
        socket.writeEventsEnabled = true;
        return;
      }
      _queue.removeFirst();
      _queueOffset = 0;
    }
    if (_drainRequested && !_finished) {
      _drainRequested = false;
      _listener.onDrain();
    }
    if (_closeAfterFlush) {
      socket.close().ignore();
      _finish(_closeCode, _closeReason, _error);
    }
  }

  // --- close ----------------------------------------------------------------

  @override
  void close(int? code, String? reason) {
    if (_closeSent || _finished) return;
    _closeSent = true;
    if (!_open) return abort();
    _write(_writer.close(code, reason ?? ''));
  }

  @override
  void abort() {
    if (_finished) return;
    // Never call the listener synchronously from a connection method.
    scheduleMicrotask(
      () => _finish(
        null,
        '',
        const WebSocketConnectionException('Connection aborted'),
      ),
    );
    _task?.cancel();
    _socket?.close().ignore();
  }

  void _fail(WebSocketException error) {
    _socket?.close().ignore();
    _finish(null, '', error);
  }

  void _finish(int? code, String reason, WebSocketException? error) {
    if (_finished) return;
    _finished = true;
    _reader.stop();
    _subscription?.cancel().ignore();
    _subscription = null;
    _queue.clear();
    _buffered = 0;
    final socket = _socket;
    _socket = null;
    socket?.close().ignore();
    _listener.onClosed(code, reason, error);
  }
}
