# Roadmap

This is the living plan for the `ws` rewrite (branch `rewrite`). It records
the decisions made so far, the evidence behind them, the work ahead, and what
still has to be checked. Update it whenever a decision is made, a check is
done or a new question comes up.

Status markers: `[x]` done, `[ ]` to do, `[~]` in progress or partly done,
`[?]` open question or decision.

---

## 1. Goals

- One WebSocket client for Dart and Flutter that is fast on mobile, desktop
  and server, and that does not load the UI (main) isolate.
- The old API does not have to be kept: the rewrite is a new major version
  (`2.0.0`). Good ideas from 1.x are kept on purpose (see §4.9).
- Production defaults: TLS (`wss://`), reconnects, heartbeats, flow control
  and clear errors.
- Web support from the same public API, with its own implementation.
- Optional: WebTransport, now that the native transport has proven itself
  (§7, M9).

## 2. Current hypotheses

The architecture below is the **best hypothesis so far**. It has already paid
off in measurements, but it is not final. The transport, the delivery path
between Rust and Dart, and the data structures (tape) are all open to
experiment (§5). The API must stay independent of them (§4.1), so a better
option can be swapped in at any time.

| Hypothesis | Status | Evidence |
|---|---|---|
| Transport in **Rust** over FFI on native platforms (today: tokio + tokio-tungstenite + rustls) | [x] confirmed, implementation open | `benchmark/RESULTS.md`. Mobile `wss`: 4.6–8× JSON throughput, 10–21× binary, ~2× less CPU per message, 5–8× lower latency |
| Fallback transport in **pure Dart** (own RFC 6455 codec on `RawSocket` / `RawSecureSocket`) | [x] accepted | Own codec is 20–50% faster than `dart:io` `WebSocket`, 2× on unicode. Covers platforms or setups without the native library |
| JSON decoding in Rust (simd-json) into a **tape**, read in Dart through lazy `Map` / `List` views | [~] best so far, format open | On mobile `d-tape` costs main as little as a Dart worker that builds models, with half the CPU |
| `Dart_CObject` trees for JSON | [x] rejected | Slowest variant everywhere: the VM rebuilds the tree on main |
| Map-building decoding in a Dart worker isolate (`c-worker*`) | [~] optional mode only | Frees main, but on Android it costs about 2× the CPU of decoding on main. Worth keeping as a decoder option for heavy model mapping |
| Binaries shipped with Dart **build hooks** (`package:hooks`, `code_assets`) | [x] works (built locally with cargo) | `benchmark/hook/build.dart` builds for macOS, Android and iOS. Prebuilt libraries bundled in the archive (D9). The hook lives in a separate package `ws_native` (D8) |
| Web: separate implementation over the browser `WebSocket` (`package:web` / JS interop) | [x] accepted | No sockets in the browser. Rust/WASM would not help with the browser's WebSocket |
| Flow control between Rust and Dart | [x] required | Without it RSS grew to 500–900 MB in flood tests |
| WebTransport | [?] optional, after 2.0 | Only practical through the Rust side (QUIC) |

## 3. Target architecture

This is the current shape. The boxes are swappable, and the experiments in
§5 may replace any of them.

```
                         ┌────────────────────── public API (Dart) ───────────────────────┐
                         │ WebSocketClient · state stream · messages · send · close        │
                         │ push · rpc · codecs · reconnect · heartbeat · metrics           │
                         └──────────────┬───────────────────────┬───────────────────┬─────┘
                                        │                       │                   │
                  native (FFI, opt-in: ws_native)   pure Dart (VM default)     web (browser)
               ┌────────────────────────────┐   ┌──────────────────────┐   ┌──────────────────┐
               │ Rust cdylib: tokio runtime │   │ RFC 6455 codec on    │   │ JS WebSocket via │
               │ tungstenite · rustls       │   │ RawSocket /          │   │ extension types  │
               │ simd-json → tape           │   │ RawSecureSocket      │   │ JSON.parse       │
               │ credit flow control        │   │ optional worker      │   │                  │
               └─────────────┬──────────────┘   └──────────────────────┘   └──────────────────┘
                             │ Dart_PostCObject (batched, external typed data, zero copy)
                             ▼
                 ReceivePort in main **or** in a worker isolate (user's choice)
```

Key properties:

- The transport is the platform's: pure Dart on the VM, the browser's on
  the web. `WsNative.install()` (package `ws_native`) or
  `WebSocketTransports.use` switches new clients to native (D8).
- All transports behave the same way: same states, events, close codes,
  errors and metrics. A shared conformance test suite enforces this (§6.2).
- Messages come in batches, once per socket read. Bytes and tapes are
  external typed data owned by Rust, released by a finalizer. Nothing is
  copied on the receiving side.

## 4. Design considerations

### 4.1 Public API

As shipped (README, DECISIONS.md D11–D13, D22); the earlier plan of
`sendText`/`sendBytes`/`sendJson`, per-connection decoders and
interceptors was dropped (see "Not in 2.0" below).

- [x] **The API does not depend on how things are done inside.**
  Transports sit behind `WebSocketTransport`; the tape and the FFI layout
  never reach users (JSON arrives as `Map`/`List` views).
- [x] One client type with three codecs: `WebSocketClient.text()`,
  `.binary()`, `.json()`; `push`, `rpc` (with `WebSocketRpc`), `ping`,
  `connect`, `reconnect`, `disconnect`, `close`, `suspend`, `resume`.
- [x] State as a sealed hierarchy with the current value and a stream
  (D22); errors as a sealed, frozen hierarchy with typed connection
  failures (D42).
- [x] Options as plain data: headers, subprotocols, timeouts, max message
  size, TLS (§4.5), proxy (§4.6), heartbeat, reconnect, send limits.
- [x] Testability: `FakeWebSocketServer` in `package:ws/testing.dart`.
- [x] Naming: the `ws` package and `WebSocketClient` stay (maintainer's
  choice).
- Not in 2.0: per-connection decoders (`isolate<T>`, protobuf),
  interceptors/middleware, a `done` future, `permessage-deflate`, a
  connection manager. A codec or `onMessage` covers the decoder cases.

### 4.2 Threading and isolates

- [x] One tokio runtime per process, shared by all isolates and connections.
  Default 2 worker threads, configurable (`install(threads:)`). Measure 1 vs 2 vs N on mobile.
- [ ] Messages can be delivered to a `SendPort` of any isolate. Rust posts
  straight to the worker when the worker decoder is used, and does not go
  through main.
- [x] Leaf FFI calls only for non-blocking work that never calls back into
  the VM (`requestDrain` is not one, D50). Never block the Dart thread in
  Rust.
- [x] Flutter **hot restart** (liveness probe and `NativeFinalizer`, D27): the native runtime and its connections outlive
  the Dart isolate. Connections must be detected and closed when their
  `ReceivePort` dies (`Dart_PostCObject` returns `false`), and with a
  `NativeFinalizer` on the connection handle.
- [ ] Isolate shutdown with open connections must not leak or crash.
- [ ] `isolate group` behaviour: several Flutter engines / isolates in the
  same process share the library.

### 4.3 Flow control (backpressure)

- [x] Credit-based (D24, per-message overhead D41): Dart grants N messages or N bytes. Rust stops reading the
  socket when credits run out, and TCP backpressure slows the server.
- [x] Credits are granted back automatically after the listener has handled a
  batch, even when it throws (D50). Applies to `Stream` pause/resume too.
- [x] Bound on the send queue: `sendBufferLimit` fails `push()` with
  `WebSocketBufferFullException` (DECISIONS.md D36).
- [ ] Same semantics in the pure-Dart transport: pause the
  `RawSocket` read events.
- [ ] Re-run the flood benchmarks: throughput must stay about the same and RSS
  must stay bounded.

### 4.4 Protocol coverage (RFC 6455 and friends)

- [x] Fragmentation, control frames between fragments, ping/pong, close
  handshake with codes and reasons, UTF-8 validation of text messages and
  close reasons (Autobahn, D28; server suite on every transport, D46).
- [ ] `permessage-deflate` (RFC 7692). tungstenite has limited or no
  support, so check the current status or a fork. `dart:io` supports it. Be
  careful about CPU and memory on mobile (context takeover).
- [x] Subprotocol negotiation (`Sec-WebSocket-Protocol`), custom handshake
  headers; response headers of a refused handshake in
  `WebSocketHandshakeException.headers`.
- [x] Max message size with a clear error (1009, every transport, D46).
- [x] Redirects during the handshake (3xx): reported, not followed, and
  retried (README "Reconnects", D48).
- [ ] HTTP/2 WebSockets (RFC 8441) — probably not needed. Note it and leave
  it out.

### 4.5 TLS

- [x] Certificate verification against the **platform trust store**
  (DECISIONS.md D35): OS verifier on Apple/Windows, system CA files on
  Linux and Android, `webpki-roots` only as a fallback.
- [x] Certificate pinning (SPKI hash) and custom CAs; Android user CAs
  on request, disabled system CAs honoured (DECISIONS.md D45).
- [x] An option to allow bad certificates, for development only. Never the
  default (`WebSocketTls.allowInsecure`; pins still apply). The benchmarks use an accept-any verifier, and it must not leak
  into the library.
- [ ] ALPN, SNI, TLS 1.2/1.3 support. Crypto provider: `ring` (as now) or
  `aws-lc-rs` — compare build complexity and binary size for mobile
  targets.
- [x] Pure-Dart transport: `SecureSocket` with `SecurityContext` options
  mapped from the same API (D35, D45).

### 4.6 Networking on mobile

- [~] **System proxy / VPN apps** (explicit HTTP CONNECT proxy done, D36;
  reading system settings open): raw sockets do not honour the system
  HTTP proxy (iOS `CFNetwork`, Android `ProxySelector`). At least support an
  explicit HTTP CONNECT proxy option. Decide whether to read the system
  proxy settings.
- [x] IPv6 and NAT64 (iOS review requirement): `getaddrinfo` + Happy
  Eyeballs (RFC 8305) in Rust, as `dart:io` does (DECISIONS.md D34). IPv4
  literals on NAT64 remain unsupported on both transports.
- [x] Network changes (Wi-Fi ↔ cellular): detect dead connections quickly
  with heartbeats and TCP keepalive, then reconnect. Both on by default
  (DECISIONS.md D41).
- [x] App lifecycle: iOS suspends sockets in the background. Reconnect on
  resume. Do not drain the battery with heartbeats in the background.
  `suspend()`/`resume()` and `ws_flutter` (DECISIONS.md D44). Still open:
  T-resume on real devices (30 s, 5 min, 2 h in the background).
- [x] App Store review: plain BSD sockets are fine; ATS does not apply
  (docs/SECURITY.md); the privacy-manifest entry for the iOS library is in
  the `ws_native` README (D53).

### 4.7 Distribution (native library)

- [x] Release workflow (D43): build every target in CI, put the libraries into
  `native/prebuilt/` with `checksums.json`, create the GitHub release (tag
  = version, assets `<target>-<file>`), run `tool/native_release.dart`,
  then publish `ws` and the staged `ws_native`. The libraries are bundled
  in the `ws_native` archive, not downloaded by the hook (D31). The same
  files on the GitHub release serve `dart run ws:install` (D32).
- [x] Apps built with `dart compile exe`: `dart run ws:install` and
  `WsNativeLibrary.open(path)` (D30, D32).
- [ ] Per-OS maps in the hook's user-defines, as `sqlite3` 3.6+ has (D31).
- [x] Targets: Android (arm64-v8a, armeabi-v7a, x86_64), iOS (device +
  simulator arm64/x64), macOS (arm64/x64), Linux (x64/arm64, glibc 2.17),
  Windows (x64/arm64): the 12 prebuilt targets (D40, D43). musl: no.
- [x] Android **16 KB page size** alignment (required for new apps on Google
  Play): `rust/build.rs`, checked by `tool/verify_native.sh` (DEVELOPMENT.md).
- [ ] Binary size budget. Today: about 2.1–2.3 MB stripped per ABI
  (macOS arm64 2,078,640 bytes, 1.06 MB gzipped; Android arm64 2,331,304
  in D35), after trimming crate features in D50. Try `opt-level="z"` for
  cold code, fewer dependencies, and a smaller TLS provider.
- [x] If the native library is missing or does not load, fall back to the
  pure-Dart transport: `install()` returns `false`, `unavailableReason`
  says why, `metrics.transport` shows the one in use (D10, D53).
- [x] ~~`dart compile exe` does not run hooks~~ — wrong: it **fails** for
  any app whose dependency graph contains a package with a hook (verified
  on 3.13.7; the check covers only entrypoints under `bin/`). Hence the
  split into `ws` + `ws_native` (DECISIONS.md D8), confirmed by the
  maintainer.
- [x] License notices for the Rust dependencies, in `ws_native`'s LICENSE
  and as `THIRD_PARTY_LICENSES` on the release (D43).

### 4.8 Web

- [x] An implementation over `package:web` `WebSocket` (JS interop, extension
  types), dart2js and dart2wasm, on the shared server suite (D46). Text and binary (`arraybuffer`); JSON decoding with `JSON.parse`
  through interop or `dart:convert`. Measure which is faster under dart2js
  and dart2wasm.
- [x] Same state machine, reconnect and heartbeat logic, shared through a
  common Dart core.
- [x] Browser limits: no custom headers, no ping/pong access (heartbeat must
  be at the application level), `bufferedAmount` for send backpressure
  (D15).
- [ ] spinify measurements showed browser slowdowns above ~15 KB; recheck.
- [?] `WebSocketStream` (Streams API with backpressure) where available.

### 4.9 Reliability features (from 1.x and beyond)

- [x] Reconnect with exponential backoff and jitter, max attempts, and a
  manual `reconnect()`; the policy is tested on real sockets (D48).
- [x] Heartbeat: protocol ping/pong on native, an application-level hook on
  web. Timeouts close the connection and trigger a reconnect. On by
  default; a draining send queue extends the wait (D41).
- [x] Metrics: bytes and messages in/out, reconnects, last error, round
  trip, transport in use, queue size (`WebSocketMetrics`).
- Not in 2.0: interceptors / middleware (1.x had `ws_interceptor.dart`);
  a codec or `onMessage` covers them without a decode in the hot path.
- [x] Logging hooks without a logging-package dependency: `onError`,
  `onStateChange`.

## 5. Exploration: transport and structures

Every idea becomes a variant in `benchmark/` and goes through the same
matrix (desktop + Android + iOS, `wss`, the spinify message sizes). It is
compared on main-isolate cost, total CPU, throughput, latency, RSS, binary
size and complexity. An idea is adopted only if it wins clearly (about ≥1.5×
on a key metric, or a smaller win with less complexity). Record each result
here, including the ones that fail.

### 5.1 Delivery path Rust → Dart

- [ ] **Shared-memory ring buffer** (SPSC) between Rust and Dart, with a port
  message only for the "empty → not empty" wakeup. Dart reads with leaf
  calls or a `Pointer` view. Removes the per-batch `Dart_PostCObject` cost
  and the finalizer per buffer.
- [ ] **Pull model:** Dart calls `ws_poll()` (leaf) after a wakeup and drains
  everything ready, instead of Rust pushing every batch.
- [ ] `NativeCallable.listener` instead of `ReceivePort`: measure.
- [ ] **Buffer pooling / arenas** for incoming bytes and tapes: reuse
  allocations instead of malloc + finalizer per message, and return buffers
  to the pool when Dart releases them.
- [ ] Batch size and flush policy: per socket read, by time, or by bytes.
  Latency vs. throughput.
- [ ] Measure the cost of the finalizer on external typed data in itself
  (GC pressure with many small messages).

### 5.2 Transport internals (Rust)

- [ ] WebSocket layer: tokio-tungstenite vs. `fastwebsockets` vs. our own
  codec in Rust (zero-copy frames, SIMD unmasking, reading straight into the
  buffer that goes to Dart).
- [ ] Runtime: tokio multi-thread vs. `current_thread` per connection vs.
  plain `mio`; number of threads on mobile; `io_uring` for Linux servers.
- [ ] TLS: rustls+ring vs. rustls+aws-lc-rs vs. BoringSSL vs. platform TLS
  (Network.framework on Apple, Conscrypt on Android). Compare speed, binary
  size and trust-store integration.
- [ ] Send path: copy in a leaf call (today) vs. pinned or handed-over
  buffers; encoding JSON in Rust from a Dart structure (probably not worth
  it — measure).
- [ ] Pure-Dart transport: whether there is any way to avoid the slow
  `SecureSocket` (e.g. Rust TLS + Dart framing) or whether it stays as a
  fallback only.

### 5.3 Data structures for decoded messages

- [ ] **Tape layout variants:** 32-bit words, a separate string table,
  interned keys, precomputed key hashes, an ASCII fast path, lazy string
  decoding, faster number reads (`Pointer`/`ByteData` vs. `Int64List`,
  `vm:prefer-inline`).
- [ ] **Shape caching:** objects with the same key order (typical for lists
  of models) share a "shape". A key lookup becomes an index lookup by
  position.
- [ ] **Byte key comparison:** compare the requested key with the UTF-8
  bytes in the tape without creating Strings.
- [ ] **Schema-driven decoding:** the user's model gives a schema (codegen or
  macros). Rust extracts only the needed fields into a compact fixed-offset
  buffer, and Dart reads fields by offset. This could get close to zero cost
  on main for model mapping.
- [ ] **Binary formats** transcoded in Rust or used natively: FlatBuffers,
  MessagePack, CBOR, Protobuf. Compare reading costs in Dart with the tape.
- [ ] **Building Dart objects from native code:** check what the Dart API DL
  allows (handles, strings, lists) when called from a non-leaf FFI call on
  the Dart thread, against `Dart_CObject` (rejected) and the tape.
- [ ] Dart worker isolate with `Isolate.exit` / shared-memory isolates (if
  the SDK makes them available) for model mapping without copying.

## 6. Verification and checks

### 6.1 Performance checks still open

- [ ] **Android `d-tape` ping-pong anomaly**: 14 KB gives 846–924 msg/s,
  below `d-bytes`, and the embedded server's CPU is 3× higher. Hypothesis:
  DVFS or core placement when main is almost idle. Check thread affinity and
  core frequency, try a different number of runtime threads, and see whether
  it reproduces against an external server.
- [ ] **Real network**: run the mobile matrix over Wi-Fi (and LTE if
  possible) against the Mac server. The CPU and main-isolate gains should
  hold; throughput will be bounded by the link.
- [ ] **Sustained load on phones** (10–15 min): throttling, latency
  stability, battery and temperature.
- [ ] **Hybrid `d-worker-model`**: Rust posts the tape straight to a worker
  isolate, the worker maps models and sends them to main. Check that it
  beats `d-tape` when mapping is heavy, and look at the Android worker CPU
  overhead.
- [ ] **Tape `fromJson` cost**: field access on a tape is ~15–45 ns against
  ~11 ns on a `Map`. Ideas: compare keys as bytes without making Strings, a
  key cache, interned keys, faster number reads.
- [ ] Main-isolate probe resolution: the idle calibration noise is about
  ±1% busy time (±100 µs/msg at 100 msg/s). Use higher rates or a different
  method (Timeline / `vm_service` CPU samples) to measure near-zero costs.
- [ ] Number of tokio threads vs. CPU and latency on mobile (1, 2, 4).
- [ ] Send path: cost of `jsonEncode` + UTF-8 on main for large payloads;
  zero-copy send.
- [ ] The `c-worker-tape` anomaly at 1 KB flood (16 MB/s), only if the worker
  decoder keeps using a tape.
- [ ] Desktop: Windows and Linux numbers (TLS backends differ).
- [ ] Benchmark regression CI: a quick matrix on every PR. Fail on large
  regressions of key metrics.

### 6.2 Correctness

- [x] **Autobahn TestSuite** (fuzzingserver) for native and pure Dart, at
  the transport level (not through `WebSocketClient`): 0 failures
  (DECISIONS.md D28, re-run in D53; reports in
  `benchmark/results/autobahn/`). [ ] Web through the browser; [ ] in CI.
- [~] A shared conformance suite run against every transport:
  `test/transport/conformance.dart` (pure Dart and native) and the Rust
  server suite, whose platform-neutral half (`server_shared.dart`) also
  runs in the browser on dart2js and dart2wasm (DECISIONS.md D46).
  Reconnect-under-load cases still to add.
- [ ] Fuzzing: the frame parser (pure Dart), the tape builder and the Dart
  tape reader (malformed JSON, deep nesting, huge numbers, invalid UTF-8,
  surrogate escapes).
- [ ] Tape compatibility: Rust and Dart tape builders must produce identical
  tapes (`test/native/tape_test.dart` and `benchmark/test/tape_test.dart`
  already compare against `jsonDecode`).
- [ ] Integer and double semantics match `jsonDecode`: integers beyond
  64 bits, `-0`, exponents, and the `1.0` vs `1` distinction.

### 6.3 Robustness

- [ ] Soak tests (hours): memory (Dart heap and native RSS), file
  descriptors, threads, and finalizers actually releasing external buffers.
- [ ] Network faults: dropped packets, server killed, half-open
  connections, slow server, DNS failures. Use `toxiproxy` or `tc`/`netem`.
- [ ] Flutter hot restart and hot reload with open connections; app
  backgrounding and foregrounding on iOS and Android; airplane mode toggles.
- [ ] Many connections at once (100+) in a Dart server process.
- [ ] Miri / sanitizers for the unsafe FFI code (CObject, external typed
  data, finalizers).

### 6.4 Platform and distribution

- [ ] Flutter builds for every target with native assets: release, profile,
  debug; App Store / TestFlight upload validation (the dylib packaged as a
  framework, code signing, privacy manifest).
- [ ] Google Play: 16 KB page alignment, all ABIs.
- [ ] Pure Dart server: `dart build cli`, Docker image (Linux x64/arm64,
  glibc and musl).
- [ ] Fallback path: delete or corrupt the native library and the pure-Dart
  transport must take over.
- [ ] Prebuilt download: offline builds, proxy environments, checksum
  mismatch errors.

### 6.5 Security

- [ ] No insecure TLS defaults; the accept-any verifier exists only in
  benchmarks and tests.
- [ ] Limits against memory exhaustion: max message size, max frame size,
  max tape depth and size.
- [ ] Handshake: `Sec-WebSocket-Accept` validation, `Origin` handling for
  the server side (if added).
- [ ] `cargo audit` / `cargo deny` in CI; supply-chain checks for the
  prebuilt binaries (reproducible builds or attestation).

## 7. Milestones

| # | Milestone | Content | Status |
|---|---|---|---|
| M0 | Benchmarks and the transport decision | Rust server, transport variants, tape, model mapping, mobile runs | [x] |
| M1a | Exploration | §5 experiments as benchmark variants; adopt or reject each with recorded numbers. Runs alongside M1–M4, because the internals are swappable | [ ] |
| M1 | API design | §4.1 API sketch, transport interface, decoder and flow-control contracts, state/error types; review before coding | [x] expert-reviewed, DECISIONS.md D11–D18, ARCHITECTURE.md |
| M2 | Core and pure-Dart transport | State machine, reconnect, heartbeat, metrics; RFC 6455 codec from `benchmark/lib/src/codec.dart` hardened; Autobahn green | [~] core, protocol layer, pure-Dart transport and fake done with tests; Autobahn 0 failed (D28) |
| M3 | Native transport | Rust crate as a library (`native/` at the repo root): connections, credits, errors, TLS with the platform verifier, proxy; Autobahn green | [~] package `ws_native` (`native/`): Rust crate with credits, drain on request, liveness probe, `catch_unwind`, hook with prebuilt/cargo/none; passes the shared conformance suite; Autobahn 0 failed (D28); OS trust store (D35), Happy Eyeballs (D34), CONNECT proxy (D36). System proxy settings open |
| M4 | Decoders | Tape (Rust and Dart builders), worker decoder, custom binary decoders; tape speedups (§5.3) | [ ] |
| M5 | Web transport | `package:web` implementation, shared conformance suite in the browser | [x] implemented; the shared server suite runs on dart2js and dart2wasm (D46) |
| M6 | Robustness | §6.3 soak, faults, lifecycle, hot restart | [ ] |
| M7 | Distribution | Prebuilt binaries, CI for every target, checksums, size budget, 16 KB pages, store validation | [ ] |
| M8 | Example and docs | Dart example in `example/` (scope to be described by the maintainer), Rust server in `server/`, migration guide from 1.x, README, API docs | [ ] |
| M9 | WebTransport (optional) | QUIC through Rust (`wtransport` / `quinn`), same message API where possible, datagrams and streams | [?] |
| — | Release 2.0.0 | After M1–M8 | [ ] |

## 8. Risks

- **Build and toolchain complexity** of shipping Rust for many targets.
  Mitigation: prebuilt binaries + checksums, the pure-Dart fallback, CI for
  every target.
- **Dart hooks and native assets are still young.** Behaviour can differ
  between Dart and Flutter versions. Pin the minimum SDK and test on stable
  and beta.
- **Platform trust store and proxy** on mobile: easy to get wrong, and
  users notice (corporate networks). See §4.5 and §4.6.
- **Two (three) transports diverge** in behaviour. Mitigation: shared
  suites on every transport (§6.2, D46); the first browser run found four
  divergences.
- **App size**: about 2.1–2.3 MB per ABI may matter for some apps. Mitigation:
  document it; the pure-Dart transport works on its own.
- **Settling on a local optimum**: the current transport and tape were the
  first design that won. Mitigation: the exploration track (§5), with
  internals kept swappable behind the API.
- **Unsafe FFI code**: memory bugs crash the whole app. Mitigation: keep it
  small, fuzz it, run sanitizers, add Miri tests.

## 9. References

- Decision log (every compared alternative, with numbers):
  `docs/DECISIONS.md`
- How the code is layered and the rules per layer: `docs/ARCHITECTURE.md`
- Benchmarks and methodology: `benchmark/README.md`
- Results (macOS, Galaxy S25 Ultra, iPad Pro M2): `benchmark/RESULTS.md`,
  raw runs in `benchmark/results/`
- Benchmark server: `server/README.md`
- Tape format: `benchmark/lib/src/tape.dart`, `benchmark/native/src/tape.rs`
- Message size limits: spinify measurements (1/14/30/60 KB, 64 KB limit in
  Centrifugo)
