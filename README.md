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
  ws: ^0.1.0
```

## Properties

- `state`: A read-only property that returns the current WebSocket connection state.
- `stateChanges`: A read-only property that returns a stream of WebSocket connection state changes.
- `stream`: A read-only property that returns a stream of message events handled by the WebSocket.

## Methods

- `connect(String url)`: Connects to the WebSocket server specified by the URL argument.
- `disconnect([int? code = 1000, String? reason = 'NORMAL_CLOSURE'])`: Closes the WebSocket connection. You can optionally pass a numeric code and a reason string to the method to send close information to the remote peer.
- `add(Object data)`: Sends data on the WebSocket connection. The data can be either a String or a List of integers holding bytes.
- `close([int? code = 1000, String? reason = 'NORMAL_CLOSURE'])`: Permanently stops the WebSocket connection and frees all resources. After calling this method, the WebSocket client is no longer usable.

## Example

```dart
import 'package:ws/ws.dart';

void main() async {
  const url = 'ws://localhost:1234';

  final client = WebSocketClient.connect(url);

  client.stream.listen((message) {
    print('Received message: $message');
  });

  client.add('Hello, server!');

  await Future<void>.delayed(const Duration(seconds: 10));
  client.close();
}
```

## Reconnection

The `ws` package provides a cross-platform WebSocket client that supports automatic reconnection in case of connection loss. The client automatically tries to reconnect to the server when the connection is lost. To handle reconnection-related events, you can register listeners for the `stateChanges` stream, which notifies you about changes in the connection state. When the connection is closed, the client tries to reconnect with a delay, which increases exponentially with each unsuccessful attempt to prevent overloading the server.

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
