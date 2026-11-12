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

- Native: ping/timeout runs in Rust (tokio timer), the isolate is not woken.
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

Open (tracked in ROADMAP): the native send queue is not bounded in bytes
(the Dart client applies `sendHighWaterMark` backpressure, but a caller
that ignores `push` futures can grow native memory); small zero-copy
payloads pin tungstenite's 128 KiB read chunks (copy payloads under a
threshold — measure first); Happy Eyeballs for IPv6; TLS `close_notify`;
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
- Status: **adopted**. The release workflow that builds every target,
  creates the GitHub release and runs the generator is still to be written
  (ROADMAP).

### D33. Project tasks in mise

- Compared: `make` (the old `Makefile`), `just`, mise tasks.
- Decision: mise, because the repository already pins Dart with it, so
  tools and tasks live in one file (`mise.toml`).
  - mise also pins Rust (through rustup, with rustfmt and clippy), `gh`,
    `actionlint` and `shellcheck`.
  - Task dependencies run in parallel. `mise run check` (format,
    analyzers, all tests) took 16 s on an M-series Mac.
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
