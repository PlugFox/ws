// ignore_for_file: avoid_print
import 'dart:async';
import 'dart:io' as io;

import 'package:ws/ws.dart';

void main([List<String>? args]) {
  // The server URL (pass it as --define=URL=...)
  const url = String.fromEnvironment('URL',
      defaultValue: 'wss://echo.plugfox.dev:443/connect');

  // Setup a WebSocket client with auto reconnect
  final client = WebSocketClient(reconnectTimeout: const Duration(seconds: 5))
    // Observing the incoming messages from the server
    ..stream.listen((message) => print('< $message'))
    // Observing the state changes (connecting, open, disconnecting, closed)
    ..stateChanges.listen((state) => print('* $state'))
    // Connect to the server url
    ..connect(url)
    // Send a message 'Hello, '
    ..add('Hello, ')
    // One more message 'world!' after first one is sent
    ..add('world!');

  // Close the connection after 1 seconds
  Timer(const Duration(seconds: 1), client.close);

  // Print the metrics after 2 seconds
  Timer(const Duration(seconds: 2), () => print('Metrics:\n${client.metrics}'));

  // Exit the process after 3 seconds
  Timer(const Duration(seconds: 3), () => io.exit(0));
}
