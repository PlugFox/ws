// ignore_for_file: avoid_print
import 'dart:async';
import 'dart:io' as io show exit;

import 'package:ws/ws.dart';

void main([List<String>? args]) {
  // The Web Socket server URL.
  // Pass it as `--define=URL=...` for `String.fromEnvironment('URL')`
  // or extract from env by `Platform.environment['URL']`
  // or parse it using `dart:args`
  const url = String.fromEnvironment('URL',
      defaultValue: 'wss://echo.plugfox.dev:443/connect');

  // Setup a WebSocket client with auto reconnecting
  // Also, we can enqueue sending before the connection is established.
  final client = WebSocketClient(
    // Common options for all platforms
    // Or use `.js(..)`, `.vm(..)`, `.selector(..)` instead of `.common(..)`
    WebSocketOptions.common(
      // The delay between reconnection attempts will be between 500ms and 15s
      connectionRetryInterval: (
        min: const Duration(milliseconds: 500),
        max: const Duration(seconds: 15),
      ),
    ),
  )
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

  // Close the connection after 1 second
  Timer(const Duration(seconds: 1), () async {
    await client.close(); // Close the connection
    print('Metrics:\n${client.metrics}'); // Print the metrics
    io.exit(0); // Exit the process
  });
}
