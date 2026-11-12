/// Every error the client reports is a [WebSocketException]. Raw
/// `SocketException`s, browser events or native error strings never reach
/// the user; they are kept as [WebSocketException.cause].
sealed class WebSocketException implements Exception {
  const WebSocketException(this.message, {this.cause});

  /// Description of the error.
  final String message;

  /// The underlying error, if any (for logs only; its type is not part of
  /// the API).
  final Object? cause;

  String get _name;

  @override
  String toString() =>
      '$_name: $message${cause == null ? '' : ' (cause: $cause)'}';
}

/// The server answered the opening handshake with something other than a
/// WebSocket upgrade. Browsers hide the HTTP status, so on the web this is
/// reported as a [WebSocketConnectionException] instead.
final class WebSocketHandshakeException extends WebSocketException {
  /// Creates the exception.
  const WebSocketHandshakeException(
    super.message, {
    this.statusCode,
    this.headers = const <String, String>{},
    super.cause,
  });

  /// HTTP status, when a response was received.
  final int? statusCode;

  /// Response headers, names in lower case.
  final Map<String, String> headers;

  @override
  String get _name => 'WebSocketHandshakeException';
}

/// DNS, TCP or TLS failure, or a connection lost without a close frame.
final class WebSocketConnectionException extends WebSocketException {
  /// Creates the exception.
  const WebSocketConnectionException(super.message, {super.cause});

  @override
  String get _name => 'WebSocketConnectionException';
}

/// Connecting, the closing handshake or a heartbeat took too long.
final class WebSocketTimeoutException extends WebSocketException {
  /// Creates the exception.
  const WebSocketTimeoutException(super.message, {super.cause});

  @override
  String get _name => 'WebSocketTimeoutException';
}

/// The connection was closed with a close frame (by the server, or by the
/// client because the server broke the protocol or a limit).
final class WebSocketClosedException extends WebSocketException {
  /// Creates the exception.
  const WebSocketClosedException(
    super.message, {
    this.code,
    this.reason = '',
    this.byServer = true,
    super.cause,
  });

  /// The close code; `null` when the close frame had none (1005).
  final int? code;

  /// The close reason.
  final String reason;

  /// Whether the server started the close.
  final bool byServer;

  @override
  String get _name => 'WebSocketClosedException';
}

/// An operation needs an open connection.
final class WebSocketNotConnectedException extends WebSocketException {
  /// Creates the exception.
  const WebSocketNotConnectedException(super.message);

  @override
  String get _name => 'WebSocketNotConnectedException';
}

/// `push()` was refused because more than
/// `WebSocketOptions.sendBufferLimit` bytes wait to be sent. The connection
/// stays open; wait (or await earlier `push()` futures) and retry.
final class WebSocketBufferFullException extends WebSocketException {
  /// Creates the exception.
  const WebSocketBufferFullException(super.message, {required this.buffered});

  /// Bytes queued when the message was refused.
  final int buffered;

  @override
  String get _name => 'WebSocketBufferFullException';
}

/// A message could not be decoded or encoded. Incoming messages that fail
/// are dropped and reported through `onError`; the connection stays open.
final class WebSocketFormatException extends WebSocketException {
  /// Creates the exception.
  const WebSocketFormatException(super.message, {super.cause});

  @override
  String get _name => 'WebSocketFormatException';
}

/// The operation is not available on this platform or transport (for
/// example a protocol ping in the browser).
final class WebSocketUnsupportedException extends WebSocketException {
  /// Creates the exception.
  const WebSocketUnsupportedException(super.message);

  @override
  String get _name => 'WebSocketUnsupportedException';
}

/// Why an RPC call failed.
enum WebSocketRpcFailure {
  /// No reply within the timeout.
  timeout,

  /// The connection was lost or `disconnect()` was called before the reply.
  disconnected,

  /// The client was closed.
  closed,

  /// The server replied with an error.
  remote,
}

/// An RPC call failed. Calls are never retried automatically.
final class WebSocketRpcException extends WebSocketException {
  /// Creates the exception.
  const WebSocketRpcException(
    super.message, {
    required this.failure,
    this.id,
    this.error,
    super.cause,
  });

  /// What went wrong.
  final WebSocketRpcFailure failure;

  /// The request id, if one was assigned.
  final Object? id;

  /// The error payload from the server, for [WebSocketRpcFailure.remote].
  final Object? error;

  @override
  String get _name => 'WebSocketRpcException';
}
