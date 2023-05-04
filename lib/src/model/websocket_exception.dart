/// {@template websocket_exception}
/// This is a custom exception class for WebSocket.
/// Do not confuse it with a native exception WebSocketException from dart:io
/// {@endtemplate}
class WSException implements Exception {
  /// {@macro websocket_exception}
  const WSException([this.message = 'An unknown WebSocket error occurred.']);

  /// Message of exception.
  final String message;

  @override
  String toString() => "WebSocketException: $message";
}
