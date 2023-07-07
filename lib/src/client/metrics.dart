import 'package:meta/meta.dart';
import 'package:ws/src/client/web_socket_ready_state.dart';

/// {@template metrics}
/// WebSocket metrics.
/// {@endtemplate}
@immutable
final class WebSocketMetrics {
  /// {@macro metrics}
  const WebSocketMetrics({
    required this.timestamp,
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
    required this.lastDisconnect,
    required this.lastUrl,
  });

  /// Create WebSocket metrics from JSON Object
  /// {@macro metrics}
  factory WebSocketMetrics.fromJson(Map<String, Object?> json) {
    R extract<T, R>(
        String key, R Function(T value) convert, R Function() fallback) {
      if (json[key] case T value) {
        try {
          return convert(value);
        } on Object {
          return fallback();
        }
      } else {
        return fallback();
      }
    }

    return WebSocketMetrics(
      timestamp: extract<int, DateTime>(
        'timestamp',
        DateTime.fromMillisecondsSinceEpoch,
        DateTime.now,
      ),
      readyState: extract<int, WebSocketReadyState>(
        'readyState',
        WebSocketReadyState.fromCode,
        () => WebSocketReadyState.closed,
      ),
      reconnectTimeout: extract<int, Duration>(
        'reconnectTimeout',
        (v) => Duration(milliseconds: v),
        () => Duration.zero,
      ),
      transferredSize: extract<String, BigInt>(
        'transferredSize',
        BigInt.parse,
        () => BigInt.zero,
      ),
      receivedSize: extract<String, BigInt>(
        'receivedSize',
        BigInt.parse,
        () => BigInt.zero,
      ),
      transferredCount: extract<String, BigInt>(
        'transferredCount',
        BigInt.parse,
        () => BigInt.zero,
      ),
      receivedCount: extract<String, BigInt>(
        'receivedCount',
        BigInt.parse,
        () => BigInt.zero,
      ),
      reconnects: (
        successful: extract<int, int>(
          'reconnectsSuccessful',
          (v) => v,
          () => 0,
        ),
        total: extract<int, int>(
          'reconnectsTotal',
          (v) => v,
          () => 0,
        )
      ),
      lastSuccessfulConnectionTime: extract<int, DateTime?>(
        'lastSuccessfulConnectionTime',
        DateTime.fromMillisecondsSinceEpoch,
        () => null,
      ),
      disconnects: extract<int, int>(
        'disconnects',
        (v) => v,
        () => 0,
      ),
      lastDisconnectTime: extract<int, DateTime?>(
        'lastDisconnectTime',
        DateTime.fromMillisecondsSinceEpoch,
        () => null,
      ),
      lastDisconnect: (
        code: extract<int?, int?>(
          'lastDisconnectCode',
          (v) => v,
          () => null,
        ),
        reason: extract<String?, String?>(
          'lastDisconnectReason',
          (v) => v,
          () => null,
        ),
      ),
      lastUrl: extract<String?, String?>(
        'lastUrl',
        (v) => v,
        () => null,
      ),
      expectedReconnectTime: extract<int, DateTime?>(
        'expectedReconnectTime',
        DateTime.fromMillisecondsSinceEpoch,
        () => null,
      ),
    );
  }

  /// Convert WebSocket metrics to JSON Object
  Map<String, Object?> toJson() => <String, Object?>{
        'timestamp': timestamp.millisecondsSinceEpoch,
        'readyState': readyState.code,
        'reconnectTimeout': reconnectTimeout.inMilliseconds,
        'transferredSize': transferredSize.toString(),
        'receivedSize': receivedSize.toString(),
        'transferredCount': transferredCount.toString(),
        'receivedCount': receivedCount.toString(),
        'reconnectsSuccessful': reconnects.successful,
        'reconnectsTotal': reconnects.total,
        'lastSuccessfulConnectionTime':
            lastSuccessfulConnectionTime?.millisecondsSinceEpoch,
        'disconnects': disconnects,
        'lastDisconnectTime': lastDisconnectTime?.millisecondsSinceEpoch,
        'expectedReconnectTime': expectedReconnectTime?.millisecondsSinceEpoch,
        'lastDisconnectCode': lastDisconnect.code,
        'lastDisconnectReason': lastDisconnect.reason,
        'lastUrl': lastUrl,
      };

  /// Timestamp of the metrics.
  final DateTime timestamp;

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
  final ({int? code, String? reason}) lastDisconnect;

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
        lastDisconnect,
        lastUrl,
      ]);

  @override
  bool operator ==(Object other) => identical(this, other);

  @override
  String toString() {
    String dateTimeRepresentation(DateTime? time, {bool ago = false}) =>
        time != null
            ? '${DateTime.now().difference(time).abs().inSeconds} seconds '
                '${ago ? 'ago' : 'from now'}'
            : 'never';
    return '- readyState: ${readyState.name}\n'
        '- reconnectTimeout: ${reconnectTimeout.inSeconds} seconds\n'
        '- transferredSize: $transferredSize\n'
        '- receivedSize: $receivedSize\n'
        '- transferredCount: $transferredCount\n'
        '- receivedCount: $receivedCount\n'
        '- reconnects: ${reconnects.successful} / ${reconnects.total}\n'
        '- lastSuccessfulConnectionTime: '
        '${dateTimeRepresentation(lastSuccessfulConnectionTime, ago: true)}\n'
        '- disconnects: $disconnects\n'
        '- lastDisconnectTime: '
        '${dateTimeRepresentation(lastDisconnectTime, ago: true)}\n'
        '- expectedReconnectTime: '
        '${dateTimeRepresentation(expectedReconnectTime)}\n'
        '- lastDisconnect: '
        '${lastDisconnect.code ?? 'unknown'} '
        '(${lastDisconnect.reason ?? 'unknown'})\n'
        '- lastUrl: ${lastUrl ?? 'not connected yet'}';
  }
}
