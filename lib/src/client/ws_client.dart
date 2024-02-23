import 'dart:async';

import 'package:ws/src/client/message_stream.dart';
import 'package:ws/src/client/metrics.dart';
import 'package:ws/src/client/state.dart';
import 'package:ws/src/client/state_stream.dart';
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
        _options = options ?? WebSocketOptions.common() {
    _init();
  }

  /// Creates a [WebSocketClient] from an existing [IWebSocketClient].
  /// This is useful for testing or if you want to use a custom implementation
  /// with reconnecting and concurrency protection.
  /// {@macro ws_client}
  WebSocketClient.fromClient(IWebSocketClient client,
      [WebSocketOptions? options])
      : _client = client,
        _options = options ?? WebSocketOptions.common() {
    _init();
  }

  /// {@macro ws_client}
  factory WebSocketClient.connect(String url, [WebSocketOptions? options]) =>
      WebSocketClient(options)..connect(url).ignore();

  final IWebSocketClient _client;
  final WebSocketEventQueue _eventQueue = WebSocketEventQueue();
  late final WebSocketMetricsManager _metricsManager =
      WebSocketMetricsManager(this);
  late final WebSocketConnectionManager _connectionManager =
      WebSocketConnectionManager(this);

  /// Current options.
  /// {@nodoc}
  final WebSocketOptions _options;

  @override
  bool get isClosed => _isClosed;
  bool _isClosed = false;

  /// Get the metrics for this client.
  WebSocketMetrics get metrics {
    final (
      :bool active,
      :int attempt,
      :DateTime? nextReconnectionAttempt,
    ) = _connectionManager.status;
    return _metricsManager.buildMetric(
      active: active,
      attempt: attempt,
      nextReconnectionAttempt: nextReconnectionAttempt,
    );
  }

  @override
  WebSocketMessagesStream get stream => _client.stream;

  @override
  WebSocketStatesStream get stateChanges => _client.stateChanges;

  @override
  WebSocketClientState get state => _client.state;

  void _init() {
    _metricsManager.startObserving();
  }

  @override
  Future<void> add(Object data) async {
    if (_isClosed) return Future<void>.error(const WSClientClosedException());
    await _eventQueue.push('add', () async {
      try {
        await _client.add(data);
      } on WSException {
        rethrow;
      } on Object catch (error, stackTrace) {
        Error.throwWithStackTrace(
          WSSendException(originalException: error),
          stackTrace,
        );
      }
    });
    _metricsManager.sent(this, data);
  }

  @override
  Future<void> connect(String url) {
    if (_isClosed) return Future<void>.error(const WSClientClosedException());
    return _eventQueue.push('connect', () async {
      _connectionManager.startMonitoringConnection(
        url,
        _options.connectionRetryInterval,
      );
      try {
        await Future<void>.sync(() => _client.connect(url))
            .timeout(_options.timeout);
      } on WSException {
        rethrow;
      } on Object catch (error, stackTrace) {
        Error.throwWithStackTrace(
          WSNotConnectedException(originalException: error),
          stackTrace,
        );
      }
      try {
        // Send first messages after connection is established:
        await _options.afterConnect?.call(this);
      } on Object {
        _client.disconnect(1006, 'AFTER_CONNECT_ERROR');
      }
    });
  }

  @override
  Future<void> disconnect(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) {
    if (_isClosed) return Future<void>.error(const WSClientClosedException());
    return _eventQueue.push('disconnect', () async {
      _connectionManager.stopMonitoringConnection();
      try {
        await Future<void>.sync(() => _client.disconnect(code, reason))
            .timeout(_options.timeout);
      } on WSException {
        rethrow;
      } on Object catch (error, stackTrace) {
        Error.throwWithStackTrace(
          WSDisconnectException(originalException: error),
          stackTrace,
        );
      }
    });
  }

  @override
  FutureOr<void> close(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) async {
    try {
      _isClosed = true;
      // Stop monitoring the connection.
      _connectionManager.stopMonitoringConnection();
      // Clear the event queue and prevent new events from being processed.
      // Returns when the queue is empty and no new events are being processed.
      Future<void>.sync(_eventQueue.close).ignore();
      // Close the internal client connection and free resources.
      await _client.close(code, reason);
    } on WSException {
      rethrow;
    } on Object catch (error, stackTrace) {
      Error.throwWithStackTrace(
        WSDisconnectException(originalException: error),
        stackTrace,
      );
    } finally {
      // Stop observing metrics.
      // Wait for the next microtask to ensure that the metrics are updated
      // from state stream, before stopping observing.
      scheduleMicrotask(() {
        _metricsManager.stopObserving();
      });
    }
  }
}
