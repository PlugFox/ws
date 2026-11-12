import 'package:meta/meta.dart';
import 'package:ws/src/client/exceptions.dart';

/// The state of a `WebSocketClient`.
///
/// ```
///             connect()                  transport open + onOpen hook
/// disconnected ───────► connecting ─────────────────────────────► open
///      ▲                   │  ▲                                      │
///      │  terminal error   │  │ timer                 lost / failed  │
///      ├───────────────────┘  └──────── reconnecting ◄───────────────┤
///      │                                                             │
///      └──────────────────────── disconnecting ◄──── disconnect() ───┘
///
/// close() from any state ──► closed (terminal)
/// ```
@immutable
sealed class WebSocketState {
  const WebSocketState();

  /// Not connected and not trying to. The initial state, and the state after
  /// `disconnect()` or a failure that is not retried.
  const factory WebSocketState.disconnected({
    int? closeCode,
    String? closeReason,
    WebSocketException? error,
  }) = WebSocketState$Disconnected;

  /// Opening a connection. [attempt] is 0 for the first try after
  /// `connect()`, and counts reconnects after that.
  const factory WebSocketState.connecting({
    required Uri url,
    required int attempt,
  }) = WebSocketState$Connecting;

  /// Connected; messages can be sent.
  const factory WebSocketState.open({required Uri url, String? protocol}) =
      WebSocketState$Open;

  /// Waiting [delay] before reconnect attempt number [attempt], because of
  /// [cause].
  const factory WebSocketState.reconnecting({
    required int attempt,
    required Duration delay,
    required WebSocketException cause,
  }) = WebSocketState$Reconnecting;

  /// The closing handshake is in progress after `disconnect()`.
  const factory WebSocketState.disconnecting({int? code, String? reason}) =
      WebSocketState$Disconnecting;

  /// `close()` was called. Terminal: the client cannot be used again.
  const factory WebSocketState.closed() = WebSocketState$Closed;

  /// Whether messages can be sent now.
  bool get isOpen => false;

  /// Whether the client was closed for good.
  bool get isClosed => false;
}

/// See [WebSocketState.disconnected].
final class WebSocketState$Disconnected extends WebSocketState {
  /// See [WebSocketState.disconnected].
  const WebSocketState$Disconnected({
    this.closeCode,
    this.closeReason,
    this.error,
  });

  /// Close code of the last connection, if there was one.
  final int? closeCode;

  /// Close reason of the last connection.
  final String? closeReason;

  /// Why the client stopped, when it was not a `disconnect()` call.
  final WebSocketException? error;

  @override
  String toString() =>
      'WebSocketState.disconnected('
      '${closeCode ?? ''}${error == null ? '' : ', $error'})';
}

/// See [WebSocketState.connecting].
final class WebSocketState$Connecting extends WebSocketState {
  /// See [WebSocketState.connecting].
  const WebSocketState$Connecting({required this.url, required this.attempt});

  /// The URL being connected to.
  final Uri url;

  /// 0 for the first try, then the reconnect attempt number.
  final int attempt;

  @override
  String toString() => 'WebSocketState.connecting($url, attempt $attempt)';
}

/// See [WebSocketState.open].
final class WebSocketState$Open extends WebSocketState {
  /// See [WebSocketState.open].
  const WebSocketState$Open({required this.url, this.protocol});

  /// The connected URL.
  final Uri url;

  /// The subprotocol selected by the server.
  final String? protocol;

  @override
  bool get isOpen => true;

  @override
  String toString() => 'WebSocketState.open($url)';
}

/// See [WebSocketState.reconnecting].
final class WebSocketState$Reconnecting extends WebSocketState {
  /// See [WebSocketState.reconnecting].
  const WebSocketState$Reconnecting({
    required this.attempt,
    required this.delay,
    required this.cause,
  });

  /// The number of the next attempt (1 for the first reconnect).
  final int attempt;

  /// Delay before the next attempt.
  final Duration delay;

  /// Why the previous connection or attempt ended.
  final WebSocketException cause;

  @override
  String toString() =>
      'WebSocketState.reconnecting(attempt $attempt in '
      '${delay.inMilliseconds} ms: $cause)';
}

/// See [WebSocketState.disconnecting].
final class WebSocketState$Disconnecting extends WebSocketState {
  /// See [WebSocketState.disconnecting].
  const WebSocketState$Disconnecting({this.code, this.reason});

  /// Close code sent to the server.
  final int? code;

  /// Close reason sent to the server.
  final String? reason;

  @override
  String toString() => 'WebSocketState.disconnecting(${code ?? ''})';
}

/// See [WebSocketState.closed].
final class WebSocketState$Closed extends WebSocketState {
  /// See [WebSocketState.closed].
  const WebSocketState$Closed();

  @override
  bool get isClosed => true;

  @override
  String toString() => 'WebSocketState.closed';
}
