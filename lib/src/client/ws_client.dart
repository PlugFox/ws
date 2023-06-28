import 'dart:async';

import 'package:ws/src/client/ws_client_fake.dart'
    // ignore: uri_does_not_exist
    if (dart.library.html) 'package:ws/src/client/ws_client_js.dart'
    // ignore: uri_does_not_exist
    if (dart.library.io) 'package:ws/src/client/ws_client_io.dart';
import 'package:ws/src/client/ws_client_interface.dart';
import 'package:ws/src/connection_manager/connection_manager.dart';
import 'package:ws/src/util/event_queue.dart';
import 'package:ws/ws.dart';

/// {@template ws_client}
/// WebSocket client.
/// With concurrency protection and reconnecting.
/// Supports both web and io platforms.
/// {@endtemplate}
/// {@category Client}
final class WebSocketClient implements IWebSocketClient {
  /// {@macro ws_client}
  WebSocketClient(
      {Duration reconnectTimeout = const Duration(seconds: 5),
      Iterable<String>? protocols})
      : reconnectTimeout = reconnectTimeout.abs(),
        _client = $platformWebSocketClient(reconnectTimeout.abs(), protocols);

  /// Creates a [WebSocketClient] from an existing [IWebSocketClient].
  /// This is useful for testing or if you want to use a custom implementation
  /// with reconnecting and concurrency protection.
  /// {@macro ws_client}
  WebSocketClient.fromClient(IWebSocketClient client,
      {Duration reconnectTimeout = const Duration(seconds: 5)})
      : reconnectTimeout = reconnectTimeout.abs(),
        _client = client;

  /// {@macro ws_client}
  factory WebSocketClient.connect(String url,
          {Duration reconnectTimeout = const Duration(seconds: 5),
          Iterable<String>? protocols}) =>
      WebSocketClient(reconnectTimeout: reconnectTimeout, protocols: protocols)
        ..connect(url).ignore();

  final IWebSocketClient _client;
  final WebSocketEventQueue _eventQueue = WebSocketEventQueue();

  @override
  bool get isClosed => _isClosed;
  bool _isClosed = false;

  @override
  final Duration reconnectTimeout;

  @override
  WebSocketMessagesStream get stream => _client.stream;

  @override
  Stream<WebSocketClientState> get stateChanges => _client.stateChanges;

  @override
  WebSocketClientState get state => _client.state;

  @override
  Future<void> add(Object data) {
    if (_isClosed) return Future<void>.error(const WSClientClosed());
    return _eventQueue.push('add', () => _client.add(data));
  }

  @override
  Future<void> connect(String url) {
    if (_isClosed) return Future<void>.error(const WSClientClosed());
    return _eventQueue.push('connect', () {
      WebSocketConnectionManager.instance
          .startMonitoringConnection(_client, url);
      return _client.connect(url);
    });
  }

  @override
  Future<void> disconnect(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) {
    if (_isClosed) return Future<void>.error(const WSClientClosed());
    return _eventQueue.push('disconnect', () {
      WebSocketConnectionManager.instance.stopMonitoringConnection(_client);
      return _client.disconnect(code, reason);
    });
  }

  @override
  FutureOr<void> close(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) async {
    _isClosed = true;
    // Stop monitoring the connection.
    WebSocketConnectionManager.instance.stopMonitoringConnection(_client);
    // Clear the event queue and prevent new events from being processed.
    // Returns when the queue is empty and no new events are being processed.
    Future<void>.sync(_eventQueue.close).ignore();
    // Close the internal client connection and free resources.
    await _client.close(code, reason);
  }
}
