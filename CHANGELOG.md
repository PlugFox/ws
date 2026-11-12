## 2.0.0-dev.1

- **BREAKING**: complete rewrite. New API: `WebSocketClient.binary()`,
  `.json()`, `.text()` with `push`, `rpc`, `ping`, `connect`, `reconnect`,
  `disconnect`, `close`; sealed states and exceptions; plain-data options
  with `vm`/`js` parts.
- New pure-Dart RFC 6455 transport on `RawSocket` (no `dart:io`
  `WebSocket`), browser transport on `package:web` (dart2js and dart2wasm).
- Reconnect with full-jitter backoff and terminal-failure detection,
  heartbeat, request/response correlation, metrics.
- Dead connections are detected by default: heartbeat on (30 s idle,
  10 s to answer; a draining upload extends the wait) and TCP keepalive
  (`WebSocketVmOptions.keepAlive`, 30 s; `TCP_USER_TIMEOUT` on Linux and
  Android).
- `WebSocketConnectionException.failure` (`dns`, `refused`,
  `unreachable`, `tls`, `proxy`, `lost`, `other`), `.tls` (`untrusted`,
  `hostname`, `expired`, `notYetValid`, `revoked`, `handshake`) and
  `.proxyStatus`. Rejected certificates are retried at the maximum
  reconnect delay; a proxy 407 stops reconnecting.
- `suspend()` and `resume()` for the app lifecycle: no heartbeat and no
  reconnects while suspended (an open connection stays open, or is closed
  with `suspend(disconnect: true)`); `resume()` checks the connection with
  one ping and replaces a dead one at once, without the backoff. Also for
  network changes. New state `WebSocketState.suspended`. The `ws_flutter`
  package calls them from the Flutter app lifecycle.
- `WebSocketTls.pins` (SHA-256 of the server key, leaf only, checked
  after the chain; mismatch: `WebSocketTlsFailure.pinMismatch`) and
  `WebSocketTls.trustUserCertificates` (Android: also trust user and MDM
  CAs). On Android both transports drop the system CAs the user disabled.
  TLS settings are built once per distinct value and reused.
- After `close()` every transport refuses sends and pings the same way.
  An explicit `Authorization` header wins over URL user info (one header
  is sent). A throwing transport listener loses no messages.
- Web: `maxMessageSize` is enforced (1009, the message is not
  delivered); a dropped connection reports no close code, like the other
  transports; an invalid close code from the server is a protocol
  violation (1002).
- `package:ws/testing.dart`: in-memory `FakeWebSocketServer` for tests.
- `package:ws/native.dart`: the Rust transport loaded from a file
  (`WsNativeLibrary.open(path).install()`), and `dart run ws:install` to
  download the released library with a SHA-256 check, for executables built
  with `dart compile exe`; it prints where the libraries' third-party
  notices are. Flutter and `dart build cli` apps use the
  `ws_native` package instead, which bundles the library through a build
  hook.

## 1.1.0-dev.1

- Migration from 'dart:html' to 'package:web'

## 1.0.0

- Stable release
- Add interceptors for incoming and outgoing messages
- Add possibility to send messages right after connection

## 1.0.0-pre.6

- **BREAKING CHANGE**: Change options to separate, platform-specific object
- You can now pass headers and other options to the IO web socket client
- Backoff full jitter strategy for reconnection attempts
- Add more metrics
- Public Fake WebSocket client for testing

## 0.1.2

- Update README.md

## 0.1.0

- Initial release
