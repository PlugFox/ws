# Decision log

Every design choice of the 2.0 rewrite that had real alternatives, with
the options compared, the evidence, and the outcome. Newest at the bottom
of each section. Code says *what* was done; this file says *why*, and what
was rejected, so nobody has to repeat the comparison.

Format: **Decision** — options — evidence — status (`adopted`,
`provisional` = adopted until a benchmark or test says otherwise,
`rejected`, `open`).

---

## Repository and process

### D1. Old 1.x code is kept in `_lib/` until 2.0 replaces it

- Options: delete now; keep on a branch; move to `_lib/`.
- Moving keeps the reference code (interceptors, metrics, the fake client,
  their tests) next to the new code while features are ported. It is
  excluded from analysis and from the published package.
- Status: **adopted**, to be deleted before 2.0.0.

### D2. `.pubignore` must repeat `.gitignore`, with declared exceptions

- With a `.pubignore` present, `dart pub publish` ignores `.gitignore`
  entirely. A pattern missing from `.pubignore` would publish local
  junk (build output, `.temp/`, local assets).
- `tool/check_pubignore.dart` fails CI when a `.gitignore` pattern is not
  in `.pubignore`, unless declared as `# publish: <pattern>`. Today the only
  exception is `/native/prebuilt/`: CI-built libraries that are not
  committed to git but go into the package archive.
- Status: **adopted**.

## Protocol layer (pure Dart, `lib/src/protocol/`)

Shared by the pure-Dart transport, the in-memory test server and the fake.
It imports only `dart:typed_data`/`dart:convert`/`dart:math`, so it also
runs (and is tested) under dart2js and dart2wasm.

### D3. Own SHA-1 instead of `package:crypto`

- SHA-1 is needed once per connection for `Sec-WebSocket-Accept`. It is
  not used for security. 60 lines, checked against the FIPS vectors and
  the RFC 6455 example.
- One dependency less for a package that wants to be dependency-light.
- Status: **adopted**.

### D4. Frame parsing: zero-copy views + exact-size buffer for big frames

- From the benchmark prototype (`benchmark/lib/src/codec.dart`), which beat
  `dart:io` `WebSocket` by 20–50% (2× on unicode text): a frame that fits
  in one socket read is delivered as a `Uint8List` view, no copy. A larger
  frame gets one buffer of the exact payload size, filled across reads.
- Hardened for the library: limits are checked **before** allocation
  (a forged 64-bit length cannot make us allocate), 64-bit lengths with the
  MSB set are rejected, control frames ≤125 bytes and unfragmented, close
  codes and reasons validated, masked server frames rejected, the reader
  stops after a close frame or a failure.
- Views keep the whole socket chunk alive. That is fine because
  `RawSocket.read()` returns a fresh list each time (never reused).
- Status: **adopted**.

### D5. UTF-8 validation: done by the decoder for single frames

- RFC 6455 requires failing the connection (1007) on invalid UTF-8 in text
  messages. Options:
  1. always validate in the frame reader;
  2. let the decoder validate (`utf8.decode`, the fused UTF-8 + JSON
     decoder and simd-json all reject malformed input anyway) and map its
     `FormatException` to 1007;
  3. validate only fragmented messages, incrementally, so Autobahn's "fail
     fast" cases pass.
- Measured (`benchmark/bin/micro.dart`, AOT, macOS M-series; payloads of
  the benchmark JSON shape):

  | payload | validator v1 (byte DFA) | validator v2 (+ 4-byte ASCII words) | `utf8.decode` | fused UTF-8+JSON |
  |---|---:|---:|---:|---:|
  | 1 KB ascii | 0.71 µs | 0.19 µs | 0.37 µs | 4.5 µs |
  | 14 KB ascii | 9.5 µs | 2.2 µs | 5.0 µs | 57 µs |
  | 60 KB ascii | 39.7 µs | 9.2 µs | 21 µs | 247 µs |
  | 1 KB unicode | 2.1 µs | 1.1 µs | 1.9 µs | 4.9 µs |
  | 14 KB unicode | 27.8 µs | 15.7 µs | 25.9 µs | 62 µs |
  | 60 KB unicode | 122 µs | 65 µs | 110 µs | 248 µs |

  Frame parsing alone costs 0.02 / 0.15 / 1.75 µs per 1 / 14 / 60 KB
  message (zero-copy). With validation in the reader it becomes 2.3 / 29 /
  122 µs (v1): the check would dominate the reader by ~100×, and add
  4–26% (v2) on top of a JSON decode that validates again anyway.
- Decision: option 2 + 3. The reader validates fragmented text
  incrementally (fail fast); single-frame text is validated by whatever
  decodes it, and a decode `FormatException` closes with 1007. A
  bytes-only consumer of text would need `validateUtf8: true`.
- Validator v2 (Höhrmann DFA + aligned 4-byte ASCII scan) is faster than
  `utf8.decode` and is used for fragments and close reasons.
- Status: **adopted**. Follow-up: a different UTF-8 approach from another
  project of the maintainer is being studied (may replace `utf8.decode` in
  the text decoder).

### D6. Masking keys from a pooled `Random.secure()`

- RFC 6455 §5.3 asks for masking keys from a strong source of entropy that
  cannot be predicted (protects proxies from cache poisoning). The
  prototype used xorshift seeded once from `Random.secure()`: fast, but
  predictable after a few frames.
- `Random.secure()` per frame costs a native call per `nextInt`. A pool of
  64 keys refilled from `Random.secure()` keeps the security property and
  makes the cost per frame negligible.
- Masking runs 32 bits at a time on an aligned destination; the key is
  rotated to the alignment phase, so it is correct on any endianness.
- Status: **adopted**.

### D7. Handshake: strict validation, no header injection

- Rejects responses that are not `101`, lack `Upgrade: websocket` /
  `Connection: Upgrade`, have a wrong `Sec-WebSocket-Accept`, select an
  extension we did not offer (we offer none yet) or a subprotocol we did
  not offer.
- User headers may not override handshake headers, and may not contain CR
  or LF.
- A non-101 status is a `HandshakeError` with the status code and
  headers. The reconnect policy uses the status code: most 4xx responses
  mean "do not retry" (see the client section).
- Redirects (3xx) are not followed for now; the error carries the
  `location` header. `permessage-deflate`: not offered yet (ROADMAP §4.4).
- Status: **adopted**.

## Packaging and distribution

### D8. Native transport lives in a separate package `ws_native`

- Verified on Dart 3.13.7 (scratch packages): when **any** package in the
  dependency graph has a build hook, `dart compile exe` and
  `dart compile aot-snapshot` fail with *"'dart compile' does not support
  build hooks, use 'dart build' instead"* — even for an entrypoint that
  never imports that package. The check is static (a `hook/build.dart`
  exists): a hook that returns without output, with the app setting
  `user_defines: {source: none}`, fails the same way (re-checked on
  3.13.7), so a hook cannot opt out for `compile exe` users. `dart run`, `dart test` and `dart build cli`
  run hooks fine; Flutter runs them for native targets; dart2js/dart2wasm
  run none.
- Scope of the check (dartdev `compile.dart` on SDK `main`, Nov 2026,
  unchanged since dart-lang/sdk#62593, fixed in Feb 2026): only entrypoints
  under the app's `bin/` are checked, against non-dev dependencies.
  Verified on 3.13.7: `dart compile exe tool/app.dart` and `main.dart` at
  the package root compile and run with the same hooked dependency. Such
  an executable has no code assets: `@Native` lookups fail at run time,
  `DynamicLibrary.open(path)` still works. The team (dcharkes) rejects a
  flag to skip the check and recommends `dart build cli`; "run hooks and
  fail only if assets are produced" was floated, not planned. Since 3.13
  `dart build cli` cross-compiles (`--target-os`, `--target-arch`); since
  3.15 it takes `--define`.
- So a hook inside `ws` itself would break `dart compile exe` (the usual
  way to build Docker images of Dart servers) for every app that depends on
  `ws`, including transitively (e.g. through `spinify`). ROADMAP §4.7 said
  only that compile exe "does not run hooks"; that was wrong.
- Options:
  1. one package `ws` with the hook (simplest for Flutter users, breaks
     `dart compile exe` for everybody);
  2. `ws` (core, pure-Dart and web transports, no hook) + `ws_native`
     (Rust library, FFI, build hook). Apps opt in to native by adding
     `ws_native` and installing its transport once;
  3. one package now, split later (a breaking change after 2.0).
- Decision: option 2. `ws` defines the transport interface and a
  registration point; `ws_native` registers the Rust transport. The hook
  lives in `ws_native/hook/build.dart` as the concept asks. If `dart
  compile` gains hook support, the packages can be merged without breaking
  the API (`ws_native` becomes a stub).
- Status: **adopted** (2026-11-12). The maintainer confirmed the split
  together with D30 (the FFI layer lives in `ws`, so a library can also be
  loaded by path) and D32 (`dart run ws:install` for `dart compile exe`).

### D9. Where the native library comes from (hook resolution order)

Order used by `ws_native/hook/build.dart`:

1. User-define override in the app's pubspec
   (`hooks: user_defines: ws_native: source: none | build | prebuilt`, and
   `library: <path>` to use a given file).
   Custom environment variables are **not** visible to hooks (verified:
   only `PATH`, `HOME` and, per hooks_runner 1.6 source, `CARGO_*` /
   `RUSTUP_*` pass through), so overrides use user-defines.
2. Prebuilt library bundled in the package archive
   (`prebuilt/<os>-<arch>[-sim]/`), checked against SHA-256 values
   committed in the package before it is handed to the tooling. The
   tooling rewrites and ad-hoc re-signs the copy, so only the source file
   can be verified.
3. `cargo build`, only when the prebuilt directory is absent (git or path
   dependency, development checkout) and cargo is installed. Never run
   cargo in the pub cache when a prebuilt exists.
4. Nothing: no code asset is emitted, and at run time `ws` falls back to
   the pure-Dart transport. The hook never throws, except on a checksum
   mismatch (that is tampering or corruption and must be loud).

- Download from GitHub Releases (the `sqlite3` 3.x approach) is not used
  while the binaries are bundled: one source of truth. An in-archive
  checksum only detects corruption; provenance needs CI attestation.
- Apple: link with `-Wl,-headerpad_max_install_names` and
  `-install_name @rpath/libws_native.dylib`; without the padding
  `dart run`/`dart test` fail in `install_name_tool` (verified).
- Android: `-Wl,-z,max-page-size=16384`, checked with `llvm-readelf -l` in
  CI. Linux: pin a glibc floor (cargo-zigbuild).
- Status: **adopted** (implementation in M7, local cargo build first).

### D10. Native load failure is detected once, by an ABI probe

- A missing code asset throws a catchable `ArgumentError` on the first FFI
  call (verified). `ws_native` calls `ws_abi_version()` once per process,
  caches the result and falls back to pure Dart only when loading fails —
  never because a connection failed.
- Status: **adopted**.

## Public API (client)

Review input: an independent expert review of the draft (Fable), checked
against the maintainer's concept (`WebSocketClient<T>` with `binary()` /
`json()`, `push`, `rpc`, `ping`, `connect`, `disconnect`, `close`).

### D11. `WebSocketClient<T>` over one core, three codecs

- `binary()` → `Uint8List`, `json()` → `Map<String, Object?>`, `text()` →
  `String`. One core, a private codec per type. Codecs are not public yet.
- JSON client: a top-level array or scalar, or a binary frame, is reported
  through `onError` and dropped; it never disconnects.
- On native, JSON objects are tape views extending
  `UnmodifiableMapBase<String, Object?>` (lists: `UnmodifiableListBase`),
  so `fromJson(Map)` and `jsonEncode` work unchanged. Views are read-only
  and lazy, and a retained sub-view keeps the whole message buffer alive;
  a deep-copy helper is provided.
- Status: **adopted**.

### D12. `push` completes when the transport accepted the message

- "Written to the socket" would need an acknowledgement per message from
  Rust; "delivered" is unknowable. The future completes synchronously-fast
  while the send queue is below its high-water mark, and only after it
  drains otherwise, so `await push(...)` gives backpressure.
- `push` while not open fails fast with a typed exception. Silent buffering
  across reconnects would replay stale messages into a new, possibly
  unauthenticated session. An opt-in bounded buffer may come later.
- Status: **adopted**.

### D13. RPC through an optional handler

```dart
abstract interface class WebSocketRpc<T> {
  (Object id, T message) encode(int seq, T data); // seq: client-wide, never reused
  Object? match(T message);                       // null → a push, goes to messages
  T decode(T reply);                              // returns the result or throws
}
```

- Without a handler, `rpc()` asserts in debug and throws `StateError`.
- A ready-made JSON handler (`id` / `error` keys) is provided.
- One deadline queue with a single timer, not a `Timer` per call.
- On disconnect/close all pending calls fail with
  `WebSocketRpcException` (`timeout | disconnected | closed | remote`).
  Calls are never replayed automatically (they may not be idempotent).
- Late or unknown replies are dropped and counted in metrics, not leaked
  into `messages`. A throwing `match` → `onError`, message treated as push.
- No cancellation in v1 (there is no wire-level cancel); per-call timeout.
- Status: **adopted**.

### D14. Reconnect policy

- `disconnect()` stops the retry loop and runs the close handshake with a
  timeout (~3 s), then aborts the socket: a close handshake with a dead
  peer would hang forever. `close()` is idempotent and terminal.
- Terminal handshake statuses (stop retrying): every 4xx except 408, 425,
  429. Retried: 5xx, network/DNS errors, TLS errors (at max backoff:
  captive portals cause them). Browsers hide the handshake status (only
  1006), so on web nothing is terminal at handshake time.
- Terminal close codes by default: 1002, 1003, 1007, 1008, 1009, 1010.
  Configurable, plus a `shouldReconnect(cause)` override. 1013 (try again
  later) waits at least the max delay.
- Full-jitter exponential backoff (as in the maintainer's SSE client). The
  attempt counter resets only after the connection stayed open for a while
  or a message arrived, otherwise accept-then-close servers get hammered
  at the minimum delay.
- `reconnect()` reconnects now (app resume, connectivity back).
- A per-attempt request provider (URL/headers) lets tokens refresh, without
  which a 401 can never recover. An awaited `onOpen` hook runs before the
  state becomes open (re-auth, resubscribe).
- Status: **adopted**.

### D15. Heartbeat

- ~~Native: ping/timeout runs in Rust (tokio timer), the isolate is not
  woken.~~ Superseded by D41: the heartbeat is a Dart timer on every
  transport; Rust answers server pings itself.
- Pure Dart: protocol ping from a Dart timer.
- Web: browsers hide ping/pong. An idle watchdog without pings would kill
  healthy idle sockets. The app-level heartbeat is a common option
  (`ping` message factory + `isPong` matcher); with it `ping()` works
  everywhere, without it `ping()` on web throws a typed unsupported error.
- After a long timer gap (suspended app, background tab), send one probe
  instead of declaring the connection dead. The watchdog must not fire
  while the client is withholding flow-control credits.
- Status: **adopted**.

### D16. Options: plain data, platform parts as sub-objects

- `WebSocketOptions<T>` holds the common options plus `vm:` and `js:`
  parts (`WebSocketVmOptions`, `WebSocketJsOptions`), all plain data
  usable on every platform. `.vm(...)`/`.js(...)` are sugar.
- No `SecurityContext` in the API (opaque, cannot be handed to rustls,
  pulls `dart:io` into the public surface): TLS settings are data — PEM
  roots, SPKI pins, `allowInsecure` (development only).
- Only the transport factory is chosen by conditional import:
  `if (dart.library.js_interop)` (true on dart2js and dart2wasm). Never
  `dart.library.html` (false on wasm). `dart compile wasm` rejects any
  reachable `dart:ffi` import (verified), so FFI stays behind the import.
- Status: **adopted**.

## Transport boundary

### D17. Typed listener callbacks; delivery experiments stay inside a transport

- `onText(Object)` would put an untyped value on the hot path. The
  decoding mode is chosen at connect time, and the listener has typed
  callbacks: UTF-8 bytes (VM), `String` (web), decoded JSON (native tape),
  binary.
- Exactly one `onClosed`, nothing after it; `send*` never calls back
  synchronously. `abort()` exists for hard teardown. Credits are counted
  in bytes.
- Rust → Dart delivery experiments (port batches, shared ring buffer +
  poll, `NativeCallable.listener`) are a private option of the native
  transport, not separate transports.
- Status: **adopted**.

### D18. Native connection handles and dead isolates

- `u64` ids in a table (never raw pointers): a stale id is a harmless miss.
  Exactly one terminal event per connection, posted last.
- Dead isolate / hot restart: abort when `Dart_PostCObject` returns false,
  a `NativeFinalizer` on the Dart connection object, and a null-post probe
  of every port on `init` and on a slow timer (idle connections never
  post, so the first mechanism alone misses them).
- `catch_unwind` at every FFI entry and task; external-data finalizers run
  on any thread, so freeing must be thread-safe and non-blocking.
- Receive credits: per-connection byte counter (`AtomicI64` + `Notify`);
  the reader waits when credits ≤ 0 but always admits one message when
  positive, so a message larger than the window cannot deadlock. The send
  queue is bounded in bytes; `ws_send` returns ok / above high-water /
  closed / too big.
- ffigen: not worth it for ~10 scalar functions; hand-written `@Native`,
  struct-free ABI, `ws_abi_version()` check.
- Status: **adopted** for M3.

## Text decoding

### D19. Fast paths for ASCII: 64-bit scan and `String.fromCharCodes`

Measured with `benchmark/bin/utf8_micro.dart` and `benchmark/bin/micro.dart`
(AOT, macOS M-series). Pure ASCII, per message:

| size | scan 4 B words | scan 8 B words | scan 8 B × 4 unrolled | `utf8.decode` | `String.fromCharCodes` | `latin1.decode` |
|---|---:|---:|---:|---:|---:|---:|
| 1 KB | 0.17 µs | 0.09 µs | 0.05 µs | 0.34 µs | 0.04 µs | 0.62 µs |
| 14 KB | 2.15 µs | 1.00 µs | 0.60 µs | 4.73 µs | 0.39 µs | 8.33 µs |
| 60 KB | 9.25 µs | 4.25 µs | 2.52 µs | 19.8 µs | 1.83 µs | 36.7 µs |

- `String.fromCharCodes(Uint8List)` on the VM copies bytes straight into a
  one-byte string; `utf8.decode` is 10× slower even on pure ASCII, and
  `latin1.decode` is slower still.
- `decodeUtf8()` (`lib/src/protocol/utf8_text.dart`): unrolled 64-bit ASCII
  scan, then `String.fromCharCodes` if pure ASCII, else `utf8.decode`.
  Result, benchmark JSON payloads:

  | payload | validator v3 | `utf8.decode` | `decodeUtf8` | fused UTF-8+JSON |
  |---|---:|---:|---:|---:|
  | 1 KB ascii | 0.06 µs | 0.36 µs | 0.10 µs | 4.5 µs |
  | 14 KB ascii | 0.61 µs | 5.04 µs | 0.98 µs | 56.7 µs |
  | 60 KB ascii | 2.53 µs | 21.4 µs | 3.86 µs | 246 µs |
  | 14 KB unicode | 11.4 µs | 25.3 µs | 25.2 µs | 56.2 µs |

  5× faster on ASCII, no loss on unicode (the scan stops at the first
  non-ASCII byte). Validator v3 (the same scan inside the DFA) is now 1% of
  a JSON decode on ASCII and ~20% on unicode-heavy text.
- 64-bit words are VM-only: dart2js has no `Uint64List`. The choice is a
  `const bool.fromEnvironment('dart.library.js_interop')` branch, so the web
  build tree-shakes the 64-bit path; tests run on VM and Chrome.
- The first source the maintainer pointed to (`/Users/fox/git/chat`) had
  no custom UTF-8 code (Rust std + browser `TextDecoder`); a second one is
  being studied.
- Open: for non-ASCII text, decode the ASCII prefix with `fromCharCodes`
  and only the rest with `utf8.decode`? Needs a concat; measure.
- Status: **adopted**.

### D20. Send path: `JsonUtf8Encoder` for JSON, `ascii.encode` first for text

Second source studied: `chat-rs/packages/chat_core_dart` (the maintainer's
binary codec). Its ideas: ASCII pre-scan + `String.fromCharCodes` for
decoding (same as D19, there with a byte loop: our 64-bit scan is faster),
a hand-written UTF-8 encoder that writes straight into a reused buffer,
and `@pragma('vm:unsafe:no-bounds-checks')` on hot loops. Measured
(`benchmark/bin/encode_micro.dart`, `ascii_encode_micro.dart`, AOT):

String → masked text frame:

| text | `utf8.encode` + frame | encode directly into the frame | same, no bounds checks |
|---|---:|---:|---:|
| 1 KB ascii | 1.97 µs | 1.80 µs | 1.83 µs |
| 14 KB ascii | 24.4 µs | 24.1 µs | 24.3 µs |
| 14 KB unicode | 38.0 µs | 42.8 µs | 43.4 µs |
| 60 KB unicode | 165 µs | 184 µs | 187 µs |

- A hand-written encoder in Dart does not beat the SDK's `utf8.encode`
  (it is worse on unicode), and the pragma makes no measurable difference
  here: the SDK's own loop is already the bottleneck. **Rejected.**
- The copy into the frame buffer is ~8% of the total; not worth
  special-casing.

String/JSON → bytes, ASCII JSON text:

| size | `utf8.encode` | `latin1.encode` | `ascii.encode` | `Uint8List.fromList(codeUnits)` | `JsonUtf8Encoder` (from map) | `jsonEncode` + `utf8.encode` |
|---|---:|---:|---:|---:|---:|---:|
| 1 KB | 1.79 µs | 0.81 µs | 0.81 µs | 3.11 µs | 8.97 µs | 15.9 µs |
| 14 KB | 22.8 µs | 10.5 µs | 10.4 µs | 40.7 µs | 111 µs | 199 µs |
| 60 KB | 93.2 µs | 44.1 µs | 43.3 µs | 170 µs | 456 µs | 733 µs |

- JSON client: `JsonUtf8Encoder` straight to bytes, 1.6–1.8× faster than
  `jsonEncode` + `utf8.encode`. **Adopted.** (Web: `jsonEncode` to a JS
  string, the browser encodes.)
- Text client on the VM: `ascii.encode` is 2.2× faster than `utf8.encode`
  for ASCII strings and throws on the first non-ASCII character; then we
  fall back to `utf8.encode`. Worst case (long ASCII prefix, non-ASCII at
  the end) costs ~1.5× `utf8.encode`. **Adopted provisionally**; revisit if
  real traffic is mostly non-ASCII.
- `latin1.encode` is as fast but encodes U+0080..U+00FF as one byte, which
  is wrong UTF-8. Not usable without a separate check.

## Client core behaviour found by tests

### D21. A fire-and-forget `connect()` never raises an unhandled error

- `connect()` fails when the attempt fails terminally or when
  `disconnect()`/`close()` comes first. Apps (and Flutter widgets) often
  call `client.connect(url)` without awaiting it; a later `close()` then
  raised an *unhandled* asynchronous error into the zone (found by a test).
- Options: complete the future normally on disconnect (hides real
  failures from callers who await); keep failing and let it crash
  unawaited callers; fail, but mark the future as handled.
- Decision: the waiter future is returned with `.ignore()` applied. An
  `await`ing caller still receives the error (ignore only adds a handler);
  an unawaited one does not crash. The failure is always visible through
  `state` (`disconnected(error: …)`) and `onError`.
- Status: **adopted**.

### D22. State events are queued; callbacks may re-enter the client

- `states` is a synchronous broadcast stream (lowest latency, no
  microtask per event). A listener that calls `close()`/`disconnect()`
  from inside a state event made the client add an event while the
  controller was still firing: `Bad state: Cannot fire new event`
  (found by an integration test with `states.firstWhere(...)`).
- Decision: `_setState` updates `state` immediately and appends to a
  queue; one loop delivers queued events in order. Re-entrant calls from
  `onStateChange` or `states` listeners are safe and keep ordering.
- The same rule applies to new callbacks: never call a sync controller
  from code a listener can reach without a guard.
- Status: **adopted**.

### D23. Time comes from `package:clock`

- `DateTime.now()` and `Stopwatch()` are not faked by `fake_async`, so the
  RPC deadlines and the heartbeat could not be tested in fake time.
  `package:clock` (dart-lang, no dependencies) is faked by `fake_async`,
  and `clock.stopwatch()` is still monotonic in production.
- Status: **adopted**. Dependencies of `ws` are now `clock`, `meta`, `web`
  (plus `hooks`/`code_assets` only until `ws_native` is split out, D8).

### D24. JSON numbers through JS interop may arrive as `double`

- In dart2wasm a number that crossed JS interop is a `double` even when
  integral (a test helper failed with `double is not a subtype of int`).
  The web transport therefore decodes JSON text with `dart:convert` (ints
  stay ints) and never takes numbers from JS objects. `WebSocketJsonRpc`
  accepts `num` ids for the same reason.
- Status: **adopted**.

### D25. `ws_native` is published from a staged copy

- The repository root is the `ws` package, and `ws_native` lives in
  `native/`. Pub does **not** skip nested packages: without an ignore rule
  the `ws` archive would contain `native/`, including every prebuilt
  library. But pub also applies ignore files of parent directories
  (verified): with `/native/` in the root `.pubignore`, publishing from
  `native/` hides everything ("The pubspec is hidden"), and negations in
  `native/.pubignore` (`!*`, `!**`, `!/**`) cannot re-include files of an
  excluded directory.
- Options: restructure into a pub workspace with both packages under
  `packages/` (moves the `ws` package root; large churn); move `ws_native`
  to its own repository (splits CI, the conformance suite and the
  benchmarks); publish `ws_native` from a copy outside the repository.
- Decision: `tool/stage_native.sh <dir> [ws-constraint]` copies `native/`
  plus `LICENSE` and replaces the path dependency on `ws` with the
  released version; CI publishes from there after building the prebuilt
  libraries. Verified with a dry run: the staged archive contains
  `prebuilt/`, `hook/`, `lib/`, `rust/` and no build output.
  `native/analysis_options.yaml` is standalone for the same reason.
- Status: **adopted**; revisit if the repository becomes a pub workspace.

### D26. No shared completed futures across zones

- `push()` returned one top-level `Future<void>.value()` for the fast
  path. A completed future runs its `then` callbacks through the zone it
  was *created* in, which was the zone of the first caller. In a second
  `FakeAsync` zone (and equally in Flutter tests or any app using zones)
  the callbacks went to a dead zone and never ran (found by the
  backpressure test).
- Decision: return a new `Future<void>.value()` each time. Cost: one small
  allocation per `push`; correctness first.
- Rule: never cache futures, completers or timers in top-level or static
  variables in this package.
- Status: **adopted**.

## Verification of the implementation

### D27. The library keeps the prototypes' numbers

Benchmark variants `ws-dart` and `ws-native` run the library through its
public API (`WebSocketClient.json()/.binary()`, all reliability features
on, reconnect off) next to the prototypes (`benchmark/bin/matrix.dart
--quick`, macOS M-series, one run per cell, raw data in
`benchmark/results/library-vs-prototypes-macos.jsonl`). Selected rows:

| case | c (proto) | ws-dart | d-tape (proto) | ws-native |
|---|---:|---:|---:|---:|
| stream json 14 KB → model, main µs/msg | 83.5 | 84.6 | 29.7 | 29.9 |
| stream json 60 KB, main µs/msg | 271 | 276 | 3.3 | 2.5 |
| stream wss json 14 KB → model, p50 latency µs | 1458 | 870 | 232 | 210 |
| flood wss json 14 KB → model, MB/s | 102 | 105 | 451 | 462 |
| flood wss binary 60 KB, MB/s | 226 | 222 | 2939 | 2677 |
| flood json 60 KB → model, peak RSS MB | 61.8 | 62.0 | 226 | 108 |
| ping-pong wss json 14 KB → model, msg/s | 3151 | 2922 | 4687 | 4791 |

- The client core, codecs and the transport boundary cost nothing
  measurable: `ws-dart` ≈ `c`, `ws-native` ≈ `d-tape`, within run-to-run
  noise.
- Receive credits (8 MiB window) cut the native flood RSS by up to half
  compared with the prototype, at the same throughput.
- Status: **confirmed**. Re-run on mobile before the release (ROADMAP §6.1).

### D28. Autobahn TestSuite: no failures on either transport

`benchmark/bin/autobahn.dart` drives the fuzzingserver (Docker,
`crossbario/autobahn-testsuite`) at the transport level, echoing every
message with its type; invalid UTF-8 closes with 1007 like the client's
codecs. Cases 12.* / 13.* (permessage-deflate) are excluded: not
implemented. Summary: `python3 benchmark/autobahn/summary.py`.

| transport | OK | NON-STRICT | INFORMATIONAL | FAILED |
|---|---:|---:|---:|---:|
| pure Dart | 296 | 2 | 3 | 0 |
| native | 287 | 11 | 3 | 0 |

- The first pure-Dart run had 25 failures (1.1.6, 1.2.6, 9.*, 10.1.1),
  all one bug: the 64 KiB limit on the handshake response was checked
  *before* searching for the end of the head, so a 101 response arriving
  in the same read as a large first frame failed with "Handshake response
  is too large". Fixed (search first) and covered by a conformance test.
- NON-STRICT 6.4.3/6.4.4 (both): invalid UTF-8 inside a single text frame
  is detected when the frame is complete, not at the first bad byte that
  arrives. Allowed by the suite; fail-fast inside a frame would need
  validation on partial payloads (D5 keeps validation in the decoder).
- NON-STRICT 3.2, 3.3, 4.1.3–4.1.5, 4.2.3–4.2.5, 5.15 (native only): a
  valid message followed by an invalid frame. Rust fails the connection
  when it reads the invalid frame; the echo of the valid message is
  produced by Dart afterwards and is rejected, because the connection is
  already closing. Inherent to the pipelined design (reading continues
  while Dart handles earlier messages); the protocol outcome (close 1002)
  is correct.
- Status: **adopted** as a CI gate candidate (needs Docker in CI).

### D29. Findings of the native transport review

An independent review (Fable) of `native/` before merging found lifecycle
bugs the conformance suite could not trigger. Fixed:

- **Abort could hang** when the writer was parked in a write to a
  black-holed network (`feed`/`flush` are not cancellable): `serve` now
  aborts the writer task on abort and bounds every wait on it to 1 s.
- **TCP EOF reported as protocol violation 1002**: tungstenite reports a
  close without a close frame as `Protocol(ResetWithoutClosingHandshake)`;
  it is now a connection error, as on the pure-Dart transport.
- **Handshake-phase protocol errors** (bad `Sec-WebSocket-Accept`, missing
  upgrade header) are reported as `WebSocketHandshakeException`, not as a
  1002 close.
- **`bufferedAmount` leaks**: sends before open are rejected (an `open`
  flag) instead of being silently dropped; queued sends left when the
  writer stops are subtracted; the counter drops only after a successful
  flush, so it really means "not handed to the OS".
- **Close code after a protocol error** (1002/1007/1009) was sent through
  the command queue and lost about half the time to an unbiased `select!`;
  it is now sent directly after the writer stops.
- **Messages after `abort()`**: the decoder stops posting, and Dart drops
  every event but the final one.
- **Credits lost when a listener throws**: the grant is in a `finally`;
  otherwise the reader would stall forever.
- **Helper tasks orphaned on panic** (a reference cycle through
  `Arc<Shared>`): aborted by a drop guard. Poisoned table mutex recovered;
  a null `postCObject` pointer is rejected.
- **Hook**: linker flags (16 KB pages, headerpad, install name) moved to
  `rust/build.rs` (a user's `CARGO_ENCODED_RUSTFLAGS` would override
  target rustflags); `cargo build --locked`; the artifact is copied out of
  the shared cargo directory (cargo does not fingerprint deployment
  targets); unsupported targets or a missing NDK fall back to pure Dart
  instead of failing the app build.
- **Hook caching** (found while running Autobahn): hooks_runner treats a
  *missing* dependency file as modified now (`hooks_runner`
  `utils/file.dart`), so declaring `prebuilt/<target>/…` that did not exist
  made every build "File modified during build" and re-ran cargo. Only
  existing files are declared now.

Open (tracked in ROADMAP): ~~the native send queue is not bounded in
bytes~~ (closed by D36, `sendBufferLimit`); ~~small zero-copy payloads pin
tungstenite's 128 KiB read chunks~~ (not reproduced with a sound measure,
D50); ~~Happy Eyeballs for IPv6~~ (adopted in D34); TLS `close_notify`;
a port probe on `init`.

### D30. Sharing the FFI layer between `ws` (path) and `ws_native` (hook)

- Proposal: the FFI transport lives in `ws` and calls the library through
  function pointers. `ws` fills them with `DynamicLibrary.open(path)` +
  `lookup` (works with `dart compile exe`, no hook). `ws_native` fills them
  from its bundled code asset with `Native.addressOf(@Native function)` and
  registers the same transport. The bundled asset's file path is not
  exposed by any API, so pointers, not paths, cross the package boundary.
- Call cost, AOT on macOS arm64, `getpid` 20 M calls (scratch benchmark,
  third round): `@Native` leaf 3.22 ns, `addressOf().asFunction` leaf
  2.62 ns, `lookupFunction` leaf 2.61 ns; non-leaf 7.30 / 6.34 / 6.41 ns.
  The `@Native` rows include a closure call; the conclusion is "no
  difference" at ~30 µs per message, not "pointers are faster".
- Problem found while implementing: Dart allows `Uint8List.address` only as
  an argument of a *leaf `@Native`* call, not of a function-pointer call
  (`address_position` error in `asFunction(isLeaf: true)` calls, Dart
  3.13.7). `@Native` symbols cannot be bound to a library loaded by path:
  they resolve via the asset id, `Dart_SetFfiNativeResolver` or the process,
  and `DynamicLibrary.open` loads `RTLD_LOCAL` on Linux.
- Fix (ABI 2): `ws_native_alloc(len)` returns a Rust-owned buffer, Dart
  copies the bytes in, and `connect`/`send`/`close` take ownership
  (`Box<[u8]>` → `Vec`). Before, Rust copied out of the Dart buffer
  (`to_vec`), so the copy count stays at one; it moved from Rust to Dart.
  `ws_native_free` releases a buffer that was not handed over.
- Cost of the new send path (AOT, macOS arm64, `alloc` + `setAll` +
  `free`, scratch benchmark, third round): 16 B 21 ns, 1 KB 37 ns, 14 KB
  248 ns, 60 KB 733 ns. For comparison, allocating and filling a Dart
  `Uint8List` of the same size takes 4 / 41 / 565 / 2016 ns. Compared with
  the old path (malloc + memcpy inside Rust), the new one adds one leaf
  call and a view, about 15–20 ns, against ~30 µs per message. Echo
  benchmark (`--filter=echo`, 3 repeats): binary 60 KB `ws-native` RTT p50
  468–556 µs, against 565 µs before. The 1 KB case is dominated by timer
  noise (p50 270–287 µs, `d-tape` 213–247 µs in the same run).
- Verified:
  - the shared conformance suite passes on both paths: the hook path
    (`native/test`) and the path loaded by `WsNativeLibrary.open`
    (`test/native`, `WS_NATIVE_LIBRARY`);
  - an app built with plain `dart compile exe` depending on `ws` loads the
    library and installs it;
  - a missing file, and a library that is not `ws_native`, are
    `ArgumentError`s naming the path;
  - an ABI mismatch is an `UnsupportedError`.
- Status: **adopted** (2026-11-12).
- Shipping: prebuilt libraries stay inside the `ws_native` archive (D9).
  Size check: `libws_native.dylib` (macOS arm64, release, `strip = true`,
  thin LTO) is 2.19 MB, 1.13 MB with `gzip -9`; about 12 targets ≈ 26 MB
  unpacked, ≈ 13 MB in the archive, below pub's 100 MB archive limit.
  Server apps can take the file from `ws_native` as a **dev** dependency:
  verified on 3.13.7 that a hooked dev dependency does not trigger the
  `dart compile exe bin/…` check.

### D31. Prebuilt libraries stay inside the `ws_native` archive

- Survey: docs/NATIVE_DISTRIBUTION.md. Alternatives compared:
  1. **Hook downloads from GitHub releases with pinned SHA-256**, as
     `sqlite3` 3.x does. Small archive. The issues it causes are
     documented: TLS on Windows, proxies, no network in Nix or CI
     sandboxes, infosec teams rejecting downloads during the build.
     Offline mode for hooks is still an open request (native#1620).
  2. **Compile in the hook only**, as `native_toolchain_rust` users do.
     Every user needs rustup, Android NDK setup and Windows MSBuild.
     `super_native_extensions` ran into these within a week of switching.
  3. **Bundle in the archive** (the Dart team's
     `prebuilt_assets_example`). Builds work offline and are hermetic.
     `pubspec.lock` pins the archive hash. Every user downloads every
     target once per version: about 13 MB gzipped (D30), within pub's
     100 MiB compressed / 256 MiB uncompressed limits.
- Decision: 3, with cargo as a fallback when a target has no prebuilt
  library (development checkouts, git dependencies, unusual targets). We
  never require rustup from pub users. Keep the SHA-256 check against
  `checksums.json`: it costs little and catches a modified pub cache.
- Adopted from the survey:
  - one file name, `libws_native.dylib`, on every Apple target (Flutter
    drops architectures whose names differ);
  - `false_secrets` for `prebuilt/**` if pub's leak validator ever flags a
    binary;
  - user-defines that accept a per-OS map, as in `sqlite3` 3.6+, for
    example `source: {default: prebuilt, linux: build}` (not implemented
    yet);
  - a supported path for `dart compile exe` (D30). `sqlite3` 3.x dropped
    `DynamicLibrary` overrides and has none.
- Status: **adopted**. Revisit if pub.dev starts hosting per-platform
  artifacts.

### D32. Getting the library for `dart compile exe` apps: `dart run ws:install`

- Proposal (maintainer): `ws` gets a CLI that downloads the library for a
  given target from the GitHub release of the same version, e.g.
  `dart run ws:install --target linux-x64 --out /app/lib`, and the app
  loads it by path (D30).
- Compared with taking the file from `ws_native` as a dev dependency (D30):
  `ws` stays hook-free and small, and nobody downloads every target. The
  download happens once, explicitly, where the network is there anyway
  (`pub get` in the Dockerfile). The build-time download problems of D31
  (Flutter builds, CI sandboxes) do not apply, because the step is opt-in
  and outside the app build.
- Requirements:
  - SHA-256 per target pinned in `ws` source and generated by the release
    workflow, as `sqlite3` does in `asset_hashes.dart`;
  - the release order is: build libraries, create the GitHub release,
    then publish `ws` and `ws_native`;
  - the ABI version is checked at load (D30);
  - proxies are taken from the environment
    (`HttpClient.findProxyFromEnvironment`).
- Flutter apps keep using `ws_native`, a package with the same binaries
  in the archive and the hook. The maintainer suggested naming it
  `ws_flutter`, but the hook also serves `dart build cli`, `dart run` and
  `dart test` without Flutter, so the name stays. A future `ws_flutter`
  would be a separate package for Flutter-only features (reconnect on app
  resume, connectivity) that depends on `ws_native`.
- Implemented: `bin/install.dart` (`dart run ws:install --target --out
  --url --sha256 --list`) and `lib/src/native/install.dart`.
  - The download is written to `<file>.part`, hashed while streaming, and
    renamed only after the hash matches. An existing file with the right
    hash is kept.
  - Without a known hash nothing is downloaded.
  - `file://` mirrors work too, which helps offline builds and tests.
  - Own incremental SHA-256 (`lib/src/native/sha256.dart`), checked
    against the FIPS 180-4 vectors and against `shasum -a 256` on a 2 MB
    library.
  - `lib/src/native/native_release.dart` (version, tag and hashes) is
    regenerated by `tool/native_release.dart`, and a test checks it
    against `pubspec.yaml`.
- Release asset names: `<target>-<file>`, for example
  `linux-x64-libws_native.so`. The hash keys are `<target>/<file>`, the
  same as in `native/prebuilt/checksums.json`. The tag is the package
  version.
- Status: **adopted**. The release workflow is D43.

### D33. Project tasks in mise

- Compared: `make` (the old `Makefile`), `just`, mise tasks.
- Decision: mise, because the repository already pins Dart with it, so
  tools and tasks live in one file (`mise.toml`).
  - mise also pins Rust (through rustup, with rustfmt and clippy), `gh`,
    `actionlint` and `shellcheck`.
  - Task dependencies run in parallel, so `mise run check` (format,
    analyzers, all tests) takes about as long as its slowest suite.
  - `make` needs `.PHONY` boilerplate and is awkward on Windows. `just`
    would be one more tool that pins nothing.
  - The `Makefile` was removed; its targets are tasks now.
- Longer task logic lives in `tool/*.sh`, checked by shellcheck, so CI
  can call the same scripts. `prebuilt.yml` calls `tool/build_native.sh`.
- Found while adding the tasks:
  - The `ws_native` dry run failed with "The pubspec is hidden" when
    staged under `build/` inside the repository. Pub applies the
    repository's `.gitignore` there too, so staging goes to a temporary
    directory (`tool/check_publish.sh`).
  - `ws_native` had no README or CHANGELOG. Both were added.
- Verified locally: `tool/build_native.sh` built `macos-arm64`,
  `ios-arm64`, `android-arm64` and `android-arm` (NDK 28.2).
  `llvm-readelf` shows 16 KB alignment, and `native_checksums.dart` wrote
  the hashes. `prebuilt.yml` has not run on GitHub yet. Windows arm64
  (`ring` needs clang) and `ubuntu-22.04-arm` are unverified.
- Status: **adopted**.

## Networking

### D34. Happy Eyeballs in the Rust transport

- Before: `TcpStream::connect((host, port))` tried the resolved addresses
  one after another. A black-holed first address (an IPv6 route that does
  not exist, a dead A record) stalled the connection for the OS connect
  timeout (~75 s) before the next address was tried.
- `dart:io` already staggers attempts: `socket_patch.dart` has a 250 ms
  `_retryDuration` and `connectNext` (checked in the 3.13.7 SDK sources).
  The two transports must behave the same (§3 of the roadmap).
- Now (`native/rust/src/net.rs`): addresses from `getaddrinfo`
  (`tokio::net::lookup_host`, so iOS synthesizes NAT64 addresses for
  IPv4-only names) are interleaved by family, starting with the family
  of the first answer (RFC 8305 §4). A new attempt starts when the
  previous one fails or after 250 ms, whichever is first. The first
  success wins and the other sockets are dropped.
- Tests: `net::tests` with paused tokio time: a quick first address
  starts no second attempt; a hanging first address loses to the second
  at exactly 250 ms; failures start the next attempt immediately; an
  earlier attempt can still win; the last error is reported.
- Not covered, same as `dart:io`: an IPv4 *literal* in the URL on an
  IPv6-only (NAT64) network. Both parse the literal and never call
  `getaddrinfo`, so no synthesized address exists. Use host names.
- Status: **adopted**.

### D35. TLS: the operating system's trust store

- Before: only the bundled Mozilla roots (`webpki-roots`). Corporate,
  MDM-installed and user-trusted CAs worked with `dart:io` but failed on
  the native transport.
- Options: `rustls-platform-verifier` everywhere; `rustls-native-certs`
  everywhere; a mix.
  - `rustls-platform-verifier` 0.7.1 on Android needs a JavaVM and an app
    `Context` passed through JNI, plus its Kotlin component. A Dart FFI
    library gets neither.
  - `rustls-native-certs` has no iOS support and cannot see Apple trust
    settings.
- Decision (`native/rust/src/tls.rs`):
  - Apple and Windows: `rustls-platform-verifier` (Security.framework,
    CryptoAPI), including user trust settings. Verification may block
    (revocation, intermediate fetch), so it runs inside
    `tokio::task::block_in_place` on the multi-thread runtime.
  - Linux: `rustls-native-certs` (honours `SSL_CERT_FILE`/`SSL_CERT_DIR`).
  - Android: PEM files from `/apex/com.android.conscrypt/cacerts`
    (Android 14+, updatable) or `/system/etc/security/cacerts`.
    User-installed CAs are not trusted, which matches the Android 7+
    default for apps.
  - If no system root loads (a minimal container), fall back to
    `webpki-roots`. `trustedRootsPem` adds roots on top.
- The default `ClientConfig` is built once per process and shared, so
  rustls' in-memory session cache resumes TLS sessions across reconnects
  (before, every connection had a new config and a cold cache).
- Size: the macOS dylib went from 2,192,992 to 2,144,848 bytes (`webpki-
  roots` is no longer linked there). Android arm64: 2,331,304 bytes.
  `prebuilt.yml` built all 12 targets on GitHub with the previous code;
  the new dependencies have been built locally for macOS, iOS and
  Android only.
- Status: **adopted**. A latency comparison of the platform verifier vs
  webpki on a real handshake is still to do.

### D36. HTTP CONNECT proxy and a hard send-queue limit

- Proxy: `WebSocketVmOptions.proxy` (`WebSocketProxy(host, port,
  username, password)`, or `.parse('http://user:pass@host:port')`). Both
  native transports open the tunnel themselves, then run TLS end to end
  through it.
  - Dart: `lib/src/transport/dart/proxy_tunnel.dart` writes `CONNECT`,
    reads the head and hands the paused subscription to
    `RawSecureSocket.secure`, or re-attaches its handlers for `ws://`.
    Bytes after the proxy's head are a protocol error: the server cannot
    speak before our request.
  - Rust: `net::tunnel` reads the head byte by byte, so nothing that
    belongs to the tunnel is consumed.
  - Credentials are sent as `Proxy-Authorization: Basic`. System proxy
    settings (iOS `CFNetwork`, Android `ProxySelector`, PAC) are not read;
    see ROADMAP §4.6.
- Send limit: `WebSocketOptions.sendBufferLimit` (default 16 MiB). While
  more than that is queued, `push()` fails with
  `WebSocketBufferFullException` and the connection stays open. Checked
  in the client before encoding, so all transports share it, and the
  queue may exceed the limit by one message. This closes the D29 item
  "the native send queue is not bounded in bytes".
- Status: **adopted**. End-to-end proxy tests run against the test server
  (`server_suite.dart`, group `proxy`; D48 adds the reconnect decisions).

### D37. Behaviour fixed by the new tests

Ported scenarios (TEST_PLAN) found three real defects:

- **Messages after `disconnect()`** were still delivered while the
  closing handshake ran. Browsers drop them (HTML: a message received
  when the state is not OPEN is ignored), and so does
  `web_socket_conformance_tests`. The client now drops them
  (`_Session._delivering`).
- **Non-Latin-1 user info** in the URL (`ws://юзер:пароль@h/`) threw from
  `latin1.encode` while building the request. It is now UTF-8, as
  RFC 7617 recommends.
- **The Dart tape builder accepted invalid numbers** (`01`, `1.`, `1e`).
  It now follows the RFC 8259 grammar, like simd-json on the Rust side. A
  conformance test feeds a raw JSON corpus (escapes, surrogates, number
  edges, duplicate keys) through every transport's decoder and compares
  the result with `jsonDecode`.
- Status: **adopted**.

### D38. Client transitions: mutate first, notify last

- An independent review (`docs/review/client.md`) reproduced four
  critical client bugs, all from user callbacks running in the middle of
  a transition: `reconnect()` from `onError` leaked a live connection;
  `disconnect()` from `onError` left the state on `reconnecting`;
  `disconnect()` during `onOpen` was undone; a listener's `connect()` got
  the previous attempt's result. Also `close()` hung with a paused
  subscription, and `close(1001)` left a half-closed client.
- Rule now: every field (session, timers, failure counters, waiters) is
  final before any user code runs: state events, `onError`,
  `shouldReconnect`, waiter completions. Waiters are detached before the
  state event and completed after it. The reconnect timer is armed before
  `onError`, and `reconnecting` is emitted only if the callback did not
  change course.
- Related fixes: `close()` validates first and returns the first call's
  future; stream controllers are closed without awaiting; a dropped
  session always completes its `disconnect()` future; connections dropped
  without a handshake are aborted by `close()`; one connect budget covers
  the request provider, the handshake and `onOpen`; the heartbeat counts
  inbound frames instead of comparing elapsed time (a timeout ≥ interval
  killed healthy connections); only `stableAfter` resets the backoff (a
  greet-then-close server was hammered 408 times in 5 minutes); a zero
  `minDelay` still backs off (100 ms base); 1013 waits `maxDelay` plus
  jitter; `Retry-After` on 429/503 is honoured (≤ 10 min); invalid
  request headers, subprotocols or user info are terminal; RPC ids are
  unique, unknown ids are delivered as messages (only replies to
  abandoned calls are dropped), calls fail at `disconnect()` with
  `disconnected` and at `close()` with `closed`, a refused send fails
  with its own error; user-code errors go to the zone when `onError` is
  absent; `bytesSent` is the encoded size on every transport.
- Each finding has a regression test in
  `test/client/client_review_test.dart`; without the fixes those tests
  fail or hang.
- Status: **adopted**.

### D39. Native transport: own client handshake, ABI 3

- tungstenite's client handshake rejects a server that selects none of
  the offered subprotocols (browsers and the Dart transport accept it),
  ignores repeated `Sec-WebSocket-Accept`/`-Protocol` and unoffered
  extensions, and sent `GET ?q` for a URL without a path. The handshake
  is now done in `native/rust/src/handshake.rs` with the same rules as
  `handshake.dart`; tungstenite takes over after the 101
  (`from_partially_read`).
- Receive credits charge 64 bytes per message on top of the payload, and
  pongs are charged too (a flood of empty messages reached 585 MB RSS
  with the 8 MiB window). The JSON tape's extra size is charged when it
  is built.
- The config JSON rejects unknown fields, so an older library cannot
  silently ignore a setting such as `proxy`. Together with the pong event
  carrying credits this is ABI 3.
- Status: **adopted**.

### D40. Portable binaries and a checked release chain

From `docs/review/distribution.md`:

- Linux libraries are linked with `cargo zigbuild` against glibc 2.17
  (they needed 2.34 and failed on Debian 11, RHEL 8, Amazon Linux 2) and
  with `-static-libgcc` (they needed `libgcc_s.so.1`, missing in
  `FROM scratch` images). Windows DLLs link the C runtime statically
  (`native/rust/.cargo/config.toml`).
- `tool/verify_native.sh` runs after every CI build: exactly 13
  `ws_native_*` exports, 16 KB LOAD alignment on Android, glibc ≤ 2.17
  and an allow-list of `NEEDED` libraries on Linux, the Apple install
  name, no VC++ runtime import on Windows.
- `package:ws_native/ws_native.dart` exports a stub on the web
  (`install()` returns `false`), so a Flutter app that targets the web
  can import it unconditionally. `WsNative.unavailableReason` says why
  the library is not used (missing asset or ABI mismatch).
- `ws_native` pins `ws` to the exact same version. `tool/native_release.dart`
  fails without `checksums.json` or with fewer than 12 hashes. The hook
  rejects an unknown `source:` and accepts a per-OS map.
  `dart run ws:install` accepts `--key value` (the README form) and
  validates its arguments.
- The old tag-triggered `deploy.yml` no longer runs on tags: it would
  publish `ws` alone, without native hashes. (Deleted in D43.)
- CI builds the test server and the native library for the VM tests on
  Linux, macOS and Windows, fails (instead of skipping) when the server is
  missing, and lints and tests `server/`.
- ~~Open: the release workflow itself, third-party licence notices for the
  Rust crates, pinned action SHAs and Dependabot for cargo.~~ Done in D43.
- Status: **adopted**; zigbuild and the Windows static CRT are verified
  only by CI.

### D41. Liveness by default: heartbeat, upload progress, TCP keepalive

From `docs/review/future_risks.md` R3, R4, 6.7, 10.1–10.3. A half-open
connection (radio loss, NAT rebinding, a load balancer reaping without a
reset) is the normal failure on mobile; with the old defaults a client
sat in `open` forever.

- `WebSocketOptions.heartbeat` defaults to `WebSocketHeartbeat()` (ping
  after 30 s without inbound traffic, 10 s to answer); `null` disables
  it. In the browser it stays inert unless an application-level `ping`
  is configured (pages cannot send protocol pings).
- Upload progress counts as liveness: when the probe times out but the
  send queue drained since it was sent (the ping waits behind a large
  upload on a slow link), the probe is repeated instead of dropping the
  connection. Pushing more is not progress. A queue that stops draining
  with no inbound traffic is a dead connection (10.3).
  Limitation: only bytes in our queue are seen; data already in the OS
  send buffer is not, so the last few hundred KB of an upload still have
  to fit in `timeout`.
- The native writer now lowers `buffered` as soon as the sink accepted
  the next message (everything before it is written, apart from at most
  tungstenite's 128 KiB write buffer), not only after the queue emptied
  and flushed. Before, an 8 MiB upload showed 8 MiB queued until the end,
  and the heartbeat dropped it after 1.8 s (server suite "a slow upload
  delays the pong without dropping"); now the count falls by about 1 MiB
  every 500 ms at 2 MiB/s and the test passes on both transports.
- TCP keepalive on both VM transports, `WebSocketVmOptions.keepAlive`,
  default 30 s: probes after 30 s idle, every 10 s, 3 probes; on Linux
  and Android `TCP_USER_TIMEOUT` = 60 s bounds unacknowledged data (a
  black-holed write). Rust uses `socket2`; Dart sets the same raw
  options (option numbers per platform in `keep_alive.dart`). Windows
  ignores the probe count (10). Best effort: refused options are
  ignored. Verified by reading the options back on macOS and Linux
  (`test/transport/keep_alive_test.dart`, Rust `keepalive_is_set`);
  Windows by CI.
- The ping schedule stays in Dart for every transport instead of moving
  into Rust (D15 said otherwise). One implementation serves the
  application-level ping that the web needs anyway; Rust already answers
  server pings without the isolate. The risk named in 6.7, a blocked
  isolate declaring a healthy connection dead, does not happen: after a
  gap the client probes first, and a pong that arrived while the isolate
  was blocked is delivered before the late timeout timer (server suite
  "a blocked isolate does not drop a healthy connection": six 500 ms
  blocks with a 100 ms interval and 200 ms timeout, both transports).
  Kernel keepalive covers the suspended app.
- Status: **adopted**.

### D42. Typed connection failures; the exception set is frozen

From `docs/review/future_risks.md` R8 and the API notes in
`docs/review/client.md`. Apps could not tell "offline" from "wrong
certificate" from "check the device clock": DNS, TCP, TLS and proxy
failures were one `WebSocketConnectionException` with a string.

- The sealed set of `WebSocketException` subclasses stays as it is and is
  fixed for 2.x, so exhaustive `switch`es over exception types keep
  compiling. Finer reasons are enums inside the classes, documented as
  "new values may be added in minor releases, switch with a default".
- `WebSocketConnectionException.failure`: `dns`, `refused`,
  `unreachable`, `tls`, `proxy`, `lost` (an established connection
  dropped without a close frame), `other` (also every connect failure in
  the browser, which hides the reason). `tls`: `untrusted`, `hostname`,
  `expired`, `notYetValid`, `revoked`, `handshake`, `other`.
  `proxyStatus`: the proxy's HTTP status. `toString()` shows the reason,
  e.g. `WebSocketConnectionException(tls: expired): …`.
- Native: `net::connect` returns a typed `ConnectError` (lookup, TCP,
  proxy with status); rustls certificate errors map to the TLS reasons.
  The Apple verifier reports expiry only as text ending in the OSStatus,
  so `-67818` (`errSecCertificateExpired`) and `-67819`
  (`errSecCertificateNotValidYet`) are mapped from it (codes checked in
  the SDK's `SecBase.h`). The closed event gains a `detail` element
  (index 7); Dart reads it only when present.
- Pure Dart: `SocketException` → DNS ("Failed host lookup"), refused
  (`ECONNREFUSED` 111/61, Windows 10061 and 1225, the code `dart:io`
  reports there), otherwise unreachable. TLS on Apple
  platforms is reported by `dart:io` only as "application verification
  failure", so the transport records the certificate in
  `onBadCertificate` and classifies by its validity dates and its
  subjectAltName (a minimal DER reader; RFC 6125 matching with one
  leading wildcard label); other platforms fall back to the BoringSSL
  text.
- Reconnect policy (D14, now implemented): certificate failures wait the
  maximum delay plus jitter, like 1013 (captive portals and TLS
  inspection cause them; retrying at 100 ms helps nobody); a TLS
  handshake failure keeps the normal backoff. A proxy 407 is terminal:
  the credentials are fixed in the options.
- Verified end to end on both VM transports against the test server,
  which now also serves an expired certificate (`wss_expired`): unknown
  CA → `untrusted`, wrong host → `hostname`, expired → `expired`,
  `nonexistent.invalid` → `dns`, port 1 → `refused`, proxy 407/502 and a
  dead proxy → `proxy`, FIN/RST mid-connection → `lost`.
- Names: the `WebSocket` prefix and the `$` state names stay (the
  maintainer's convention, as in ws 1.x and spinify). The doc of
  `WebSocketException` tells users who also import `dart:io` to import
  this package with a prefix.
- Status: **adopted**.


### D43. One tag releases the packages

From `docs/review/distribution.md` F6–F8, F14, F15, F19, F20.

- `.github/workflows/release.yml` runs on a version tag (`2.0.0`,
  `2.0.0-dev.1`). The jobs, in order:
  1. `check`: `tool/release_check.dart` checks that the tag equals the
     version of both pubspecs, `wsVersion` and the first heading of both
     CHANGELOGs.
  2. `build`: the 12 targets come from `prebuilt.yml`, which verifies each
     binary (D40). Tag builds use no cache.
  3. `notices`: `cargo about` writes `THIRD_PARTY_LICENSES`.
  4. `package`:
     - `native_checksums.dart --check` verifies the libraries;
     - `native_release.dart` writes the hashes and the commit into
       `package:ws`;
     - `ws_native` is staged with the notices appended to its `LICENSE`;
     - both publish dry runs run;
     - the release assets are collected.
  5. `release`: a draft GitHub release with the assets
     (`<target>-<file>`, `checksums.json`, `THIRD_PARTY_LICENSES`,
     `native_release.dart`) and a build-provenance attestation of the
     libraries.
  6. `publish` (GitHub environment `pub.dev` with a required reviewer):
     undraft the release, then publish `ws`.
  7. `publish-native`: wait until pub.dev serves that `ws`, then publish
     `ws_native`.

  A push to `rewrite` that touches the release tooling runs steps 1–4
  without releasing anything. This is how the workflow is tested.
- **The hashes are not committed.** They exist only after the build. The
  published `ws` therefore differs from the tag by one file,
  `lib/src/native/native_release.dart`. That file is a release asset, and
  it names the commit the libraries were built from. The alternative,
  committing the hashes first, would need bit-for-bit reproducible builds
  on five operating systems.
- **Every release carries the libraries** (F20). `nativeReleaseTag` stays
  equal to `wsVersion`, so a Dart-only patch also rebuilds and attaches
  all 12 targets. This costs one CI run and keeps `ws:install` simple.
- **The draft is published before `ws`.** `dart run ws:install` therefore
  never points at assets that do not exist yet. If publishing fails
  afterwards, the publish jobs can be re-run.
- **`ws_native` allows exactly one `ws`.** pub warns about a
  single-version constraint, which would fail the dry run, so the pin is a
  range with nothing else inside it:
  - `>=2.1.0 <2.1.1` (pub excludes `2.1.1-dev.x`);
  - `>=2.1.0-dev.3 <2.1.0-dev.3.0`.

  Checked with `pub_semver`: neither neighbour is allowed.
- **Staging uses `git ls-files native` plus `prebuilt/`** (F19). Untracked
  files are never published. `test/` and the Rust tooling configuration
  stay out. A `--release` staging fails unless `checksums.json` lists
  exactly the 12 targets and matches the files.
- **Licences** (F8):
  - `cargo deny` (`deny.toml` in `native/rust` and `server`) allows only
    MIT, Apache-2.0 (also WITH LLVM-exception), ISC, BSD-3-Clause, Zlib,
    Unicode-3.0 and CDLA-Permissive-2.0. It also checks RustSec
    advisories, wildcard versions and unknown sources.
  - `audit.yml` runs it on dependency changes and weekly.
  - The notices go into the `ws_native` `LICENSE` after a line of 80
    hyphens, so Flutter's `showLicensePage` lists them. They are also a
    release asset, and `ws:install` prints their URL.
  - 121 crate entries today. Platform-only crates (jni, schannel,
    security-framework) are included.
- **Toolchains** (F15):
  - `rust-toolchain.toml` in `native/rust` and `server` pins Rust 1.99.0,
    the same version as `mise.toml`. `tool/rust_toolchain.sh` installs it
    in CI and fails if the three disagree.
  - zig and cargo-zigbuild are pinned too.
  - Dart for format, analyze and release is 3.13.7. The test jobs stay on
    `stable` as an early warning.
  - A job runs the lowest bounds: Dart 3.10.0 with `pub downgrade` for
    `lib` and `bin`, and the VM tests on 3.10.0. Tests cannot run with
    downgraded dev dependencies, because analyzer 6 does not build on
    Dart 3.
- **Supply chain** (F14, F29):
  - every action is pinned to a commit SHA, with the version in a comment;
  - checkout v7, upload-artifact v7, download-artifact v8 (Node 24);
  - `setup-chrome` is dropped, because the runner's Chrome is used;
  - `permissions: contents: read` is the default, and write permissions
    are granted per job;
  - Dependabot covers pub (`/`, `/native`), cargo (`/native/rust`,
    `/server`) and github-actions.
- `deploy.yml` is deleted. `release:hashes` in mise now verifies
  `checksums.json` instead of regenerating it (F7b).
- Manual setup, once (DEVELOPMENT.md):
  - the first `ws_native` upload, because OIDC cannot create a package;
  - automated publishing on pub.dev for both packages, with tag pattern
    `{{version}}` and environment `pub.dev`;
  - required reviewers on that environment;
  - immutable releases in the repository settings.
- Status: **adopted**. The dry-run path is verified in CI. The tag path
  (draft, attestation, OIDC publish) can only be verified by the first
  real tag.

### D44. Suspend and resume; `ws_flutter` ties them to the app lifecycle

From `docs/review/future_risks.md` 2.1, 2.3, 2.5, 3.2, 3.3 and 5.1
(Top 20 #2).

- **Why.** Android and iOS freeze a backgrounded app; the kernel may kill
  its sockets, and neither side learns it until a timeout. On return, Dart
  timers that are overdue fire together. Without help the client notices a
  dead socket only through the heartbeat (up to 30 s idle + 10 s timeout),
  then waits for the backoff. Meanwhile, a backgrounded app that keeps
  reconnecting in Doze only drains the battery.
- **`suspend({bool disconnect = false})`** (synchronous):
  - stops the heartbeat and any reconnect wait; no attempt starts until
    `resume()`, `connect()` or `reconnect()`;
  - an open connection stays open, so a short trip to another app costs
    nothing, and Android keeps delivering messages while the process runs;
  - `disconnect: true` closes it with 1000 "Suspended" (browsers reject
    1001 from scripts, D38) and fails pending calls;
  - a connection that ends while suspended is not retried: the state
    becomes the new `WebSocketState.suspended(cause:)`. Terminal failures
    (close 1008, 4xx) still end in `disconnected`;
  - pending `connect()` futures keep waiting; nothing happens before
    `connect()` or after `close()`.
- **`resume()`**:
  - resets the backoff (`attempt` 0);
  - open connection: one ping, the application ping when configured.
    **Any** inbound frame answers it, not only the pong (a backlog of
    messages proves the socket alive), and a draining upload extends the
    wait, as for the heartbeat (D41). No answer within the heartbeat
    timeout (10 s with the heartbeat off): dropped and replaced with a
    zero delay. A connection lost while the check is pending is also
    replaced at once. Then the heartbeat restarts;
  - no ping available (browser without an application ping): the
    connection is trusted;
  - no connection: connects now; still connecting: waits for that attempt;
  - completes when the connection is confirmed or open again; concurrent
    calls share one check;
  - the same call serves network changes (2.5): a socket bound to the old
    interface fails the check.
- **`connect()` and `reconnect()` end the suspension.** An explicit request
  to be connected wins, for example a push message that wakes the app to
  sync. `connect()` to the same URL while suspended is `resume()`.
- **`ws_flutter`** (`flutter/`, published with the same version and tag
  as `ws`, D43): `WebSocketLifecycle(client, {disconnectInBackground,
  networkChanges, binding})` on `AppLifecycleListener`:
  - `paused` → `suspend()`. Only Android and iOS report `paused`; a
    minimized desktop window or a background tab is never suspended;
  - `resumed` after `hidden` or `paused` → `resume()`. `inactive` alone
    (a dialog, the notification shade) does nothing;
  - each `networkChanges` event in the foreground → `resume()`. It is a
    `Stream<Object?>`, so the package has no plugin dependency;
    `connectivity_plus`' `onConnectivityChanged` fits. Events in the
    background are ignored: they would end the suspension.
  - CI tests it on Flutter 3.38.1 (the first with Dart 3.10.0; 3.38.0 ships a 3.10.0
    beta that `^3.10.0` rejects) and
    3.47.9. `tool/flutter_sdk.sh` downloads the official archive and
    checks its SHA-256 against Flutter's release manifest.
- **Measured** on loopback, 20 runs each, heartbeat timeout 600 ms
  (`suspend()` then `resume()`):

  | Transport | live peer (`/echo`) | silent peer (`/silent`) |
  |---|---|---|
  | pure Dart | median 0.14 ms, max 1.8 ms | median 606 ms, max 609 ms |
  | Rust | median 0.12 ms, max 0.6 ms | median 605 ms, max 607 ms |

  A dead socket is replaced about 6 ms after the timeout (new TCP and
  handshake included), instead of interval + timeout + backoff.
- Tests: 20 in `test/client/lifecycle_test.dart` (fake transport and
  fake time), 3 on real sockets in the shared server suite (pure Dart and
  Rust), 6 in `flutter/test` (lifecycle transitions, network events,
  a real client over the fake transport).
- Not done: monotonic clocks stop during device sleep (2.2), so
  `stableAfter` and RPC deadlines stretch; the lifecycle event, not clock
  arithmetic, drives the decision. A field test on devices (T-resume:
  30 s, 5 min, 2 h in the background on iOS and Android) needs hardware.
  Background keep-alive is impossible on both platforms (2.3); the README
  says so and points to push messages.
- Status: **adopted**.

### D45. Trust model: pins, user CAs on Android, disabled system CAs

Review item R2 / future_risks 3.5: on Android the Rust transport read only
the system CA directory, so user and MDM CAs (corporate TLS inspection)
failed, user-*disabled* system CAs stayed trusted, and D16's SPKI pins did
not exist.

- **`WebSocketTls.pins`**: base64 SHA-256 of the leaf certificate's
  SubjectPublicKeyInfo, optional `sha256/` prefix (OkHttp/HPKP format,
  `openssl … | openssl dgst -sha256 -binary | base64`).
  - Checked **after** the chain: an untrusted chain is reported as
    `untrusted`, a trusted chain with another key as the new
    `WebSocketTlsFailure.pinMismatch` (a certificate failure, so retried
    at the maximum delay, D14). `allowInsecure` skips the chain, never the
    pins: a pinned self-signed development server works.
  - **Leaf only, on every transport.** `dart:io` exposes only the peer
    certificate (`RawSecureSocket.peerCertificate`), no chain. Matching
    intermediates in Rust only would make the native transport and its
    pure-Dart fallback accept different servers. The docs ask for a
    backup pin.
  - Rust: a `ServerCertVerifier` wrapping the platform or webpki
    verifier, so Apple and Windows keep their OS evaluation. Dart: checks
    `peerCertificate` after `secure()`, direct and through the proxy
    tunnel. The DER walk to the SPKI is the same in both
    (`tls.rs` `spki`, `certificate.dart`), tested against OpenSSL 3.6
    pins (RSA, EC P-384) and an X.509 v1 layout (no `version`), and in
    Rust against rcgen's own SPKI encoding.
  - An invalid pin fails `connect()` once, without retrying
    (`tls: other`, cause `FormatException`).
  - SHA-256 reuses the incremental implementation of `ws:install` (D32),
    moved to `lib/src/util/sha256.dart`; no `package:crypto`.
- **`WebSocketTls.trustUserCertificates`** (default false, as Android 7+):
  Android also trusts `/data/misc/user/<uid / 100000>/cacerts-added`,
  as `<certificates src="user"/>` does for the platform stacks. Elsewhere
  it has no effect: Apple and Windows verifiers already use the system
  trust settings, including user and MDM CAs.
- **Disabled system CAs** (`cacerts-removed`) are always honoured on
  Android, by both transports, as Conscrypt does.
- **Verified on a device** (Samsung S22 Ultra, Android 16, API 36):
  - an app process is in group `everybody` (9997) and can list
    `/data/misc/user/0` (`drwxr-x--- system everybody`). `run-as` cannot
    (it lacks the group), so an `adb` check gives the wrong answer;
  - the user store holds DER files, the system stores PEM after a text
    dump; both loaders accept either;
  - `dart:io` already prefers the Conscrypt APEX over the system image
    (`runtime/bin/security_context_linux.cc`), as Rust did. The two differ
    a lot: on this device the APEX has 120 roots, `/system` 149, of which
    37 are not in the APEX (the distrusted Entrust and CommScope roots,
    15-year-old roots such as DigiCert Global Root CA and GlobalSign R1).
    Reading `/system` first would trust what Android no longer does;
  - reading the store takes 17–20 ms, loading it into a
    `SecurityContext` 10 ms. So the pure-Dart transport builds its own
    context only when it changes something (disabled CAs present, or
    user CAs asked for), once per setting; otherwise `dart:io`'s default
    stays (first connection with echo, one run each: 171 ms before, 105 ms after).
- **Configurations are cached per distinct setting** (roots, pins, user
  CAs, insecure), at most 16, in both transports. Before, any
  `trustedRootsPem` built a new rustls config per connection (cold
  session cache; native.md "custom roots rebuilding the TLS config"). A
  session resumes only under the configuration that verified it, so a
  pinned connection never resumes a session whose certificate was not
  checked against its pins (TLS 1.3 resumption skips the verifier).
- Tests:
  - Rust (30): pins after the chain, insecure plus pins, the cache
    (shared, order-insensitive, bounded), SPKI against rcgen and OpenSSL,
    the Android store layout (APEX first, disabled removed, DER and PEM).
  - Dart: SHA-256 (FIPS 180-4 vectors and padding edges), SPKI, pin
    parsing, the Android store layout, PEM round trip through
    `SecurityContext`; 6 real-socket tests per transport (right key with
    a backup pin, wrong key, chain first, insecure plus pins, through a
    `CONNECT` tunnel, invalid pin). The test server exposes the SPKI of
    its certificate (`wss_spki`, rcgen's encoding), so expected pins do
    not come from the code under test.
  - Devices (`device_test/`, `tool/device_test.sh`, new): the pin tests
    on both transports pass on the Samsung (Android 16) and an iPad Pro
    (iOS 26.6.1), where the pin wraps Security.framework. First
    connection with echo: Android 105 ms Dart / 61 ms Rust; iPad
    200–250 ms Dart / 30 ms Rust (LAN).
- Found on the way: on iOS a fresh install gets `EHOSTUNREACH` ("No route
  to host", errno 65) for LAN addresses until the local network
  permission is granted (TN3179); both transports report it as
  `unreachable`. The device harness waits for the server first.
- User CA on the device (a test CA installed through Settings, stored
  as `cacerts-added/bcc17572.0`, DER, named by OpenSSL's
  `subject_hash_old`): without `trustUserCertificates` both transports
  report `untrusted`, with it both connect. First / second connection
  with echo: Rust 53 / 11 ms, Dart 97 / 58 ms (the second reuses the
  cached configuration and resumes the TLS session).
- Not done: a disabled system CA on a device (unit-tested with the
  device's file layout only). NSC pin sets and `debug-overrides` are
  still not read (no JNI, `docs/SECURITY.md`); certificate transparency
  and revocation as before (3.7).
- Status: **adopted**.

### D46. The browser runs the shared server suite

- Context: the web transport had four tests of its own; the server suite
  (`server_suite.dart`) imported `dart:io` and ran only on the VM
  transports (docs/review/tests_docs.md T-3, D-3).
- Decision: the platform-neutral half moved to
  `test/transport/server_shared.dart` (no `dart:io`): protocol
  violations, valid edge cases with exact content, `maxMessageSize`,
  close semantics, flood, slow reader, liveness, suspend/resume. The VM
  runs it inside `server_suite.dart` (pure Dart and native); the browser
  runs it from `web_server_test.dart`, with the Rust server started in a
  hybrid isolate (`test/support/server_hybrid.dart`, `/log` relayed over
  the channel as JSON). The expectations differ only by two
  capabilities: `protocolPing` (heartbeats use an application ping
  otherwise) and `protocolCloseCodes` (browsers report a protocol
  violation as 1006). TLS, proxies, handshake faults and the blocked
  isolate stay VM-only. The browser CI job builds the server and fails
  when it is missing.
- Found, all fixed with the test that found it:
  - The web transport ignored `maxMessageSize`. It now closes with 1009
    and does not deliver the message, counting UTF-8 bytes (a surrogate
    pair is 4). The browser has already buffered the message, so this is
    consistency, not a memory bound.
  - A dropped connection reported close code 1006 on the web and `null`
    on the VM. 1005, 1006 and 1015 never appear on the wire; the web
    transport reports `null` for all three now.
  - Chrome passes an invalid close code from the server (999) through to
    the page. The web transport reports it as a protocol violation, 1002,
    `byServer: false`, like the pure-Dart transport.
  - The native transport reported the same case as the server closing
    with 1002 (`byServer: true`): tungstenite answers an invalid code with
    1002 and returns its own frame instead of the server's. It is now
    `ERR_PROTOCOL`.
- Observed, not changed:
  - Chrome rejects a non-minimal payload length (5 bytes in a 16-bit
    length; RFC 6455 §5.2 makes minimal lengths the sender's MUST) and
    fails the connection. The VM transports accept it.
  - Chrome fragments large outgoing messages (a 256 KiB message arrives
    as several frames).
  - A browser cannot drop TCP: after `abort()` it keeps the socket until
    its own closing timeout, so a server that never answers a close sees
    the connection for longer than on the VM.
- Measured: 47 browser tests per compiler (46 pass, the protocol ping
  test is skipped on the web), about 10 s per compiler; 5 runs on dart2js
  and 3 on the VM without a failure.
- Status: **adopted**.

### D47. Every suite checks the transport contract

- Context: the `TransportListener` contract ("callbacks never
  synchronous from a connection method; `onOpen` at most once; exactly
  one `onClosed`, always last") was only checked indirectly, through the
  client (TEST_PLAN T28). The session-identity test could not fail
  (docs/review/tests_docs.md T-4): the fake echoed our close, so the old
  connection ended and the fake itself dropped its events.
- Decision: `test/support/checked_transport.dart` wraps the transport of
  every test in `conformance.dart`, `server_suite.dart` and
  `server_shared.dart` (VM, native by path and through the hook, and the
  browser on dart2js and dart2wasm). The wrapper records any callback made
  while `connect` or a connection method is on the stack, events after
  `onClosed`, a second `onOpen`, messages or pongs before `onOpen`, and
  `onDrain` without `requestDrain`. After each test it waits up to 5 s
  for every connection's `onClosed` and fails the test otherwise.
  `checked_transport_test.dart` breaks each rule once to show the checker
  catches it.
- The session-identity test now keeps the old connection open
  (`echoClose = false`) and sends text, binary, a pong, a drain and a
  late close through it after the URL switch. Removing either the
  `_current` guard, the `_delivering` guard or the guard on `onClosed`
  makes it fail (each checked by hand).
- Result: no violation on any transport. The fake transport is not
  wrapped: its tests run in fake time, and it is a test double whose
  timing the tests set.
- Status: **adopted**.

### D48. The reconnect policy on real sockets

- Context: with reconnect on, only 403 and "connection refused" were
  pinned on real sockets; every other server-suite test ran with
  reconnect off (docs/review/tests_docs.md §4 item 4; TEST_PLAN C12, T6).
- Decision: each failure gets one test that records the client's first
  decision: `reconnecting` (with the delay) or `disconnected` (terminal).
  `minDelay` 10 ms, `maxDelay` 2 s, so "soon" (< 500 ms) and "after
  `maxDelay`" are told apart.
  - Every transport (`server_shared.dart`, browser included): server
    close 1000, 1001, 1011, 1012, 4000, empty close and a dropped
    connection are retried soon; 1013 waits `maxDelay`; 1003 and 1008
    stop; a protocol violation stops on the VM (the client sent 1007) and
    is retried in the browser, which reports it as 1006, the same as a
    drop.
  - VM transports (`server_suite.dart`): HTTP 400, 401, 403, 404, 410
    stop; 408, 425, 429, 500, 502, 503 are retried; `Retry-After: 3` on
    429 and 503 makes the delay at least 3 s; a 302 is reported and
    retried, not followed; a broken 101 is retried; untrusted, expired
    and wrong-host certificates and a pin mismatch are retried after
    `maxDelay`; proxy 407 stops and proxy 502 is retried; refused and DNS
    failures are retried soon.
- Result: the policy matches the documentation on pure Dart, native and
  the browser; no change to the client. Four mutations of the policy (408
  terminal, 1013 not delayed, 407 retried, `Retry-After` ignored) each
  fail exactly one of the new tests.
- Status: **adopted**.

### D49. Re-entrancy, random sequences and exact content

- Context: D22 promises that the client survives calls from its own
  callbacks, but the only `onStateChange` in the tests recorded states
  (TEST_PLAN C22, C24). Binary payloads were checked by length only, and
  the flood test counted identical messages, so order was never checked
  (docs/review/tests_docs.md T-5, T-6).
- Decision: `test/client/reentrancy_test.dart` over the fake, in fake
  time, on the VM and in the browser:
  - each of 9 operations (`connect` to the same or another URL,
    `disconnect`, `reconnect`, `suspend`, `suspend(disconnect: true)`,
    `resume`, `push`, `close`) is called from `onStateChange` the first
    time each of 6 states is reached, in a scenario that passes through
    all of them;
  - 300 fixed seeds of 60 random steps: client operations, the server
    accepting, rejecting (403, 429, 503), dropping, closing (1000, 1001,
    1008, 1013, 4000) and sending, `autoAccept` changes, time passing; one
    step in eight of `onStateChange` also calls a random operation.
  After every step: nothing after `closed`, at most one live connection,
  `open` only on an open connection, no new connection between
  `disconnect()`/`close()` and the next `connect()`. At the end, after
  `close()` and 15 minutes: the state is `closed`, the stream is done,
  every returned future completed, every connection ended.
- Exact content: echoed and received binary messages (70 000 and 100 000
  bytes) are compared in full, with a pattern that is not periodic in
  256 so a block offset shows; a fragmented 70 000-byte binary message
  from the server likewise. Two numbered streams from `/stream` (20 000
  binary, 5 000 JSON with non-ASCII text) must arrive complete, in order,
  and end with `1000 done`, on every transport including the browser.
- Result: no bug found. The 5 000-seed run passes (2 s). Mutations:
  emitting states after `closed`, and dropping `_wanted = false` in
  `disconnect()`, each fail the suite; two other guards (the `!_wanted`
  check when a session ends and in the reconnect timer) cannot be broken
  observably, since `disconnect()` already cancels the timer and marks
  the session.
- Status: **adopted**.

### D50. The remaining native findings

docs/review/native.md, findings 5–30 (1–4 and 11 were fixed earlier, 9
and 10 in D45). Each was checked against the current code first;
measurements on an M-series Mac, release builds, the Rust test server.

- **5, small payloads pin read chunks: not reproduced.** The review
  measured RSS: +60 MB after keeping 571 of 4 M 16-byte messages. A
  control run that keeps nothing grows RSS by the same 60 MB (freed
  memory is not returned to the OS). Measured with
  `malloc_zone_statistics` (bytes in use), keeping 572 messages costs
  +1.3–1.6 MB, about 2.5 KB each, not 128 KiB; the measure is sensitive
  (572 kept 64 KiB messages: +46.8 MB). Copying payloads up to 32 KiB
  into the Dart heap (`Dart_CObject_kTypedData`) removed even that, but
  cost 2–4 % throughput on small messages, so it was not kept.
- **6, tape memory and blocking: fixed.** JSON texts above 64 KiB are
  parsed under `block_in_place`, so the worker hands its other tasks
  (every connection's reads, writes, pongs) to another thread; above
  16 MiB the text goes to Dart instead of a tape (peak was about 28 times
  the text). Echo round trip on a second connection while 10 documents
  arrive, one worker thread:

  | Documents | Before: echo p50 / max, peak RSS | After |
  |---|---|---|
  | 1 MiB | 9 / 28 ms, 75 MB | 0 / 0–12 ms, 72–81 MB |
  | 8 MiB | 12–15 / 19–32 ms, 475 MB | 0 / 18–22 ms, 475 MB |
  | 32 MiB | 1 / 84 ms, 1829 MB, 735 ms total | 1 / 233 ms, 400–490 MB, 2370 ms total |

  Above the limit Dart decodes on its own thread (slower, longer pauses)
  but memory stays at a quarter: 1.8 GB gets a mobile app killed. A
  4 MiB limit was tried first and dropped: 8 MiB documents then took
  560 ms instead of 180 and the echo p50 rose to 53 ms.
- **7, `buffered` under steady load:** fixed in D41.
- **8, OS verifier blocks `abort()`: fixed.** The verification runs on
  the blocking pool; the connection waits under `block_in_place` and
  polls a task-local cancel flag every 20 ms, so `abort()` and the
  connect timeout end the attempt at once (an abandoned check finishes
  on its own thread). Rust tests with a verifier that blocks for 3 s:
  aborted after 100 ms it returns within 600 ms, and a single-worker
  runtime keeps running other tasks meanwhile.
- **12, hook cache keys: fixed.** The shared cargo target directory is
  per NDK level or Apple deployment target, and the NDK clang wrapper is
  `…-clang.cmd` on Windows hosts; a skipped build prints why
  (`native/test/hook_test.dart`).
- **13, no Rust-side connect deadline:** kept. The client's
  `connectTimeout` aborts, and with 8 that works during TLS too; direct
  users of the transport must do the same.
- **14 (schemes), 16 (repeated headers):** already fixed by the own
  handshake (D39).
- **15, calls after `close()`: fixed** together with T-12. After
  `close()` every transport throws `WebSocketNotConnectedException` for
  sends and protocol pings (native ignored the ping and let a second
  `close()` stop its writer). New shared test on all three transports;
  it also shows that browsers drop messages after `close()` (the HTML
  spec fires message events only while open), which the client does
  anyway.
- **17, a throwing listener: fixed, and worse than reported.** The
  native port handler ran in the root zone, so a listener exception was
  an uncaught error there and ended the isolate; the pure-Dart transport
  lost the rest of the chunk. Both now deliver every message and report
  the error to the zone of `connect()`. Shared test on every transport.
- **18, `ws_native_alloc`:** returns null when memory is not available
  instead of aborting (Rust test).
- **19, leaf calls:** `requestDrain` may post an event from inside the
  call, so it is no longer a leaf call. `send` stays a leaf call; its
  UTF-8 check runs at several GB/s.
- **20, NAT64 for IPv4 literals:** as `dart:io`; not changed.
- **21, proxy response read byte by byte:** kept (about 100 reads per
  connection, under a millisecond); the status line must now start with
  `HTTP/` (Rust test).
- **22, `race` error:** the error of the first (preferred) address is
  returned, not whichever failed last.
- **23, cached TLS errors:** build errors are not cached (D45 cache);
  the Linux fallback to the bundled roots is documented in
  docs/SECURITY.md.
- **24, server close 250 ms late: not reproduced.** Server close after
  100 ms, five runs each: Dart 101–105 ms, native 98–107 ms, with the
  server holding TCP open or closing it.
- **25, pongs overtaking data:** any incoming traffic proves liveness,
  so order does not matter to the heartbeat; not changed.
- **26, two `Authorization` headers: fixed on both VM transports.** An
  explicit header (any case) wins over URL user info; test against the
  server's `/headers`.
- **27:** received buffers are documented as read-only.
- **28:** `tokio-rustls` without `logging`, simd-json without
  `serde_impl`, tokio-tungstenite with `handshake` only, the unreachable
  `E::Tls` arm removed: the macOS library shrinks from 2,128,496 to
  2,062,080 bytes (−3.1 %).
- **29:** a test compares the ABI number in Dart and Rust.
- **30:** the 32-bit finalizer token is commented.
- Status: **adopted**.

### D51. The remaining protocol findings

docs/review/protocol.md F10–F18 (F13 was fixed earlier, F11 in D46, F18
in D50 as native 26).

- **F10, UTF-8 validator speed: fixed.** The word-wise ASCII scan is
  entered only for a run of two ASCII bytes with at least 16 bytes left,
  not after every multi-byte character. 1 MiB, AOT, validate vs
  `utf8.decode`:

  | Text | Before | After | `utf8.decode` |
  |---|---|---|---|
  | ASCII | 0.04 ms | 0.04 ms | 0.45 ms |
  | mostly-ASCII JSON | 0.93 ms | 0.89 ms | 2.0 ms |
  | Cyrillic | 3.82 ms | 2.51 ms | 2.1 ms |
  | CJK | 3.41 ms | 2.24–2.35 ms | 1.9 ms |
  | emoji | 4.46 ms | 2.25 ms | 1.9 ms |

- **F12, close frames: fixed on every transport.** A reason over 123
  bytes is cut at the last whole character; codes that may not appear on
  the wire throw `ArgumentError` (pure Dart and native). Before, the
  native transport sent a 140-byte reason as is (a control frame over
  125 bytes) and the browser threw `SyntaxError`; the client limited
  reasons already, so only direct transport users were affected. Shared
  test: the server receives `'é' * 61` for `'é' * 70`, as valid UTF-8.
- **F14, closing TCP: fixed on both VM transports.** After the closing
  handshake the client sends FIN, reads and drops until the server's FIN,
  for at most 1 s, and reports the end at once. Reproduced first: a
  server that keeps writing after its close frame never received our
  close (`close: null`, 0 bytes, 5 of 5 runs) on pure Dart and native;
  now it gets close 1000 and a clean FIN. Browsers treat data after a
  close frame as a failure (1006). A valid server does not send after
  its close frame, but a broadcast racing the close does.
- **F15, response parsing: strict in Dart and Rust alike.** `HTTP/1.1`
  (1.0 only for a refusal, so a 403 from an old proxy keeps its status),
  exactly three digits (`+101` and `0x65` were accepted; Rust's
  `u16::from_str` accepts `+101` too), token header names (no
  `Upgrade : websocket`), no obsolete line folding. The same ten cases in
  both test suites.
- **F16, `103 Early Hints` before the 101: fixed on both VM transports**
  (up to 16 interim responses are skipped). New server endpoint
  `/early-hints?count=`; shared test. Chrome fails such a handshake
  (`skipsInterim: false` in the browser run).
- **F17, hosts:** a non-ASCII host name (which `Uri` percent-encodes)
  makes `connect()` throw `ArgumentError` on the VM transports ("give it
  in punycode"), which the client treats as terminal; browsers convert
  IDN themselves. An IPv6 zone id is kept out of the `Host` header and
  decoded for the socket (pure Dart; unit-tested, no link-local server in
  the suite). Full IDNA (UTS #46 mapping) is not worth a dependency here.
- Found on the way: the contract checker (D47) waited for `onClosed`
  from a connection whose `connect()` had thrown; fixed.
- Status: **adopted**.

### D52. The remaining client findings

docs/review/client.md F23, F27–F29 (the others were fixed in D38).

- **F23, RPC timers: fixed.** A call moves the single RPC timer only
  when its deadline is earlier than the armed one (O(1)); a reply does
  not touch it; the timer, when it fires, expires what is due and re-arms
  for the earliest deadline left. Before, both recomputed the minimum
  over every pending call. Burst over the fake transport, AOT:

  | Calls | Issue before / after | Answer before / after |
  |---|---|---|
  | 2 000 | 18 / 1–3 ms | 20 / 3–5 ms |
  | 8 000 | 151 / 3–6 ms | 169 / 9–16 ms |
  | 32 000 | 1902 / 24–28 ms | 2406 / 47–50 ms |

  Test: 20 000 calls issued and answered under 1 s (1.68 s on the old
  code); per-call deadlines in fake time; no RPC timer once no call
  waits.
- **F28, wall-clock deadlines: fixed.** Deadlines are measured on a
  monotonic `Stopwatch`. `clock.stopwatch()` from package:clock reads
  `clock.now()`, the wall clock, so it is used only when a test replaced
  the clock (fake time).
- **F27, synchronous vs future errors: kept and documented.** Malformed
  arguments (URL, scheme, close code or reason) throw synchronously, as
  programming errors do in Dart; everything that depends on state or the
  network fails the future. Already pinned by tests; now in the dartdoc.
- **F29, `open.url`: fixed.** It is the URL the `request` provider
  returned and the transport used. Because that URL may carry a token,
  the `toString()` of `connecting` and `open` now leaves out user info and
  the query (`wss://host:8443/s?…`); before, it printed `user:pass@` from
  the given URL.
- Status: **adopted**.

### D53. The remaining distribution findings

docs/review/distribution.md F21–F30.

- **F21:** `WsNative.unavailableReason` existed; both READMEs now print it
  when `install()` returns `false`.
- **F22:** the `compile exe` examples (README, `package:ws/native.dart`)
  fall back to pure Dart when the file is missing, built for another
  architecture or from another release, and say to run the installer
  again after every `ws` upgrade. The dartdoc also stops claiming that
  `compile exe` "does not run build hooks" (it refuses such packages;
  tests_docs D-6).
- **F23:** the iOS library imports `fstat` (Rust std), a required-reason
  API (file timestamps); `clock_gettime` is not on Apple's list. The
  `ws_native` README shows the `PrivacyInfo.xcprivacy` entry (`C617.1`),
  and `tool/verify_native.sh` fails when the set of such imports changes.
- **F24:** `repository` points at each package's directory; both packages
  have `issue_tracker`, `topics` and an `example/`.
- **F25:** `test/` and the 1.x screenshot are no longer published (archive
  392 KB → 76 KB); README links into the repository are absolute; the
  "branch `rewrite`" note is gone. The staged `ws_native` already left
  out its tests.
- **F26:** CI formats every Dart directory `mise run fmt:check` does
  (examples included) and runs actionlint (release pinned by SHA-256)
  and shellcheck. Clippy with `--all-targets` was already in both Rust
  jobs.
- **F27:** `tool/check_publish.sh` prints dry-run warnings and tolerates
  them outside CI (a dirty tree is the normal state while developing);
  errors, and warnings on CI, fail. It says when `native/prebuilt` is
  missing, since the staged archive then is not the release's.
- **F28:** DEVELOPMENT and `build_native.sh` state the real floors (iOS
  13.0, macOS 10.15, API 21; Rust: macOS arm64 11.0, iOS simulator arm64
  14.0) against Flutter 3.47's templates (iOS 15.0, macOS 12.0, minSdk
  24); `verify_native.sh` fails when `minos` exceeds them (local builds:
  macOS arm64 11.0, iOS arm64 13.0).
- **F29:** done in D43 (current majors, pinned by SHA).
- **F30:** `tool/autobahn.sh` pins the image by digest (the one D28 used)
  and waits for the port instead of `sleep 3`. Re-run after this
  session's protocol changes (D50, D51): pure Dart 296 OK, 2 non-strict,
  3 informational, 0 failed, as in D28; native 289 / 9 / 3 / 0, two
  better than D28 (4.1.5 and 4.2.5, reserved opcodes after a text frame,
  are now OK). Not in CI: a run takes minutes and needs Docker.
- Status: **adopted**.

### D54. Every connect attempt owns its socket, TLS stage included

GitHub issue #24 and an outside report on ws 1.0.0 (2026-11-12, Toxiproxy):
1.x wrapped `io.WebSocket.connect` in `Future.timeout`. The timed-out
attempt kept running with its socket (12 failed attempts left 12 open
sockets, still open a second after `close()`), and a reconnect that hung
never left `connecting`, so no further attempt was made.

2.0 already gave each attempt a session that the connect timeout,
`close()` and a newer attempt abort. A regression group
(`server_suite.dart`, "abandoned connect attempts", both VM transports)
drives a plain TCP listener that accepts and never answers, and checks
on the server side that each socket is closed. It found one hole: the
pure-Dart transport opened `wss://` with `RawSecureSocket.startConnect`,
whose `ConnectionTask.cancel()` only works until TCP is up. An attempt
aborted during the TLS handshake (ClientHello sent, no answer) kept its
socket until the server answered or the OS gave up. TLS now always runs
through `RawSecureSocket.secure` over a TCP socket the transport holds,
as the proxy path already did, so `abort()` closes it at any stage.
Native was not affected (dropping the tokio task closes the socket).

Pinned by tests, on pure Dart and native:
- `ws` and `wss`: after 4 timed-out attempts, the first 3 sockets are
  closed by the client and attempts keep coming; after `close()` all are.
- #24 itself: open, the connection is cut, then every reconnect hangs:
  attempts 1, 2, 3 each time out and the next one starts.
- `close()` during the handshake returns within 1 s (connect timeout
  30 s), fails `connect()` with `WebSocketNotConnectedException` and
  closes the socket.
- A 101 written after the timeout cannot open the client: the socket is
  gone by then.

The same report's second defect, an exception from
`WSInterceptor.wrap(onSend:)` escaping `await add()` into the zone (exit
code 255), has no 2.0 counterpart: interceptors are gone, and the user
code left on the send path (`toJson`, `WebSocketRpc.encode`, the
heartbeat ping builder) fails only its own call. A new test pins
`toJson`: `push()` returns normally, its future fails with
`WebSocketFormatException` (cause: the `StateError`), the zone sees
nothing, and the connection stays open.

- Status: **adopted**.

### D55. dart2wasm `json.decode` is quadratic only with asserts

The browser suite's "JSON documents of 1 MiB and 17 MiB" test took 28 s
under dart2wasm on CI (30 s limit) and then timed out; dart2js took
under 1 s. Measured in Chrome with `dart test` (Dart 3.13.7): 6.6 MB of
small objects 21 s, doubling the input quadrupled the time; flat lists,
one large map and long strings were linear. The cause is in the SDK's
wasm JSON listener: each container start wraps the parent's elements in
`GrowableList.withDataAndLength`, whose constructor asserts the type of
every element, so a list of n containers costs O(n²) with asserts on.

Compiled with `dart compile wasm -O2` and run in Node 26, the same
decode is linear and fast:

| Input | asserts off | asserts on |
|---|---|---|
| 100k two-element lists (1.3 MB) | 11 ms | 10.7 s |
| 200k small objects (13 MiB) | 108 ms | 43 s |

`JSON.parse` plus `dartify` was 0.2 s for 15 MiB, so a JS-backed decoder
would not pay off in release builds, and it would lose the int/double
distinction (`1.0`). No library change: release builds are not
affected. Under dart2wasm the test drops the 17 MiB case, which exists
for the native transport's 16 MiB tape limit (D50). Debug (asserts on)
Flutter web builds with dart2wasm will be slow on large nested JSON;
reported as https://github.com/dart-lang/sdk/issues/64924.

- Status: **adopted**.
