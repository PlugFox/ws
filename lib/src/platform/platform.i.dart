import 'dart:async';

import 'package:ws/src/model/web_socket_ready_state.dart';

/// Crossplatform binding to the native WebSocket.
///
/// The WebSocket object provides the API for creating and managing
/// a WebSocket connection to a server, as well as for sending
/// and receiving data on the connection.
/// {@category Transport}
abstract interface class IWebSocketPlatformTransport implements Sink<Object> {
  /// Returns the current state of the connection.
  /// 0	: CONNECTING - Socket has been created. The connection is not yet open.
  /// 1	: OPEN       - The connection is open and ready to communicate.
  /// 2	: CLOSING    - The connection is in the process of closing.
  /// 3	: CLOSED     - The connection is closed or couldn't be opened.
  WebSocketReadyState get readyState;

  /// The close code set when the WebSocket connection is closed.
  /// If there is no close code available this property will be null.
  int? get closeCode;

  /// The close reason set when the WebSocket connection is closed.
  /// If there is no close reason available this property will be null.
  String? get closeReason;

  /// The extensions property is initially null.
  /// After the WebSocket connection is established
  /// this string reflects the extensions used by the server.
  String? get extensions;

  /// Connects to the WebSocket server.
  /// [url] - the URL that was used to establish the connection.
  Future<void> connect(String url);

  /// Closes the WebSocket connection.
  /// Set the optional [code] and [reason] arguments
  /// to send close information to the remote peer.
  /// If they are omitted, the peer will see
  /// `No Status Rcvd (1005)` code with no reason,
  /// indicates that no status code was provided even though one was expected.
  /// https://developer.mozilla.org/en-US/docs/Web/API/CloseEvent/code
  void disconnect([int? code, String? reason]);

  /// Sends data on the WebSocket connection.
  /// The data in data must be either a String, or a List<int> holding bytes.
  @override
  FutureOr<void> add(/* String || List<int> */ Object data);

  /// Permanently stops the WebSocket connection and frees all resources.
  /// After calling this method the WebSocket client is no longer usable.
  ///
  /// Use [disconnect] to temporarily close the connection.
  /// And reconnect with [connect] method later.
  @override
  void close([int? code = 1000, String? reason = 'Normal Closure']);

  /// On message received.
  abstract final void Function(Object data) onReceived;

  /// On message sent.
  abstract final void Function(Object data) onSent;

  /// Receive error from native WebSocket client.
  abstract final void Function(Object error, StackTrace stackTrace) onError;

  /// On connection established.
  abstract final void Function(String url) onConnected;

  /// On connection closed.
  abstract final void Function(int? code, String? reason) onDisconnected;
}
