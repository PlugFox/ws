# Review: protocol layer and Dart / web transports

Scope: `lib/src/protocol/**`, `lib/src/transport/dart/**`,
`lib/src/transport/web/**`, `test/protocol/**`, `test/transport/**`.
Reviewed against the working tree of branch `rewrite` on 2026-11-12 around
18:55 local time. The tree was being edited while the review ran; line
numbers refer to that snapshot. Three things I found at the start were fixed
by others during the review and are listed under "Fixed during the review".

How findings were checked:

- **verified (exp)**: reproduced with a throwaway script against the real
  code (pure-protocol script, local raw `ServerSocket`s, and a copy of
  `server/target/debug/ws_bench_server` for TLS / proxy). Output is quoted.
- **verified (read)**: follows directly from the code, no run needed.
- **suspected**: reasoning only, not reproduced.

Test runs at the snapshot: `dart test test/protocol test/transport` on the VM
passes (one failure seen mid-review, `EOF, junk and a huge head`, went away
with the head-limit fix); `-p chrome` (dart2js) and `-p chrome -c dart2wasm`
pass for `test/protocol` and `web_transport_test.dart`; `-p node
test/protocol` fails in every test that builds a `FrameWriter` or
`HandshakeRequest` (`Random.secure()` on Node: `Value of "this" must be of
type nullish`). Node is not a target today, see F22.

## What is in good shape

Checked and found correct, so nobody needs to re-check:

- Masking keys and `Sec-WebSocket-Key` come from `Random.secure()` (CSPRNG);
  the 64-key pool does not weaken that. 1000 keys were unique, 16 bytes each.
- Word-wise masking round-trips for every alignment (existing tests).
- UTF-8 DFA: 2 000 000 random inputs (biased to boundary bytes, unaligned
  views, random split point for the incremental path) agree with
  `utf8.decode`; 0 mismatches. Fragmented text is validated incrementally and
  fails in the fragment that carries the bad byte; close reasons are
  validated.
- Reader: RSV bits, masked server frames, control frames > 125 or fragmented,
  reserved opcodes, close code ranges, 1-byte close payload, 64-bit lengths
  with the MSB set, lengths above 2^32 on dart2js (multiplication, no
  shifts), size limit checked before the payload buffer is allocated.
- `Sec-WebSocket-Accept`, `Upgrade` / `Connection` token lists, duplicated
  accept header, unoffered extension, unknown subprotocol: all rejected
  (checked against `/bad-101` of the Rust server).
- TLS on the Dart side: unknown CA fails, wrong host name fails, both also
  through a CONNECT tunnel; `trustedRootsPem` adds to the platform store;
  `allowInsecure` is the only way to skip verification. Close frames (echo of
  a server close, and our 1002 after a violation) do reach the server over
  TLS before the socket is closed (checked with `/log?id=`).
- `abort()` during TCP connect, TLS handshake, pending handshake response,
  pending CONNECT and TLS-in-tunnel: exactly one `onClosed`, no stray zone
  errors.
- `bufferedAmount` accounting of the write queue, including partial writes.

## Findings

Severity: **High** = remote party can crash / exhaust the client or inject
into the request; **Medium** = wrong result or hang in a realistic case;
**Low** = RFC nit, hardening, performance.

| # | Sev | Where | Status | Failure scenario | Fix |
|---|---|---|---|---|---|
| F1 | High | `lib/src/protocol/frame_reader.dart:220-231` | verified (exp) | Zero-length continuation frames are unbounded. Each 2-byte frame adds a `Uint8List` view and a list slot; `maxMessageSize` never trips because the message length stays 0. Fed 25 MiB of `00 00` with a 1024-byte limit: no failure, RSS +821 MiB in 0.76 s (about 32x amplification, no upper bound). `test/protocol/protocol_edge_test.dart:153` pins 10 000 such frames as accepted; TEST_PLAN P9 asks for a bound. | Do not store empty fragments at all (`if (payload.isNotEmpty) fragments.add`). Add a fragment-count limit (for example 1024 non-empty fragments, or `maxMessageSize / 16`) failing with 1009 or 1008. |
| F2 | High | `lib/src/protocol/frame_reader.dart:175, 212, 225` | verified (exp) | Fragments are zero-copy views of the socket read chunk, so one 1-byte fragment pins a whole 64 KiB chunk. A server that sends a 1-byte continuation followed by 64 KiB of pings, repeatedly, keeps memory proportional to everything it ever sent, not to `maxMessageSize`. 8000 fragments (message = 8000 bytes, limit 65 536): no failure, RSS +574 MiB. With the default 64 MiB limit the bound is 64 MiB x 64 KiB. | Copy a fragment into an owned buffer when it is a view (`payload.length < payload.buffer.lengthInBytes`), or append fragments into one growing buffer capped at `maxMessageSize`. This also removes the join copy at the end. |
| F3 | High | `lib/src/protocol/handshake.dart:73-75` | verified (exp) | Subprotocols are written into `Sec-WebSocket-Protocol` without any check, while headers are now validated. `protocols: ['chat\r\nX-Injected: yes\r\nCookie: stolen=1']` through the public `WebSocketOptions.protocols` put `X-Injected` and `Cookie` lines on the wire. Any app that builds the protocol list from external input (a common place for tokens) has a request-header injection. Empty strings and `a, b` are also emitted as is. | Validate every protocol as an RFC 9110 token (reuse `_isToken`), reject empty and duplicate values (RFC 6455 section 4.1), throw the same `ArgumentError`. Best done once in `WebSocketOptions` / the client so the web and native transports behave alike (the browser throws `SyntaxError` for the same input). |
| F4 | High | `lib/src/transport/dart/dart_transport.dart:83-91`, `lib/src/protocol/handshake.dart:79` | verified (exp) | `Uri.decodeComponent(uri.userInfo)` throws `FormatException` for user info that is not UTF-8 after percent-decoding (`ws://user:%ff@host/`). `_start` catches only `ArgumentError`, so the error escapes as an unhandled zone error and `onClosed` is never called: the attempt hangs until the client's connect timer, then the retry loop repeats it forever. | Catch `Object` (or `FormatException` too) around `encode()`. Better: decode user info leniently (fall back to the raw bytes / latin1) or validate the URL in the client before the first attempt and make it a terminal error. |
| F5 | High | `lib/src/transport/dart/dart_transport.dart:40, 83-91` | verified (exp) | Regression from the fix for invalid headers: `encode()` now runs before the first `await`, so `_fail` calls `listener.onClosed` synchronously inside `WebSocketTransport.connect()`, before the caller has the connection object. This breaks the documented contract (`transport.dart:56-58`, "never synchronously"). With the real client the state stream loops `connecting -> reconnecting` every backoff step for a configuration error that can never succeed (reserved header `Host`: 5 attempts in 6 s, `connect()` never completes). | Report through `scheduleMicrotask` (as `abort()` does). Separately, make an invalid request terminal: validate headers / protocols / URL in the client when options are set or in `connect()`, and throw `ArgumentError` to the caller instead of retrying. |
| F6 | Medium | `lib/src/transport/dart/dart_transport.dart:261-263, 340-354` | verified (exp) | Pongs are queued without limit. A server that floods pings and does not read grows the client's write queue: 96 MiB of 125-byte pings gave `bufferedAmount` = 99 MiB, connection still open. `sendBufferLimit` only guards `push()`. | Keep at most one pending pong (RFC 6455 section 5.5.3 allows answering only the latest ping): if a pong is still queued, replace its payload instead of appending. Or fail with 1008 when control replies exceed a small budget. |
| F7 | Medium | `lib/src/transport/dart/dart_transport.dart:182-194` with `269-279`, `282-297` | verified (exp) | A received close frame (or our own protocol failure) is forgotten when the write queue is not empty and the peer then closes TCP. Server sends close 4001 + FIN while 32 MiB wait in the queue: client reports `code=null`, `Connection closed without a close frame` instead of 4001. Same after a violation: the 1002 `WebSocketClosedException` stored in `_error` is replaced by a plain connection error, so the client treats a terminal protocol error as retryable network loss. | In the `readClosed` / `closed` / `onError` / `onDone` handlers use the stored result when `_closeAfterFlush` is set: `_finish(_closeCode, _closeReason, _error)`. |
| F8 | Medium | `lib/src/transport/dart/dart_transport.dart:282-297, 356-379`; `lib/src/client/client.dart:768` | verified (exp) | No timer covers closes started by the transport or by the decoder. After a protocol violation the reader stops, the 1002 frame waits behind queued data, and nothing ever finishes the connection if the peer does not read (observed: no `onClosed` until the server closed TCP). Likewise `client.dart:768` calls `connection.close(1007, ...)` without the `disconnectTimeout` timer that `disconnect()` and `_dropSession` arm, so a silent server holds the session until a heartbeat (if any) fires. | Arm a close timer inside the transport whenever `_closeSent` becomes true (one place, covers user close, echo, failure), or have `onFailure` drop the queue and write the close frame first. In the client, arm `closeTimer` in `_onDecodeError`. |
| F9 | Medium | `lib/src/transport/dart/dart_transport.dart:321-354` | verified (exp) | Sends before open are not rejected at the transport: before the TCP connect they are silently dropped (`_socket == null`, no error, `bufferedAmount` 0); after the connect but before the 101 a masked frame is written straight after the HTTP request, and in the proxy path into the middle of the CONNECT exchange. The client guards this today, but the transport interface is public (`package:ws/transport.dart`) and the web transport throws in the same situation. | `if (!_open || _closeSent || _finished) throw WebSocketNotConnectedException` in `_send`. |
| F10 | Medium | `lib/src/protocol/utf8_validator.dart:32-37, 53-76` | verified (exp) | The validator is 2x slower than `utf8.decode` on non-ASCII text (1 MiB: Cyrillic 4.9 ms vs 2.4 ms, CJK 4.3 vs 2.1, emoji 4.8 vs 2.4), because `_skipAscii` is entered after every multi-byte character, runs the alignment loop and allocates a `Uint64List` view when it happens to be aligned. Fragmented text and `validateUtf8: true` pay this and then decode again. ASCII (55 us) and mostly-ASCII JSON (1.05 ms vs 2.3 ms) are fine. | Enter the word scan only when it can pay off: `if (state == _accept && stop - i >= 16 && bytes[i] < 0x80)`. Create the 64-bit view once per `add` call instead of per run. Re-measure with `benchmark/bin/utf8_micro.dart`. |
| F11 | Medium | `lib/src/transport/web/web_transport.dart:39-45` | verified (read) | `maxMessageSize`, `headers`, `tls` and `proxy` are silently ignored in the browser. The size limit matters: the same code that is protected on the VM accepts a message of any size on the web. Headers are documented as ignored; the rest is not. | Enforce `maxMessageSize` in `_onMessage` (string length / `byteLength`), closing with 1009. Document the others in `WebSocketVmOptions`, or assert in debug mode when they are set on the web. |
| F12 | Low | `lib/src/protocol/frame_writer.dart:73-81` | verified (exp) | `FrameWriter.close` cuts the reason at 123 bytes without regard to character boundaries: `'é' * 62` (124 bytes) produces invalid UTF-8 and the peer answers 1007. Codes are not checked either: 1005, 1006, 0, 70000, -1 are all written. The client's `_checkClose` hides both for the public API; direct transport users and future internal reasons are not covered. | Truncate at the last complete code point (walk back while `(b & 0xC0) == 0x80`), and assert / throw unless `CloseCode.isValidOnWire(code)`. |
| F13 | Low | `lib/src/protocol/frame_reader.dart:155-165, 180` | verified (exp) | Failures that are known from the header are reported only after the whole payload arrived: a reserved opcode, a continuation with no message in progress, or a new data frame inside a fragmented message are buffered up to `maxMessageSize` first (header for opcode 3 with 1 MiB length: no failure yet). Also the payload buffer is allocated at full size from the header alone: an 11-byte header reserves 64 MiB of address space with the default limit (pages are lazily zeroed, so RSS does not move until data arrives). | Validate the opcode and the fragmentation state before buffering (Autobahn "fail fast"). For frames above about 1 MiB grow the buffer as data arrives, or lower the default `maxMessageSize` (most clients use 1 to 16 MiB). |
| F14 | Low | `lib/src/transport/dart/dart_transport.dart:375-378` | suspected | After echoing a close frame the socket is closed in both directions at once. If the server sent anything after its close frame that is still unread, the kernel answers with RST, which can discard our close frame on the server side (the server then logs a reset instead of a clean close). RFC 6455 section 7.1.1 asks the client to let the server close TCP first. | `shutdown(SocketDirection.send)`, keep reading (discarding) until `readClosed`, with a short timer (1 to 2 s) before the hard close. |
| F15 | Low | `lib/src/protocol/handshake.dart:156-173` | verified (exp) | Status line parsing is loose: `int.tryParse` accepts `0x65` and `+101` as 101, `HTTP/1.0` and `HTTP/1.x` pass, a header written as `Upgrade : websocket` is accepted, obsolete line folding is ignored. None is exploitable by itself (the accept key must still match), but a response a strict parser rejects is accepted here, which is the usual ingredient of response-smuggling through intermediaries. | Require `HTTP/1.1`, exactly three ASCII digits, no whitespace before the colon; reject lines that start with space or tab. |
| F16 | Low | `lib/src/protocol/handshake.dart:178-185` and `dart_transport.dart:224-250` | verified (exp) | Informational responses before the 101 fail the handshake (`103 Early Hints` then `101`: `Unexpected HTTP status 103`). Rare for WebSocket, but CDNs do send 103. | Skip 1xx responses other than 101 and keep reading. |
| F17 | Low | `lib/src/protocol/handshake.dart:63-68` | verified (exp) | Host edge cases: an IDN host (`ws://пример.рф`) becomes `Host: %D0%BF...` and is also what is passed to DNS, so it cannot connect; a zone id (`ws://[fe80::1%25eth0]`) is sent in `Host` and passed to the resolver as `fe80::1%25eth0`. `dart:io` `HttpClient` has the same IDN limitation. | Reject non-ASCII / percent-encoded hosts early with a clear `ArgumentError` ("use punycode"), strip the zone id from `Host` and decode `%25` for the socket address. |
| F18 | Low | `lib/src/protocol/handshake.dart:76-98` | verified (exp) | A URL with user info plus a user `Authorization` header sends two `Authorization` lines; servers pick either. | Let an explicit `Authorization` header win and skip the Basic line (compare names case-insensitively). |
| F19 | Low | `lib/src/transport/dart/dart_transport.dart:163-179` | verified (exp) | The head is re-concatenated and re-scanned from byte 0 on every read. A server that drips the response one byte per segment costs about 1.2 s of CPU for 64 KiB. Bounded, and the client's connect timer covers the wall time. | Remember the scan position (start at `previousLength - 3`) and keep chunks in the builder until the terminator is found. |
| F20 | Low | `lib/src/transport/dart/dart_transport.dart:57` | verified (read) | `validateUtf8: false` is hard-coded and `TransportRequest` has no switch, so the bytes codec (`codec.dart:50`) and custom codecs receive text frames that were never validated. DECISIONS D5 says such consumers "would need `validateUtf8: true`", but there is no way to ask for it. | Add the flag to `TransportRequest` (set by codecs that do not decode), or validate in `BytesCodec.fromTextBytes`. |
| F21 | Low | `lib/src/transport/web/web_transport.dart:95, 115-119, 66-78` | suspected / read | Web parity and speed: (a) on dart2wasm `JSArrayBuffer.toDart.asUint8List()` is a JS-backed view, every index is an interop call; codecs that scan bytes will be slow, a copy into a Dart `Uint8List` may be faster for small messages (measure). (b) `sendTextBytes` decodes UTF-8 to a string that the browser encodes again, and a `FormatException` from it is not a `WebSocketException`. (c) the close handler ignores `wasClean`; 1015 (TLS failure) is reported as a normal close with no error. (d) a URL with user info reaches `new WebSocket(...)` unchanged; browsers reject or drop credentials. | (a) benchmark both; (b) route text through `sendText` on the web in the codecs, wrap the decode; (c) report an error when `!wasClean`; (d) strip user info on the web and document it. |
| F22 | Low | `lib/src/protocol/handshake.dart:22-27`, `frame_writer.dart:16` | verified (exp) | `HandshakeRequest` creates up to 16 `Random.secure()` instances per request (2 us, harmless), and `Random.secure()` throws under dart2js on Node, so the protocol layer cannot run there. | Use one static `Random.secure()`. Only matters for Node if a server-side JS target is ever planned. |
| F23 | Low | `lib/src/protocol/frame_reader.dart:175` | verified (exp) | A single-frame message is a view of the read chunk: a 1-byte message handed to the user keeps 64 KiB alive for as long as the user holds it (documented as "safe to keep", but the cost is not). | Copy messages smaller than, say, 1/4 of their backing buffer, or document the retention. |
| F24 | Low | `lib/src/transport/dart/dart_transport.dart:99-105, 143-153` | verified (exp) | A malformed `trustedRootsPem` surfaces as `TLS handshake failed` (cause `BAD_PKCS12_DATA`) on every attempt and is retried as a network error; non-latin1 characters in the PEM are silently truncated by `codeUnits`. The `SecurityContext` is rebuilt for every attempt. | Build and cache the context once per options object; report a bad PEM as a terminal configuration error. |

### Outside my area, noticed while testing

- `server/src/endpoints/bench.rs:35`: `/echo` breaks out of its loop on
  `Message::Close` before tungstenite flushes the queued close reply, so
  every client-initiated close against `/echo` ends as
  `Connection closed without a close frame` (1006) on the client. The same
  client code gets a clean `1000` from a `dart:io` server. Tests that end
  with `client.close()` against `/echo` therefore never exercise a clean
  close handshake. Verified (exp).
- `lib/src/client/client.dart` retries configuration errors (F4, F5) and
  measures `bytesSent` from `bufferedAmount` deltas; the first is covered
  above, the second belongs to the client review.

### Fixed during the review (re-verified on the snapshot)

- wss through a proxy never worked: `RawSecureSocket.secure` threw
  `Subscription passed to TLS upgrade is paused`. Now passes, including
  certificate and host-name failures through the tunnel.
- Invalid headers (`Host`, non-latin1 values) escaped as unhandled errors
  with no `onClosed`. Now reported, but see F4 (user info still escapes) and
  F5 (reported synchronously).
- The 64 KiB head limit was skipped when the terminator arrived in the same
  read (`/big-head?kb=100` opened). Now rejected.

## Missing tests worth adding

- F1 / F2: reader memory bound under empty and 1-byte fragments (assert the
  failure code, not memory).
- F3: protocol list validation, at client and handshake level.
- F4 / F5: `connect()` with bad user info / reserved header never calls the
  listener before `connect()` returns, and ends with one `onClosed`.
- F6: ping flood against `/silent?read=0`-style server keeps
  `bufferedAmount` bounded.
- F7 / F8: close frame or violation while the queue is blocked
  (`rawServer()` with a paused subscription reproduces both).
- Proxy: data sent by the proxy right after its `200` head; abort during
  TLS-in-tunnel; `[::1]` target (works today, not pinned).
- Web: message above `maxMessageSize`; close with `wasClean == false`.

## Future risks to plan for

- **permessage-deflate (RFC 7692).** The reader rejects RSV1 and the
  handshake rejects any extension, which is correct today. Adding it needs:
  the limit applied to the *inflated* size while inflating (zip bombs: 1 MiB
  on the wire can be 1 GiB), a cap on the compression ratio, context
  takeover memory per connection (32 KiB window each way),
  `client_max_window_bits` negotiation, RSV1 only on the first fragment, and
  never on control frames. Compressing secrets together with
  attacker-influenced data on one context is the CRIME / BREACH pattern;
  offer `client_no_context_takeover` or keep it opt-in. F1 and F2 become
  worse with compression, fix them first.
- **Redirects.** Not followed today (the error carries `location`). If
  added: cap the count (the server has `/redirect-loop`), never downgrade
  `wss` to `ws`, drop `Authorization`, `Cookie` and URL user info when the
  host or scheme changes, re-run TLS verification per hop, and keep the
  proxy setting. The request provider callback is the safer place to let the
  application decide.
- **HTTP/2 and HTTP/3 WebSocket (RFC 8441, RFC 9220).** The Dart transport
  sends no ALPN, so servers pick HTTP/1.1. If ALPN is ever set (or a shared
  HTTP client is reused), an `h2`-only front end will break the hand-written
  handshake. Extended CONNECT changes the handshake (no key / accept), flow
  control replaces the write queue, and `bufferedAmount` needs a new
  definition.
- **Handshake authentication.** `401` with `WWW-Authenticate` (Digest,
  Negotiate) and `407` from the proxy are plain failures today. Decide
  whether the request provider is the only supported way to refresh
  credentials; document that proxy credentials travel in clear text to an
  `http://` proxy and that an `https://` proxy is not supported.
- **Proxy details.** CONNECT to port 80 is refused by many corporate
  proxies; environment variables (`HTTPS_PROXY`, `NO_PROXY`) and PAC are not
  read; SOCKS5 is not supported. The tunnel code rejects any byte after the
  proxy's head, which is right for a client-speaks-first protocol but must
  be revisited if anything server-first is ever tunnelled.
- **TLS options.** Client certificates (mTLS), certificate pinning and a
  minimum TLS version are common requests; the data-only `WebSocketTls`
  design (no `SecurityContext` in the API) needs fields for them, and they
  must mean the same in rustls. Session resumption and `close_notify`
  handling differ between BoringSSL (`dart:io`) and rustls: a truncation
  without `close_notify` should be an abnormal close in both.
- **Cookies and Origin.** Browsers send them automatically, the VM
  transports do not. Servers that check `Origin` for CSRF protection accept
  any native client; document that `Origin` can be set through headers and
  that it is not a security boundary for native apps.
- **Sending fragmented / streamed messages.** The writer emits one frame per
  message and copies the payload. Large uploads will need fragmentation with
  interleaved control frames and back-pressure per fragment; the close and
  pong paths (F6, F8) must stay ahead of queued data.
- **Timers inside the transport.** The pure-Dart transport has no timers at
  all (handshake, close, idle); every guarantee depends on the client arming
  one. Each new path that closes on its own (F8) repeats the problem. A
  single close deadline inside the transport removes the class.
- **32-bit and JS number semantics.** The protocol code is written to be
  exact on dart2js, but it is only exercised there by unit tests, and not at
  all on Node (F22) or on 32-bit ARM, where `Uint64List` views in
  `_skipAscii64` are slow. Keep the chrome and dart2wasm runs of
  `test/protocol` in CI, and add an armv7 run if Flutter armv7 stays
  supported.
- **Slow or hostile peers in general.** Per-connection budgets for control
  frames per second, fragments per message and bytes buffered for replies
  would turn F1, F2 and F6 from individual fixes into one policy; Node `ws`,
  tungstenite and OkHttp each have such limits.
