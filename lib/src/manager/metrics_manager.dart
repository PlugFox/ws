import 'dart:async';

import 'package:meta/meta.dart';
import 'package:ws/src/client/metrics.dart';
import 'package:ws/src/client/state.dart';
import 'package:ws/src/client/web_socket_ready_state.dart';
import 'package:ws/src/client/ws_client_interface.dart';

@internal
final class WebSocketMetricsManager {
  WebSocketMetricsManager(IWebSocketClient client)
      : _client = WeakReference<IWebSocketClient>(client);

  final WeakReference<IWebSocketClient> _client;

  StreamSubscription<Object>? _receiveObserver;

  StreamSubscription<WebSocketClientState>? _stateObserver;

  final $WebSocketMetrics _metrics = $WebSocketMetrics();

  void startObserving() {
    stopObserving();
    final metrics = _metrics;
    _receiveObserver = _client.target?.stream.listen(
      (data) => _onDataReceived(metrics, data),
      cancelOnError: false,
    );
    _stateObserver = _client.target?.stateChanges.listen(
      (state) => _onStateChanged(metrics, state),
      cancelOnError: false,
    );
  }

  void stopObserving() {
    _receiveObserver?.cancel().ignore();
    _stateObserver?.cancel().ignore();
    _receiveObserver = null;
    _stateObserver = null;
  }

  void _onDataReceived($WebSocketMetrics metrics, Object data) {
    metrics
      ..receivedCount += BigInt.one
      ..receivedSize += switch (data) {
        String text => BigInt.from(text.length),
        List<int> bytes => BigInt.from(bytes.length),
        _ => BigInt.zero,
      };
  }

  void _onStateChanged($WebSocketMetrics metrics, WebSocketClientState state) {
    switch (state) {
      case WebSocketClientState$Connecting connecting:
        metrics
          ..reconnects = (
            successful: metrics.reconnects.successful,
            total: metrics.reconnects.total + 1
          )
          ..lastUrl = connecting.url;
        break;
      case WebSocketClientState$Open open:
        metrics
          ..reconnects = (
            successful: metrics.reconnects.successful + 1,
            total: metrics.reconnects.total
          )
          ..lastSuccessfulConnectionTime = DateTime.now()
          ..lastUrl = open.url;
        break;
      case WebSocketClientState$Disconnecting _:
        break;
      case WebSocketClientState$Closed closed:
        metrics
          ..disconnects += 1
          ..lastDisconnectTime = DateTime.now()
          ..lastDisconnect =
              (code: closed.closeCode, reason: closed.closeReason);
        break;
    }
  }

  @internal
  void sent(IWebSocketClient client, Object data) => _metrics
    ..transferredCount += BigInt.one
    ..transferredSize += switch (data) {
      String text => BigInt.from(text.length),
      List<int> bytes => BigInt.from(bytes.length),
      _ => BigInt.zero,
    };

  @internal
  WebSocketMetrics buildMetric({
    required bool active,
    required int attempt,
    required DateTime? nextReconnectionAttempt,
  }) {
    final metrics = _metrics;
    final readyState =
        _client.target?.state.readyState ?? WebSocketReadyState.closed;
    final lastDisconnectTime = metrics.lastDisconnectTime;
    return WebSocketMetrics(
      timestamp: DateTime.now(),
      readyState: readyState,
      transferredSize: metrics.transferredSize,
      receivedSize: metrics.receivedSize,
      transferredCount: metrics.transferredCount,
      receivedCount: metrics.receivedCount,
      reconnects: metrics.reconnects,
      lastSuccessfulConnectionTime: metrics.lastSuccessfulConnectionTime,
      disconnects: metrics.disconnects,
      lastDisconnectTime: lastDisconnectTime,
      lastDisconnect: metrics.lastDisconnect,
      lastUrl: metrics.lastUrl,
      isReconnectionActive: active,
      currentReconnectAttempts: attempt,
      nextReconnectionAttempt: switch (readyState) {
        WebSocketReadyState.open => null,
        WebSocketReadyState.connecting => DateTime.now(),
        WebSocketReadyState.disconnecting => null,
        WebSocketReadyState.closed => nextReconnectionAttempt,
      },
    );
  }
}

@internal
final class $WebSocketMetrics {
  /// The total number of bytes sent.
  BigInt transferredSize = BigInt.zero;

  /// The total number of bytes received.
  BigInt receivedSize = BigInt.zero;

  /// The total number of messages sent.
  BigInt transferredCount = BigInt.zero;

  /// The total number of messages received.
  BigInt receivedCount = BigInt.zero;

  /// The total number of times the connection has been re-established.
  ({int successful, int total}) reconnects = (successful: 0, total: 0);

  /// The total number of times the connection has been disconnected.
  int disconnects = 0;

  /// The time of the last successful connection.
  DateTime? lastSuccessfulConnectionTime;

  /// The time of the last disconnect.
  DateTime? lastDisconnectTime;

  /// The last disconnect reason.
  ({int? code, String? reason}) lastDisconnect = (code: null, reason: null);

  /// The last URL used to connect.
  String? lastUrl;
}
