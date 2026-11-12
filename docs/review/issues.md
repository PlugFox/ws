# GitHub issues and the 1.0.0 reports against 2.0

Open issues at github.com/PlugFox/ws/issues on 2026-11-13, plus an
outside report on the published ws 1.0.0 (two defects, one limitation),
each checked against branch `rewrite`. "Test" names the regression test.

| Item | 1.x cause | 2.0 status | Test |
|---|---|---|---|
| #24 reconnect stuck in `connecting` | `Future.timeout` around `WebSocket.connect`; the manager retried only from `closed` | Fixed. Each attempt has one `connectTimeout` budget and is aborted; the next attempt is scheduled. TLS-stage leak on pure Dart found and fixed (D54) | `server_suite.dart` "abandoned connect attempts", both VM transports |
| Report: timeout leaves sockets open | same; `close()` only closed a finished socket | Fixed (D54); measured on the server side, not by `lsof` | same group: `ws`, `wss`, `close()` during the handshake, late 101 |
| Report: `WSInterceptor.wrap` error escapes `await add()` | `async` `onSend` behind a `void` chain | Not applicable: no interceptors. Send-path user code fails only its call | `client_edge_test.dart` "a throwing toJson fails that push, not the zone"; rpc encode and ping builder already covered |
| Report: silent network loss not detected | no ping, reconnect only on `closed` | Heartbeat on by default (30 s idle, 10 s timeout), protocol pings on VM; TCP keepalive 30 s | `server_shared.dart` liveness: silent server dropped and reconnected, a live one keeps its socket |
| #25 iOS: no state change after a Wi-Fi switch | same as above | As above, plus `resume()` on foreground and on network changes in `ws_flutter` (`networkChanges:`) | liveness tests; field test on devices still open (T-resume) |
| #26 expose `pingInterval` | dart:io ping not exposed | Superseded by `WebSocketHeartbeat` (interval, timeout, app-level ping for browsers) | liveness tests |
| #23 `web ^1.1.1` | 1.1.0-dev.1 pinned `web ^0.5.1` | Fixed: `web: ^1.1.0` | resolution |
| #19 no messages under WASM | — (not reproduced on 1.x here) | The shared server suite runs in Chrome under dart2js and dart2wasm on CI (601 tests) | `web_server_test.dart`, `web_transport_test.dart` |

## Limits that stay

- Browsers cannot send protocol pings. Without an application-level ping
  (`WebSocketHeartbeat.ping`/`isPong`), a silent connection in the
  browser is found only when the browser itself notices.
- `open` and a completed `push()` mean the local side accepted the
  message, not that the remote application got it. Delivery guarantees
  need application acks; `rpc()` gives a reply per request.

## Follow-up

- README "Dead connections and delivery": the browser needs an
  application-level heartbeat; an own ping/pong through `rpc()` is
  recommended on every platform, and `rpc()` replies are the way to
  acknowledge delivery.
- Issues #19, #23, #24, #25, #26 closed on 2026-11-13 with a comment each.
- dart2wasm JSON decoding with asserts (D55):
  https://github.com/dart-lang/sdk/issues/64924.
