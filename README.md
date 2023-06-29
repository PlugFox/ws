# Cross-platform WebSocket client

[![Pub](https://img.shields.io/pub/v/ws.svg)](https://pub.dev/packages/ws)
[![Actions Status](https://github.com/PlugFox/ws/actions/workflows/checkout.yml/badge.svg)](https://github.com/PlugFox/ws/actions)
[![Coverage](https://codecov.io/gh/PlugFox/ws/branch/master/graph/badge.svg)](https://codecov.io/gh/PlugFox/ws)
[![License: MIT](https://img.shields.io/badge/license-MIT-purple.svg)](https://opensource.org/licenses/MIT)
[![Linter](https://img.shields.io/badge/style-linter-40c4ff.svg)](https://pub.dev/packages/linter)

The `ws` package provides a cross-platform WebSocket client for both Dart and Flutter applications. It allows you to connect to a WebSocket server, send and receive messages, and handle the connection state changes.

## Installation

Add the following dependency to your `pubspec.yaml` file:

```yaml
dependencies:
  ws: <version>
```

## Properties

- `reconnectTimeout`: A read-only property that returns the reconnect timeout. The default value is 5 seconds.
- `state`: A read-only property that returns the current WebSocket connection state.
- `stateChanges`: A read-only property that returns a stream of WebSocket connection state changes.
- `stream`: A read-only property that returns a stream of message events handled by the WebSocket.
- `metrics`: A read-only property that returns a `WebSocketMetrics` object with metrics about the WebSocket connection.

## Methods

- `connect(String url)`: Connects to the WebSocket server specified by the URL argument.
- `disconnect([int? code = 1000, String? reason = 'NORMAL_CLOSURE'])`: Closes the WebSocket connection. You can optionally pass a numeric code and a reason string to the method to send close information to the remote peer.
- `add(Object data)`: Sends data on the WebSocket connection. The data can be either a String or a List of integers holding bytes.
- `close([int? code = 1000, String? reason = 'NORMAL_CLOSURE'])`: Permanently stops the WebSocket connection and frees all resources. After calling this method, the WebSocket client is no longer usable.

## Example

```dart
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
```

## Reconnection

The `ws` package provides a cross-platform WebSocket client that supports automatic reconnection in case of connection loss. The client automatically tries to reconnect to the server when the connection is lost. To handle reconnection-related events, you can register listeners for the `stateChanges` stream, which notifies you about changes in the connection state. When the connection is closed, the client tries to reconnect with a delay, which increases exponentially with each unsuccessful attempt to prevent overloading the server.

---

## Metrics

The `ws` package provides a cross-platform WebSocket client that supports metrics. The client automatically collects metrics about the number of sent and received messages, as well as the number of sent and received bytes. To get the metrics, you can use the `metrics` property, which returns a `WebSocketMetrics` object. The metrics are updated on demand, so you can get the latest values at any time.

---

## JSON

The `ws` package provides a cross-platform WebSocket client that supports JSON. The client automatically decodes incoming messages from JSON to Dart objects. To get the decoded messages, you can use the `client.stream.json` property, which returns a `Stream<Map<String, Object?>>` of decoded messages.

---

## Features and Roadmap

- [x] Cross-platform WebSocket client for Dart and Flutter
- [x] Support for secure WebSocket connections (wss://)
- [x] Connection state changes
- [x] Fake client
- [x] Reconnection to new URL
- [x] Concurrency
- [x] Auto reconnection after network problems
- [x] Handy stream of messages with automatic JSON decoding
- [x] Metrics & TX/RX bytes and counters
- [ ] Reusing client between isolates
- [ ] 95% test coverage

---

## More resources

- [RFC 6455: The WebSocket Protocol](https://tools.ietf.org/html/rfc6455)
- [WebSocket API on MDN](https://developer.mozilla.org/en-US/docs/Web/API/WebSockets_API)
- [Dart HTML WebSocket library](https://api.dart.dev/stable/dart-html/WebSocket-class.html)
- [Dart IO WebSocket library](https://api.dart.dev/stable/dart-io/WebSocket-class.html)

---

## Coverage

[![](https://codecov.io/gh/PlugFox/ws/branch/master/graphs/sunburst.svg)](https://codecov.io/gh/PlugFox/ws/branch/master)

---

## Changelog

Refer to the [Changelog](https://github.com/PlugFox/ws/blob/master/CHANGELOG.md) to get all release notes.

---

## Maintainers

[Plague Fox](https://plugfox.dev)

---

## License

[MIT](https://opensource.org/licenses/MIT)

---

## Tags

web, socket, ws, wss, WebSocket, cross, platform
