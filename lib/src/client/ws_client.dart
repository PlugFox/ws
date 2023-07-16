import 'dart:async';

import 'package:ws/src/client/message_stream.dart';
import 'package:ws/src/client/metrics.dart';
import 'package:ws/src/client/state.dart';
import 'package:ws/src/client/websocket_exception.dart';
import 'package:ws/src/client/ws_client_fake.dart'
    // ignore: uri_does_not_exist
    if (dart.library.html) 'package:ws/src/client/ws_client_js.dart'
    // ignore: uri_does_not_exist
    if (dart.library.io) 'package:ws/src/client/ws_client_vm.dart';
import 'package:ws/src/client/ws_client_interface.dart';
import 'package:ws/src/client/ws_options.dart';
import 'package:ws/src/manager/connection_manager.dart';
import 'package:ws/src/manager/metrics_manager.dart';
import 'package:ws/src/util/event_queue.dart';

/// {@template ws_client}
/// WebSocket client.
/// With concurrency protection and reconnecting.
/// Supports both web and io platforms.
/// {@endtemplate}
/// {@category Client}
final class WebSocketClient implements IWebSocketClient {
  /// {@macro ws_client}
  WebSocketClient([WebSocketOptions? options])
      : _client = $platformWebSocketClient(options),
        _options = options {
    WebSocketMetricsManager.instance.startObserving(this);
  }

  /// Creates a [WebSocketClient] from an existing [IWebSocketClient].
  /// This is useful for testing or if you want to use a custom implementation
  /// with reconnecting and concurrency protection.
  /// {@macro ws_client}
  WebSocketClient.fromClient(IWebSocketClient client,
      [WebSocketOptions? options])
      : _client = client,
        _options = options {
    WebSocketMetricsManager.instance.startObserving(this);
  }

  /// {@macro ws_client}
  factory WebSocketClient.connect(String url, [WebSocketOptions? options]) =>
      WebSocketClient(options)..connect(url).ignore();

  final IWebSocketClient _client;
  final WebSocketEventQueue _eventQueue = WebSocketEventQueue();

  /// Current options.
  /// {@nodoc}
  final WebSocketOptions? _options;

  @override
  bool get isClosed => _isClosed;
  bool _isClosed = false;

  /// Get the metrics for this client.
  WebSocketMetrics get metrics =>
      WebSocketMetricsManager.instance.buildMetricFor(this);

  @override
  WebSocketMessagesStream get stream => _client.stream;

  @override
  Stream<WebSocketClientState> get stateChanges => _client.stateChanges;

  @override
  WebSocketClientState get state => _client.state;

  @override
  Future<void> add(Object data) async {
    if (_isClosed) return Future<void>.error(const WSClientClosed());
    await _eventQueue.push('add', () => _client.add(data));
    WebSocketMetricsManager.instance.sent(this, data);
  }

  @override
  Future<void> connect(String url) {
    if (_isClosed) return Future<void>.error(const WSClientClosed());
    return _eventQueue.push('connect', () {
      WebSocketConnectionManager.instance.startMonitoringConnection(
        this,
        url,
        _options?.connectionRetryInterval,
      );
      return _client.connect(url);
    });
  }

  @override
  Future<void> disconnect(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) {
    if (_isClosed) return Future<void>.error(const WSClientClosed());
    return _eventQueue.push('disconnect', () {
      WebSocketConnectionManager.instance.stopMonitoringConnection(this);
      return _client.disconnect(code, reason);
    });
  }

  @override
  FutureOr<void> close(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) async {
    try {
      _isClosed = true;
      // Stop monitoring the connection.
      WebSocketConnectionManager.instance.stopMonitoringConnection(this);
      // Clear the event queue and prevent new events from being processed.
      // Returns when the queue is empty and no new events are being processed.
      Future<void>.sync(_eventQueue.close).ignore();
      // Close the internal client connection and free resources.
      await _client.close(code, reason);
    } finally {
      // Stop observing metrics.
      // Wait for the next microtask to ensure that the metrics are updated
      // from state stream, before stopping observing.
      scheduleMicrotask(() {
        WebSocketMetricsManager.instance.stopObserving(this);
      });
    }
  }
}
