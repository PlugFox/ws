import 'dart:async';
import 'dart:developer';

import 'package:meta/meta.dart';
import 'package:ws/src/client/ws_client.i.dart';
import 'package:ws/src/model/state.dart';
import 'package:ws/src/model/status_codes.dart';
import 'package:ws/src/model/websocket_exception.dart';
import 'package:ws/src/platform/platform.dart';
import 'package:ws/src/util/constants.dart';

abstract base class _WebSocketClientBase implements IWebSocketClient {
  _WebSocketClientBase() : _controller = StreamController<Object>.broadcast() {
    _transport = $getWebSocketTransport(
      onReceived: _$onReceived,
      onSent: _$onSent,
      onError: _$onError,
      onConnected: _$onConnected,
      onDisconnected: _$onDisconnected,
    );
  }

  /// Output stream of data from native WebSocket client.
  /// {@nodoc}
  final StreamController<Object> _controller;

  /// {@nodoc}
  late final IWebSocketPlatformTransport _transport;

  /// Last URL used to connect.
  /// {@nodoc}
  String? _lastUrl;

  @override
  @nonVirtual
  Stream<Object> get stream => _controller.stream;

  @override
  @mustCallSuper
  FutureOr<void> add(Object data) => _transport.add(data);

  @override
  @mustCallSuper
  Future<void> connect(String url) {
    _lastUrl = url;
    return _transport.connect(url);
  }

  @override
  @mustCallSuper
  void disconnect([int? code = 1000, String? reason = 'NORMAL_CLOSURE']) {
    _setState(
      (_) => WebSocketClientState.closing(
        closeCode: code,
        closeReason: reason,
      ),
    );
    _transport.disconnect(code, reason);
  }

  @override
  @mustCallSuper
  void close([int? code = 1000, String? reason = 'NORMAL_CLOSURE']) {
    _transport.close(code, reason);
    _controller.close().ignore();
  }

  /// On message received from native WebSocket client.
  /// {@nodoc}
  @nonVirtual
  void _$onReceived(Object data) {
    if (_controller.isClosed) {
      assert(false, 'Cannot receive data to a closed stream controller.');
    } else {
      _controller.add(data);
    }
  }

  /// On message sent.
  /// {@nodoc}
  @nonVirtual
  void _$onSent(Object data) {}

  /// Receive error from native WebSocket client.
  /// {@nodoc}
  @nonVirtual
  @pragma('vm:invisible')
  void _$onError(Object error, StackTrace stackTrace) {
    debugger(when: $kDebugWS);
    if (_controller.isClosed) {
      assert(false, 'Cannot receive error to a closed stream controller.');
    } else {
      switch (error) {
        case WSException e:
          error = e;
        case String e:
          error = WSUnknownException(e);
        case StateError e:
          debugger(when: $kDebugWS);
          error = WSUnknownException(e.message);
        case UnsupportedError e:
          debugger(when: $kDebugWS);
          error = WSUnsupportedException(e.message ?? 'Unsupported exception.');
        case Exception e:
          debugger(when: $kDebugWS);
          error = WSUnknownException(e.toString());
        case Error e:
          debugger(when: $kDebugWS);
          error = WSUnknownException(e.toString());
        case Object:
          debugger(when: $kDebugWS);
          error = WSUnknownException(error.toString());
      }
      _controller.addError(error, stackTrace);
    }
  }

  /// On connection established.
  /// {@nodoc}
  @nonVirtual
  void _$onConnected(String url) {
    assert(url.isNotEmpty, 'URL cannot be empty.');
    assert(!_controller.isClosed, 'Controller already closed.');
    _setState((_) => WebSocketClientState.open(url: url));
  }

  /// On connection closed.
  /// {@nodoc}
  @nonVirtual
  void _$onDisconnected(int? code, String? reason) {
    _setState(
      (_) => WebSocketClientState.closed(
        closeCode: code,
        closeReason: reason,
      ),
    );
  }

  /// Set client state.
  /// {@nodoc}
  void _setState(WebSocketClientState Function(WebSocketClientState state) fn);
}

/// {@template ws_client}
/// WebSocket client.
/// {@endtemplate}
/// {@category Client}
final class WebSocketClient extends _WebSocketClientBase
    with _WebSocketClientStateController, _WebSocketClientHealthCheck {
  /// {@macro ws_client}
  WebSocketClient();

  /// {@macro ws_client}
  factory WebSocketClient.connect(String url) =>
      WebSocketClient()..connect(url);

  @override
  Future<void> connect(String url) {
    _$enableReconnection();
    return super.connect(url);
  }

  @override
  @mustCallSuper
  void disconnect([int? code = 1000, String? reason = 'NORMAL_CLOSURE']) {
    _$disableReconnection();
    super.disconnect(code, reason);
  }
}

base mixin _WebSocketClientStateController on _WebSocketClientBase {
  final StreamController<WebSocketClientState> _stateController =
      StreamController<WebSocketClientState>.broadcast();

  WebSocketClientState _state = WebSocketClientState.closed(
    closeCode: WebSocketStatusCodes.normalClosure.code,
    closeReason: 'INITIAL_CLOSED_STATE',
  );

  @override
  WebSocketClientState get state => _state;

  @override
  late final Stream<WebSocketClientState> stateChanges =
      _stateController.stream;

  @override
  void _setState(WebSocketClientState Function(WebSocketClientState state) fn) {
    if (_stateController.isClosed) {
      assert(false, 'Cannot receive data to a closed stream controller.');
      return;
    }
    _stateController.add(_state = fn(state));
  }

  @override
  void close([int? code = 1000, String? reason = 'NORMAL_CLOSURE']) {
    super.close(code, reason);
    _stateController.close();
  }
}

base mixin _WebSocketClientHealthCheck on _WebSocketClientBase {
  /// Reconnection timer.
  /// {@nodoc}
  Timer? _reconnectionTimer;

  /// Enable reconnection after manual/interactive connection.
  /// {@nodoc}
  void _$enableReconnection() {
    _reconnectionTimer = Timer.periodic(
      const Duration(seconds: 15),
      (timer) {
        if (!_transport.readyState.isClosed) {
          return;
        } else if (_lastUrl case String url) {
          connect(url).ignore();
        }
      },
    );
  }

  /// Disable reconnection after manual/interactive disconnection or close.
  /// {@nodoc}
  void _$disableReconnection() {
    _reconnectionTimer?.cancel();
    _reconnectionTimer = null;
  }
}
