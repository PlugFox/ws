# WS: Cross-platform WebSocket client

[![Pub](https://img.shields.io/pub/v/ws.svg)](https://pub.dev/packages/ws)
[![Actions Status](https://github.com/PlugFox/ws/actions/workflows/checkout.yml/badge.svg)](https://github.com/PlugFox/ws/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-purple.svg)](https://opensource.org/licenses/MIT)
[![GitHub stars](https://img.shields.io/github/stars/plugfox/ws?style=social)](https://github.com/plugfox/ws/)

A WebSocket client for Dart and Flutter on every platform, built for
reliability: reconnects with backoff, heartbeats, request/response calls,
typed states and errors, and a fake server for your tests.

> **2.0 is a rewrite** (`2.0.0-dev`). The design, the measurements behind
> it and the plan are in [`docs/`](https://github.com/plugfox/ws/tree/master/docs).

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
| `reconnect()` | Drops the connection and connects again now. |
| `disconnect()` | Closes; `connect` can be called again. |
| `close()` | Closes for good and releases everything. |
| `suspend()` / `resume()` | Pause for the app background; on return check the connection and reconnect at once. |
| `state` / `states` | `disconnected`, `connecting`, `open`, `reconnecting`, `disconnecting`, `suspended`, `closed`. |
| `messages` | Incoming messages (or `WebSocketOptions.onMessage` for the lowest overhead). |
| `metrics` | Counters, last error, round trip, transport in use. |

### Reconnects

Exponential backoff with jitter above a `minDelay` floor, capped at
`maxDelay`. The client stops by itself when retrying cannot help: a
handshake rejected with 4xx (except 408, 425, 429), a proxy 407, or close
codes 1002, 1003, 1007–1010. After close 1013 (try again later) and a
rejected certificate it waits `maxDelay`, after `Retry-After` (429, 503)
at least what the server asked. Redirects (3xx) are reported, not
followed, and retried. Override with
`WebSocketReconnect(shouldReconnect: ...)`. Refresh tokens per attempt
with `WebSocketOptions.request`, and authenticate before the client reports
`open` with `onOpen`.

### Background and network changes

Android and iOS freeze apps in the background and kill their sockets
without telling either side. Call `client.suspend()` when the app goes to
the background: the heartbeat and reconnects stop, and an open connection
stays open (`suspend(disconnect: true)` closes it). Call `client.resume()`
when it returns, and when the network changes: one ping checks the
connection, and a dead one is replaced at once, without the backoff. In
Flutter, [`ws_flutter`](https://pub.dev/packages/ws_flutter) does both.

No platform lets a socket live in the background for long: iOS gives
seconds, Android cuts the network in Doze. Wake the app with a push message
(APNs, FCM), reconnect and fetch what was missed. In browsers, hidden tabs
keep their connection, but timers are throttled to once a minute: a
heartbeat may come late, so prefer server pings there (the browser answers
them itself).

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
  if (!WsNative.install()) {
    // Pure Dart is used; say why (unsupported target, build skipped...).
    debugPrint('ws_native: ${WsNative.unavailableReason}');
  }
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
import 'dart:io';

import 'package:ws/native.dart';

void main() {
  try {
    WsNativeLibrary.open('/app/lib/libws_native.so').install();
  } on Object catch (e) {
    // Missing file, another architecture, musl, or a library from another
    // ws release (ABI mismatch): the pure-Dart transport keeps working.
    stderr.writeln('ws: native transport unavailable: $e');
  }
  // ...
}
```

Run the installer again after every `ws` upgrade: the library must come
from the same release.

The library contains Rust crates whose licences ask for their notices next
to binary copies: ship `THIRD_PARTY_LICENSES` from the same GitHub release
(the installer prints its URL).

### Certificates

Native platforms check certificates against the system's trust store
(`dart:io` or the OS verifier); the browser decides on the web. Options
in `WebSocketVmOptions.tls`:

```dart
const tls = WebSocketTls(
  // The server's key must match one of these (leaf only). Keep a backup.
  pins: [
    'sha256/p0dAcXTqjb1pOyqx9b1+gv64MBFcxSTFRp0WzCzKgG8=',
    'sha256/if3U7Jiwl3/2MhGRAt/JaFJabO8K+rGnjmQ/ny4D3L4=',
  ],
  // Android: also trust user and MDM CAs (corporate TLS inspection).
  trustUserCertificates: true,
);
```

Android's `network_security_config` and Apple's ATS do not apply to this
library: it opens its own sockets. See [docs/SECURITY.md](https://github.com/plugfox/ws/blob/master/docs/SECURITY.md).

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

- [docs/ARCHITECTURE.md](https://github.com/plugfox/ws/blob/master/docs/ARCHITECTURE.md) — layers and the rules per layer
- [docs/DECISIONS.md](https://github.com/plugfox/ws/blob/master/docs/DECISIONS.md) — every compared alternative, with numbers
- [docs/SECURITY.md](https://github.com/plugfox/ws/blob/master/docs/SECURITY.md) — trust stores, pins, platform policies that do not apply
- [docs/ROADMAP.md](https://github.com/plugfox/ws/blob/master/docs/ROADMAP.md) — plan, open checks, milestones
- [docs/NATIVE_DISTRIBUTION.md](https://github.com/plugfox/ws/blob/master/docs/NATIVE_DISTRIBUTION.md) — how Dart packages ship native code
- [docs/DEVELOPMENT.md](https://github.com/plugfox/ws/blob/master/docs/DEVELOPMENT.md) — tools and tasks (`mise run check`), releasing
- [benchmark/RESULTS.md](https://github.com/plugfox/ws/blob/master/benchmark/RESULTS.md) — transport measurements
