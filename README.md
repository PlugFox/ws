# WS: Cross-platform WebSocket client

> [!WARNING]
> **`2.0.0-dev` is a development pre-release of a complete rewrite.**
> Try it, measure it, report what breaks, but weigh these risks before
> shipping it to production:
>
> - **The API may still change** before `2.0.0`, including breaking changes
>   without a deprecation period. Pin an exact version (`ws: 2.0.0-dev.1`)
>   and read the [CHANGELOG](https://github.com/plugfox/ws/blob/master/CHANGELOG.md)
>   before every upgrade.
> - **Not yet proven in the field**: no long soak tests (hours of traffic,
>   memory and file descriptors), no network-fault suite (packet loss,
>   half-open connections), the background/resume behaviour has not been
>   field-tested on devices for long periods.
> - **`ws_native` ships prebuilt Rust libraries** for 12 targets. App Store
>   / TestFlight upload validation and Google Play's 16 KB page alignment
>   are not verified yet. If the library cannot load, the client falls back
>   to the pure-Dart transport, so check `WsNative.unavailableReason`.
> - **Not compatible with 1.x**: there is no migration guide yet. Stay on
>   `ws: ^1.0.0` if you cannot spend time on the migration.
>
> Status and the open items: [docs/ROADMAP.md](https://github.com/plugfox/ws/blob/master/docs/ROADMAP.md).
> Issues: [github.com/plugfox/ws/issues](https://github.com/plugfox/ws/issues).

[![Pub](https://img.shields.io/pub/v/ws.svg)](https://pub.dev/packages/ws)
[![Actions Status](https://github.com/PlugFox/ws/actions/workflows/checkout.yml/badge.svg)](https://github.com/PlugFox/ws/actions)
[![License: MIT](https://img.shields.io/badge/license-MIT-purple.svg)](https://opensource.org/licenses/MIT)
[![GitHub stars](https://img.shields.io/github/stars/plugfox/ws?style=social)](https://github.com/plugfox/ws/)

A WebSocket client for Dart and Flutter on every platform, built for
reliability: reconnects with backoff, heartbeats, request/response calls,
typed states and errors, and a fake server for your tests. With the
optional Rust transport `ws_native` it is
[4–32× faster at receiving](#performance) than `web_socket_channel` on desktop and
mobile.

The design, the measurements behind it and the plan are in
[`docs/`](https://github.com/plugfox/ws/tree/master/docs).

## Install

Requirements: Dart 3.10+ (Flutter 3.38+).

| Package | What | When |
|---|---|---|
| [`ws`](https://pub.dev/packages/ws) | The client, with a pure-Dart transport on native platforms and the browser's `WebSocket` on the web | Always |
| [`ws_native`](https://pub.dev/packages/ws_native) | The Rust transport: TLS, framing and JSON parsing off the Dart thread | Android, iOS, macOS, Windows, Linux apps; recommended on mobile |
| [`ws_flutter`](https://pub.dev/packages/ws_flutter) | Suspends the client in the background, checks and reconnects on resume and on network changes | Flutter apps on Android and iOS |

Pub picks pre-releases only when asked, so give the version explicitly.

**Flutter app:**

```sh
flutter pub add ws:2.0.0-dev.1 ws_native:2.0.0-dev.1 ws_flutter:2.0.0-dev.1
```

**Dart (CLI, server, `dart build cli`):**

```sh
dart pub add ws:2.0.0-dev.1 ws_native:2.0.0-dev.1
```

**Web only:** `ws` alone; `ws_native` does nothing in the browser.

Or in `pubspec.yaml`:

```yaml
dependencies:
  ws: 2.0.0-dev.1
  ws_native: 2.0.0-dev.1   # optional: the Rust transport
  ws_flutter: 2.0.0-dev.1  # optional: Flutter app lifecycle
```

Then install the native transport once at startup (it becomes the default
for every client created afterwards):

```dart
import 'package:flutter/widgets.dart';
import 'package:ws_native/ws_native.dart';

void main() {
  if (!WsNative.install()) {
    // Pure Dart is used; say why (unsupported target, build skipped...).
    debugPrint('ws_native: ${WsNative.unavailableReason}');
  }
  runApp(const App());
}
```

- `ws_native` bundles prebuilt libraries for Android (arm64, armv7, x64),
  iOS (device, simulator arm64 and x64), macOS (arm64, x64), Windows (x64,
  arm64) and Linux glibc (x64, arm64). Its build hook adds the one for the
  target: no Rust toolchain and no network at build time.
- **iOS:** declare the file-timestamp API in your app's privacy manifest
  (see the [`ws_native` README](https://pub.dev/packages/ws_native)).
- **`dart compile exe`** cannot use packages with build hooks: see
  [Platforms](#platforms) for `dart run ws:install`.
- Opt out per app with `hooks: user_defines: ws_native: source: none`.

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

### Dead connections and delivery

A connection can die without a FIN or RST: a Wi-Fi switch, a NAT that
forgot it, a server that hangs. The client then still says `open`. The
heartbeat finds it: after `interval` (30 s) without incoming traffic it
sends a ping and drops the connection, then reconnects, if nothing
arrives within `timeout` (10 s). On native platforms that ping is a
protocol ping by default. **Browsers cannot send protocol pings**: there
the heartbeat works only with an application-level ping, so without one a
dead connection in the browser is found only when the browser notices.

Implement your own ping/pong in any case. A protocol pong comes from the
server's WebSocket layer and proves the socket, not that the server
application still answers. An application ping works the same on every
platform:

```dart
final client = WebSocketClient.json(
  options: WebSocketOptions(
    rpc: const WebSocketJsonRpc(),
    heartbeat: WebSocketHeartbeat(
      ping: () => {'type': 'ping'}, // the server answers {"type": "pong"}
      isPong: (message) => message['type'] == 'pong',
    ),
  ),
);

// Or an end-to-end check through the request path, with its own timeout:
await client.rpc({'method': 'ping'}, timeout: const Duration(seconds: 5));
```

`open` and a completed `push()` mean the local side accepted the message,
not that the server application processed it. When that matters, send it
with `rpc()` and treat the reply as the acknowledgement; on a timeout or a
disconnect, decide whether to send it again (make such requests
idempotent, e.g. with your own message id). Nothing is resent
automatically.

### Platforms

| Platform | Transport |
|---|---|
| Android, iOS, macOS, Windows, Linux | pure Dart (RFC 6455 on raw sockets), or Rust with `ws_native` |
| Web (dart2js, dart2wasm) | the browser's `WebSocket` |

**Flutter, `dart build cli`, `dart run`, `dart test`:** add `ws_native`
and call `WsNative.install()` at startup ([Install](#install)). Its build
hook bundles the library for each target.

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

## Performance

`ws` against [`web_socket_channel`](https://pub.dev/packages/web_socket_channel),
the most used WebSocket package on pub.dev, on the same benchmark. Full
tables (CPU, memory, latency percentiles, size × count grid, run-to-run
spread): [benchmark/COMPARISON.md](https://github.com/plugfox/ws/blob/master/benchmark/COMPARISON.md);
how it was measured: [benchmark/METHODOLOGY.md](https://github.com/plugfox/ws/blob/master/benchmark/METHODOLOGY.md).

| | |
|---|---|
| Date | 2026-11-16 / 17 |
| Packages | `ws` 2.0.0-dev.1, `ws_native` 2.0.0-dev.1, `web_socket_channel` 3.0.3 |
| Toolchain | Dart 3.14.0 (AOT), Flutter 3.47.9 (release), Rust 1.100.0 |
| macOS | MacBook Pro, Apple M3 Max (14 cores, 36 GB), macOS 27.0.1 |
| iPad | iPad Pro 11" (4th gen), Apple M2, iPadOS 26.6.1 |
| Android | Samsung Galaxy S22 Ultra (SM-S908B), Exynos 2200, Android 16 |
| Browser | Chrome 157.0.8103.52 (headless) on the same Mac, dart2js `-O4` and dart2wasm `-O2` |
| Setup | wss (TLS 1.3) over loopback to a Rust server (tokio + tungstenite + rustls); on the iPad and the phone the server runs inside the app. Median of 3 runs |

Columns: **ws** is `WebSocketClient.json` / `.binary` with the built-in
transport (pure Dart on native platforms, the browser's `WebSocket` on the
web), **ws + ws_native** the same client with the Rust transport,
**wsc** `web_socket_channel` with `jsonEncode` / `jsonDecode`. Loopback
measures the client's own cost: on a real network the link adds latency
and caps throughput, the CPU savings remain.

### Receive throughput, MB/s

The server sends as fast as it can (flood); higher is better.

| Platform | Messages | ws | ws + ws_native | wsc |
|---|---|---:|---:|---:|
| macOS | JSON 14 KB | 122.8 | **581.0** | 108.6 |
| macOS | JSON 14 KB → typed models | 108.9 | **449.8** | 97.0 |
| macOS | binary 60 KB | 231.4 | **2 962.7** | 227.4 |
| iPad | JSON 14 KB | 63.1 | **486.9** | 58.6 |
| iPad | JSON 14 KB → typed models | 58.4 | **400.6** | 54.6 |
| iPad | binary 60 KB | 94.5 | **1 755.9** | 93.4 |
| Android | JSON 14 KB | 14.6 | **144.1** | 14.0 |
| Android | JSON 14 KB → typed models | 14.1 | **113.8** | 13.7 |
| Android | binary 60 KB | 16.4 | **531.1** | 16.8 |
| Chrome, dart2js | JSON 14 KB | 306.7 | — | 299.4 |
| Chrome, dart2js | binary 60 KB | 2 230.1 | — | 2 308.5 |
| Chrome, dart2wasm | JSON 14 KB | 209.4 | — | 206.8 |
| Chrome, dart2wasm | binary 60 KB | 2 214.1 | — | 2 302.4 |

"→ typed models" maps every message to model classes (all fields), as an
app does. `ws_native` parses JSON in Rust (simd-json) and hands Dart lazy
views, so reading only a few fields is cheaper still.

### UI thread cost

The server sends JSON 14 KB at 500 messages/s; how much main-isolate time
each message takes (receiving, TLS, decoding). This is what costs frames in
a Flutter app; lower is better. ≈0: within the measurement noise.

| Platform | ws | ws + ws_native | wsc |
|---|---:|---:|---:|
| macOS | 95 µs (4.8 %) | **≈0** | 110 µs (5.5 %) |
| iPad | 119 µs (6.0 %) | **≈0** | 135 µs (6.7 %) |
| Android | 432 µs (21.6 %) | **126 µs (6.3 %)** | 428 µs (21.4 %) |

Delivery latency at the same rate, p50 / p99: macOS 788 / 1 525 µs (ws),
**152 / 242 µs** (ws + ws_native), 814 / 1 573 µs (wsc); Android 1 875 /
4 324, **828 / 1 385**, 2 082 / 4 606 µs.

### Request/response (spinify's method)

The client sends a message, waits for the echo, then sends the next one,
10 000 times. Round trips per second (MB/s one way); higher is better.
[spinify](https://pub.dev/packages/spinify) counts sent plus received
messages, so its numbers are twice these for the same run.

| Platform | Messages | ws | ws + ws_native | wsc |
|---|---|---:|---:|---:|
| macOS | JSON 1 KB | 11 894 (12.7) | **20 604 (22.0)** | 10 706 (11.5) |
| macOS | JSON 14 KB | 3 123 (44.7) | **5 577 (79.9)** | 2 504 (35.9) |
| macOS | binary 14 KB | 7 064 (101.3) | **18 515 (265.4)** | 7 057 (101.2) |
| macOS | binary 60 KB | 1 988 (122.1) | **8 361 (513.7)** | 1 996 (122.7) |
| iPad | JSON 14 KB | 1 494 (21.4) | **4 498 (64.4)** | 1 321 (18.9) |
| iPad | binary 60 KB | 869 (53.4) | **5 959 (366.1)** | 873 (53.6) |
| Android | JSON 14 KB | 440 (6.3) | 418 (6.0) | 392 (5.6) |
| Android | binary 14 KB | 512 (7.3) | **2 351 (33.7)** | 510 (7.3) |
| Android | binary 60 KB | 138 (8.5) | **1 519 (93.3)** | 138 (8.4) |
| Chrome, dart2js | JSON 14 KB | 4 869 (69.8) | — | 4 865 (69.7) |
| Chrome, dart2js | binary 60 KB | 7 320 (449.7) | — | 7 148 (439.2) |
| Chrome, dart2wasm | JSON 14 KB | 3 862 (55.3) | — | 3 629 (52.0) |
| Chrome, dart2wasm | binary 60 KB | 4 866 (299.0) | — | 4 857 (298.4) |

### What the numbers say

- **ws + ws_native** is 4–13× faster than `web_socket_channel` on macOS,
  6–19× on the iPad and 6–32× on Android at receiving, and uses 2–19× less
  CPU per message. TLS, framing and JSON parsing run on Rust threads, so
  the UI isolate stays free.
- **ws without ws_native** is never slower than `web_socket_channel`
  beyond the run-to-run noise (it is 0–13 % faster). Both sit on
  `dart:io`'s TLS, which caps every Dart client on Android at about
  15 MB/s; `ws_native` does not hit that ceiling.
- **In browsers** both use the browser's `WebSocket`: equal from 14 KB up,
  `ws` 6–24 % faster on 1 KB messages (a synchronous `onMessage` instead of
  a `Stream`). dart2wasm decodes JSON slower than dart2js.
- **Sending large JSON on slow phones** is bound by encoding it on the
  main isolate, the same in every client (Android JSON 14 KB above: about
  450 µs per request on the Exynos' A710 cores). Use binary messages for
  heavy traffic there: that is where `ws_native` gains most (3–11× in
  request/response, 32× in throughput).
- **Message size and count**: throughput in MB/s grows with message size
  until TLS and decoding become the limit (from about 30 KB for the Dart
  clients); the first 100 round trips run at about 30–100 % of the steady speed
  (the smaller the message, the lower; browsers also warm up their JIT). Tables in
  COMPARISON.md.

## Documentation

- [docs/ARCHITECTURE.md](https://github.com/plugfox/ws/blob/master/docs/ARCHITECTURE.md) — layers and the rules per layer
- [docs/DECISIONS.md](https://github.com/plugfox/ws/blob/master/docs/DECISIONS.md) — every compared alternative, with numbers
- [docs/SECURITY.md](https://github.com/plugfox/ws/blob/master/docs/SECURITY.md) — trust stores, pins, platform policies that do not apply
- [docs/ROADMAP.md](https://github.com/plugfox/ws/blob/master/docs/ROADMAP.md) — plan, open checks, milestones
- [docs/NATIVE_DISTRIBUTION.md](https://github.com/plugfox/ws/blob/master/docs/NATIVE_DISTRIBUTION.md) — how Dart packages ship native code
- [docs/DEVELOPMENT.md](https://github.com/plugfox/ws/blob/master/docs/DEVELOPMENT.md) — tools and tasks (`mise run check`), releasing
- [benchmark/COMPARISON.md](https://github.com/plugfox/ws/blob/master/benchmark/COMPARISON.md) — `ws` vs `web_socket_channel` on macOS, iPad, Android and Chrome
- [benchmark/METHODOLOGY.md](https://github.com/plugfox/ws/blob/master/benchmark/METHODOLOGY.md) — how the benchmarks are run and measured
- [benchmark/RESULTS.md](https://github.com/plugfox/ws/blob/master/benchmark/RESULTS.md) — transport prototypes, the measurements behind the design
