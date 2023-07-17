import 'dart:async';

import 'package:ws/src/client/state.dart';

/// Stream of WebSocket's [WebSocketClientState] changes.
/// {@category Client}
/// {@category Entity}
final class WebSocketStatesStream extends StreamView<WebSocketClientState> {
  /// Stream of WebSocket's [WebSocketClientState] changes.
  WebSocketStatesStream(super.stream);

  /// Connection has not yet been established, but the WebSocket is trying.
  late final Stream<WebSocketClientState$Connecting> connecting =
      whereType<WebSocketClientState$Connecting>();

  /// Connection is open and ready to communicate.
  late final Stream<WebSocketClientState$Open> open =
      whereType<WebSocketClientState$Open>();

  /// Connection is in the process of closing.
  late final Stream<WebSocketClientState$Disconnecting> disconnecting =
      whereType<WebSocketClientState$Disconnecting>();

  /// Connection has been closed or couldn't be opened.
  late final Stream<WebSocketClientState$Closed> closed =
      whereType<WebSocketClientState$Closed>();

  /// Filtered stream of data of [WebSocketClientState].
  Stream<T> whereType<T extends WebSocketClientState>() =>
      transform<T>(StreamTransformer<WebSocketClientState, T>.fromHandlers(
        handleData: (data, sink) => switch (data) {
          T valid => sink.add(valid),
          _ => null,
        },
      )).asBroadcastStream();
}
