// ignore_for_file: avoid_print

import 'dart:async';

import 'package:ws/ws.dart';

void main([List<String>? args]) {
  // The server URL
  const url = 'wss://echo.plugfox.dev:443/connect';

  // Setup a WebSocket client with auto reconnect
  final client = WebSocketClient(reconnectTimeout: const Duration(seconds: 5))
    // Observing the incoming messages
    ..stream.listen((message) => print('< $message'))
    // Observing the state changes
    ..stateChanges.listen((state) => print('* $state'))
    // Connect to the server
    ..connect(url)
    // Send a message
    ..add('Hello, ').ignore()
    // One more message after first one
    ..add('world!').ignore();

  // Close the connection after 2 seconds
  Timer(const Duration(seconds: 2), client.close);

  // Print the metrics after 3 seconds
  Timer(const Duration(seconds: 3), () => print(client.metrics));
}
