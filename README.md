# WS: Cross-platform WebSocket client

[![Pub](https://img.shields.io/pub/v/ws.svg)](https://pub.dev/packages/ws)
[![Actions Status](https://github.com/PlugFox/ws/actions/workflows/checkout.yml/badge.svg)](https://github.com/PlugFox/ws/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-purple.svg)](https://opensource.org/licenses/MIT)
[![GitHub stars](https://img.shields.io/github/stars/plugfox/ws?style=social)](https://github.com/plugfox/ws/)

A WebSocket client for Dart and Flutter on every platform, built for
reliability: reconnects with backoff, heartbeats, request/response calls,
typed states and errors, and a fake server for your tests.

> **2.0 is a rewrite** (branch `rewrite`, `2.0.0-dev`). The design, the
> measurements behind it and the plan are in [`docs/`](docs/).

## Install

```yaml
dependencies:
  ws: ^2.0.0-dev.1
  # Optional, for mobile/desktop: the Rust transport (TLS, framing and
  # JSON parsing off the Dart thread; 4–21× faster on phones).
  ws_native: ^2.0.0-dev.1
```

## Use

```dart
import 'package:ws/ws.dart';

Future<void> main() async {
  final client = WebSocketClient.json(
    options: WebSocketOptions(
      rpc: const WebSocketJsonRpc(), // {"id": n, ...} correlation
      heartbeat: const WebSocketHeartbeat(),
      onStateChange: print,
    ),
  );

  client.messages.listen((message) => print('push: $message'));
  await client.connect('wss://example.com/ws');

  await client.push({'type': 'subscribe', 'channel': 'news'});
  final reply = await client.rpc({'method': 'time'});
  print('reply: $reply');

  await client.close();
}
```

Three flavours, one core: `WebSocketClient.json()` (`Map<String, Object?>`),
`.text()` (`String`) and `.binary()` (`Uint8List`).

| | |
|---|---|
| `connect(url)` | Connects and keeps the connection up. Completes when open. |
| `push(message)` | Sends. Fails fast when not connected — nothing is replayed after a reconnect. Waits when the send queue is above `sendHighWaterMark`. |
| `rpc(request)` | Sends and waits for the matching reply (`WebSocketOptions.rpc`). |
| `ping()` | Round trip check (protocol ping; application-level ping on the web). |
| `reconnect()` | Reconnect now, e.g. when the app returns to the foreground. |
| `disconnect()` | Closes; `connect` can be called again. |
| `close()` | Closes for good and releases everything. |
| `state` / `states` | `disconnected`, `connecting`, `open`, `reconnecting`, `disconnecting`, `closed`. |
| `messages` | Incoming messages (or `WebSocketOptions.onMessage` for the lowest overhead). |
| `metrics` | Counters, last error, round trip, transport in use. |

### Reconnects

Full-jitter exponential backoff between `minDelay` and `maxDelay`. The
client stops by itself when retrying cannot help: a handshake rejected with
4xx (except 408, 425, 429), or close codes 1002, 1003, 1007–1010. Override
with `WebSocketReconnect(shouldReconnect: ...)`. Refresh tokens per attempt
with `WebSocketOptions.request`, and authenticate before the client reports
`open` with `onOpen`.

### Platforms

| Platform | Transport |
|---|---|
| Android, iOS, macOS, Windows, Linux | pure Dart (RFC 6455 on raw sockets), or Rust with `ws_native` |
| Web (dart2js, dart2wasm) | the browser's `WebSocket` |

**Flutter, `dart build cli`:** add `ws_native`. Its build hook bundles the
library for each target, and the libraries ship inside the package, so no
Rust toolchain or network is needed at build time.

```dart
import 'package:ws_native/ws_native.dart';

void main() {
  WsNative.install(); // returns false (pure Dart is used) if unavailable
  runApp(const App());
}
```

Opt out per app:

```yaml
hooks:
  user_defines:
    ws_native:
      source: none # none | build | prebuilt
```

**`dart compile exe` (servers, Docker):** `dart compile exe` refuses
packages with build hooks, so don't add `ws_native`. Download the library
of the same `ws` release instead, with its SHA-256 checked, and load it by
path:

```dockerfile
RUN dart pub get
RUN dart run ws:install --target linux-x64 --out /app/lib
RUN dart compile exe bin/server.dart -o /app/server
```

```dart
import 'package:ws/native.dart';

void main() {
  WsNativeLibrary.open('/app/lib/libws_native.so').install();
  // ...
}
```

### Testing your code

```dart
import 'package:ws/testing.dart';

final server = FakeWebSocketServer();
final client = WebSocketClient.text(
  options: WebSocketOptions(transport: server),
);
await client.connect('ws://test');
server.last.sendText('hello');   // to the client
print(server.last.received);    // from the client
server.last.drop();             // simulate a lost connection
```

## Documentation

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — layers and the rules per layer
- [docs/DECISIONS.md](docs/DECISIONS.md) — every compared alternative, with numbers
- [docs/ROADMAP.md](docs/ROADMAP.md) — plan, open checks, milestones
- [docs/NATIVE_DISTRIBUTION.md](docs/NATIVE_DISTRIBUTION.md) — how Dart packages ship native code
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) — tools and tasks (`mise run check`), releasing
- [benchmark/RESULTS.md](benchmark/RESULTS.md) — transport measurements
