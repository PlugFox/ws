import 'dart:async';

import 'package:ws/src/client/state.dart';
import 'package:ws/src/client/ws_client_fake.dart'
    // ignore: uri_does_not_exist
    if (dart.library.html) 'package:ws/src/client/ws_client_js.dart'
    // ignore: uri_does_not_exist
    if (dart.library.io) 'package:ws/src/client/ws_client_io.dart';
import 'package:ws/src/client/ws_client_interface.dart';
import 'package:ws/src/util/event_queue.dart';

/// {@template ws_client}
/// WebSocket client.
/// With concurrency protection and reconnecting.
/// Supports both web and io platforms.
/// {@endtemplate}
/// {@category Client}
final class WebSocketClient implements IWebSocketClient {
  /// {@macro ws_client}
  WebSocketClient({Duration reconnectTimeout = const Duration(seconds: 5)})
      : _client = $platformWebSocketClient(reconnectTimeout);

  /// {@macro ws_client}
  factory WebSocketClient.connect(String url) =>
      WebSocketClient()..connect(url).ignore();

  final IWebSocketClient _client;
  final WebSocketEventQueue _eventQueue = WebSocketEventQueue();

  @override
  Stream<Object> get stream => _client.stream;

  @override
  Stream<WebSocketClientState> get stateChanges => _client.stateChanges;

  @override
  WebSocketClientState get state => _client.state;

  @override
  FutureOr<void> add(Object data) => _client.add(data);

  @override
  Future<void> connect(String url) =>
      _eventQueue.push('Connect', () => _client.connect(url));

  @override
  void disconnect([int? code = 1000, String? reason = 'NORMAL_CLOSURE']) =>
      _eventQueue.push('Disconnect', () => _client.disconnect(code, reason));

  @override
  void close([int? code = 1000, String? reason = 'NORMAL_CLOSURE']) =>
      _eventQueue.push('Close', () => _client.close(code, reason));
}
