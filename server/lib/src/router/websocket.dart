import 'dart:async';

import 'package:shelf/shelf.dart' show Request, Response;
import 'package:shelf_web_socket/shelf_web_socket.dart' show webSocketHandler;
import 'package:web_socket_channel/web_socket_channel.dart'
    show WebSocketChannel;

final FutureOr<Response> Function(Request request) $webSocket =
    webSocketHandler((WebSocketChannel webSocket) {
  void push(Object message) {
    try {
      print('ws < $message');
      webSocket.sink.add(message);
    } catch (e) {
      print('e $e');
    }
  }

  webSocket.stream.listen(
    (Object? message) {
      print('ws > $message');
      switch (message) {
        case "ping":
          push("pong");
          break;
      }
    },
    onError: (Object error) => print('ws > [error] $error'),
    onDone: () => print('ws > [done]'),
    cancelOnError: true,
  );
});
