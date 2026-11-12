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
                         │ reconnect · heartbeat · metrics · interceptors · decoders       │
                         └──────────────┬───────────────────────┬───────────────────┬─────┘
                                        │                       │                   │
                     native (FFI, default)          pure Dart (fallback)      web (browser)
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

- The transport is picked at runtime: native, then pure Dart, then web, with
  a way to force one (for tests and debugging).
- All transports behave the same way: same states, events, close codes,
  errors and metrics. A shared conformance test suite enforces this (§6.2).
- Messages come in batches, once per socket read. Bytes and tapes are
  external typed data owned by Rust, released by a finalizer. Nothing is
  copied on the receiving side.

## 4. Design considerations

### 4.1 Public API

- [ ] **The API must not depend on how things are done inside.** Transport,
  delivery path and data structures stay internal behind small interfaces,
  so an experiment (§5) can replace one without breaking users. The public
  types are message, decoder result and `Map` / `List` views, never the
  tape or the FFI layout.

- [ ] Typed messages: text, binary, and decoded (JSON tape view or user
  model). Avoid `Object`-typed streams in the hot path.
- [ ] A **decoder** abstraction chosen per connection:
  - `raw`: text as `String`, or as UTF-8 bytes without making a `String`;
    binary as `Uint8List`.
  - `jsonTape` (default for JSON): lazy `Map<String, Object?>` /
    `List<Object?>` views over the tape, mapped with ordinary
    `fromJson(Map)`.
  - `isolate<T>(T Function(Object? json))`: mapping runs in a Dart worker
    isolate and models come to main ready to use. For very heavy models.
  - Custom binary decoders (protobuf, MessagePack, FlatBuffers) that run in
    the worker.
- [ ] Send API: `sendText(String)`, `sendBytes(Uint8List)`,
  `sendJson(Object?)`. Optionally encode JSON off main for large payloads.
  The send path should be zero-copy where the buffer can be handed over or
  pinned.
- [ ] Futures and streams that work with `async`/`await` and cancellation:
  `connect()`, `close(code, reason)`, `done`. No unawaited work left behind.
- [ ] Connection state as a sealed class hierarchy (`connecting`, `open`,
  `closing`, `closed(code, reason)`, `reconnecting(attempt, delay)`), with
  the current value plus a stream.
- [ ] Errors as a sealed hierarchy: handshake (HTTP status, headers), TLS,
  DNS/connect, protocol violation, message too big, closed by the peer,
  flow-control overflow. Never throw raw `SocketException` or Rust strings
  at the user.
- [ ] Options: headers, subprotocols, connect/handshake timeouts, max message
  size, compression, TLS settings (§4.5), proxy (§4.6), heartbeat,
  reconnect policy, flow-control window, decoder.
- [ ] Testability: a fake transport and an in-memory server, so app code is
  testable without sockets (1.x had `ws_client_fake.dart`).
- [?] Single connection object vs. a client that manages several connections
  (the old `ConnectionManager`). Proposal: one connection per object; a
  separate optional manager.
- [?] Naming: keep the `ws` package name and `WebSocketClient`, or rename to
  avoid clashing with `dart:io` `WebSocket`.

### 4.2 Threading and isolates

- [ ] One tokio runtime per process, shared by all isolates and connections.
  Default 2 worker threads, configurable. Measure 1 vs 2 vs N on mobile.
- [ ] Messages can be delivered to a `SendPort` of any isolate. Rust posts
  straight to the worker when the worker decoder is used, and does not go
  through main.
- [ ] Leaf FFI calls (`@Native(isLeaf: true)`) only for non-blocking work:
  enqueue a send, set credits, close. Never block the Dart thread in Rust.
- [ ] Flutter **hot restart**: the native runtime and its connections outlive
  the Dart isolate. Connections must be detected and closed when their
  `ReceivePort` dies (`Dart_PostCObject` returns `false`), and with a
  `NativeFinalizer` on the connection handle.
- [ ] Isolate shutdown with open connections must not leak or crash.
- [ ] `isolate group` behaviour: several Flutter engines / isolates in the
  same process share the library.

### 4.3 Flow control (backpressure)

- [ ] Credit-based: Dart grants N messages or N bytes. Rust stops reading the
  socket when credits run out, and TCP backpressure slows the server.
- [ ] Credits are granted back automatically after the listener has handled a
  batch. Applies to `Stream` pause/resume too.
- [x] Bound on the send queue: `sendBufferLimit` fails `push()` with
  `WebSocketBufferFullException` (DECISIONS.md D36).
- [ ] Same semantics in the pure-Dart transport: pause the
  `RawSocket` read events.
- [ ] Re-run the flood benchmarks: throughput must stay about the same and RSS
  must stay bounded.

### 4.4 Protocol coverage (RFC 6455 and friends)

- [ ] Fragmentation, control frames between fragments, ping/pong, close
  handshake with codes and reasons, UTF-8 validation of text messages and
  close reasons.
- [ ] `permessage-deflate` (RFC 7692). tungstenite has limited or no
  support, so check the current status or a fork. `dart:io` supports it. Be
  careful about CPU and memory on mobile (context takeover).
- [ ] Subprotocol negotiation (`Sec-WebSocket-Protocol`), custom handshake
  headers, cookies; give access to the handshake response headers.
- [ ] Max frame and message sizes with a clear error.
- [ ] Redirects during the handshake (3xx): decide on and document the
  behaviour.
- [ ] HTTP/2 WebSockets (RFC 8441) — probably not needed. Note it and leave
  it out.

### 4.5 TLS

- [x] Certificate verification against the **platform trust store**
  (DECISIONS.md D35): OS verifier on Apple/Windows, system CA files on
  Linux and Android, `webpki-roots` only as a fallback.
- [ ] Certificate pinning (SPKI hash) and custom CAs.
- [ ] An option to allow bad certificates, for development only. Never the
  default. The benchmarks use an accept-any verifier, and it must not leak
  into the library.
- [ ] ALPN, SNI, TLS 1.2/1.3 support. Crypto provider: `ring` (as now) or
  `aws-lc-rs` — compare build complexity and binary size for mobile
  targets.
- [ ] Pure-Dart transport: `SecureSocket` with `SecurityContext` options
  mapped from the same API.

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
- [ ] App lifecycle: iOS suspends sockets in the background. Reconnect on
  resume. Do not drain the battery with heartbeats in the background.
- [ ] App Store review: plain BSD sockets are fine. Document that ATS does
  not apply, and what users must state in privacy manifests (if anything).

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
- [ ] Targets: Android (arm64-v8a, armeabi-v7a, x86_64), iOS (device +
  simulator arm64/x64), macOS (arm64/x64), Linux (x64/arm64, glibc; musl?),
  Windows (x64/arm64).
- [ ] Android **16 KB page size** alignment (required for new apps on Google
  Play); verify the produced `.so`.
- [ ] Binary size budget. Today: ~1.7 MB stripped per ABI (~0.9 MB
  gzipped). Try `opt-level="z"` for cold code, fewer dependencies, a smaller
  TLS provider, and dropping the embedded benchmark server from the library.
- [ ] If the native library is missing or does not load, fall back to the
  pure-Dart transport. Log it, and make it visible through metrics.
- [x] ~~`dart compile exe` does not run hooks~~ — wrong: it **fails** for
  any app whose dependency graph contains a package with a hook (verified
  on 3.13.7; the check covers only entrypoints under `bin/`). Hence the
  split into `ws` + `ws_native` (DECISIONS.md D8), confirmed by the
  maintainer.
- [ ] License notices for the Rust dependencies (tokio, rustls, ring, …).

### 4.8 Web

- [ ] An implementation over `package:web` `WebSocket` (JS interop, extension
  types). Text and binary (`arraybuffer`); JSON decoding with `JSON.parse`
  through interop or `dart:convert`. Measure which is faster under dart2js
  and dart2wasm.
- [ ] Same state machine, reconnect and heartbeat logic, shared through a
  common Dart core.
- [ ] Browser limits: no custom headers, no ping/pong access (heartbeat must
  be at the application level), `bufferedAmount` for send backpressure.
- [ ] spinify measurements showed browser slowdowns above ~15 KB; recheck.
- [?] `WebSocketStream` (Streams API with backpressure) where available.

### 4.9 Reliability features (from 1.x and beyond)

- [ ] Reconnect with exponential backoff and jitter, max attempts, and a
  manual `reconnect()`.
- [x] Heartbeat: protocol ping/pong on native, an application-level hook on
  web. Timeouts close the connection and trigger a reconnect. On by
  default; a draining send queue extends the wait (D41).
- [ ] Metrics: bytes and messages in/out, reconnects, last error, round
  trip, transport in use, queue sizes and flow-control stalls.
- [ ] Interceptors / middleware for outgoing and incoming messages (1.x had
  `ws_interceptor.dart`). They must not force a decode in the hot path.
- [ ] Logging hook without a logging-package dependency.

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

- [x] **Autobahn TestSuite** (fuzzingserver) for native and pure Dart: 0
  failures (DECISIONS.md D28). [ ] Web through the browser; [ ] in CI.
- [~] A shared conformance suite run against every transport
  (`test/transport/conformance.dart`): pure Dart and native pass; web has
  its own browser test. Flow control and reconnect-under-load cases still
  to add.
- [ ] Fuzzing: the frame parser (pure Dart), the tape builder and the Dart
  tape reader (malformed JSON, deep nesting, huge numbers, invalid UTF-8,
  surrogate escapes).
- [ ] Tape compatibility: Rust and Dart tape builders must produce identical
  tapes (`test/tape_test.dart` already compares against `jsonDecode`).
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
| M5 | Web transport | `package:web` implementation, shared conformance suite in the browser | [~] implemented, tested on dart2js and dart2wasm |
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
- **Two (three) transports diverge** in behaviour. Mitigation: one shared
  conformance suite.
- **App size**: ~1–2 MB per ABI may matter for some apps. Mitigation:
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
