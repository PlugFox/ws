import 'package:meta/meta.dart';
import 'package:ws/src/client/web_socket_ready_state.dart';

/// {@template metrics}
/// WebSocket metrics.
/// {@endtemplate}
@immutable
final class WebSocketMetrics {
  /// {@macro metrics}
  const WebSocketMetrics({
    required this.readyState,
    required this.reconnectTimeout,
    required this.transferredSize,
    required this.receivedSize,
    required this.transferredCount,
    required this.receivedCount,
    required this.reconnects,
    required this.lastSuccessfulConnectionTime,
    required this.disconnects,
    required this.lastDisconnectTime,
    required this.expectedReconnectTime,
    required this.lastDisconnectReason,
    required this.lastUrl,
  });

  /// The current state of the connection.
  final WebSocketReadyState readyState;

  /// Timeout between reconnection attempts.
  final Duration reconnectTimeout;

  /// The total number of bytes sent.
  final BigInt transferredSize;

  /// The total number of bytes received.
  final BigInt receivedSize;

  /// The total number of messages sent.
  final BigInt transferredCount;

  /// The total number of messages received.
  final BigInt receivedCount;

  /// The total number of times the connection has been re-established.
  final ({int successful, int total}) reconnects;

  /// The time of the last successful connection.
  final DateTime? lastSuccessfulConnectionTime;

  /// The total number of times the connection has been disconnected.
  final int disconnects;

  /// The time of the last disconnect.
  final DateTime? lastDisconnectTime;

  /// The time of the next expected reconnect.
  final DateTime? expectedReconnectTime;

  /// The last disconnect reason.
  final ({int? code, String? reason}) lastDisconnectReason;

  /// The last URL used to connect.
  final String? lastUrl;

  @override
  int get hashCode => Object.hashAll([
        readyState,
        reconnectTimeout,
        transferredSize,
        receivedSize,
        transferredCount,
        receivedCount,
        reconnects,
        lastSuccessfulConnectionTime,
        disconnects,
        lastDisconnectTime,
        expectedReconnectTime,
        lastDisconnectReason,
        lastUrl,
      ]);

  @override
  bool operator ==(Object other) => identical(this, other);

  @override
  String toString() {
    String dateTimeRepresentation(DateTime? time, {bool ago = false}) =>
        time != null
            ? '${DateTime.now().difference(time).abs().inSeconds} seconds '
                '${ago ? 'ago' : 'from now'}}'
            : 'never';
    return 'readyState: ${readyState.name}\n'
        'reconnectTimeout: ${reconnectTimeout.inSeconds} seconds\n'
        'transferredSize: $transferredSize\n'
        'receivedSize: $receivedSize\n'
        'transferredCount: $transferredCount\n'
        'receivedCount: $receivedCount\n'
        'reconnects: ${reconnects.successful} / ${reconnects.total}\n'
        'lastSuccessfulConnectionTime: '
        '${dateTimeRepresentation(lastSuccessfulConnectionTime, ago: true)}\n'
        'disconnects: $disconnects\n'
        'lastDisconnectTime: '
        '${dateTimeRepresentation(lastDisconnectTime, ago: true)}\n'
        'expectedReconnectTime: '
        '${dateTimeRepresentation(expectedReconnectTime)}\n'
        'lastDisconnectReason: '
        '${lastDisconnectReason.code ?? 'unknown'} '
        '(${lastDisconnectReason.reason ?? 'unknown'})\n'
        'lastUrl: ${lastUrl ?? 'not connected yet'}';
  }
}
