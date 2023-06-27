import 'dart:async';

import 'package:ws/src/model/state.dart';

/// WebSocket client interface.
/// {@category Client}
abstract interface class IWebSocketClient implements Sink<Object> {
  /// The current state of the WebSocket connection.
  WebSocketClientState get state;

  /// Stream of state changes for the WebSocket connection.
  abstract final Stream<WebSocketClientState> stateChanges;

  /// Stream of message events handled by this WebSocket.
  abstract final Stream<Object> stream;

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
  void disconnect([int? code = 1000, String? reason = 'NORMAL_CLOSURE']);

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
  void close([int? code = 1000, String? reason = 'NORMAL_CLOSURE']);
}
