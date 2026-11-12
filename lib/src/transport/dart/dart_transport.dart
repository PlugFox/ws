import 'dart:async';
import 'dart:collection';
import 'dart:convert' show utf8;
import 'dart:io' as io;
import 'dart:typed_data';

import 'package:ws/src/client/exceptions.dart';
import 'package:ws/src/client/tls_pins.dart';
import 'package:ws/src/protocol/frame.dart';
import 'package:ws/src/protocol/frame_reader.dart';
import 'package:ws/src/protocol/frame_writer.dart';
import 'package:ws/src/protocol/handshake.dart';
import 'package:ws/src/transport/dart/certificate.dart';
import 'package:ws/src/transport/dart/errors.dart';
import 'package:ws/src/transport/dart/keep_alive.dart';
import 'package:ws/src/transport/dart/proxy_tunnel.dart';
import 'package:ws/src/transport/dart/trust.dart';
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
  ) {
    // A hopeless URL fails before a connection exists (terminal for the
    // client, which treats ArgumentError from connect() so).
    HandshakeRequest.checkHost(request.url);
    return _DartConnection(request, listener).._start();
  }
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
    final Uint8List request;
    try {
      request = handshake.encode();
    } on Object catch (e) {
      // ArgumentError (headers, protocols) or FormatException (user info).
      // Never call the listener synchronously from connect().
      await null;
      return _fail(
        WebSocketHandshakeException('Invalid handshake request', cause: e),
      );
    }
    final host = HandshakeRequest.socketHost(uri);
    final proxy = _request.proxy;
    final tls = _request.tls;
    // Idle subscription left by the proxy tunnel, re-used below (and
    // cancelled in _finish).
    // ignore: cancel_subscriptions
    StreamSubscription<io.RawSocketEvent>? tunnel;
    io.X509Certificate? rejected;
    try {
      final context = handshake.secure ? securityContext(tls) : null;
      // Records the rejected certificate: on Apple platforms dart:io only
      // says "application verification failure", so its dates and names
      // classify the failure (D42).
      bool onBadCertificate(io.X509Certificate certificate) {
        if (tls.allowInsecure) return true;
        rejected = certificate;
        return false;
      }

      final io.ConnectionTask<io.RawSocket> task;
      if (proxy != null) {
        task = await io.RawSocket.startConnect(proxy.host, proxy.port);
      } else if (handshake.secure) {
        task = await io.RawSecureSocket.startConnect(
          host,
          handshake.port,
          context: context,
          onBadCertificate: onBadCertificate,
        );
      } else {
        task = await io.RawSocket.startConnect(host, handshake.port);
      }
      if (_finished) return task.cancel();
      _task = task;
      var socket = await task.socket;
      _task = null;
      if (_finished) return socket.close().ignore();
      _socket = socket;
      socket.setOption(io.SocketOption.tcpNoDelay, true);
      if (_request.keepAlive case final idle?) setKeepAlive(socket, idle);
      if (proxy != null) {
        tunnel = await openTunnel(socket, host, handshake.port, proxy);
        if (_finished) return;
        if (handshake.secure) {
          socket = await io.RawSecureSocket.secure(
            socket,
            subscription: tunnel,
            host: host,
            context: context,
            onBadCertificate: onBadCertificate,
          );
          tunnel = null;
          if (_finished) return socket.close().ignore();
          _socket = socket;
        }
      }
      if (handshake.secure && tls.pins.isNotEmpty) {
        final certificate = (socket as io.RawSecureSocket).peerCertificate;
        if (certificate == null ||
            !matchesPin(certificate.der, pinDigests(tls.pins))) {
          throw const WebSocketConnectionException(
            'TLS: the server certificate matches none of the pins',
            failure: WebSocketConnectionFailure.tls,
            tls: WebSocketTlsFailure.pinMismatch,
          );
        }
      }
    } on Object catch (e) {
      return _fail(
        e is WebSocketException && e is! WebSocketConnectionException
            ? e
            : connectFailure(
                e,
                rejected: rejected,
                tlsHost: host,
                host: proxy == null ? host : proxy.host,
                // Until the tunnel is up, every socket error is the proxy's.
                viaProxy: proxy != null && tunnel == null,
              ),
      );
    }

    final head = BytesBuilder(copy: false);
    var interim = 0;
    void onEvent(io.RawSocketEvent event) {
      if (_finished) return _onLingerEvent(event);
      switch (event) {
        case io.RawSocketEvent.read:
          final data = _socket?.read();
          if (data == null || data.isEmpty) return;
          if (_open) return _reader.add(data);
          head.add(data);
          var bytes = head.takeBytes();
          // Search before checking the limit: the 101 response and the
          // first (large) frame often arrive in one read (found by
          // Autobahn 1.1.6 / 9.x).
          var end = HandshakeResponse.headEnd(bytes);
          // Informational responses (103 Early Hints from a CDN) precede
          // the 101; skip them (docs/review/protocol.md F16).
          while (end >= 0 &&
              end <= _maxHeadSize &&
              interim < 16 &&
              HandshakeResponse.isInterim(
                Uint8List.sublistView(bytes, 0, end),
              )) {
            interim++;
            bytes = Uint8List.sublistView(bytes, end + 4);
            end = HandshakeResponse.headEnd(bytes);
          }
          if (end >= 0 && end <= _maxHeadSize) {
            return _onHandshake(handshake, bytes, end);
          }
          if (end > _maxHeadSize || bytes.length > _maxHeadSize) {
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
          // A close frame we already received (or our protocol failure)
          // wins over the TCP end (protocol.md F7).
          if (_closeAfterFlush) return _finishStored();
          _finish(
            null,
            '',
            _open
                ? const WebSocketConnectionException(
                    'Connection closed without a close frame',
                    failure: WebSocketConnectionFailure.lost,
                  )
                : const WebSocketHandshakeException(
                    'Connection closed during the handshake',
                  ),
          );
      }
    }

    void onError(Object e) => _finished
        ? _endLinger()
        : _closeAfterFlush
        ? _finishStored()
        : _finish(
            null,
            '',
            WebSocketConnectionException(
              'Socket error',
              failure: e is io.TlsException
                  ? WebSocketConnectionFailure.tls
                  : WebSocketConnectionFailure.lost,
              tls: e is io.TlsException ? tlsFailure(e) : null,
              cause: e,
            ),
          );
    void onDone() => _finished
        ? _endLinger()
        : _closeAfterFlush
        ? _finishStored()
        : _finish(null, '', _lost);
    if (tunnel != null) {
      _subscription = tunnel
        ..onData(onEvent)
        ..onError(onError)
        ..onDone(onDone);
    } else {
      _subscription = _socket!.listen(
        onEvent,
        onError: onError,
        onDone: onDone,
        cancelOnError: true,
      );
    }
    _write(request);
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
  void onText(Uint8List payload) => _deliver(_listener.onTextBytes, payload);

  @override
  void onBinary(Uint8List payload) => _deliver(_listener.onBinary, payload);

  /// A throwing listener loses neither the error nor the rest of the
  /// frames read in the same chunk (as on the native transport).
  void _deliver(void Function(Uint8List) to, Uint8List payload) {
    try {
      to(payload);
    } on Object catch (error, stack) {
      Zone.current.handleUncaughtError(error, stack);
    }
  }

  @override
  void onPing(Uint8List payload) {
    if (_closeSent) return;
    // At most one pong waits in the queue: answering only the latest ping
    // is allowed (RFC 6455 §5.5.3), and a ping flood from a server that
    // does not read must not grow memory (protocol.md F6).
    final queued = _queuedPong;
    // Not the head: the head may be partly written already.
    if (queued != null && !identical(_queue.firstOrNull, queued)) {
      if (_queue.remove(queued)) _buffered -= queued.length;
    }
    final pong = _writer.frame(Opcode.pong, payload);
    _queuedPong = _write(pong) ? null : pong;
  }

  /// The pong frame waiting in [_queue], if any.
  Uint8List? _queuedPong;

  @override
  void onPong(Uint8List payload) => _listener.onPong();

  @override
  void onClose(int? code, String reason) {
    _closeCode = code;
    _closeReason = reason;
    if (!_closeSent) {
      _markCloseSent();
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
      _markCloseSent();
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
    if (!_open || _closeSent || _finished) {
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

  /// Writes or queues [bytes]; `true` if they went to the socket at once.
  bool _write(Uint8List bytes) {
    final socket = _socket;
    if (socket == null || _finished) return true;
    if (_queue.isEmpty) {
      final written = socket.write(bytes);
      if (written == bytes.length) return true;
      _queue.add(bytes);
      _queueOffset = written;
      _buffered += bytes.length - written;
      socket.writeEventsEnabled = true;
    } else {
      _queue.add(bytes);
      _buffered += bytes.length;
    }
    return false;
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
      if (identical(_queue.removeFirst(), _queuedPong)) _queuedPong = null;
      _queueOffset = 0;
    }
    if (_drainRequested && !_finished) {
      _drainRequested = false;
      _listener.onDrain();
    }
    if (_closeAfterFlush) {
      _linger(socket);
      _finish(_closeCode, _closeReason, _error);
    }
  }

  // --- close ----------------------------------------------------------------

  @override
  void close(int? code, String? reason) {
    if (_closeSent || _finished) return;
    _markCloseSent();
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

  /// Bounds every closing handshake this transport starts (user close,
  /// echo of a server close, protocol failure): a peer that never reads or
  /// never closes TCP cannot keep the connection (protocol.md F8).
  static const Duration _closeTimeout = Duration(seconds: 5);
  Timer? _closeTimer;

  void _markCloseSent() {
    _closeSent = true;
    _closeTimer ??= Timer(_closeTimeout, () {
      _socket?.close().ignore();
      _finishStored();
    });
  }

  /// Finishes with the close frame received or the failure recorded, or
  /// as a lost connection if there is neither.
  void _finishStored() => _finish(
    _closeCode,
    _closeReason,
    _error ?? (_closeCode == null && !_closeAfterFlush ? _lost : null),
  );

  static const _lost = WebSocketConnectionException(
    'Connection closed',
    failure: WebSocketConnectionFailure.lost,
  );

  void _finish(int? code, String reason, WebSocketException? error) {
    if (_finished) return;
    _finished = true;
    _closeTimer?.cancel();
    _reader.stop();
    _queue.clear();
    _buffered = 0;
    final socket = _socket;
    _socket = null;
    if (_lingering == null) {
      _subscription?.cancel().ignore();
      _subscription = null;
      socket?.close().ignore();
    }
    _listener.onClosed(code, reason, error);
  }

  /// The socket after the closing handshake, until the server closes TCP.
  io.RawSocket? _lingering;
  Timer? _lingerTimer;

  /// Ends the closing handshake as RFC 6455 §7.1.1 asks: the server closes
  /// TCP first. Closing both directions at once while the server is still
  /// sending makes the kernel answer with RST, which can discard our close
  /// frame on the server (docs/review/protocol.md F14). So: FIN, read and
  /// drop whatever still comes, close at the server's FIN or after 1 s.
  /// The listener hears the end at once; only the socket lingers.
  void _linger(io.RawSocket socket) {
    _lingering = socket;
    try {
      socket.shutdown(io.SocketDirection.send);
    } on Object {
      return _endLinger();
    }
    _lingerTimer = Timer(const Duration(seconds: 1), _endLinger);
  }

  void _endLinger() {
    final socket = _lingering;
    if (socket == null) return;
    _lingering = null;
    _lingerTimer?.cancel();
    _subscription?.cancel().ignore();
    _subscription = null;
    socket.close().ignore();
  }

  /// Socket events after [_finish]: only a lingering socket gets them.
  void _onLingerEvent(io.RawSocketEvent event) {
    final socket = _lingering;
    if (socket == null) return;
    switch (event) {
      case io.RawSocketEvent.read:
        socket.read();
      case io.RawSocketEvent.readClosed:
      case io.RawSocketEvent.closed:
        _endLinger();
      case io.RawSocketEvent.write:
    }
  }
}
