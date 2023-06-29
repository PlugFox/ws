import 'dart:async';

import 'package:meta/meta.dart';
import 'package:ws/src/client/metrics.dart';
import 'package:ws/src/client/state.dart';
import 'package:ws/src/client/web_socket_ready_state.dart';
import 'package:ws/src/client/ws_client_interface.dart';

/// {@nodoc}
@internal
final class WebSocketMetricsManager {
  /// {@nodoc}
  static final WebSocketMetricsManager instance =
      WebSocketMetricsManager._internal();

  /// {@nodoc}
  WebSocketMetricsManager._internal();

  /// {@nodoc}
  final Expando<StreamSubscription<Object>> _receiveObservers =
      Expando<StreamSubscription<Object>>();

  /// {@nodoc}
  final Expando<StreamSubscription<WebSocketClientState>> _stateObservers =
      Expando<StreamSubscription<WebSocketClientState>>();

  /// {@nodoc}
  final Expando<$WebSocketMetrics> _metrics = Expando<$WebSocketMetrics>();

  /// {@nodoc}
  void startObserving(IWebSocketClient client) {
    stopObserving(client);
    final metrics = _getMetrics(client);
    _receiveObservers[client] = client.stream.listen(
      (data) => _onDataReceived(metrics, data),
      cancelOnError: false,
    );
    _stateObservers[client] = client.stateChanges.listen(
      (state) => _onStateChanged(metrics, state),
      cancelOnError: false,
    );
  }

  /// {@nodoc}
  void stopObserving(IWebSocketClient client) {
    _receiveObservers[client]?.cancel().ignore();
    _stateObservers[client]?.cancel().ignore();
    _receiveObservers[client] = null;
    _stateObservers[client] = null;
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
          ..lastDisconnectReason =
              (code: closed.closeCode, reason: closed.closeReason);
        break;
    }
  }

  /// {@nodoc}
  $WebSocketMetrics _getMetrics(IWebSocketClient client) =>
      _metrics[client] ??= $WebSocketMetrics();

  /// {@nodoc}
  @internal
  void sent(IWebSocketClient client, Object data) => _getMetrics(client)
    ..transferredCount += BigInt.one
    ..transferredSize += switch (data) {
      String text => BigInt.from(text.length),
      List<int> bytes => BigInt.from(bytes.length),
      _ => BigInt.zero,
    };

  /// {@nodoc}
  @internal
  WebSocketMetrics buildMetric(IWebSocketClient client) {
    final metrics = _getMetrics(client);
    final readyState = client.state.readyState;
    final lastDisconnectTime = metrics.lastDisconnectTime;
    final reconnectTimeout = client.reconnectTimeout;
    return WebSocketMetrics(
      readyState: readyState,
      reconnectTimeout: reconnectTimeout,
      transferredSize: metrics.transferredSize,
      receivedSize: metrics.receivedSize,
      transferredCount: metrics.transferredCount,
      receivedCount: metrics.receivedCount,
      reconnects: metrics.reconnects,
      lastSuccessfulConnectionTime: metrics.lastSuccessfulConnectionTime,
      disconnects: metrics.disconnects,
      lastDisconnectTime: lastDisconnectTime,
      lastDisconnectReason: metrics.lastDisconnectReason,
      lastUrl: metrics.lastUrl,
      expectedReconnectTime: switch (readyState) {
        WebSocketReadyState.open => null,
        WebSocketReadyState.connecting => DateTime.now(),
        WebSocketReadyState.disconnecting
            when reconnectTimeout > Duration.zero =>
          DateTime.now().add(reconnectTimeout),
        WebSocketReadyState.closed
            when reconnectTimeout > Duration.zero &&
                lastDisconnectTime != null =>
          lastDisconnectTime.add(reconnectTimeout),
        _ => null,
      },
    );
  }
}

/// {@nodoc}
@internal
final class $WebSocketMetrics {
  /// The total number of bytes sent.
  /// {@nodoc}
  BigInt transferredSize = BigInt.zero;

  /// The total number of bytes received.
  /// {@nodoc}
  BigInt receivedSize = BigInt.zero;

  /// The total number of messages sent.
  /// {@nodoc}
  BigInt transferredCount = BigInt.zero;

  /// The total number of messages received.
  /// {@nodoc}
  BigInt receivedCount = BigInt.zero;

  /// The total number of times the connection has been re-established.
  /// {@nodoc}
  ({int successful, int total}) reconnects = (successful: 0, total: 0);

  /// The total number of times the connection has been disconnected.
  int disconnects = 0;

  /// The time of the last successful connection.
  DateTime? lastSuccessfulConnectionTime;

  /// The time of the last disconnect.
  DateTime? lastDisconnectTime;

  /// The last disconnect reason.
  ({int? code, String? reason}) lastDisconnectReason =
      (code: null, reason: null);

  /// The last URL used to connect.
  String? lastUrl;
}
