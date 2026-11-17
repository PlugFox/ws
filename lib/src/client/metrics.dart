import 'package:meta/meta.dart';
import 'package:ws/src/client/exceptions.dart';

/// A snapshot of a client's counters.
@immutable
final class WebSocketMetrics {
  /// Creates a snapshot.
  const WebSocketMetrics({
    required this.transport,
    required this.messagesSent,
    required this.messagesReceived,
    required this.bytesSent,
    required this.bytesReceived,
    required this.connects,
    required this.reconnectAttempts,
    required this.decodeErrors,
    required this.droppedReplies,
    required this.pendingRpc,
    required this.bufferedAmount,
    this.lastRoundTrip,
    this.lastError,
    this.openedAt,
  });

  /// Name of the transport in use (`dart`, `web`, `native`, `fake`).
  final String transport;

  /// Messages handed to the transport.
  final int messagesSent;

  /// Messages delivered to the application (including RPC replies).
  final int messagesReceived;

  /// Payload bytes sent (encoded size).
  final int bytesSent;

  /// Payload bytes received, where the transport reports them.
  final int bytesReceived;

  /// Successful opens.
  final int connects;

  /// Reconnect attempts made.
  final int reconnectAttempts;

  /// Incoming messages dropped because they could not be decoded.
  final int decodeErrors;

  /// RPC replies that matched no pending call (late or unknown).
  final int droppedReplies;

  /// RPC calls waiting for a reply.
  final int pendingRpc;

  /// Bytes waiting in the send queue.
  final int bufferedAmount;

  /// Round trip of the last ping.
  final Duration? lastRoundTrip;

  /// The last error.
  final WebSocketException? lastError;

  /// When the current connection opened.
  final DateTime? openedAt;

  @override
  String toString() =>
      'WebSocketMetrics(transport: $transport, '
      'sent: $messagesSent, received: $messagesReceived, '
      'connects: $connects, reconnects: $reconnectAttempts)';
}
