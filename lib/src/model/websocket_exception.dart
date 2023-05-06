/// {@template websocket_exception}
/// This is a custom exception class for WebSocket.
/// Do not confuse it with a native exception WebSocketException from dart:io
/// {@endtemplate}
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
final class WSNotConnected extends WSException {
  /// {@macro not_connected_exception}
  const WSNotConnected([super.message = 'WebSocket is not connected.']);
}

/// {@template unknown_exception}
/// Unknown WebSocket exception.
/// {@endtemplate}
final class WSUnknownException extends WSException {
  /// {@macro unknown_exception}
  const WSUnknownException(
      [super.message = 'An unknown WebSocket error occurred.']);
}

/// {@template socket_exception}
/// Exception thrown when a socket operation fails.
/// {@endtemplate}
final class WSSocketException extends WSException {
  /// {@macro socket_exception}
  const WSSocketException(super.message);
}

/// {@template http_exception}
/// Exception thrown when a socket operation fails.
/// {@endtemplate}
final class WSHttpException extends WSException {
  /// {@macro http_exception}
  const WSHttpException(super.message);
}
