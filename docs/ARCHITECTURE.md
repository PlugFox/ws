# Architecture

How `ws` 2.0 is put together, and the rules each layer must follow. The
reasons behind the choices are in [DECISIONS.md](DECISIONS.md) (referred to
as D*n*); the plan and open work in [ROADMAP.md](ROADMAP.md).

## Layers

```
 app code
    │  WebSocketClient<T>.binary() / .json() / .text()
    ▼
┌─────────────────────────────── lib/src/client ───────────────────────────────┐
│ client.dart   state machine · reconnect · heartbeat · rpc · metrics          │
│ codec.dart    T ⇄ transport payloads (bytes / String / decoded JSON)         │
│ options, state, exceptions, rpc, metrics   ← public data types               │
└──────────────────────────────────┬───────────────────────────────────────────┘
                                   │ TransportRequest / TransportListener /
                                   │ TransportConnection   (lib/transport.dart)
         ┌─────────────────────────┼─────────────────────────┬──────────────────┐
         ▼                         ▼                         ▼                  ▼
 transport/dart (VM)      transport/web (browser)     native (FFI)       transport/fake
 RawSocket + protocol/    package:web WebSocket       Rust library via   in-memory server
 frame reader/writer,                                 function pointers  for tests
 handshake                                            (D30)
         │
         ▼
 lib/src/protocol   pure Dart, no dart:io: frames, handshake, UTF-8, SHA-1
```

- `lib/ws.dart`: the public API. `lib/testing.dart`: the fake server.
  `lib/transport.dart`: the transport interface and registry, for
  transport packages. `lib/native.dart` (VM only): the Rust transport,
  `WsNativeLibrary.open(path)` and `NativeSymbols`.
- `bin/install.dart` (`dart run ws:install`) downloads the released
  library for `dart compile exe` apps (D32).
- The platform transport is chosen by one conditional import in
  `transport_registry.dart` (`dart.library.js_interop` → web, otherwise
  `dart.library.io` → pure Dart). Nothing else is conditional; options
  are plain data on every platform (D16). `dart:ffi` is reachable only
  through `lib/native.dart`, never from `lib/ws.dart` (dart2wasm rejects
  it).
- `WebSocketTransports.use(t)` replaces the default for new clients; that
  is how `WsNativeLibrary.install()` plugs in.

## Native library: one transport, two ways to load it

```text
 ws_native (hook bundles the library)      app built with dart compile exe
   @Native externals (asset id)              dart run ws:install → file
   Native.addressOf(...) ──┐                 DynamicLibrary.open + lookup
                           ▼                          │
                     NativeSymbols  ◄─────────────────┘
                           │  asFunction once
                           ▼
              WsNativeLibrary → NativeTransport (package:ws)
```

- No `@Native` in `ws`: such functions bind only through the asset id, the
  embedder's resolver or the process, not to a library opened by path. So
  every call goes through function pointers, and pointers into Dart
  `Uint8List`s cannot be passed. Bytes are copied into buffers from
  `ws_native_alloc`, and Rust takes ownership (ABI 2, D30).
- `ws_native` contains only the hook, the `@Native` declarations and
  `WsNative.install()`. `WebSocketOptions.transport` overrides it
  per client.

## Client core

One implementation (`WebSocketClientImpl<T>`) for every transport and
message type. Each connection attempt is a *session* object that is also
the transport listener. When a session is replaced (reconnect, URL switch,
disconnect), the client simply stops recognizing it: every callback checks
`identical(_session, this)` first, so late events from an old connection
can never affect the new one. This replaces the "generation counter" and
the per-connection `disconnected` flag of the maintainer's SSE client;
the session's `dispose()` is its teardown stack (timers, waiters).

State machine (also in the `WebSocketState` docs):

```
disconnected ──connect()──► connecting ──open + onOpen hook──► open
     ▲                        │  ▲                               │
     │ terminal failure       │  └── timer ── reconnecting ◄─────┤ lost
     ├────────────────────────┘                                  │
     └──────────── disconnecting ◄──────── disconnect() ─────────┘
close() from anywhere ──► closed (terminal)
```

Rules the core follows:

- **Exactly one terminal event per session** (`onClosed`), then the
  session is dead. Transports guarantee it; the core relies on it.
- **No synchronous re-entry**: transports never call the listener from
  inside a `TransportConnection` method (`abort()` reports in a
  microtask). The core queues state events so user callbacks may call
  back into the client (D22).
- **Callbacks are guarded**: a throwing `onMessage`, `onStateChange`,
  `isPong` or RPC `match` goes to `onError`; it never breaks the client.
- **Nothing is replayed**: no buffered pushes or RPC calls survive a
  disconnect (D12, D13). Pending RPC calls fail with a typed reason.
- **The backoff resets only when a connection proved stable**
  (`stableAfter` or a received message), so accept-then-close servers are
  not hammered (D14).
- **Time** comes from `package:clock` so everything is testable in fake
  time (D23).

## Transport contract

`TransportListener` callbacks, in order: `onOpen` at most once, then any
number of messages / `onPong` / `onDrain`, then exactly one `onClosed`.

| Callback | Pure Dart | Web | Native | Fake |
|---|---|---|---|---|
| text | `onTextBytes` (zero-copy view) | `onTextString` | `onJson` (tape) or `onTextBytes` | `onTextBytes` |
| binary | `onBinary` | `onBinary` (ArrayBuffer view) | `onBinary` (external typed data) | `onBinary` |
| pong | protocol | — (no browser API) | protocol, handled in Rust | `autoPong` |
| drain | write queue emptied | polled `bufferedAmount` | credit / queue event | `drain()` |
| handshake failure | `WebSocketHandshakeException` with status | `WebSocketConnectionException` (browser hides it) | with status | `reject(status)` |

Text validation: fragmented text is validated incrementally by the frame
reader; single-frame text is validated by the decoder, and invalid UTF-8
closes the connection with 1007 (D5).

## Protocol layer

`lib/src/protocol` is pure Dart with no I/O: `FrameReader` (incremental
parser, zero-copy views, limits before allocation), `FrameWriter`
(masking with pooled secure keys, 32-bit masking), `HandshakeRequest` /
`HandshakeResponse`, `Utf8Validator` (DFA with a 64-bit ASCII scan on the
VM), `decodeUtf8`, `sha1`. It is shared by the pure-Dart transport and the
tests, and runs on dart2js and dart2wasm (no 64-bit-only code on the web
path). Performance numbers: D4–D6, D19, D20.

## Testing

| Layer | Where | Runs on |
|---|---|---|
| protocol | `test/protocol` | VM, Chrome (dart2js), Chrome (dart2wasm) |
| client core | `test/client` over `FakeWebSocketServer`, `fake_async` | VM, dart2js, dart2wasm |
| pure-Dart transport | `test/transport/dart_transport_test.dart`: `conformance.dart` (against `dart:io` and a raw socket server) and `server_suite.dart` | VM |
| native transport | `test/native` (loaded by path) and `native/test` (bundled by the hook): the same two suites | VM |
| Rust test server suite | `server_suite.dart` (TLS, proxy, handshake faults; VM only) + `server_shared.dart` (close, traffic, protocol violations, liveness; every transport, D46) | VM, dart2js, dart2wasm |
| web transport | `test/transport/web_server_test.dart` (the shared half, server in a hybrid isolate) and `web_transport_test.dart` | dart2js, dart2wasm |

Autobahn TestSuite results for the pure-Dart and native transports are in
D28. Still to come (ROADMAP §6): randomized operation sequences over the
fake, native lifecycle and soak tests.
