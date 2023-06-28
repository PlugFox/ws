import 'dart:async';

import 'package:meta/meta.dart';
import 'package:ws/interface.dart';
import 'package:ws/src/util/constants.dart';
import 'package:ws/src/util/logger.dart';

/// {@nodoc}
@internal
abstract base class WebSocketClientBase implements IWebSocketClient {
  /// {@nodoc}
  WebSocketClientBase({this.reconnectTimeout = const Duration(seconds: 5)})
      : _dataController = StreamController<Object>.broadcast(),
        _stateController = StreamController<WebSocketClientState>.broadcast(),
        _state = WebSocketClientState.initial();

  @override
  bool get isClosed => _isClosed;
  bool _isClosed = false;

  @override
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
  late final WebSocketMessagesStream stream =
      WebSocketMessagesStream(_dataController.stream);

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
  FutureOr<void> add(Object data) async {
    if ($kDebugWS) {
      var text = data.toString();
      text = text.length > 100 ? '${text.substring(0, 97)}...' : text;
      fine('> $text');
    }
  }

  @override
  @mustCallSuper
  FutureOr<void> disconnect(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) async {
    if (state.readyState.isClosed) return;
    setState((_) => WebSocketClientState.disconnecting(
          closeCode: code,
          closeReason: reason,
        ));
  }

  @override
  @mustCallSuper
  FutureOr<void> close(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) async {
    _isClosed = true;
    try {
      await disconnect(code, reason);
    } on Object {
      /* ignore */
    }
    _dataController.close().ignore();
    _stateController.close().ignore();
  }

  /// {@nodoc}
  @protected
  void setState(
      WebSocketClientState Function(WebSocketClientState state) change) {
    final newState = change(_state);
    if (newState == _state || _stateController.isClosed) return;
    _stateController.add(_state = newState);
    info('WebSocketClient state changed to $newState');
  }

  /// {@nodoc}
  @protected
  void onConnected(String url) {
    _lastUrl = url;
    setState((_) => WebSocketClientState.open(url: url));
  }

  /// {@nodoc}
  @protected
  void onSent(Object data) {
    if ($kDebugWS) {
      var text = data.toString();
      text = text.length > 100 ? '${text.substring(0, 97)}...' : text;
      fine('Sent: $text');
    }
  }

  /// On data received callback.
  /// {@nodoc}
  @protected
  void onReceivedData(Object? data) {
    if (data == null || _dataController.isClosed) return;
    _dataController.add(data);
    if ($kDebugWS) {
      var text = data.toString();
      text = text.length > 100 ? '${text.substring(0, 97)}...' : text;
      fine('< $text');
    }
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
