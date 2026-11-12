# Review: native library (`native/rust`, FFI layer, build hook)

Date: 2026-11-12. Scope: `native/rust/{Cargo.toml,build.rs,src/*.rs}`,
`lib/src/native/native_library.dart`, `native_transport.dart`,
`native/lib/**`, `native/hook/build.dart`. `server/` was not reviewed.

Line numbers are those of the working tree at the end of the review;
`lib.rs` and `native_transport.dart` were being edited while it ran (the
repeated-header check in `open()` and header validation / user info in
`native_transport.dart` landed during the review and are covered).

How claims were checked:

- `cargo test --release` (17 passed), `cargo clippy --release --all-targets`
  (clean; `clippy::pedantic` reports 35 style warnings, mostly integer
  casts), `cargo audit` (108 crates scanned, no advisory reported).
- Rust experiments in a scratch copy of the crate, calling `open()` against
  local TCP servers and `build_tape` directly.
- Dart experiments (AOT executable, macOS arm64) that load the release
  `libws_native.dylib` by path and drive `NativeTransport` against a raw
  frame server in another isolate; memory from `/usr/bin/time -l`.
- Reading the sources of tungstenite 0.30.0, rustls-platform-verifier 0.7.1
  and `dart_native_api.h` of Dart 3.13.7.

"Verified" below names the method. "Suspected" means reasoning from the
code only.

## What was checked and found sound

- **External typed data ownership.** `post_batch` releases the buffers only
  when `Dart_PostCObject` returns false. `dart_native_api.h` (3.13.7) states
  that on false "ownership of external typed data in the message remains
  with the caller", and on true the finalizers run even if the isolate dies.
  No double free, no leak. The `Dart_CObject` layout, type codes (8, 13) and
  finalizer signature match the header.
- **Buffers from `ws_native_alloc`.** One owner at any time; `connect`,
  `send`, `close` take the buffer before `guard`, so a panic cannot leak or
  double free it. A zero-length buffer is handled.
- **Panics.** Every entry point that can panic is inside `catch_unwind`;
  a panic in the connection task still produces the final event.
- **Dead isolate.** An isolate that calls `Isolate.exit()` with an open,
  idle connection: the server saw the socket close 2 ms later (the
  `NativeFinalizer` path). Verified by experiment.
- **Abort wake-ups.** `notify_waiters` + `notify_one` plus the sticky
  `aborted` flag leave no lost wake-up in `run` and `serve` (checked every
  interleaving by reading).
- **Final event ordering.** The decoder is awaited before `serve` returns, so
  the closed event is posted after every batch.
- **Happy Eyeballs core (`race`).** No branch combination leaves the
  `select!` with all branches disabled; dropped attempts close their sockets;
  cancellation by dropping the future is clean.
- **JSON acceptance.** 50 malformed and edge-case documents: simd-json never
  accepted a document that serde_json rejects. It rejects integers outside
  `i64`/`u64` (for example `18446744073709551616`); those fall back to raw
  text and the Dart decoder, so the result is the same, only slower.
- **`allow_insecure`.** Builds its own `ClientConfig` per connection, so its
  session cache is never shared with verified connections.

## Findings

Sorted by severity. There is no critical finding (no memory-unsafety or
remote code execution was found).

| # | Severity | Where | Failure scenario | Status | Suggested fix |
|---|---|---|---|---|---|
| 1 | high | `lib.rs:579-583` (`into_client_request`), `native_transport.dart:83` | A URL with a query and no path, `wss://host?token=x`. `Uri.toString()` keeps it as is, and tungstenite sends the request line `GET ?token=x HTTP/1.1`, which is not a valid request target. Strict servers and proxies answer 400. The pure-Dart transport sends `GET /?token=x`. | Verified: Rust test against a local server captured `GET ?x=1 HTTP/1.1`; Dart prints `ws://host?x=1` for that URL. | Normalise in Dart before encoding the config (`path.isEmpty ? url.replace(path: '/') : url`) and add the case to the shared conformance suite. |
| 2 | high | `lib.rs:601-605`, tungstenite `handshake/client.rs:272` | The client offers subprotocols and the server selects none (allowed by RFC 6455, common with servers that ignore the header). tungstenite fails with `SubProtocol error: Server sent no subprotocol`; the pure-Dart transport connects with `protocol == null`. The same app works on one transport and never connects on the other. | Verified: Rust test, `ERR kind=2 "SubProtocol error: Server sent no subprotocol"`; `HandshakeResponse.validate` in Dart accepts a missing header. | Decide the contract once. To match Dart: do not put the header where tungstenite tracks it (run the handshake with tungstenite's check bypassed, or validate the response in `open()` and map only `InvalidSubProtocol` to an error). Add a conformance test. |
| 3 | high | `lib.rs:824` (credits), `:830` (pong), `:911-928` | The receive window counts payload bytes only. A message costs about 50 bytes of bookkeeping (peer box, finalizer record, port message) whatever its size, and an empty message costs no credit at all. A server (or a bug) that floods tiny messages while the isolate is busy grows memory far past the window. Pong frames are posted without any limit. | Verified: 24 M one-byte messages with the default 8 MiB window peaked at 464 MB RSS; 8 M empty messages 432 MB, 24 M empty messages 585 MB (baseline 15 MB; 300 k messages of 125 bytes: 42 MB). | Charge `len + overhead` (for example 64 bytes) per message on both sides of the credit protocol, and coalesce pong events (one pending flag instead of one post per frame). |
| 4 | medium | `lib.rs:56`, `:154` | The JSON config is part of the ABI but is not versioned: `proxy` was added with `ABI_VERSION` still 2, and unknown fields are ignored. A library built before this change, used with the new Dart code, passes the ABI check and connects **directly**, silently ignoring the proxy. The same will happen to any later security option (pinning, client certificates). | Verified: `git diff` shows `proxy` added without a bump; a config with unknown fields deserialises (`accepted=true`). | `#[serde(deny_unknown_fields)]` on `Config` and `Proxy`, bump `ABI_VERSION` to 3 now, and state in the comment that config fields are covered by the version. |
| 5 | medium | `lib.rs:929`, `:934`; tungstenite `frame/mod.rs` (`split_to(len).freeze()`) | Text and binary payloads are views into tungstenite's 128 KiB read chunks. One retained small message keeps its whole chunk alive, and Dart accounts only the view's length, so the GC sees no pressure. An app that keeps the last message per topic can hold hundreds of MB of native memory behind a few KB of Dart objects. Known as open in D29; the numbers are new. | Verified: 4 M binary messages of 16 bytes, keeping 571 of them (9 KB): RSS +60 MB after forced GCs (571 x 128 KiB = 71 MB expected). | Copy payloads below a threshold (for example 4 KiB, or below a fraction of the chunk) into an exact-size allocation before posting. |
| 6 | medium | `tape.rs:25-116`, `lib.rs:920` | Tape memory is not related to the credit charged. A JSON text of small scalars expands 9x in the posted buffer and 28x at peak (input copy, simd-json nodes of 24 bytes, tape words). With the default 64 MiB message limit one message can need about 1.8 GB; the window charges 64 MiB. The work also runs synchronously on a runtime worker (2 by default), so a large document delays reads, writes and pongs of every connection. | Verified: 8 MiB of `1,1,1,...` gives a 75 MB tape buffer and 234 MB peak RSS. | Skip the tape above a size limit (fall back to raw text), drop the `to_vec` copy by taking ownership of the message bytes when unique, charge the tape size as credit, and move documents above a threshold to `spawn_blocking`. |
| 7 | medium | `lib.rs:261`, `:744-755` | The send queue is unbounded (known, D29). In addition `buffered` only decreases when the command queue is momentarily empty, so under a steady stream `bufferedAmount` keeps growing although data is being written; a producer that waits for a low-water mark other than zero stalls until the queue fully drains. | Suspected (reading). | Subtract after each successful `feed` that tungstenite wrote out, or track bytes handed to the sink separately; add a byte limit with a "too big / above high water" result as D18 describes. |
| 8 | medium | `tls.rs:166-177` | Certificate verification on Apple and Windows runs under `block_in_place` inside the connection task. While it blocks (Windows: revocation retrieval with a 10 s timeout per `rustls-platform-verifier`; Apple: whatever network fetch the system decides on), `abort()` and the Dart connect timeout have no effect and the closed event is late. | Suspected (source of rustls-platform-verifier 0.7.1 read; not reproduced). | Document it, or run verification in `spawn_blocking` behind a channel so the task stays cancellable. Add a test on a network with OCSP/CRL hosts black-holed. |
| 9 | medium | `tls.rs:62-68` | With `trustedRootsPem` a new `ClientConfig` is built for every connection: the root store (or OS verifier) is rebuilt and there is no TLS session resumption at all. Reconnects are the main use of this library, and private CAs are where custom roots are used. | Verified by reading (`build(extra)` per call; test `default_config_is_shared` covers only the default). | Cache configs keyed by a hash of the PEM (small LRU). |
| 10 | medium | `tls.rs:1-17`, `:121-142` | Android: only the system directory is read. User-installed CAs (`/data/misc/user/<id>/cacerts-added`), user-disabled system CAs (`cacerts-removed`) and the app's Network Security Config are ignored, while the module comment says MDM and user-installed CAs work. A root the user disabled is still trusted. The roughly 150 files are also read synchronously on a runtime worker during the first `wss` connect. | Verified by reading. This matches `dart:io`, so it is a documentation and expectation problem more than a regression. | Correct the comment and D35; read the added/removed directories if the behaviour is wanted; load roots in `spawn_blocking`. |
| 11 | medium | `native_transport.dart:65`, `:80` (new code) | `HandshakeRequest.basicAuthorization(url)` runs outside the `try`. For `ws://%ff:p@host/` `Uri.decodeComponent` throws `FormatException`, synchronously out of `connect()`, after the `RawReceivePort` was created: the port is never closed (the isolate cannot exit) and the contract "errors arrive through `onClosed`" is broken. | Verified: `Uri.parse` accepts the URL and `decodeComponent` throws `FormatException: Invalid UTF-8 byte`. | Move the call into the `try` (catch `FormatException` too), or create the port after validation. |
| 12 | medium | `hook/build.dart:146`, `:201-208` | (a) The shared cargo target directory is not keyed by iOS/macOS deployment target or NDK API level; the comment admits cargo does not fingerprint them. A build for a higher API level or deployment target is reused for a later lower one and can fail to load on old devices. (b) On Windows hosts the NDK compiler wrapper is `...-clang.cmd`; the hook asks for `...-clang`, cargo fails and the app silently ships without the native transport. | Suspected (reading; development and git-dependency builds only, prebuilt libraries are not affected). | Put the deployment target / API level in the target directory name; append `.cmd` on Windows; print a clear warning when falling back. |
| 13 | low | `lib.rs:577-640`, `net.rs:45`, `:143-158` | No timeout in Rust for DNS, TCP, proxy response, TLS or the upgrade. The client's `connectTimeout` calls `abort()`, but a caller that uses the transport directly waits for OS timeouts (75–130 s) or for ever (a proxy or server that accepts and stays silent). `getaddrinfo` runs on the blocking pool and cannot be cancelled; each aborted attempt leaves a thread until the resolver returns. | Verified by reading. | An overall deadline in `open()` passed in the config (default above the Dart timeout). |
| 14 | low | `lib.rs:585` | Any scheme other than `wss` is treated as plain text on port 80 (`https://`, a typo). The client normalises `http(s)`, a direct transport user gets an unencrypted connection without an error. | Verified by reading. | Reject schemes other than `ws` and `wss` with a handshake error. |
| 15 | low | `lib.rs:301`, writer loop | After `close()` the library still accepts `send` (returns 0) and drops the message when tungstenite refuses it; `ping` is ignored silently. The pure-Dart transport throws `WebSocketNotConnectedException('The connection is closing')`. A second `close()` makes the writer task exit. | Verified by reading both transports. | A `closing` flag set by `ws_native_close`; `send`/`ping` return "closed" after it. |
| 16 | low | `lib.rs:468` | Response headers of a failed handshake go through a `HashMap`: repeated headers (`Set-Cookie`, `WWW-Authenticate`) keep only one value, in arbitrary order. | Verified by reading. | Join repeated values with `, ` as the Dart parser does. |
| 17 | low | `native_transport.dart:141-157` | If a listener callback throws, the `finally` grants the credits but the rest of the batch (up to 63 messages) is dropped silently. | Verified by reading. | Catch per item, report through the zone, continue the batch. |
| 18 | low | `lib.rs:217-220` | `ws_native_alloc` returns null only for a size above `isize::MAX` (verified: capacity-overflow panic caught). A real allocation failure calls `handle_alloc_error`, which aborts the process, so the Dart `StateError('out of memory')` path is mostly dead. `ws_native_free` is exported but not bound in Dart. | Verified (test) / reading. | `std::alloc::alloc` with a null check, or remove the claim; bind `free` or delete it. |
| 19 | low | `native_library.dart:24-31` | All calls are leaf calls. `ws_native_request_drain` can call `Dart_PostCObject` from inside a leaf call, and `ws_native_send` validates UTF-8 of the whole text message while the thread cannot reach a safepoint (GC of the isolate group waits). | Suspected; no failure observed. | Make `requestDrain` non-leaf; skip validation for `sendText` (Dart already produced valid UTF-8) and keep it for `sendTextBytes`. |
| 20 | low | `net.rs:43-45` | An IPv4 literal host is parsed by tokio without `getaddrinfo`, so no NAT64 address is synthesised on IPv6-only networks (the comment suggests otherwise). Same as `dart:io`. | Suspected (tokio `ToSocketAddrs` behaviour, not run on such a network). | Call `getaddrinfo` for IPv4 literals on Apple targets, or document it. |
| 21 | low | `net.rs:143-173` | Proxy response: one `recv` system call per byte (about 100–300 per connect); a refusal is reported as a generic connection error without the status code, so the app cannot tell 407 from 403; the status line is not checked to start with `HTTP/`. The `Proxy-Authorization` value is written without a CR/LF check (safe today because Dart builds it with base64). | Verified by reading. | `peek`-based or buffered read with the remainder handed on; a dedicated error kind with the status; reject control characters in Rust too. |
| 22 | low | `net.rs:96-104` | `race` returns the error that arrived last, not the first or the most specific one, and has no per-attempt timeout. | Verified (existing test `all_failing_returns_last_error`). | Keep the first error (as `dart:io` does) or prefer non-timeout errors. |
| 23 | low | `tls.rs:56-60`, `:97-99` | A failure to build the default TLS configuration is cached for the life of the process. On Linux a broken `SSL_CERT_FILE` silently widens trust to the bundled Mozilla roots. | Verified by reading. | Do not cache errors; log or report when the fallback bundle is used. |
| 24 | low | `lib.rs:845`, `:872` | Close initiated by the server is reported up to 250 ms later than on the Dart transport. A panic in the decoder task is reported as "Connection closed without a close frame". TLS `close_notify` is not sent (known). | Verified by reading. | Shorter wait or finish on flush; map a decoder `JoinError` to an internal error. |
| 25 | low | `lib.rs:830` | Pong events are posted by the reader directly and can overtake data messages still queued for the decoder. | Verified by reading. | Route pongs through the decoder channel if order matters to heartbeat logic. |
| 26 | low | `native_transport.dart:82`, `lib.rs:602` | New code: a user header `authorization` (any other case) plus URL user info sends two `Authorization` headers. Subprotocol names are not validated for the native path; an invalid one surfaces as a connection error from `HeaderValue::from_str`. | Verified by reading. | Compare header names case-insensitively; validate protocols with the same token check as headers. |
| 27 | nit | `lib.rs:929`, `:934` | Native delivers unmodifiable `Uint8List`s, the Dart transport modifiable ones; code that edits a received buffer in place works on one transport only. | Verified by reading. | Document in the listener contract. |
| 28 | nit | `Cargo.toml` | `tokio-rustls` feature `logging` and simd-json default features are not needed (binary is 2.14 MB stripped). The `E::Tls` arm in `map_error` is unreachable (tokio-rustls errors arrive as `io::Error`). Crate version 0.1.0 is unrelated to the package version. | Verified by reading. | Trim features and measure; remove the dead arm. |
| 29 | nit | `native_library.dart:9`, `lib.rs:56` | The ABI number is duplicated by hand; nothing fails if only one side changes. | Verified by reading. | A test that reads both files and compares. |
| 30 | nit | `native_transport.dart:127` | The finalizer token is the id as a pointer; on 32-bit Android it is truncated to 32 bits (ids are sequential, so harmless until 2^32 connections). | Verified by reading. | Note it in a comment. |

Changes that landed during the review:

- **Repeated `Sec-WebSocket-Accept` / `-Protocol` check in `open()`**: correct
  and placed after tungstenite's own validation. The stream is dropped
  without a close frame, which is acceptable for a failed handshake.
- **Header validation and user info in `native_transport.dart`**: fixes two
  gaps measured before the change (tungstenite sent no `Authorization` for
  `ws://user:pass@host`, and forwarded `Sec-WebSocket-Extensions`, which
  would let a server enable compression that tungstenite cannot read).
  Remaining problems: findings 11 and 26. Finding 1 is not fixed by it:
  `url.replace(userInfo: '')` keeps `ws://host:8080?x=1` without a path.

## Future risks to plan for

- **iOS networking model.** BSD sockets do not trigger VPN on demand, do not
  follow system proxy or PAC settings, and are reclaimed when the app is
  suspended. Same as `dart:io`, but users of a "native" transport may expect
  `URLSession` behaviour. Test resume-from-background and document the
  limits.
- **Thread pool sizing.** JSON tape building and (on Apple/Windows)
  certificate verification run on the two default workers. Many connections
  with heavy JSON will delay pings on unrelated connections. Plan a metric
  and a rule for `threads`, or move decoding to the blocking pool.
- **Runtime lifetime.** The runtime is never shut down and each loaded copy
  of the library (bundled plus one opened by path) has its own runtime,
  connection table and session cache. Hot restart keeps threads and the
  session cache. Decide whether a second copy should be refused.
- **Version skew.** Beyond finding 4: event layouts, error kinds and the tape
  version are checked only by one number. Keep a compatibility test that
  loads the previous release's library with the current Dart code.
- **Proxy scope.** Only unauthenticated or Basic `CONNECT` over plain TCP.
  HTTPS proxies, Digest/NTLM/Negotiate, system proxy discovery and
  `NO_PROXY` rules will be asked for; decide what is out of scope and say so.
- **Trust features.** Certificate pinning, client certificates and
  revocation on Linux/Android (none today) are common requests; each adds
  config fields (see finding 4) and a per-config cache (finding 9).
- **Extensions.** No permessage-deflate on either transport; with header
  validation now in place a server cannot enable it by accident, but servers
  that require it will not work.
- **Cross-compilation.** `ring` needs clang for `aarch64-pc-windows-msvc`;
  Linux needs a pinned glibc floor (D9 mentions cargo-zigbuild, not yet in
  the hook); Android cdylibs have no SONAME unless one is passed to the
  linker; simd-json on `armv7-linux-androideabi` uses the non-SIMD path and
  should be benchmarked before claiming the D27 numbers on 32-bit devices.
  None of these targets was built in this review (only
  `aarch64-apple-darwin`).
- **Dependency behaviour changes.** Findings 1 and 2 are tungstenite
  defaults; an upgrade can change handshake strictness again. Pin the minor
  version and keep the shared conformance suite as the gate for upgrades;
  run `cargo audit` in CI.
- **Resolver threads.** `getaddrinfo` cannot be cancelled; a reconnect loop
  during a DNS outage holds one blocking thread per attempt until the
  resolver gives up. Bound concurrent lookups or reuse a pending lookup for
  the same host.
- **Liveness probe.** One null post per connection every 30 s wakes the
  isolate on idle mobile apps, and it runs only after the handshake. The
  probe on `init` from D18 is still not implemented.
