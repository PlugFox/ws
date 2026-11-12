import 'dart:async';
import 'dart:convert' show utf8;
import 'dart:js_interop';
import 'dart:typed_data';

import 'package:web/web.dart' as web;
import 'package:ws/src/client/exceptions.dart';
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
  _WebConnection(TransportRequest request, this._listener) {
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
      web.EventStreamProviders.closeEvent.forTarget(socket).listen((close) {
        _finish(
          close.code == 1005 ? null : close.code,
          close.reason,
          close.code == 1006
              ? WebSocketConnectionException(
                  _open
                      ? 'Connection lost'
                      : 'Could not connect (the browser hides the reason)',
                )
              : null,
        );
      }),
    ];
  }

  final TransportListener _listener;
  web.WebSocket? _socket;
  List<StreamSubscription<web.Event>> _subscriptions =
      const <StreamSubscription<web.Event>>[];
  bool _open = false;
  bool _finished = false;
  Timer? _drainTimer;

  void _onMessage(web.MessageEvent event) {
    final data = event.data;
    if (data.isA<JSString>()) {
      _listener.onTextString((data as JSString).toDart);
    } else if (data.isA<JSArrayBuffer>()) {
      _listener.onBinary((data as JSArrayBuffer).toDart.asUint8List());
    }
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
      socket.close(code, reason ?? '');
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
