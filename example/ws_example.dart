// ignore_for_file: avoid_print
import 'dart:async';
import 'dart:io' as io show exit;

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
    // Send a message
    ..add('Hello, ') // > Hello,
    // One more message after first is sent
    ..add('world!'); // > world!

  Timer(const Duration(seconds: 1), () async {
    await client.close(); // Close the connection
    print('Metrics:\n${client.metrics}'); // Print the metrics
    io.exit(0); // Exit the process
  });
}
