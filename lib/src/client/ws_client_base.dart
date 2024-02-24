// ignore_for_file: avoid_types_on_closure_parameters, omit_local_variable_types

import 'dart:async';
import 'dart:collection';

import 'package:meta/meta.dart';
import 'package:ws/src/client/message_stream.dart';
import 'package:ws/src/client/state.dart';
import 'package:ws/src/client/state_stream.dart';
import 'package:ws/src/client/web_socket_ready_state.dart';
import 'package:ws/src/client/ws_client_interface.dart';
import 'package:ws/src/client/ws_interceptor.dart';
import 'package:ws/src/util/constants.dart';
import 'package:ws/src/util/logger.dart';

/// {@nodoc}
@internal
abstract base class WebSocketClientBase implements IWebSocketClient {
  /// {@nodoc}
  WebSocketClientBase({
    Iterable<String>? protocols,
    Iterable<WSInterceptor>? interceptors,
  })  : _dataController = StreamController<Object>.broadcast(),
        _stateController = StreamController<WebSocketClientState>.broadcast(),
        _state = WebSocketClientState.initial(),
        protocols = protocols != null
            ? UnmodifiableListView<String>(protocols.toList(growable: false))
            : null {
    final chain =
        interceptors?.toList(growable: false) ?? const <WSInterceptor>[];
    _buildSendChain(chain);
    _buildReceiveChain(chain);
  }

  @override
  bool get isClosed => _isClosed; // coverage:ignore-line
  bool _isClosed = false;

  /// {@nodoc}
  @protected
  final List<String>? protocols;

  /// On message sent callback interceptors chain.
  late final void Function(Object data) _onSentChain;

  /// On message received callback interceptors chain.
  late final void Function(Object data) _onReceivedDataChain;

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

  @protected
  @visibleForOverriding
  void push(Object data);

  @override
  @nonVirtual
  FutureOr<void> add(Object data) {
    _onSentChain(data);
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

  /// On data received callback.
  /// {@nodoc}
  @protected
  void onReceivedData(Object? data) {
    if (data == null) return;
    _onReceivedDataChain(data);
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

  /// Build push interceptors
  void _buildSendChain(List<WSInterceptor> interceptors) {
    void Function(Object) fn = (Object data) {
      push(data);
      // coverage:ignore-start
      if ($debugWS) {
        var text = data.toString();
        text = text.length > 100 ? '${text.substring(0, 97)}...' : text;
        fine('> $text');
      }
      // coverage:ignore-end
    };
    for (var i = interceptors.length - 1; i >= 0; i--) {
      final interceptor = interceptors[i];
      fn = (data) => interceptor.onSend(data, fn);
    }
    _onSentChain = fn;
  }

  /// Build receive interceptors
  void _buildReceiveChain(List<WSInterceptor> interceptors) {
    void Function(Object) fn = (Object data) {
      if (_dataController.isClosed) return;
      _dataController.add(data);
      // coverage:ignore-start
      if ($debugWS) {
        var text = data.toString();
        text = text.length > 100 ? '${text.substring(0, 97)}...' : text;
        fine('< $text');
      }
      // coverage:ignore-end
    };
    for (var i = interceptors.length - 1; i >= 0; i--) {
      final interceptor = interceptors[i];
      fn = (data) => interceptor.onMessage(data, fn);
    }
    _onReceivedDataChain = fn;
  }
}
