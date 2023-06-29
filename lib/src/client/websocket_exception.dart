/// {@template websocket_exception}
/// This is a custom exception class for WebSocket.
/// Do not confuse it with a native exception WebSocketException from dart:io
/// {@endtemplate}
/// {@category Entity}
sealed class WSException implements Exception {
  /// {@macro websocket_exception}
  const WSException([this.message = 'An WebSocket error occurred.']);

  /// Message of exception.
  final String message;

  @override
  String toString() => message;
}

/// {@template not_connected_exception}
/// Exception thrown when a WebSocket is not connected.
/// {@endtemplate}
/// {@category Entity}
final class WSNotConnected extends WSException {
  /// {@macro not_connected_exception}
  const WSNotConnected([super.message = 'WebSocket is not connected.']);
}

/// {@template unknown_exception}
/// Unknown WebSocket exception.
/// {@endtemplate}
/// {@category Entity}
final class WSUnknownException extends WSException {
  /// {@macro unknown_exception}
  const WSUnknownException(
      [super.message = 'An unknown WebSocket error occurred.']);
}

/// {@template socket_exception}
/// Exception thrown when a socket operation fails.
/// {@endtemplate}
/// {@category Entity}
final class WSSocketException extends WSException {
  /// {@macro socket_exception}
  const WSSocketException(super.message);
}

/// {@template http_exception}
/// Exception thrown when a socket operation fails.
/// {@endtemplate}
/// {@category Entity}
final class WSHttpException extends WSException {
  /// {@macro http_exception}
  const WSHttpException(super.message);
}

/// {@template unsupported_exception}
/// The operation was not allowed by the object.
/// {@endtemplate}
/// {@category Entity}
final class WSUnsupportedException extends WSException {
  /// {@macro unsupported_exception}
  const WSUnsupportedException(super.message);
}

/// {@template client_closed}
/// The operation was not allowed by the object.
/// {@endtemplate}
/// {@category Entity}
final class WSClientClosed extends WSException implements StateError {
  /// {@macro client_closed}
  const WSClientClosed({
    String message = 'WebSocket client is closed.',
    this.stackTrace,
  }) : super(message);

  @override
  final StackTrace? stackTrace;
}
