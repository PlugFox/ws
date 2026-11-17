# Test plan: scenarios to port

A list of test scenarios taken from other WebSocket test suites and mapped onto
`ws` 2.0's layers. Each scenario is marked covered, partial or missing, and lists
the server behaviour it needs. This file does not contain code. Coverage: VM line coverage 95.3%
(`mise run coverage`, 2026-11-13; 63.6% when this plan was written). Where a scenario would
reach uncovered lines, they are named as `file:line`.

## 1. Sources examined

| Key | Source | What was used |
|---|---|---|
| WS1 | `_lib/test` (ws 1.x) | `client/client_test.dart`, `model/*_test.dart`, `metrics/metrics_test.dart`, `connection_manager_test.dart` |
| SPN | [spinify `test/`](https://github.com/PlugFox/spinify/tree/master/test) | `unit/spinify_test.dart` (state, reconnect, ping, RPC), `smoke/smoke_test.dart`, `unit/model_test.dart` |
| SDK | dart-lang/sdk `tests/standalone/io/web_socket*_test.dart` | `web_socket_test`, `_protocol_test`, `_ping_test`, `_error_test`, `_abnormal_close_test`, `_compression_test`, `_protocol_processor_test` |
| CT | dart-lang/http `pkgs/web_socket_conformance_tests/lib/src` | all 8 groups (34 tests); details in section 4 |
| NODE | websockets/ws `test/websocket.test.js` (W), `receiver.test.js` (R), `sender.test.js` (S) | client-relevant cases |
| GO | gorilla/websocket `conn_test.go` (C), `client_server_test.go` (CS) | framing, EOF, limits, handshake |
| TG | snapview/tungstenite-rs `tests/*.rs`, `src/protocol/**` unit tests | close/reset, auto-pong flush, size limits, overflow |
| OK | square/okhttp `RealWebSocketTest.kt` (R), `WebSocketHttpTest.kt` (H), `WebSocketReaderTest.kt` | keepalive, close timeout, handshake headers, queue overflow |
| PY | python-websockets `tests/asyncio/test_client.py` (C), `test_connection.py` (K), `tests/test_protocol.py` (P) | reconnect loop, keepalive, close timeouts, redirects, backpressure |
| OURS | `test/protocol/protocol_test.dart`, `test/client/client_test.dart`, `test/transport/conformance.dart` (dart + native), `test/transport/web_transport_test.dart` | used to set the status column |

Out of scope: permessage-deflate (we never offer it, and RSV1 already fails
with 1002), server-side checks, and the Node/OkHttp-specific listener APIs.

## 2. Scenarios

Layers: **P** protocol unit (no I/O), **C** client core over
`FakeWebSocketServer` + `fake_async`, **T** transport conformance (dart:io +
Rust FFI through `transportConformance`; web as well when the server can be
spawned through hybrid), **N** native only, **W** web only.

**Server** column: `—` = no server, or the fake is enough. `raw` = the
in-test `rawServer()` in `conformance.dart` can do it today (VM only, so not
the web transport). `/x` = needs the fault endpoint from section 3. Using the
endpoint is also how the web transport gets the scenario.

**Status**: ✅ covered, ◐ partial, ❌ missing.

### 2.1 Protocol (P), pure unit tests

| ID | Scenario | Source | Server | Status |
|---|---|---|---|---|
| P1 | RSV2 / RSV3 set → 1002 (only RSV1 is tested today) | NODE R "RSV2 is on / RSV3 is on"; PY P test_bad_reserved_bits | — | ✅ unit tests (tests_docs §4) |
| P2 | RSV1 on a control frame and on a continuation frame → 1002 | NODE R "control frame has RSV1", "RSV1 on and opcode is 0" | — | ✅ unit tests (tests_docs §4) |
| P3 | New text/binary frame in the middle of a fragmented message → 1002 (`frame_reader.dart:199`) | NODE R "opcode 1/2 in the middle of a fragmented message"; OK Reader twoFrameNotContinuation | — | ✅ unit tests (tests_docs §4) |
| P4 | Fragmented text: the final fragment ends mid-codepoint, or the bad byte is only in a later fragment → 1007 (`frame_reader.dart:205,229,233`) | NODE R "invalid UTF-8 (2/2)"; TG; Autobahn 6.x | — | ✅ unit tests (tests_docs §4) |
| P5 | Single-frame text ending mid-codepoint (`81 01 E2`) → 1007 through the decoder path | Autobahn 6.3; NODE R "invalid UTF-8 (1/2)" | — | ✅ unit tests (tests_docs §4) |
| P6 | Close codes on the wire: 0, 999, 1004, 1006, 1015, 1016–2999, ≥5000 → 1002; 1000–1003, 1007–1014, 3000–4999 accepted | NODE R "invalid close code"; OK Reader closeOutOfRangeThrows; Autobahn 7.9.x | — | ✅ unit tests (tests_docs §4) |
| P7 | 64-bit length: all bits set, and 2^32+5 (no wrap at 32 bits on dart2js), against the limit | TG frame::parse_overflow, length_above_usize_max_rejected; GO C TestReadLimit overflow | — | ✅ unit tests (tests_docs §4) |
| P8 | Non-minimal length encoding (`7E 00 05`): decide the policy (NODE, GO and TG all accept it), then pin it | RFC 6455 §5.2; agent finding | — | ✅ unit tests (tests_docs §4) |
| P9 | Many zero-length continuations (fragment-count DoS) are bounded | NODE R "too many message fragments", "too many buffered chunks" | — | ✅ unit tests (tests_docs §4) |
| P10 | Thousands of tiny frames in one chunk; frame sizes 0/1/125/126/127/65535/65536 across read chunk sizes | NODE R "thousands of frames"; GO C TestFraming | — | ✅ unit tests (tests_docs §4) |
| P11 | Message exactly at `maxMessageSize` with an interleaved ping passes; +1 byte fails | GO C TestReadLimit "is enforced" | — | ✅ unit tests (tests_docs §4) |
| P12 | Control payload exactly 125 passes; empty ping/pong/close passes | GO C TestControl | — | ✅ unit tests (tests_docs §4) |
| P13 | Unsolicited pong is reported and the data stream is unaffected; ping/pong/text/binary keep their order | TG protocol::receive_messages | — | ✅ unit tests (tests_docs §4) |
| P14 | Handshake validation (`handshake.dart:160–192`): missing `Upgrade`, missing `Connection`, a token list (`Upgrade: foo, websocket`, `Connection: keep-alive, Upgrade`), mixed-case header names, missing vs duplicated `Sec-WebSocket-Accept`, an unoffered extension, an unknown subprotocol, a protocol when none was offered | OK H missing/wrong Connection, Upgrade, MagicHeader; NODE W subprotocol/extension cases; GO CS TestDialExtraTokensInRespHeaders; SDK testBadUpgradeResponse | — | ✅ unit tests (tests_docs §4) |
| P15 | `user:pass@host` URL → `Authorization: Basic` with percent-decoding (`handshake.dart:77–79`); IPv6 literal → `Host: [::1]:port` | NODE W "authorization header if url has userinfo"; SDK testBasicAuthentication | — | ✅ unit tests (tests_docs §4) |
| P16 | Junk or partial head: `headEnd` returns -1 (`:122`); a non-HTTP status line → `FormatException`; `HandshakeError.toString` (`:218`) | PY C test_junk_handshake_response | — | ✅ unit tests (tests_docs §4) |
| P17 | Writer: close reason of exactly 123 bytes, and a multibyte char straddling 123 | OK H closeReasonMaximumLength; CT "too long close reason" | — | ✅ unit tests (tests_docs §4) |

### 2.2 Client core (C), fake transport

| ID | Scenario | Source | Server | Status |
|---|---|---|---|---|
| C1 | `toString` and fields of every state, exception and `WebSocketMetrics` (`state.dart` 40%, `exceptions.dart` 47%, `metrics.dart` 17%) | WS1 model/state_test, websocket_exception_test, metrics_test; SPN model_test Metric ToString | — | ✅ unit tests (tests_docs §4) |
| C2 | Metrics over a lifecycle: `connects`, `reconnectAttempts`, `bytesSent/Received`, `lastError`, `openedAt`, `pendingRpc`, `bufferedAmount` | SPN Metrics; WS1 metrics "from client" | — | ◐ |
| C3 | Full sequence connect→disconnect→connect→disconnect→close emits states in order; `states`/`messages` are done after close | SPN Change_client_states, Closed_after_close | — | ◐ |
| C4 | Idempotence: disconnect ×10 on a disconnected client; close() twice; disconnect after close | SPN Disconnect_disconnected; CT "close after close"; PY K test_close_idempotency; SDK testDoubleCloseClient | — | ✅ unit tests (tests_docs §4) |
| C5 | Using a closed client: push/rpc/ping/reconnect/disconnect error with typed errors (`client.dart:288,291`) | WS1 "Closed client" group, "try use closed client" | — | ✅ unit tests (tests_docs §4) |
| C6 | connect() repeated while connecting to the same URL is joined, not restarted (`client.dart:272`); several URLs in a row | SPN Few_connects_in_a_row; WS1 "connect to different URLs" | — | ◐ |
| C7 | close() (terminal) while connecting / during the handshake / during the reconnect wait | NODE W "#close while connecting (1/3–3/3)", "#terminate while connecting"; OK H closeWithoutSuccessfulConnect | — | ◐ (disconnect only) |
| C8 | Close 1013 → the next delay is `maxDelay` (`client.dart:446`) | NODE W "allows close code 1013" | — | ✅ unit tests (tests_docs §4) |
| C9 | Request provider throws → `WebSocketConnectionException`, retried (`client.dart:333–339`) | PY C test_reconnect (token refresh variants) | — | ✅ unit tests (tests_docs §4) |
| C10 | `onOpen` hook throws → connection dropped, cause reported (`client.dart:371–374`) | WS1 "first message after connect" (inverse) | — | ✅ unit tests (tests_docs §4) |
| C11 | `shouldReconnect` throws → reported, treated as no (`client.dart:465`) | PY C test_reconnect_with_custom_process_exception (raising variant) | — | ✅ unit tests (tests_docs §4) |
| C12 | Mixed failure sequence: stall (timeout) → drop → 503 → ok (stable) → drop → 402 terminal; check attempts, delays, final state | PY C test_reconnect | — | ◐ policy per failure on real sockets (D48); mixed sequences in the random runs (D49) |
| C13 | Late events from a replaced session (old fake connection sends or closes after a URL switch or reconnect) are ignored (the `identical(_session, …)` guards) | ARCHITECTURE "session identity"; SPN Reconnect_after_disconnected_transport | — | ✅ `client_edge_test.dart` "events of a replaced connection are ignored" (D47) |
| C14 | After a local disconnect nothing more reaches `messages`; messages already delivered before close stay delivered | CT "peer writes after close are ignored"; PY K test_close_preserves_queued_messages | — | ✅ unit tests (tests_docs §4) |
| C15 | push of an unencodable value (JSON cyclic or unsupported) → `WebSocketFormatException`, connection stays open (`client.dart:572`, `codec.dart:161`) | WS1 "emit unsupported data type" | — | ✅ unit tests (tests_docs §4) |
| C16 | Codec branches: binary client receives text, json client receives binary, invalid JSON in binary (`codec.dart:72–91,129–137`) | WS1 message_stream_test | — | ✅ unit tests (tests_docs §4) |
| C17 | RPC: 100 concurrent calls with shuffled replies; not connected (`:592`); `encode` throws (`:601`); `match` throws (`:723`); `decode` throws → `remote` (`:737`); the send fails after registration (`:610`) | SPN RPC_many_requests, smoke RPC Concurrency | — | ✅ unit tests (tests_docs §4) |
| C18 | Manual `ping()`: not connected (`:691`), timeout (`:938`), two concurrent pings resolved by one pong | PY K test_acknowledge_previous_ping; OK R afterSocketClosedPingFailsWebSocket | — | ✅ unit tests (tests_docs §4) |
| C19 | Heartbeat `ping` builder throws (`:913`); `isPong` throws (`:713`) | — (callback guarding rule) | — | ✅ unit tests (tests_docs §4) |
| C20 | Heartbeat stops on disconnect/close: no pending timers left (`fake_async.pendingTimers`) | PY K test_keepalive_terminates_*; SDK web_socket_ping_test testPingCancelledOnClose | — | ✅ unit tests (tests_docs §4) |
| C21 | Any inbound traffic defers the idle ping; a long-lived chatty connection never pings; unsolicited pongs count as liveness (document this: OK treats them as not acknowledging) | SPN Missing_pings, Emulate_long_lifespan; OK R unexpectedPongsDoNotInterfereWithFailureDetection | — | ◐ |
| C22 | Re-entrancy: `onStateChange`/listeners call `disconnect()`, `connect()`, `close()` synchronously for each state (D22) | — (rule from ARCHITECTURE) | — | ✅ `reentrancy_test.dart`, 9 operations × 6 states (D49) |
| C23 | Races: `connect`+`close`, `disconnect`+server close, `reconnect()`+`disconnect()` in the same microtask; pushes awaiting drain when the connection drops | NODE W "calls the callback if the socket is forcibly closed"; GO C TestConcurrencyWriteControl | — | ◐ |
| C24 | Randomized operation sequences (seeded) over the fake, checking invariants: one terminal state, no event after `closed`, waiters always complete | ROADMAP §6; SPN Change_client_states | — | ✅ `reentrancy_test.dart`, 300 seeds × 60 steps (D49) |
| C25 | No `onError`: errors go to the zone handler (`client.dart:225`) | — | — | ✅ unit tests (tests_docs §4) |
| C26 | `close`/`disconnect` validation: codes 0, 999, 1004–1006, 2999, 5000 rejected; 3000 and 4999 accepted; reason of exactly 123 bytes accepted | CT close_local "reserved close code …"; OK R clientCloseWith0Fails | — | ✅ unit tests (tests_docs §4) |
| C27 | Backoff: `maxDelay` cap, jitter bounds, `minDelay ≥ maxDelay` | WS1 connection_manager "backoffDelay", "minDelay >= maxDelay"; SPN util Backoff | — | ◐ |

### 2.3 Transport conformance (T): dart:io, native, and web where marked

| ID | Scenario | Source | Server | Web? | Status |
|---|---|---|---|---|---|
| T1 | Server drops TCP right after 101 (FIN, then RST variant), no close frame → `onClosed(null, …, ConnectionException)`, client reconnects (`dart_transport.dart:146–151`) | NODE W "close event raised when server closes"; SDK abnormal_close, testForceCloseServerEnd; CT disconnect_after_upgrade | raw / `/drop` | yes | ✅ server suite, every transport (D46) |
| T2 | Server closes during the handshake (after reading the request) → `HandshakeException('closed during the handshake')`, retried | PY C test_connection_closed_during_handshake | raw / `/eof-handshake` | yes | ✅ server suite, every transport (D46) |
| T3 | TCP accepted, no response → `connectTimeout` on real transports, retried | PY C test_timeout_during_handshake; NODE W "opening handshake timeout"; GO CS TestHandshakeTimeout; OK H callTimeoutAppliesToSetup | raw / `/hang` | yes | ✅ server suite, every transport (D46) |
| T4 | Junk (non-HTTP) response, and an oversized response head → handshake errors (`dart_transport.dart:136,191`) | PY C test_junk_handshake_response | raw / `/junk`, `/big-head` | no (browser hides) | ✅ server suite, every transport (D46) |
| T5 | Invalid 101 variants end to end (as in P14), plus whether each is terminal or retried | OK H *Header tests; NODE W Accept/Upgrade/subprotocol/extension; TG handshake.rs | raw / `/bad-101` | no | ✅ server suite, every transport (D46) |
| T6 | Status matrix: 200 without upgrade, 400/401/403/404 terminal, 408/425/429 retried, 500/503 retried, `Retry-After` exposed in `headers` | OK H notFound, non101RetainsBody; CT no_upgrade; SDK testNoUpgrade; PY C test_reconnect | `/status` | yes | ✅ with reconnect on (D48) |
| T7 | Redirect 301/302/307/308: decide the policy (browsers fail; PY follows with a limit and strips credentials cross-origin) and pin it | NODE W followRedirects/maxRedirects; PY C test_redirect*, test_redirect_limit, test_redirect_to_insecure_uri | `/redirect` | yes | ✅ server suite, every transport (D46) |
| T8 | Close-handshake timeout: server ignores our close frame → abort after `disconnectTimeout`, state reaches disconnected | OK R clientCloseCancelsConnectionAfterTimeout; OK H clientCancelsIfCloseIsNotAcknowledged; PY K test_close_timeout_waiting_for_close_frame; NODE W closeTimeout | `/ignore-close` | yes | ✅ server suite, every transport (D46) |
| T9 | Server answers our close but keeps TCP open → the client closes TCP itself in bounded time | PY K test_close_timeout_waiting_for_connection_closed | `/close?then=hold` | yes | ❌ |
| T10 | Server sends close, then RST at once (no wait for our reply) → still a clean close with its code | TG connection_reset::test_evil_server_close; PY P test_client_receives_close_then_eof; OK R serverCloseBreaksReadMessageLoop | `/close?then=rst` | yes | ✅ server suite, every transport (D46) |
| T11 | Server close codes 1000/1001/1011/1012/1013/1014/3000/4999 with reasons (including a multibyte one at 123 bytes); an empty close → `code == null` (1005) | NODE W 1013/1014; CT close_remote; OK R emptyCloseInitiatesShutdown | `/close` | yes | ✅ server suite, every transport (D46) |
| T12 | Simultaneous close (both sides send close in the same instant) → one `onClosed` | OK R clientAndServerCloseClosesConnection, serverCloseThenClientClose; NODE W "closes cleanly after simultaneous errors" | `/close?after_ms=` | yes | ❌ |
| T13 | Data frames arriving after we sent close (before the ack) are dropped (CT) or delivered (TG); decide and pin | TG receive_after_init_close; CT "peer writes after close are ignored" | `/flood` + disconnect | yes | ❌ |
| T14 | Malformed frame after our close / inside the close reply → still terminates with one `onClosed` | OK R protocolErrorInCloseResponseClosesConnection, protocolErrorAfterCloseDoesNotSendClose | `/raw` | no | ❌ |
| T15 | EOF inside a frame header, inside a payload, and between fragments → connection error, never a partial message | GO C TestEOFWithinFrame, TestEOFBeforeFinalFrame; PY P EOFTests; OK R socketClosedDuringMessageKillsWebSocket | raw / `/raw?end=fin` | no | ✅ server suite, every transport (D46) |
| T16 | Garbage or protocol violation after 101 → we send close 1002 and the client reports 1002; variants with a pending write | CT peer_protocol_errors "bad data after upgrade (with write)"; NODE W "emits error if an error occurs" | raw / `/raw` | no | ✅ server suite, every transport (D46) |
| T17 | Close frame arriving between non-final fragments → close wins, partial message discarded | GO C TestCloseFrameBeforeFinalMessageFrame | raw / `/raw` | no | ❌ |
| T18 | Invalid UTF-8 (single and fragmented) end to end → 1007, not retried; on native both the `raw` and `json` text modes | PY K test_recv_non_utf8_text; NODE R invalid UTF-8 | raw / `/raw` | no | ✅ server suite, every transport (D46) |
| T19 | Server ping with payload → pong with the same payload; ping during a fragmented message; unsolicited pong → `onPong` and nothing else | NODE W "pong called automatically"; PY P test_client_receives_ping*; TG receive_messages | `/ping`, `/pong` | n/a | ✅ server suite, every transport (D46) |
| T20 | Auto-pong still goes out while the app is busy and the socket write would block | TG auto_pong_flush | `/ping` + `/slow-read` | n/a | ❌ |
| T21 | Slow reader: `bufferedAmount` grows, `requestDrain` → `onDrain`, push awaits above the high-water mark, close waits for queued data (`dart_transport.dart:262–324`) | NODE W bufferedAmount tests; NODE S "consume all data before closing"; OK H overflowOutgoingQueue; PY K test_write_limit | `/slow-read` | yes | ❌ |
| T22 | Stall in the middle of a frame (header plus half the payload, then silence) → heartbeat or idle timeout fires | OK H readTimeoutAppliesWithinFrames | `/stall` | yes | ❌ |
| T23 | Half-open TCP (peer silent, no FIN) → heartbeat timeout reconnects | OK H unacknowledgedPingFailsConnection; PY K test_keepalive_times_out; SPN Ping_without_pong | raw / `/silent` | yes (app ping) | ✅ (dart, native) |
| T24 | Burst: 10k small messages each way keep their order; server floods right after 101 | CT "peer writes after close"; NODE R thousands of frames; WS1 "Dublicate send and receive" | `/flood` (≈ `/stream?rate=0`) | yes | ◐ in order from the server, every transport (D49); client-to-server order open |
| T25 | Sizes: empty text, empty binary, exactly `maxMessageSize`, 16 MiB, 64-bit length path | CT payload_transfer "empty string/binary", "large string/binary"; GO C TestFraming; SDK testMessageLength | `/echo` | yes | ◐ (no empties) |
| T26 | Send / ping / close after `onClosed` are no-ops or typed errors, with nothing on the wire | CT "sendText/sendBytes after close"; NODE W "#send … readyState 2 or 3"; TG no_send_after_close | `/close` | yes | ✅ every transport alike (D50): sends and pings throw `WebSocketNotConnectedException`, close and abort do nothing |
| T27 | `abort()` while TCP connecting, during TLS, during the handshake → exactly one `onClosed`, no socket leak | NODE W "#terminate while connecting"; OK H closeWithoutSuccessfulConnect | `/hang` | yes | ❌ |
| T28 | Contract checker: wrap every listener in the suite with an assertion decorator (`onOpen` ≤1, one `onClosed` last, nothing synchronous from connection methods) | ARCHITECTURE "Transport contract" | — | yes | ✅ `test/support/checked_transport.dart` on every transport (D47) |
| T29 | TLS: `allowInsecure` against self-signed; `trustedRootsPem` with the server's cert; unknown CA rejected; hostname mismatch rejected; `wss://` against a plain port and `ws://` against a TLS port (`dart_transport.dart:85–113`) | PY C(Secure) test_reject_invalid_server_certificate/_hostname; NODE W "cannot connect to secure server via ws://"; TG wss_fails_when_no_tls | `/ca.pem`, TLS ports | no | ✅ server suite, every transport (D46) |
| T30 | IPv6: `ws://[::1]:port` connects and sends the right `Host` | — (requested) | `/headers` on `::1` | yes | ✅ server suite, every transport (D46) |
| T31 | Request headers seen by the server: custom headers, userinfo auth, protocols, no forbidden headers overridden | SDK testAdditionalHeaders; PY C test_additional_headers; OK H clientIncludesForbiddenHeader | `/headers` | partial | ✅ server suite, every transport (D46) |
| T32 | Server picks a protocol we did not offer / none when offered → typed error (CT expects the connect to throw) | CT protocol_tests "protocol mismatch"; TG handshake.rs | `/bad-101?protocol=` | no | ✅ server suite, every transport (D46) |
| T33 | Connect timeout does not apply after open (an idle connection longer than `connectTimeout` survives) | OK H callTimeoutDoesNotApplyOnceConnected | `/echo` | yes | ❌ |

### 2.4 Native only (N) and web only (W)

| ID | Scenario | Source | Server | Status |
|---|---|---|---|---|
| N1 | `textMode.json`: invalid JSON → decode error reported, connection stays open (`client.dart:1000–1010` `onJson` path) | client decode-error rule | `/raw` or `/echo` | ❌ |
| N2 | 100 clients on one native library at once; open/close loops; process exit with open connections | ROADMAP §6 soak; SPN Create_and_close_multiple_clients | `/echo` | ❌ |
| N3 | Drop or close from Dart while Rust is delivering a large stream (credit and drain events, no use-after-free) | — | `/stream?rate=0` | ❌ |
| N4 | Rust answers server pings while Dart is blocked (busy isolate) | TG auto_pong_flush | `/ping` | ❌ |
| W1 | 1006 abnormal close maps to an error and reconnects (`web_transport.dart:70`) | SDK abnormal_close; CT disconnect_after_upgrade | `/drop` | ✅ shared server suite in the browser (D46) |
| W2 | Empty close (1005) → `code == null` (`web_transport.dart:68`) | CT close_local "close" | `/close-empty` | ✅ shared server suite in the browser (D46) |
| W3 | `bufferedAmount` polling drain under a slow reader | NODE W bufferedAmount | `/slow-read` | ✅ shared server suite in the browser (D46) |
| W4 | Application-level heartbeat against a silent server reconnects (no protocol ping in the browser) | SPN Ping_without_pong | `/silent` | ✅ shared server suite in the browser (D46) |
| W5 | Handshake failure of any kind is a retried `ConnectionException` (no status) | OUR web test | `/status`, `/bad-101` | ✅ shared server suite and web_transport_test (D46) |

## 3. Fault endpoints (Rust test server)

Implemented: `server/README.md` lists every endpoint as it exists now
(including later additions such as `/early-hints`). This section is the
original proposal, kept for the reasoning. At the time, the server handed
every TCP stream to `accept_hdr_async_with_config` before routing, so
tungstenite answered every handshake, auto-ponged and validated frames.
Fault endpoints needed their own front end:

1. Read the HTTP head yourself (cap: 16 KiB) and route on the path.
2. HTTP-level faults answer directly.
3. Everything else writes its own `101` and then either:
   - wraps the stream with `WebSocketStream::from_raw_socket(stream, Role::Server, cfg)` for the well-behaved cases, or
   - writes raw bytes for the scripted cases.

Keep `/echo` and `/stream` unchanged for the benchmarks.

Common rules:

- All query values are decimal unless named `hex`.
- `id=<token>` on any WebSocket endpoint records what the client sent (see `/log`).
- `then=` and `end=` take one of:
  - `fin`: shutdown(Write) then close
  - `rst`: SO_LINGER 0 then close
  - `hold`: keep TCP open, read and discard
  - `wait`: wait for the client's close frame and FIN

### 3.1 Handshake-level

| Endpoint | Semantics | Serves |
|---|---|---|
| `/status?code=403&body=text&header=Retry-After:5` (repeatable `header`) | Reply `HTTP/1.1 <code>` with the headers and body, `Connection: close`, then close. `code=200` means "no upgrade". | T6, W5, C12 |
| `/redirect?code=302&to=<urlenc>` and `/redirect-loop?n=` | 3xx with `Location`. The loop redirects to itself n times, then to `/echo`. | T7 |
| `/hang` | Read the request, never respond, keep TCP open until the client goes. | T3, T27 |
| `/delay?ms=500&then=<path>` | Sleep, then handle as `<path>` (e.g. `then=/echo`). | T3 (just under/over timeout), C7 on real transports |
| `/eof-handshake?partial=0\|1` | After the request: close (`partial=1` writes `HTTP/1.1 101 Swi` first). | T2 |
| `/junk` | Write `220 smtp.example ESMTP\r\n` and close. | T4 |
| `/big-head?kb=64` | 101 with a filler header of the given size. | T4 |
| `/bad-101?omit=accept\|upgrade\|connection&accept=wrong\|dup\|nonb64&upgrade=<v>&connection=<v>&protocol=<v>&extensions=<v>` | A crafted 101. Unset parts are correct; `protocol` is sent even if not offered. On success it continues as `/echo`. | T5, T32, P14 e2e |
| `/headers` | 101, then one text message: JSON `{method, target, version, headers:[[k,v],…], peer}`, then echo. | T30, T31, P15 e2e |

### 3.2 After the handshake

| Endpoint | Semantics | Serves |
|---|---|---|
| `/close?code=&reason=&after_ms=0&after_msgs=0&then=wait\|fin\|rst\|hold` | After the delay or N echoed messages, send close (code omitted → empty payload). Then `wait` (normal), `fin`/`rst` without waiting for our close, or `hold` (reply to our close but keep TCP open). | T9–T12, T26, W2 |
| `/close-empty` | Shorthand for `/close` with no code. | W2, T11 |
| `/drop?after_ms=0&after_msgs=0&mode=fin\|rst` | Drop TCP with no close frame. | T1, W1 |
| `/ignore-close` | Echo, but never answer a close frame; keep TCP open. | T8 |
| `/silent?read=1` | After 101 send nothing, never pong. `read=0` also stops reading, so the TCP window fills. | T23, W4, T21 (with `read=0`) |
| `/slow-read?bps=4096&pause_ms=0` | Read at most `bps`, or stop reading for `pause_ms` then resume. Echo nothing. Report the total received in `/log`. | T20, T21, W3 |
| `/flood?count=10000&size=16&kind=text\|binary&then=close\|stay` | Send `count` messages right after 101 as fast as possible. Like `/stream?rate=0&count=`, but with no JSON payload cost and a `then`. | T13, T24, N3 |
| `/ping?payload=<hex>&interval_ms=0&count=1` | Send pings. The server reports `pong-ok`/`pong-mismatch` as a text message per ping. | T19, T20, N4 |
| `/pong?interval_ms=100` | Send unsolicited pongs, never answer pings. | T19, C21 e2e |
| `/stall?size=1000&sent=500` | Send a binary frame header for `size` but only `sent` payload bytes, then hold. | T22 |
| `/fragment?size=&parts=&kind=text\|binary&ping_between=0\|1&split_utf8=0\|1` | Send one message in `parts` fragments. `split_utf8=1` places boundaries inside codepoints. | T15–T17 (valid half), P4 e2e |
| `/raw?hex=<bytes>&preset=<name>&chunk=0&delay_ms=0&end=wait\|fin\|rst\|hold` | Write the bytes (or a preset) after 101, in chunks of `chunk` bytes (0 = one write) `delay_ms` apart, then `end`. See the preset list below. | T14–T18, N1 |
| `/log?id=` (plain HTTP GET) | JSON of what connection `id` received: frames `{op, fin, len, masked, rsv}`, close code and reason, bytes read, how TCP ended (`fin`/`rst`/`open`). Lets tests assert what the client sent without a channel back over the socket. | T8, T12, T16, T21, T26 |

Presets for `/raw` keep URLs short and shared by every transport:

| Group | Presets |
|---|---|
| Header bits and opcodes | `rsv1`, `rsv2`, `rsv3`, `opcode3`, `opcodeB`, `masked` |
| Control frames | `ping-nofin`, `ping126` |
| Fragmentation | `cont-first`, `text-mid-frag`, `close-mid-frag` |
| Close frames | `close1b`, `close-code-<n>`, `close-bad-utf8` |
| UTF-8 | `utf8-bad`, `utf8-trunc`, `utf8-frag-bad` |
| Lengths | `len64-msb`, `len64-max`, `len-nonmin` |
| Truncation and limits | `eof-in-header`, `eof-in-payload`, `eof-between-frags`, `zero-cont-x10000` |

### 3.3 TLS, addressing, harness

| Item | Semantics |
|---|---|
| TLS ports | Keep the current self-signed `localhost/127.0.0.1` port and add a port whose cert is for `wrong.invalid`. Both serve every endpoint. |
| `/ca.pem` (plain HTTP) | The PEM of the generated certificate, for `trustedRootsPem` tests (T29). |
| IPv6 | Also bind `[::1]` when available; report it in the ready line (T30). |
| Startup | `--port 0 --tls-port 0` prints one JSON ready line `{"ws":p,"wss":p,"wss_badhost":p,"ws6":p\|null}`. VM tests spawn it in `setUpAll`. Web tests use a `hybridMain` that spawns the binary and forwards the ports. `start_embedded` can serve the native tests in-process. |

## 4. `web_socket_conformance_tests` feasibility

| Question | Finding |
|---|---|
| Entry point | `testAll(Future<WebSocket> Function(Uri uri, {Iterable<String>? protocols}) factory)`. Groups are also exported singly: `testCloseLocal`, `testCloseRemote`, `testConnectUri`, `testDisconnectAfterUpgrade`, `testNoUpgrade`, `testPayloadTransfer`, `testPeerProtocolErrors`, `testProtocols`. |
| Interface | `package:web_socket` (hosted, 1.x) `abstract interface class WebSocket` has `sendText`, `sendBytes`, `Future<void> close([int? code, String? reason])`, `Stream<WebSocketEvent> events`, `String protocol`. Events: `TextDataReceived`, `BinaryDataReceived`, `CloseReceived([code, reason])` (value equality). Errors: `WebSocketException`, `WebSocketConnectionClosed`. |
| Servers | Each group has a `*_server.dart` with `hybridMain(StreamChannel)` (dart:io `HttpServer`, some with `detachSocket` for raw bytes). It runs in-process on the VM and through `spawnHybridUri('package:web_socket_conformance_tests/…')` in the browser, so no setup is needed beyond resolving the package. |
| Distribution | `publish_to: none` (not on pub.dev). Use a git dependency pinned to a sha (`url: https://github.com/dart-lang/http`, `path: pkgs/web_socket_conformance_tests`). It pulls `dart_style`, and through it `analyzer`, which may conflict. Recommended: a separate unpublished package `test_conformance/` (`publish_to: none`, depending on `ws` by path) so `ws`'s pubspec stays clean and `dart pub publish` is not affected. |
| Expected semantics | Bad scheme (`https:`) → `ArgumentError` from the factory. Non-101 or a protocol mismatch → `WebSocketException`. `close()` validates synchronously (only 1000 and 3000–4999; reason ≤123 UTF-8 bytes). `close()` with no code sends an empty close (the server must see 1005). After a local close, `events` ends without `CloseReceived` and later peer data is dropped. After a remote close or a drop, exactly one `CloseReceived` (1005/1006 accepted), then done. Sending after close, or a second close, throws `WebSocketConnectionClosed`. |
| Adapter level | **Wrap `WebSocketTransport`, not `WebSocketClient`.** The transport contract maps 1:1: `onOpen` completes the factory, an `onClosed` before open fails it, and `onClosed` after open emits `CloseReceived(code ?? 1005 or 1006 on error)` and closes `events`. `close(null)` already sends an empty frame. The client wrapper clashes on: default code 1000, http→ws rewriting, reconnect, and async validation. The adapter itself must do code/reason validation, the `https` → `ArgumentError` check, the synchronous throw after close, and drop events after a local close. About 120 lines. |
| Transports | dart:io: VM. Native: VM, behind the existing skip-if-no-library check. Web: `-p chrome` and `-p chrome -c dart2wasm` with hybrid servers. One adapter serves all three. |
| What it adds | 34 tests overlapping T1, T6, T11, T16, T25, T26, T32, C26, plus the "peer writes after close" semantics (T13). It has no ping, close-timeout, redirect, reconnect, backpressure, TLS or half-open tests, so section 3 is still needed. |
| Verdict | Feasible and cheap. Adopt it as the first step (cross-transport close semantics), then build the Rust fault server for the rest. |

## 5. Suggested order

1. Free coverage, no server: C1, C4, C5, C8–C11, C15–C20, C25, P1–P7, P14–P17. These hit most of the uncovered lines in `state`, `exceptions`, `metrics`, `codec`, `handshake`, `frame_reader` and `client`.
2. `web_socket_conformance_tests` through a transport adapter (section 4).
3. With the `rawServer()` that exists today (VM: dart + native): T1, T2, T3, T4, T15, T16, T17, T18.
4. Rust fault server (section 3), then T5–T14, T19–T22, T27, T29–T30, N*, W*.
5. C22–C24 (re-entrancy, races, randomized sequences).
