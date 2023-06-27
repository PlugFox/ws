import 'dart:async';

import 'package:meta/meta.dart';
import 'package:ws/interface.dart';

/// {@nodoc}
@internal
abstract base class WebSocketClientBase implements IWebSocketClient {
  /// {@nodoc}
  WebSocketClientBase({this.reconnectTimeout = const Duration(seconds: 5)})
      : _dataController = StreamController<Object>.broadcast(),
        _stateController = StreamController<WebSocketClientState>.broadcast(),
        _state = WebSocketClientState.initial;

  /// Delay between reconnection attempts.
  /// {@nodoc}
  @protected
  final Duration reconnectTimeout;

  String? _lastUrl;

  /// Last URL used to connect.
  /// {@nodoc}
  String? get lastUrl => _lastUrl;

  /// Output stream of data from native WebSocket client.
  /// {@nodoc}
  @protected
  final StreamController<Object> _dataController;

  @override
  late final Stream<Object> stream = _dataController.stream;

  /// Current ready state of the WebSocket connection.
  /// {@nodoc}
  abstract final WebSocketReadyState readyState;

  @override
  WebSocketClientState get state => _state;
  WebSocketClientState _state;

  /// Output stream of state changes.
  /// {@nodoc}
  @protected
  final StreamController<WebSocketClientState> _stateController;

  @override
  late final Stream<WebSocketClientState> stateChanges =
      _stateController.stream;

  @override
  @mustCallSuper
  FutureOr<void> connect(String url) async {
    _lastUrl = url;
    setState((_) => WebSocketClientState.connecting(url: url));
  }

  @override
  @mustCallSuper
  FutureOr<void> disconnect(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) async {
    setState((_) => WebSocketClientState.disconnecting(
          closeCode: code,
          closeReason: reason,
        ));
  }

  /// {@nodoc}
  @protected
  void setState(
          WebSocketClientState Function(WebSocketClientState state) change) =>
      _stateController.add(_state = change(_state));

  /// {@nodoc}
  @protected
  void onConnected(String url) {
    _lastUrl = url;
    setState((_) => WebSocketClientState.open(url: url));
  }

  /// {@nodoc}
  @protected
  void onSent(Object data) {}

  /// On data received callback.
  /// {@nodoc}
  @protected
  void onReceivedData(Object? data) {
    if (data == null) return;
    _dataController.add(data);
  }

  /// {@nodoc}
  @protected
  void onDisconnected(int? code, String? reason) {
    setState((_) => WebSocketClientState.closed(
          closeCode: code,
          closeReason: reason,
        ));
  }

  /// Error callback.
  /// {@nodoc}
  @protected
  void onError(Object error, StackTrace stackTrace) {}
}
