import 'package:shelf/shelf_io.dart' as shelf_io;
import 'package:shelf_web_socket/shelf_web_socket.dart';
import 'package:web_socket_channel/web_socket_channel.dart';

void main() {
  var handler = webSocketHandler((WebSocketChannel webSocket) {
    webSocket.stream.listen((Object? message) {
      print('Message received: $message');
      switch (message) {
        case "ping":
          webSocket.sink.add("pong");
          break;
      }
    });
  });

  shelf_io.serve(handler, 'localhost', 8080).then<void>((server) {
    print('Serving at ws://${server.address.host}:${server.port}');
  });
}
