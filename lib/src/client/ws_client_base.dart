import 'dart:async';
import 'dart:collection';

import 'package:meta/meta.dart';
import 'package:ws/src/client/message_stream.dart';
import 'package:ws/src/client/state.dart';
import 'package:ws/src/client/state_stream.dart';
import 'package:ws/src/client/web_socket_ready_state.dart';
import 'package:ws/src/client/ws_client_interface.dart';
import 'package:ws/src/util/constants.dart';
import 'package:ws/src/util/logger.dart';

/// {@nodoc}
@internal
abstract base class WebSocketClientBase implements IWebSocketClient {
  /// {@nodoc}
  WebSocketClientBase({Iterable<String>? protocols})
      : _dataController = StreamController<Object>.broadcast(),
        _stateController = StreamController<WebSocketClientState>.broadcast(),
        _state = WebSocketClientState.initial(),
        protocols = protocols != null
            ? UnmodifiableListView(protocols.toList(growable: false))
            : null;

  @override
  bool get isClosed => _isClosed; // coverage:ignore-line
  bool _isClosed = false;

  /// {@nodoc}
  @protected
  final List<String>? protocols;

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
  late final WebSocketStatesStream stateChanges =
      WebSocketStatesStream(_stateController.stream);

  @override
  @mustCallSuper
  FutureOr<void> connect(String url) async {
    setState((_) => WebSocketClientState.connecting(url: url));
  }

  @override
  @mustCallSuper
  FutureOr<void> add(Object data) async {
    // coverage:ignore-start
    if ($debugWS) {
      var text = data.toString();
      text = text.length > 100 ? '${text.substring(0, 97)}...' : text;
      fine('> $text');
    }
    // coverage:ignore-end
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
    } on Object {/* ignore */} // coverage:ignore-line

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
    setState((_) => WebSocketClientState.open(url: url));
  }

  /// {@nodoc}
  @protected
  void onSent(Object data) {
    // coverage:ignore-start
    if ($debugWS) {
      var text = data.toString();
      text = text.length > 100 ? '${text.substring(0, 97)}...' : text;
      fine('Sent: $text');
    }
    // coverage:ignore-end
  }

  /// On data received callback.
  /// {@nodoc}
  @protected
  void onReceivedData(Object? data) {
    // coverage:ignore-start
    if (data == null || _dataController.isClosed) return;
    // coverage:ignore-end
    _dataController.add(data);
    // coverage:ignore-start
    if ($debugWS) {
      var text = data.toString();
      text = text.length > 100 ? '${text.substring(0, 97)}...' : text;
      fine('< $text');
    }
    // coverage:ignore-end
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
