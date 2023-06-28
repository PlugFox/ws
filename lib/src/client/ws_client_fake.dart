import 'dart:async';

import 'package:meta/meta.dart';
import 'package:ws/src/client/message_stream.dart';
import 'package:ws/src/client/state.dart';
import 'package:ws/src/client/ws_client_interface.dart';

/// {@nodoc}
@internal
IWebSocketClient $platformWebSocketClient(Duration reconnectTimeout) =>
    WebSocketClient$Fake(reconnectTimeout: reconnectTimeout);

/// {@nodoc}
final class WebSocketClient$Fake implements IWebSocketClient {
  /// {@nodoc}
  WebSocketClient$Fake({this.reconnectTimeout = const Duration(seconds: 5)});

  @override
  final Duration reconnectTimeout;

  @override
  bool get isClosed => true;

  @override
  WebSocketClientState get state => WebSocketClientState.initial();

  @override
  Stream<WebSocketClientState> get stateChanges => throw UnimplementedError();

  @override
  WebSocketMessagesStream get stream =>
      WebSocketMessagesStream(const Stream<Object>.empty());

  @override
  FutureOr<void> add(Object data) {}

  @override
  FutureOr<void> connect(String url) {}

  @override
  FutureOr<void> disconnect(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) {}

  @override
  FutureOr<void> close([int? code = 1000, String? reason = 'NORMAL_CLOSURE']) {}
}
