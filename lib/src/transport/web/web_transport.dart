import 'dart:async';
import 'dart:convert' show utf8;
import 'dart:js_interop';
import 'dart:typed_data';

import 'package:web/web.dart' as web;
import 'package:ws/src/client/exceptions.dart';
import 'package:ws/src/protocol/frame.dart' show CloseCode;
import 'package:ws/src/protocol/frame_writer.dart';
import 'package:ws/src/transport/transport.dart';

/// The platform transport in the browser.
WebSocketTransport createPlatformTransport() => const WebTransport();

/// The browser's `WebSocket` (dart2js and dart2wasm).
///
/// Browsers do not expose handshake headers, the HTTP status of a failed
/// handshake, or ping/pong. Custom headers are ignored; heartbeats need an
/// application-level ping (DECISIONS.md D15).
final class WebTransport implements WebSocketTransport {
  /// Creates the transport.
  const WebTransport();

  @override
  String get name => 'web';

  @override
  bool get supportsPing => false;

  @override
  bool get decodesJson => false;

  @override
  TransportConnection connect(
    TransportRequest request,
    TransportListener listener,
  ) => _WebConnection(request, listener);
}

final class _WebConnection implements TransportConnection {
  _WebConnection(TransportRequest request, this._listener)
    : _maxMessageSize = request.maxMessageSize {
    try {
      _socket = web.WebSocket(
        request.url.toString(),
        <JSString>[for (final p in request.protocols) p.toJS].toJS,
      )..binaryType = 'arraybuffer';
    } on Object catch (e) {
      // Invalid URL or blocked port: the constructor throws synchronously.
      scheduleMicrotask(
        () => _finish(
          null,
          '',
          WebSocketConnectionException('Could not connect', cause: e),
        ),
      );
      return;
    }
    final socket = _socket!;
    _subscriptions = <StreamSubscription<web.Event>>[
      web.EventStreamProviders.openEvent.forTarget(socket).listen((_) {
        _open = true;
        final protocol = socket.protocol;
        _listener.onOpen(protocol.isEmpty ? null : protocol);
      }),
      web.EventStreamProviders.messageEvent
          .forTarget(socket)
          .listen(_onMessage),
      web.EventStreamProviders.closeEvent
          .forTarget(socket)
          .listen((close) => _onClose(close.code, close.reason)),
    ];
  }

  final TransportListener _listener;
  final int _maxMessageSize;
  web.WebSocket? _socket;
  List<StreamSubscription<web.Event>> _subscriptions =
      const <StreamSubscription<web.Event>>[];
  bool _open = false;
  bool _finished = false;
  Timer? _drainTimer;

  void _onClose(int code, String reason) {
    switch (code) {
      // 1005 (no code), 1006 (no close frame) and 1015 (TLS) never appear
      // on the wire: no close code, like the other transports.
      case 1005:
        _finish(null, reason, null);
      case 1006 || 1015:
        _finish(
          null,
          '',
          _open
              ? const WebSocketConnectionException(
                  'Connection lost',
                  failure: WebSocketConnectionFailure.lost,
                )
              : const WebSocketConnectionException(
                  'Could not connect (the browser hides the reason)',
                ),
        );
      // Chrome passes invalid codes (999, 1004, 2000...) through; the
      // other transports fail the connection with 1002 (RFC 6455 §7.4).
      case _ when !CloseCode.isValidOnWire(code):
        _finish(
          CloseCode.protocolError,
          'Invalid close code',
          WebSocketClosedException(
            'Protocol violation by the server: invalid close code $code',
            code: CloseCode.protocolError,
            reason: 'Invalid close code',
            byServer: false,
          ),
        );
      default:
        _finish(code, reason, null);
    }
  }

  void _onMessage(web.MessageEvent event) {
    if (_finished) return;
    final data = event.data;
    if (data.isA<JSString>()) {
      final text = (data as JSString).toDart;
      if (_utf8Length(text, _maxMessageSize) > _maxMessageSize) {
        return _tooBig();
      }
      _listener.onTextString(text);
    } else if (data.isA<JSArrayBuffer>()) {
      final buffer = (data as JSArrayBuffer).toDart;
      if (buffer.lengthInBytes > _maxMessageSize) return _tooBig();
      _listener.onBinary(buffer.asUint8List());
    }
  }

  /// The browser has no message limit; enforce it like the other
  /// transports do (1009, never delivered), after the fact.
  void _tooBig() {
    const reason = 'Message too big';
    try {
      _socket?.close(CloseCode.messageTooBig, reason);
    } on Object {
      // Already closing.
    }
    _finish(
      CloseCode.messageTooBig,
      reason,
      const WebSocketClosedException(
        'Protocol violation by the server: $reason',
        code: CloseCode.messageTooBig,
        reason: reason,
        byServer: false,
      ),
    );
  }

  @override
  int get bufferedAmount => _socket?.bufferedAmount ?? 0;

  void _checkOpen() {
    if (!_open || _finished || _socket?.readyState != web.WebSocket.OPEN) {
      throw const WebSocketNotConnectedException('The connection is closing');
    }
  }

  @override
  void sendText(String text) {
    _checkOpen();
    _socket!.send(text.toJS);
  }

  @override
  void sendTextBytes(Uint8List bytes) {
    _checkOpen();
    // The browser needs a string for a text message.
    _socket!.send(utf8.decode(bytes).toJS);
  }

  @override
  void sendBinary(Uint8List bytes) {
    _checkOpen();
    _socket!.send(bytes.toJS);
  }

  /// Browsers have no drain event; poll `bufferedAmount` until it is zero.
  @override
  void requestDrain() {
    if (_drainTimer != null) return;
    _drainTimer = Timer.periodic(const Duration(milliseconds: 16), (t) {
      if (_finished) return t.cancel();
      if (bufferedAmount == 0) {
        t.cancel();
        _drainTimer = null;
        _listener.onDrain();
      }
    });
  }

  @override
  void ping() => throw const WebSocketUnsupportedException(
    'Browsers do not allow sending pings',
  );

  @override
  void close(int? code, String? reason) {
    final socket = _socket;
    if (socket == null || _finished) return;
    if (code == null) {
      socket.close();
    } else {
      // The browser throws for a longer reason; cut it like the other
      // transports do.
      final bytes = FrameWriter.closeReason(reason ?? '');
      socket.close(code, utf8.decode(bytes));
    }
  }

  @override
  void abort() {
    if (_finished) return;
    // The browser cannot drop a socket without a closing handshake; report
    // the end now and ignore whatever the socket does later.
    try {
      _socket?.close();
    } on Object {
      // Already closing.
    }
    scheduleMicrotask(
      () => _finish(
        null,
        '',
        const WebSocketConnectionException('Connection aborted'),
      ),
    );
  }

  void _finish(int? code, String reason, WebSocketException? error) {
    if (_finished) return;
    _finished = true;
    _drainTimer?.cancel();
    for (final s in _subscriptions) {
      s.cancel().ignore();
    }
    _listener.onClosed(code, reason, error);
  }
}

/// UTF-8 length of [text], or any value above [limit] once it is exceeded.
/// UTF-16 code units never encode to fewer bytes, nor to more than three.
int _utf8Length(String text, int limit) {
  if (text.length > limit) return text.length;
  if (text.length * 3 <= limit) return text.length;
  var bytes = 0;
  for (var i = 0; i < text.length && bytes <= limit; i++) {
    final unit = text.codeUnitAt(i);
    // A surrogate pair is 4 bytes: 2 + 2.
    bytes += unit < 0x80
        ? 1
        : unit < 0x800 || unit & 0xF800 == 0xD800
        ? 2
        : 3;
  }
  return bytes;
}
