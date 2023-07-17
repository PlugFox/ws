import 'dart:async';

import 'package:meta/meta.dart';
import 'package:ws/src/client/message_stream.dart';
import 'package:ws/src/client/state.dart';
import 'package:ws/src/client/state_stream.dart';
import 'package:ws/src/client/ws_client_interface.dart';
import 'package:ws/src/client/ws_options.dart';

// coverage:ignore-start
/// {@nodoc}
@internal
IWebSocketClient $platformWebSocketClient(WebSocketOptions? options) =>
    WebSocketClientFake(protocols: options?.protocols);
// coverage:ignore-end

/// {@template ws_client_fake}
/// Fake WebSocket client for testing purposes.
/// {@endtemplate}
/// {@category Testing}
@visibleForTesting
final class WebSocketClientFake implements IWebSocketClient {
  /// {@macro ws_client_fake}
  @visibleForTesting
  WebSocketClientFake({Iterable<String>? protocols})
      : protocols = protocols?.toList(),
        _controller = StreamController<Object>.broadcast(),
        _stateController = StreamController<WebSocketClientState>.broadcast(),
        _isClosed = false,
        _state = WebSocketClientState.initial();

  /// {@nodoc}
  @visibleForTesting
  final List<String>? protocols;

  @override
  @visibleForTesting
  bool get isClosed => _isClosed;
  bool _isClosed;

  @override
  @visibleForTesting
  WebSocketClientState get state => _state;
  WebSocketClientState _state;

  final StreamController<WebSocketClientState> _stateController;

  @override
  @visibleForTesting
  late final WebSocketStatesStream stateChanges =
      WebSocketStatesStream(_stateController.stream);

  final StreamController<Object> _controller;

  @override
  @visibleForTesting
  WebSocketMessagesStream get stream =>
      WebSocketMessagesStream(_controller.stream);

  @override
  @visibleForTesting
  FutureOr<void> add(Object data) =>
      Future<void>.delayed(const Duration(milliseconds: 25));

  /// Emulate receiving data from the server.
  @visibleForTesting
  void loopBack(Object data) => _controller.add(data);

  @override
  @visibleForTesting
  FutureOr<void> connect(String url) async {
    _stateController.add(_state = WebSocketClientState.connecting(url: url));
    await Future<void>.delayed(const Duration(milliseconds: 25));
    _stateController.add(_state = WebSocketClientState.open(url: url));
  }

  @override
  @visibleForTesting
  FutureOr<void> disconnect(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) async {
    _stateController.add(_state = WebSocketClientState.disconnecting(
        closeCode: code, closeReason: reason));
    await Future<void>.delayed(const Duration(milliseconds: 25));
    _stateController.add(_state =
        WebSocketClientState.closed(closeCode: code, closeReason: reason));
  }

  @override
  @visibleForTesting
  FutureOr<void> close(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) async {
    _isClosed = true;
    await disconnect(code, reason);
    await Future<void>.delayed(Duration.zero);
    await _stateController.close();
    await _controller.close();
  }
}
