/// {@template websocket_exception}
/// This is a custom exception class for WebSocket.
/// Do not confuse it with a native exception WebSocketException from dart:io
/// {@endtemplate}
/// {@category Client}
/// {@category Entity}
sealed class WSException implements Exception {
  /// {@macro websocket_exception}
  const WSException(
      {this.originalException, this.message = 'An WebSocket error occurred.'});

  /// Original exception.
  final Object? originalException;

  /// Code of exception.
  abstract final String code;

  /// Message of exception.
  final String message;

  @override
  String toString() => message;
}

/// {@template not_connected_exception}
/// Exception thrown when a WebSocket is not connected.
/// {@endtemplate}
/// {@category Client}
/// {@category Entity}
final class WSNotConnectedException extends WSException {
  /// {@macro not_connected_exception}
  const WSNotConnectedException(
      {super.originalException, super.message = 'WebSocket is not connected.'});

  @override
  String get code => 'ws_not_connected';
}

/// {@template client_closed_exception}
/// Exception thrown when a WebSocket client is closed.
/// {@endtemplate}
/// {@category Client}
/// {@category Entity}
final class WSClientClosedException extends WSException {
  /// {@macro client_closed_exception}
  const WSClientClosedException(
      {super.originalException, super.message = 'WebSocket client is closed.'});

  @override
  String get code => 'ws_client_closed';
}

/// {@template client_send_exception}
/// Send operation failed.
/// {@endtemplate}
/// {@category Client}
/// {@category Entity}
final class WSSendException extends WSException {
  /// {@macro client_send_exception}
  const WSSendException(
      {super.originalException, super.message = 'WebSocket send failed.'});

  @override
  String get code => 'ws_send_exception';
}

/// {@template client_disconnect_exception}
/// Disconnect operation failed.
/// {@endtemplate}
/// {@category Client}
/// {@category Entity}
final class WSDisconnectException extends WSException {
  /// {@macro client_disconnect_exception}
  const WSDisconnectException(
      {super.originalException,
      super.message = 'WebSocket error occurred during disconnect.'});

  @override
  String get code => 'ws_disconnect_exception';
}
