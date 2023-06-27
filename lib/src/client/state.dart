import 'package:meta/meta.dart';
import 'package:ws/src/client/status_codes.dart';
import 'package:ws/src/client/web_socket_ready_state.dart';

/// Whether the stream controller is permanently closed.
///
/// The controller becomes closed by calling the [close] method.
///
/// If the controller is closed,
/// the "done" event might not have been delivered yet,
/// but it has been scheduled, and it is too late to add more events.
//bool get isClosed;

/// A future which is completed when the stream controller is done.
//Future<void> get done;

/// {@template web_socket_client_state}
/// WebSocket client state.
///
/// {@category Client}
/// {@category Entity}
/// {@endtemplate}
@immutable
sealed class WebSocketClientState {
  /// {@macro web_socket_client_state}
  const WebSocketClientState();

  /// {@macro web_socket_client_state}
  const factory WebSocketClientState.connecting({
    required String url,
  }) = WebSocketClientState$Connecting;

  /// {@macro web_socket_client_state}
  const factory WebSocketClientState.open({
    required String url,
  }) = WebSocketClientState$Open;

  /// {@macro web_socket_client_state}
  const factory WebSocketClientState.disconnecting({
    required int? closeCode,
    required String? closeReason,
  }) = WebSocketClientState$Disconnecting;

  /// {@macro web_socket_client_state}
  const factory WebSocketClientState.closed({
    required int? closeCode,
    required String? closeReason,
  }) = WebSocketClientState$Closed;

  /// The initial state of the web socket client.
  /// {@macro web_socket_client_state}
  static WebSocketClientState get initial => WebSocketClientState.closed(
        closeCode: WebSocketStatusCodes.normalClosure.code,
        closeReason: 'INITIAL_CLOSED_STATE',
      );

  /// Returns the current state of the connection.
  /// 0	: CONNECTING - Socket has been created. The connection is not yet open.
  /// 1	: OPEN       - The connection is open and ready to communicate.
  /// 2	: CLOSING    - The connection is in the process of closing.
  /// 3	: CLOSED     - The connection is closed or couldn't be opened.
  abstract final WebSocketReadyState readyState;
}

/// {@macro web_socket_client_state}
final class WebSocketClientState$Connecting extends WebSocketClientState {
  /// {@macro web_socket_client_state}
  const WebSocketClientState$Connecting({
    required this.url,
  });

  @override
  WebSocketReadyState get readyState => WebSocketReadyState.connecting;

  /// The URL connected to.
  final String url;

  @override
  int get hashCode => readyState.code ^ url.hashCode;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is WebSocketClientState$Connecting && other.url == url;

  @override
  String toString() => 'WebSocketClientState.connecting';
}

/// {@macro web_socket_client_state}
final class WebSocketClientState$Open extends WebSocketClientState {
  /// {@macro web_socket_client_state}
  const WebSocketClientState$Open({
    required this.url,
  });

  @override
  WebSocketReadyState get readyState => WebSocketReadyState.open;

  /// The URL connected to.
  final String url;

  @override
  int get hashCode => readyState.code ^ url.hashCode;

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is WebSocketClientState$Open && other.url == url;

  @override
  String toString() => 'WebSocketClientState.open';
}

/// {@macro web_socket_client_state}
final class WebSocketClientState$Disconnecting extends WebSocketClientState {
  /// {@macro web_socket_client_state}
  const WebSocketClientState$Disconnecting({
    required this.closeCode,
    required this.closeReason,
  });

  @override
  WebSocketReadyState get readyState => WebSocketReadyState.disconnecting;

  /// The close code set when the WebSocket connection is closed.
  /// If there is no close code available this property will be null.
  final int? closeCode;

  /// The close reason set when the WebSocket connection is closed.
  /// If there is no close reason available this property will be null.
  final String? closeReason;

  @override
  int get hashCode => Object.hashAll(<int?>[
        readyState.code,
        closeCode,
        closeReason?.hashCode
      ].whereType<int>());

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is WebSocketClientState$Disconnecting &&
          other.closeCode == closeCode &&
          other.closeReason == closeReason;

  @override
  String toString() => 'WebSocketClientState.disconnecting';
}

/// {@macro web_socket_client_state}
final class WebSocketClientState$Closed extends WebSocketClientState {
  /// {@macro web_socket_client_state}
  const WebSocketClientState$Closed({
    required this.closeCode,
    required this.closeReason,
  });

  @override
  WebSocketReadyState get readyState => WebSocketReadyState.closed;

  /// The close code set when the WebSocket connection is closed.
  /// If there is no close code available this property will be null.
  final int? closeCode;

  /// The close reason set when the WebSocket connection is closed.
  /// If there is no close reason available this property will be null.
  final String? closeReason;

  @override
  int get hashCode => Object.hashAll(<int?>[
        readyState.code,
        closeCode,
        closeReason?.hashCode
      ].whereType<int>());

  @override
  bool operator ==(Object other) =>
      identical(this, other) ||
      other is WebSocketClientState$Closed &&
          other.closeCode == closeCode &&
          other.closeReason == closeReason;

  @override
  String toString() => 'WebSocketClientState.closed';
}
