# Review: tests, documentation, benchmarks

Scope: `test/**`, `native/test/**`, `benchmark/test`, Rust unit tests in
`native/rust`, all Markdown docs, dartdoc in `lib/`, and the benchmark
harness. Reviewed on branch `rewrite`, working tree of 2026-11-12, after
`test/transport/server_suite.dart` and `test/support/test_server.dart`
landed. Other agents were editing the tree during the review: line numbers
in `docs/ROADMAP.md`, `docs/DECISIONS.md` and `server_suite.dart` may have
moved by a few lines, and two intermediate test runs failed on half-written
edits (a compile error in `native_transport.dart`, a placeholder in
`server_suite.dart`). The numbers below are from the last runs, after those
edits settled.

## 1. What was run

| Command | Result | Wall time |
|---|---|---|
| `dart test --platform vm` (server binary built, no `WS_NATIVE_LIBRARY`) | **239 passed, 13 skipped, 45 failed** (all 45 are finding T-1) | 16.6 s |
| `mise run coverage` (= VM run with `WS_NATIVE_LIBRARY`) | 295 passed, 0 failed; **96.0% lines (1915/1995)** | ~17 s |
| `cd native && dart test` (hook path; cargo build, `native/prebuilt/` is empty) | 13 passed | 13.1 s |
| `dart test --platform chrome` | 114 passed | ~4 s |
| `dart test --platform chrome --compiler dart2wasm` | 114 passed | ~2 s |
| `cd native/rust && cargo test --release` | 17 passed | <1 s |
| `dart analyze` (root and `native/`, `public_member_api_docs` on) | no issues | |
| `dart doc --dry-run` | 1 warning (D-12) | |
| `python3 benchmark/autobahn/summary.py benchmark/build/autobahn/index.json` | matches DECISIONS D28 exactly (296/2/3/0 and 287/11/3/0) | |

Before the server suite existed the same commands gave 194 passed / 13
skipped (VM), 97 passed (native by path + transport), and 91.7% coverage.
The older 13-test conformance file was repeated 8 times on both VM
transports with no failed run. The server suite was run 5 times in total;
its only failures were the two mid-edit states mentioned above.

Coverage per file (VM only; `web_transport.dart` and `transport_stub.dart`
are never loaded on the VM and are missing from the report, so the total
overstates package coverage):

| File | Lines | Not covered |
|---|---|---|
| `lib/src/transport/transport_registry.dart` | 66.7% (2/3) | 19 |
| `lib/src/protocol/utf8_validator.dart` | 73.1% (38/52) | 21, 78–95 (the non-64-bit scan, used on the web) |
| `lib/src/transport/dart/proxy_tunnel.dart` | 83.3% (40/48) | 66–67, 84, 91–97 |
| `lib/src/native/install.dart` | 88.9% (48/54) | 35–36, 60, 116–120 |
| `lib/src/client/codec.dart` | 93.0% (53/57) | 91, 165–169 |
| `lib/src/transport/dart/dart_transport.dart` | 93.4% (185/198) | 183, 203, 239–241, 315–316, 328–329, 351–352 |
| `lib/src/transport/fake/fake_transport.dart` | 93.9% (93/99) | incl. the dead `onJson` branch |
| `lib/src/native/native_transport.dart` | 95.5% (106/111) | 48, 109, 205, 212–213 |
| `lib/src/client/client.dart` | 96.9% (445/459) | 316, 525, 598, 780, 839, 861, 934, 940, 993–1002 (`onTextString`), 1012 |
| everything else | 96–100% | |

State of the suite in one paragraph: the protocol and client-core tests are
good. They assert outcomes (states, close codes, bytes on the wire, timer
counts), run in fake time, and pass on VM, dart2js and dart2wasm. The new
server suite closed the largest gaps of the first pass of this review (TLS,
proxy, handshake faults, abnormal endings, IPv6, real backpressure) for the
two VM transports. What remains: the default `dart test` run is broken by
the way the suite is wired (T-1), CI never runs it (T-2), the browser runs
neither suite (T-3), and several tests assert less than their names say.

## 2. Test findings

Severity: **High** = the suite fails, does not run where it is claimed to
run, or a test cannot fail for the thing it names; **Medium** = a test that
can pass while the behaviour is broken, or a real parity/flakiness risk;
**Low** = hygiene.

| # | Sev | Where | Finding | Fix |
|---|---|---|---|---|
| T-1 | High | `test/native/native_library_test.dart:43-61` | `dart test --platform vm` fails with 45 errors (`LateInitializationError: Local 'library' has not been initialized`) when the server binary is built and `WS_NATIVE_LIBRARY` is not set. That is exactly `mise run test:vm` and the CI command. The inner `group('native', …, skip: TestServer.skip)` passes an explicit `skip: false`, which overrides the skip of the enclosing `loaded by path` group, so its tests run while `setUpAll` (which opens the library) does not. | Give the inner group one combined condition: `skip: _library == null ? '…' : TestServer.skip`. Or drop the inner `skip` and call `markTestSkipped` from `serverConformance`'s `setUpAll` when the binary is missing. |
| T-2 | High | `.github/workflows/checkout.yml:54-68`, `native.yml:166-175`, `test/support/test_server.dart:35-37` | No workflow builds `server/`, so in CI every server-backed test is skipped without a failure. TLS, proxy, handshake-fault and protocol-violation coverage (about 90 of the 295 tests) exists only on a workstation. The skip is silent by design (`TestServer.skip`). The moment CI does build the server, the VM job hits T-1. | Build the server in the VM and native jobs (`cargo build --release` in `server/`, cached). In CI set an env var that turns a missing binary into a failure instead of a skip. |
| T-3 | High | `test/transport/web_transport_test.dart:1-104`; `conformance.dart:7`; `server_suite.dart:14`; ROADMAP:73, :416 | The web transport runs neither shared suite. Both import `dart:io` (directly or through `test_server.dart`), so the browser has its own 4 tests: echo, ping unsupported, close 4001, rejected handshake. Not covered on web: abnormal close 1006, empty close (1005 to `null`), `disconnect(code)` as seen by the server, `maxMessageSize`, the `bufferedAmount` polling drain, application-level heartbeat against a silent server, `abort()`, large messages, the JSON corpus. ROADMAP:73 says a shared suite enforces identical behaviour and M5 lists "shared conformance suite in the browser". | The Rust server makes this cheap now: a `hybridMain` that spawns it and returns the ports. Split `serverConformance` into a platform-neutral body taking URLs and a capability record (`rawFrames`, `status`, `headers`, `tls`, `proxy`), and run it on all three transports. `/close`, `/close-empty`, `/drop`, `/flood`, `/slow-read`, `/silent`, `/ignore-close` all work from a browser. |
| T-4 | High | `test/client/client_edge_test.dart:260-282` | `events of a replaced connection are ignored` cannot fail. `client.connect('ws://b')` closes the old connection; the fake echoes the close (`echoClose` defaults to `true`, `fake_transport.dart:274`), so `old.isOpen` is false and `old.sendText('stale')` on line 275 never runs. Even with `echoClose = false`, `_later` (`fake_transport.dart:135-137`) drops events of an ended connection inside the fake. The `identical(_session, this)` guard described in ARCHITECTURE:72-77 is not exercised here. | Set `old.echoClose = false` before the switch, remove the `if`, assert `got == ['fresh']`; also send `old.pong()` and `old.drain()` and assert no state change. Remove the `if (connection.isOpen)` on line 298 too (true today, but it would hide a regression). |
| T-5 | High | `conformance.dart:127-135`, `:311-319`; `web_transport_test.dart:41-42`; `server_suite.dart` (`fragment … ping_between`, `.length, 100`) | Binary and fragmented payloads are checked by length only (`70000`, `100000`, `100`). A masking bug past the first block, a wrong offset in a zero-copy view or a swapped buffer would pass. Text is compared in full. | Compare content (`orderedEquals(sent)`), and use a non-constant pattern for the 100 000-byte frame (`List.filled(100000, 7)` hides offset bugs). |
| T-6 | Medium | `server_suite.dart` `a flood of 20000 messages arrives complete and in order` | The test counts messages and nothing else. Order and content are not checked, so the name promises more than the test delivers. On the native transport this is the test that crosses batch and credit boundaries, where reordering would come from. | Have `/flood` number the messages (or use `/stream`) and assert the sequence; assert that no 20 001st message arrives. |
| T-7 | Medium | `server_suite.dart` `a slow reader fills the queue; push waits, then drains` | Backpressure is detected by a 5 ms real-time race (`Future.any` with `Future.delayed(5 ms)`). A `push` that is merely slow on a loaded machine counts as "waited", so the test can pass with no backpressure at all. It also does not check `bufferedAmount`, that the server received every byte, or `sendBufferLimit`. | Assert on state, not time: after the loop `c.metrics`/`bufferedAmount` is above `sendHighWaterMark` and the pending future is not complete after `pumpEventQueue`; then read `/log` and assert the byte count. Add a case for `WebSocketBufferFullException` on a real transport. |
| T-8 | Medium | `server_suite.dart` TLS and fault groups | Several assertions accept almost any failure. "An unknown CA is rejected" and "a certificate for another host is rejected" expect `WebSocketConnectionException`, which a refused connection also produces. "wss to a plain port…", "EOF, junk and a huge head" expect any `WebSocketException`; a 5 s `connectTimeout` would pass. No TLS or handshake-fault test checks whether the client retries or stops (every client uses `WebSocketReconnect.disabled()`). | Assert on the cause or message (certificate/handshake), assert `isNot(isA<WebSocketTimeoutException>())` as the header test already does, and add one test per class with reconnect on that pins terminal vs retried. |
| T-9 | Medium | `server_suite.dart:376`, `:400`, `:412` (`anyOf(1002, 1009)`) | The suite hides a real divergence. For a 64-bit length with the MSB set the pure-Dart transport closes with 1002 and the native one with 1009 (seen in a run made while the expectation was still exact). With `anyOf` the difference is accepted for good, and `shouldReconnect` users see different codes per transport. | Decide one code (RFC 6455 §5.2 makes MSB-set a protocol error: 1002), map it in the Rust transport, and make the expectation exact. If the divergence stays, record it in DECISIONS and in the transport table of ARCHITECTURE. |
| T-10 | Medium | `fake_transport.dart:135-137`, `:199-203` | Fake vs real parity: the fake drops events queued before a close. `server.last..sendText('x')..close()` delivers nothing, because `close()` sets `_ended` synchronously and the queued microtask then sees it. Real transports deliver data that precedes the close frame. A user testing "last message, then close" against the fake gets a result production never shows. | Keep a per-connection event queue and let `close()`/`fail()` take their place in it. Add a fake test: message then close delivers the message, then `onClosed`. |
| T-11 | Medium | `fake_transport.dart:171-173`, `:223` | Fake vs real parity: the fake delivers only `onTextBytes`. `_decodeJson` is hard-wired to `false`, so the `onJson` branch is dead, and there is no way to deliver `onTextString`. The client's `onTextString` path (`client.dart:993-1002`, the only one used on the web) and `onJson` path are covered only by browser and native runs, never in fake time; their decode-error handling has no test (TEST_PLAN N1). | `FakeWebSocketServer(delivery: bytes \| string \| json)`, reporting `decodesJson` to match. Run the JSON, decode-error and `isPong` client tests once per mode. |
| T-12 | Medium | `dart_transport.dart` (`_send`), `native_transport.dart` (`ping`, `_send`), `web_transport.dart:113`, `:137-139`, `fake_transport.dart:262-266` | Transports disagree on calls after close and no test pins it (TEST_PLAN T26). `ping()` on a closing connection throws `WebSocketNotConnectedException` on dart and fake, is a silent no-op on native, and throws `WebSocketUnsupportedException` on web. `sendTextBytes` with invalid UTF-8 throws `WebSocketFormatException` on native, a plain `FormatException` on web (`utf8.decode`), and is sent as is on dart. The fake ignores `maxMessageSize`. | Write the contract into the `TransportConnection` docs, then add shared cases: send/ping/close after `onClosed`, and after a local `close()` before the ack. |
| T-13 | Medium | `conformance.dart:152-179`, `:264-290` | Two tests take 3 s each on every transport because `client.close()` waits for the default `disconnectTimeout`: the corpus server never listens on its socket and `rawServer` never answers a close frame. The wait is real time and asserts nothing; it is 6 of the ~16 s of a VM run. The explicit test now exists in the server suite (`a server ignoring our close…`), so these waits are pure cost. | Answer the close in both servers, or pass `disconnectTimeout: 100 ms`. |
| T-14 | Medium | `test/client/client_test.dart:295-330`; `lib/src/util/backoff.dart` | `delays grow while attempts keep failing` asserts only an upper bound (`lessThanOrEqualTo(100 + 100 * 2^(i-1))`). A constant delay passes. `backoffDelay` has no direct test, and the jitter uses an unseeded `Random`, so the client tests cannot assert exact values. | Unit-test `backoffDelay` with a stub `Random` (0 and 0.999…): floor, ceiling per attempt, cap at `maxDelay`, `min >= max`, attempt ≥ 31. |
| T-15 | Medium | `native/test/native_transport_test.dart`; `native/hook/build.dart:37-150` | The hook path runs only the 13-test conformance file, not the server suite. The hook itself has no test: source selection (`none`, `build`, `prebuilt`, `library:`), checksum mismatch, missing `checksums.json` and the fall back to pure Dart. `native/prebuilt/` is empty locally, so the prebuilt path every user will take is never run before a release. | Call `serverConformance` from `native/test` too. Move `_prebuilt` and the source decision into a small library with unit tests on a temp directory. Add a CI job that runs `cd native && dart test` after `ci:prebuilt` filled `native/prebuilt/`. |
| T-16 | Medium | `native/rust/src/lib.rs:941-961`, `tape.rs`, `dart.rs` | Rust unit coverage is thin where the `unsafe` is: 2 tests in `lib.rs` (layout, config defaults), none in `tape.rs` or `dart.rs`. Lifecycle, credits, `catch_unwind` and finalizers are tested only through Dart. The Rust tape builder is compared with the Dart one only through a 7-document corpus (`conformance.dart:155-163`); ROADMAP:365 lists tape compatibility as open. | Export a test-only `ws_native_tape(bytes)` and compare byte-for-byte with Dart `buildTape` over the random-document generator in `test/native/tape_test.dart:69-101`. Feed truncated and corrupt tapes to `TapeDocument` and require `FormatException`. Run the Rust tests under Miri or ASan in CI. |
| T-17 | Medium | `.github/workflows/native.yml:149-150` | Native conformance runs on Ubuntu and macOS only. Windows is a declared platform of `ws_native` with its own TLS verifier and DLL naming, and the Rust transport is never exercised there. | Add `windows-latest` to the matrix. |
| T-18 | Medium | `conformance.dart:338-345` | `masked server frame fails the connection with 1002` polls real time (100 × 10 ms) until `received` is non-empty, then indexes `out[0..7]`. A first read of fewer than 8 bytes throws `RangeError`; nothing in 1 s fails with an index error instead of a message. Not seen in 8 repeated runs. The server suite's `/log` endpoint does this properly. | Move the assertion to the server suite (`/raw?preset=masked&id=…`, then `server.log(id)`), or wait on a `Completer` with a timeout. |
| T-19 | Low | `test/support/test_server.dart:48`, `:86-94` | The server's stderr is drained and discarded, so a panic or a bind failure shows up as a 20 s timeout with no cause. `log()` polls 200 × 25 ms. Each test file starts its own server process; fine today with two files. | Buffer stderr and attach it to the failure (`addTearDown` + `printOnFailure`). |
| T-20 | Low | `conformance.dart:244-246` | `connection refused is retried` binds port 0, closes it and reuses the number; another process or test file can take the port in between. `server_suite.dart` uses port 1 for the dead proxy, which is safer. | Use the same trick, or keep a listener that destroys every accepted socket. |
| T-21 | Low | repo root | No `dart_test.yaml`: default 30 s timeout per test, no tags for `native`/`server`/`browser`, no presets shared by mise and CI. | Add one with `timeout: 15s`, tags and presets. |
| T-22 | Low | `test/client/client_test.dart:252-259`, `lib/src/client/client.dart:596-598` | `rpc without a handler throws` expects `AssertionError`. With assertions off the client returns a failed future with `StateError` (line 598, uncovered). The test pins the debug-only behaviour, and the two modes differ in kind: a synchronous throw vs an asynchronous error. | Drop the `assert`, always return the failed future (or always throw), and test that. |
| T-23 | Low | `mise.toml` (`coverage`), workflows | Coverage is a local task: no CI job, no floor, no browser coverage. | Run it in CI with a floor (today 96.0%). |
| T-24 | Low | `benchmark/test/tape_test.dart` | A second tape test for the prototype tape in `benchmark/lib/src/tape.dart`. The assertions are meaningful (24 documents against `jsonDecode`, lookups on a 500-item document, 8 invalid inputs), but no mise task or workflow runs it. | Add `cd benchmark && dart test` to `mise run test`, or delete the prototype tape once the benchmarks use the library one. |
| T-25 | Low | `test/client/*.dart` | Most fake-time tests end with an unawaited `client.close()` and do not check `async.pendingTimers`; only two do (`client_edge_test.dart:134`, `:586`). | Put the check into the shared `fake()` helper. |

## 3. Does the conformance suite run identically on all transports?

| Transport | How it runs | `conformance.dart` (13) | `server_suite.dart` (~48) | Same code as the others? |
|---|---|---|---|---|
| pure Dart | `test/transport/dart_transport_test.dart` | yes | yes, if the server binary exists | yes |
| native, by path | `test/native/native_library_test.dart` + `WS_NATIVE_LIBRARY` | yes | yes, if the binary exists; broken without the library (T-1) | yes |
| native, by hook | `native/test/native_transport_test.dart` | yes | **no** (T-15) | partly; cargo path only, prebuilt path never |
| web, dart2js and dart2wasm | `test/transport/web_transport_test.dart` | **no** | **no** | no: 4 separate tests (T-3) |
| CI | workflows | dart ×3 OS, native ×2 OS | **never** (T-2) | |

So the honest statement today is: pure Dart and native-by-path share about
61 transport tests on a developer machine; CI checks 13 of them; the
browser checks none of them.

Both suites drive `WebSocketClient`, not the transport interface, so they
check the client plus a transport. Nothing checks the `TransportListener`
contract directly ("`onOpen` at most once, exactly one `onClosed`, last,
never synchronous", `transport.dart:56-58`). TEST_PLAN T28 proposes a
checking decorator installed in `setUp`; with two suites calling
`WebSocketTransports.use(create())` in one place each, it is a 40-line
change that makes every existing test verify the contract.

## 4. Missing tests, ranked

Ranked by (risk of an unnoticed production bug) × (how cheap the test is
now that the Rust server exists). IDs in brackets are from
`docs/TEST_PLAN.md`.

1. **Make the existing server suite count**: fix T-1, build the server in CI (T-2). No new tests, and it is worth more than any item below.
2. **The browser on the shared suites** (T-3): 1006, empty close, `disconnect(code)` seen by the server, flood, slow reader with the polling drain, application heartbeat against `/silent`, `maxMessageSize` [W1–W5, T21 on web].
3. **Transport contract checker** around every listener [T28], plus the repaired session-identity test (T-4) [C13].
4. **Reconnect policy on real transports**: with reconnect on, which failures are terminal and which are retried: 401/403/404 vs 408/425/429/500/503, TLS trust failure, proxy 407, bad 101, close 1008 vs 1001/1012, 1013 waiting `maxDelay`. Today only 403 and "refused" are pinned on real sockets; everything else in the server suite runs with reconnect disabled [T6, C12].
5. **Calls after close** on every transport, one behaviour (T-12) [T26].
6. **Exact backpressure** (T-7) including `sendBufferLimit`, and exact flood ordering (T-6) [T21, T24].
7. **Remaining close variants**: server answers our close but holds TCP open (`/close?then=hold`), simultaneous close, data frames after our close, malformed frame after our close [T9, T12, T13, T14].
8. **Client paths for `onTextString` and `onJson`** in fake time, with decode errors (T-11) [N1].
9. **Tape equivalence Rust vs Dart** over random documents, plus corrupt and truncated tapes (T-16). A tape is external memory read by Dart; a bad length must be a `FormatException`, not an out-of-bounds read.
10. **Stalls and aborts**: stall in the middle of a frame until the heartbeat fires (`/stall`), `abort()`/`close()` during TCP connect, during TLS and during the handshake with exactly one `onClosed`, and `connectTimeout` not applying after open [T22, T27, T33].
11. **Auto-pong under load**: pong still sent while Dart is busy (native) and while the socket write would block [T20, N4].
12. **Build hook** resolution and checksum errors; server suite on the hook path (T-15).
13. **`backoffDelay` unit tests** with a stub `Random` (T-14) [C27].
14. **Re-entrancy and randomized sequences** over the fake: `disconnect()`/`connect()`/`close()` called from `onStateChange` in each state; seeded random operations with invariants (one terminal state, nothing after `closed`, every waiter completes) [C22, C24]. D22 promises re-entrancy safety; the only `onStateChange` in the tests (`client_test.dart:494`) just records.
15. **Native lifecycle**: 100 clients on one library, open/close loops, close from Dart during `/flood`, isolate exit with open connections [N2, N3].
16. **Exact content checks** for binary and fragmented messages (T-5); trivial to do.

`docs/TEST_PLAN.md` against the suite today: the status column is badly out
of date. Marked ❌ or ◐ in the plan but covered now:

- by `protocol_edge_test.dart`, `client_edge_test.dart`, `model_test.dart`, `codec_test.dart`: P1–P17, C1, C4, C5, C8–C11, C14–C20, C25, C26;
- by `server_suite.dart` on dart + native: T1, T2, T3, T4, T5, T7 (policy: not followed), T8, T10, T11, T15, T16, T18 (raw mode), T19 (server ping), T29, T30, T31, T32, most of T6 and T25, and loosely T21 and T24 (T-6, T-7).

Still open as the plan says: C12, C22, C24, most of C23; T9, T12–T14, T17
end to end (`close-mid-frag` is in the server's preset list but not in the
suite), T20, T22, T26, T27, T28, T33; every N row; every W row. C13 has a
test by name but is not tested (T-4).

## 5. Documentation findings

| # | Sev | Where | Finding | Fix |
|---|---|---|---|---|
| D-1 | High | `docs/ROADMAP.md:82-253` (§4) vs `:410-420` (§7) | The §4 checklists are almost all `[ ]` for things that are implemented, tested and reported as done elsewhere in the same file: sealed states and errors (`:106`, `:109`), fake transport (`:116`), fragmentation/ping/close/UTF-8 (`:156`), subprotocols and headers (`:162`), max message size (`:164`), custom CAs (`:175`; `trustedRootsPem` exists and is tested), the allow-insecure option (`:176`; `WebSocketTls.allowInsecure`), web implementation (`:232`), reconnect (`:245`), heartbeat (`:247`), metrics (`:249`). The file calls itself the living plan. A reader cannot tell what is left for 2.0. | Tick what is done with a pointer to the D-number or the test; move what was dropped (D-2) to a "not in 2.0" list. |
| D-2 | High | `docs/ROADMAP.md:53`, `:88-103`, `:251`; `CHANGELOG.md:1-17` | The roadmap describes an API that was not built: `sendText`/`sendBytes`/`sendJson`, a per-connection `decoder` (`raw`, `jsonTape`, `isolate<T>`), interceptors/middleware, `done`. The shipped API is `push`/`rpc` with three codecs (README, D11–D13). The diagram at `:53` still lists "interceptors · decoders". CHANGELOG 1.0.0 advertised interceptors, and the 2.0.0-dev.1 entry does not say they are gone. | Rewrite §3–§4.1 to the shipped API. Add a "Removed / changed from 1.x" list to the CHANGELOG (interceptors, constructor, option classes) and mention the proxy, TLS options and `sendBufferLimit` that it also omits. |
| D-3 | High | `docs/ROADMAP.md:73`, `:416`, `:433-434` | "A shared conformance test suite enforces" identical behaviour of all transports, and the risk "transports diverge" is mitigated by "one shared conformance suite". The web transport runs neither suite and CI runs a fifth of it (section 3). | State the real coverage until T-2 and T-3 are done. |
| D-4 | High | `docs/ARCHITECTURE.md:135-146` | The Testing section is stale. Autobahn is "still to come" (ROADMAP:356 and D28 say done). The table has no rows for the native transport, the tape, the installer, `conformance.dart` or the server suite, and it names `dart_transport_test.dart` as the home of the raw-server tests; that file only wires the two suites. | Replace the table with the one in section 3 of this review; drop Autobahn from "still to come". |
| D-5 | Medium | `docs/DECISIONS.md:862`; `docs/DEVELOPMENT.md:11-17` | D36 still ends with "End-to-end proxy tests wait for the test server"; they exist now. DEVELOPMENT does not say that the VM and native tests need `mise run server:build`, that they are skipped without it, or that `WS_TEST_SERVER` overrides the path. | Update D36; add a bullet to DEVELOPMENT. |
| D-6 | Medium | `lib/native.dart:5-7` vs `README.md:104-105`, `docs/ROADMAP.md:223-225` | The library dartdoc says `dart compile exe` "does not run build hooks". README and ROADMAP say that is wrong: it refuses to compile a program that depends on a package with hooks, and ROADMAP strikes the old wording out. | Use the README wording. |
| D-7 | Medium | `docs/ROADMAP.md:218`, `:435` vs `docs/DECISIONS.md:832-833` | Binary size: ROADMAP says "~1.7 MB stripped per ABI (~0.9 MB gzipped)" and "~1–2 MB per ABI". D35 gives 2,144,848 bytes for macOS (equal to the file on disk today) and 2,331,304 for Android arm64. | Update to ~2.1–2.3 MB and re-measure the gzip figure. |
| D-8 | Medium | `docs/DECISIONS.md:618` vs D34, D36 | D29's "Open" list still has "Happy Eyeballs for IPv6" (adopted in D34) and "the native send queue is not bounded" (closed by D36). | Strike both with pointers. |
| D-9 | Medium | `docs/ROADMAP.md:216`, `:387` vs `docs/DEVELOPMENT.md:30-31` | 16 KB page alignment is `[ ]` twice in ROADMAP; DEVELOPMENT says the Android libraries are aligned and how to verify it. | Tick `:216`; keep `:387` for "verified on a Play upload". |
| D-10 | Medium | `docs/TEST_PLAN.md:5-6`, `:14`, `:149-151`, status columns | "Coverage baseline 63.6%": today 96.0%. About 50 rows have a stale status (list at the end of section 4). §3 describes the old server ("hands every TCP stream to `accept_hdr_async_with_config`") as current. `file:line` references into `client.dart`, `handshake.dart` and `dart_transport.dart` predate later edits. Line 14 is an absolute path on the author's machine. | Refresh statuses and the baseline, turn §3 into "implemented, see server/README.md", use symbol names instead of line numbers, and the repository URL instead of the local path. |
| D-11 | Medium | `README.md:68`; `lib/src/util/backoff.dart:3-4` | "Full-jitter exponential backoff". The code computes `min + random(0, min(max - min, min * 2^attempt))`. AWS full jitter is `random(0, min(cap, base * 2^attempt))`, with no floor. The floor is a sound choice, but the name and the cited article describe another algorithm. | "Exponential backoff with jitter above a `minDelay` floor", with the formula in the `WebSocketReconnect` dartdoc. |
| D-12 | Low | `lib/src/client/options.dart:80` | `dart doc --dry-run`: "unresolved doc reference [attempt]" in `WebSocketReconnect.shouldReconnect`. The only dartdoc warning. | Write `` `attempt` ``. |
| D-13 | Low | `example/ws_example.dart:7`, `server/README.md:9` | "`make server`". The Makefile was removed (D33); the task is `mise run server`. | Replace. |
| D-14 | Low | `docs/ROADMAP.md:366` | Refers to `test/tape_test.dart`; the files are `test/native/tape_test.dart` and `benchmark/test/tape_test.dart`. | Fix the path. |
| D-15 | Low | `docs/ROADMAP.md:56`, `:70` vs `README.md:20-22`, `:79` | The diagram labels native "(FFI, default)" and pure Dart "(fallback)", and says the transport is picked "native, then pure Dart". As shipped, pure Dart is the default and native is opt-in through `ws_native` and `WsNative.install()` (D8). | Relabel. |
| D-16 | Low | `docs/ARCHITECTURE.md:118` | Transport table: drain for Fake is "—". The fake has `drain()` and delivers `onDrain` (`fake_transport.dart:108-114`). | "`drain()`". |
| D-17 | Low | `docs/DEVELOPMENT.md:8` vs `.github/workflows/checkout.yml:44-45` | "`mise run check`: what CI checks". CI formats `bin/ lib/ test/ tool/`; the task also formats `native/…` and runs actionlint and shellcheck, which no workflow runs, and it builds the server, which CI does not. | Make CI call the mise tasks, or say "a superset of CI". |
| D-18 | Low | `docs/DECISIONS.md:760` | "`mise run check` took 16 s". The VM tests alone take about 16 s now. | Drop or date the number. |
| D-19 | Low | `docs/DECISIONS.md:543-572` (D28), `docs/ROADMAP.md:356` | The Autobahn table and case lists match `benchmark/build/autobahn/index.json` exactly. But `benchmark/build/` is git-ignored, so the evidence is not in the repository, and Autobahn is not in CI. The harness drives the transport directly; `WebSocketClient` is not under Autobahn. | Commit `index.json` (117 KB) under `benchmark/results/autobahn/`; say "transport level" in ROADMAP. |
| D-20 | Low | `README.md:66-73` | The 1013 rule (wait `maxDelay`, `client.dart:446`, tested) and the redirect policy (3xx is reported, not followed; tested in the server suite) are not documented for users. | One sentence each under "Reconnects". |

Public API documentation: `public_member_api_docs` is on in both packages
and the analyzer reports nothing, so every public member has a doc comment.
`dart doc` has the single warning D-12.

Checked and correct: README install and usage snippets against the API used
in tests; `dart run ws:install --target … --out …` (`bin/install.dart:14-21`);
hook `source: none | build | prebuilt` and `library:`
(`native/hook/build.dart:10-11,37-78`); terminal 4xx except 408/425/429
(`client.dart:473`); terminal close codes 1002, 1003, 1007–1010
(`options.dart:49-56`); D28 Autobahn numbers; D35 macOS dylib size; the
description of `test:native` in DEVELOPMENT; the workflow behind the README
badge. The performance numbers in the docs are covered in section 6.

## 6. Benchmarks

Method: the harness code was read, and every table in `benchmark/RESULTS.md`
was regenerated by script from the raw `.jsonl` files (1023 + 222 + 222
cells, 0 mismatches). The micro benchmarks were run once from the prebuilt
AOT binaries on a loaded machine; the matrix was not re-run. Two causes
below are inferred from code and data and were not instrumented; they are
marked "likely".

Verdict: the tables are an honest copy of the data. The weaknesses are that
the macOS raw data is not committed, some columns measure the harness, and
the headline claims in README, ROADMAP and DECISIONS depend on a baseline,
a scenario or a single run that the text does not state.

### 6.1 Methodology facts

| Question | Answer |
|---|---|
| Warm-up | None in the macro benchmarks: a fresh process per run, timing starts at the first message, nothing is discarded. Stream has a 1 s idle calibration of the main-isolate probe, excluded from CPU. Micros warm up for 100 ms. |
| AOT | Yes for everything measured: `dart build cli` for the bench binary, Flutter release for the mobile app, AOT bundles for the micros. Only the orchestrator `bin/matrix.dart` runs under `dart run`. The build mode is not recorded in the data. |
| Repeats | 3 on macOS, 2 on mobile, 1 for library-vs-prototypes (D27). No variance, range or confidence interval is printed anywhere. |
| Aggregation | Not a per-metric median: the run with the median *key* metric is selected and all its other columns are printed (`bin/matrix.dart:100-103`). With 2 runs it selects the higher key value: best-of-2 for flood and ping-pong, worst-of-2 for stream. |
| CPU accounting | `getrusage(RUSAGE_SELF)`, user + system. Includes the Rust and GC threads, excludes the desktop server process: correct. On mobile the embedded server's CPU is subtracted. |
| Percentiles | `lib/src/stats.dart:16` is nearest-rank and correct. Samples are small: 60 for the 1 MB stream (p99 is the second-largest value), 200 for mobile 60 KB. |
| Fairness | Same payloads, masking on, compression off, `TCP_NODELAY` on, UTF-8 validated in the decoder for every variant. Differences: native variants use 2 tokio threads plus main against one thread; rows that are not "→ model" favour tape variants, which never materialise the body; the TLS cipher suite is neither pinned nor recorded. |
| Provenance | No file records commit, date, machine, OS, toolchain versions, arguments or run index. The mobile files carry only `device`. |

Run-to-run spread computed from the macOS data (range ÷ median over 3 runs):

| Metric | Median spread | p90 spread |
|---|---:|---:|
| ping-pong msg/s | 1.2% | 3.0% |
| stream CPU | 1.9% | 5.7% |
| flood MB/s | 2.0% | 8.7% |
| stream latency p50 | 3.3% | 8.6% |
| stream latency p99 | 7.6% | 42% |
| flood peak RSS | 8.8% | 56% |
| longest main stall | 9.2% | 228% |
| echo p50 | 13.8% | 24% |

On Android (2 runs) the flood spread is 7.5% median, 24% at p90.

### 6.2 Findings

| # | Sev | Where | Finding | Fix |
|---|---|---|---|---|
| B-1 | High | `benchmark/RESULTS.md:28-179`; `mise.toml` (`bench`, `bench:quick`); `bin/matrix.dart:37` | The macOS matrix comes from `benchmark/build/results.jsonl`, which is git-ignored. The claims built on it (among them "own codec 20–50% faster than `dart:io`") have no committed data. `bench:quick` passes no `--out` and so overwrites the same file. | Commit it as `benchmark/results/matrix-macos.jsonl`, name it in RESULTS.md, give `bench:quick` its own output. |
| B-2 | High | `benchmark/lib/src/runner.dart:135-138`; `RESULTS.md:44-58` | On the first message, with the probe running, the harness re-encodes the message on the main isolate (`utf8.encode(jsonEncode(…))`) to learn its size. The "longest main-isolate stall" table largely measures that: worker variants, which decode off main, show one outlier that scales with the payload (about 0.2 ms at 14 KB, 0.7–0.8 ms at 60 KB, 12 ms at 1 MB, but 13–23 µs for binary) while their stall p99 is 0–1 µs. It also adds about 200 µs/msg to the 1 MB row (60 messages). Likely cause. | Take the size from the wire length or after `probe.stop()`. Report stall p99 next to max. |
| B-3 | High | `runner.dart:144`, `lib/src/probe.dart:32-40`; `RESULTS.md:73`, `:225-228`, `:314-317`; `docs/ROADMAP.md:35` | Stream latency of the variants that run TLS on main looks inflated by the probe's 100 µs spin chunks. macOS, wss, 14 KB: codec on main p50 1422 µs, same codec in a worker 357 µs; without TLS the order is reversed (223 vs 294), and in ping-pong (no probe) main wins. The same cell is unstable between runs (`c-tape` p50 1430, 844, 1450; in D27 `c` 1458 vs `ws-dart` 870). The mobile "5–8× lower latency" claim comes only from these rows. Likely cause. | Measure latency in a pass without the probe, or report both. Do not quote wss stream latency ratios until then. |
| B-4 | High | `README.md:21`; `docs/ROADMAP.md:35` | "4–21× faster on phones" / "4.6–8× JSON, 10–21× binary" is true only for wss flood on loopback, native against the *best pure-Dart worker variant*, best of 2 passes, in bursts of 0.2–0.5 s. Against `dart:io` the same cells give 3.1–11× (JSON) and 18.5–33× (binary). Ping-pong gives 0.87–2.6×: on Android `d-tape` is slower than `dart:io` (2775 vs 3192 msg/s at 1 KB, 924 vs 1058 at 14 KB). | State scenario, transport and baseline; add the ping-pong range; remove the unqualified number from the README install snippet. |
| B-5 | High | `bin/matrix.dart:100-103`; `RESULTS.md:5`, `:185-186` | "Median of 3 runs" is the selected-run rule above. 473 of 946 non-key cells differ from the per-metric median, 58 by more than 10% and 29 by more than 25% (stall max of `c-worker-raw` printed as 2852 µs; runs 385, 2852, 217). For worker variants the stream key metric is noise around zero, so the selection is arbitrary. | Per-metric medians, min–max per cell, an odd repeat count of at least 5. |
| B-6 | High | `benchmark/app/lib/main.dart:59-64`; `RESULTS.md:239-246` | Android pass 2 is slower than pass 1 in flood (median ratio 0.965, minimum 0.54) and the table prints the faster pass. `d-tape` wss 14 KB is 268 and 146 MB/s, so the "1.9× over `d-bytes`" cell is one run. No thermal state, battery level or pass index is recorded. | At least 5 passes, pass index and thermal state recorded, median with range. |
| B-7 | Medium | `docs/DECISIONS.md:537-538` (D27); `RESULTS.md:385-386` | "The client core … cost nothing measurable … within run-to-run noise" rests on one run per cell, so no noise was measured there. Against the 3-run spread of the prototypes (mostly 0.4–3%) several rows that the D27 table does not show are outside it: `ws-native` ÷ `d-tape` flood binary 0.90, wss binary 0.91, ping-pong 14 KB → model 0.915; `ws-dart` ÷ `c` ping-pong wss 0.928, flood 60 KB → model 0.93; `ws-native` stream CPU at 50 B 1.31×. The 28 numbers the table does show match the data. | Re-run with 3+ repeats, publish every row with its spread, soften the sentence. |
| B-8 | Medium | `docs/DECISIONS.md:539-540`; `RESULTS.md:386` | "Receive credits cut the native flood RSS by up to half" holds in 2 of 7 rows (226→108, 203→119 MB), is unchanged in 4 and worse in 1 (wss 14 KB: 137.5→180.9). One run; flood RSS spread is 8.8% median and 56% at p90; and RSS scales with the message count (prototype 60 KB → model: 226 MB quick, 667 MB full). | Re-run at full counts with repeats and plot RSS against count to show that it is bounded. |
| B-9 | Medium | D27 data vs working tree | Every result file predates the current code: `native/rust/src/lib.rs`, `native_transport.dart`, `dart_transport.dart` and `client.dart` were changed after the last benchmark run of the day. | Stamp each row with git SHA (+dirty), time, machine, OS, Dart/Rust/Flutter versions, arguments, repeat index; re-run D27 before the release. |
| B-10 | Medium | `docs/ROADMAP.md:36`; `docs/DECISIONS.md:51-52` | "Own codec is 20–50% faster than `dart:io` `WebSocket`" holds for JSON only (flood 1.18–1.23×, ping-pong 1.17–1.50×, stream main 1.12–1.23×) and reflects the byte-level JSON pipeline rather than framing. Binary is at parity or slower: flood 1.01×, wss flood 0.98×, ping-pong 0.93×. | Say "JSON path on bytes". Add a `dart:io` variant with `addUtf8Text` + `JsonUtf8Encoder` to isolate the codec. |
| B-11 | Medium | `runner.dart:230-246`; `RESULTS.md:167-179` | The echo scenario cannot separate variants: p50 spread is 14% median, p99 31% (up to 96%); round trip is 7–8× ping-pong for the same payload, so it measures timer and wake-up behaviour. | Drop it from comparisons, or pace by busy-wait for 10 s+ with 5+ repeats. |
| B-12 | Medium | `runner.dart:239-242`; server stream endpoint | Coordinated omission is not corrected in the two open-loop scenarios: timestamps are taken at the actual send, after the sender has already fallen behind. | Stamp the intended send time (`start + seq / rate`); discard the first ~200 ms. |
| B-13 | Medium | mobile data; `RESULTS.md:193-199`; `docs/ROADMAP.md:325`, `:343` | Mobile CPU and idle figures are less stable than the text says. The embedded server's CPU for identical work varies 2–3× with the client variant (Android wss binary flood: 944 ms vs 306 ms). The "±1%" probe noise floor is exceeded: −2.9% on Android, −1.8% on iOS, 23 of 48 iOS stream runs negative. The `d-tape` ping-pong anomaly described as "14 KB" is also present at 1 KB and 60 KB. In flood wss binary 60 KB the embedded server uses 0.8–1.1 cores, so those cells are server-bound. | Treat mobile CPU ratios as approximate, state the floor per platform and print "<floor" instead of negatives, widen the anomaly note, mark server-bound cells. |
| B-14 | Low | `docs/DECISIONS.md:74-89`, `:352-426`, `:629-652` | Micro-benchmark numbers have no data file and no mise task; each is one 400 ms window. The v1/v2 validator numbers in D5 cannot be reproduced (the code is v3). The FFI call-cost numbers ("scratch benchmark") have no source in the repository. The ABI 2 echo numbers come from the uncommitted `benchmark/build/abi2-echo.jsonl`. | A `bench:micro` task writing to `benchmark/results/`; mark D5 numbers as historical. |
| B-15 | Low | `runner.dart:127`, `:153`; `bin/matrix.dart:60-86`, `:119`; `RESULTS.md:116-126`, `:257-264`, `:346-353`; `benchmark/README.md:73` | Smaller issues. CPU is read before connect, so handshake and runtime start-up are charged per message. Variants run in a fixed order, so drift correlates with the variant. If port 9090 is taken (for example by `mise run server`) the matrix measures whatever listens there; failed runs are dropped silently. Flood "peak RSS" is queue depth at N messages and the mobile RSS tables are a process high-water mark (510.4 MB in nearly every Android cell). The app defaults to 3 repeats, the README builds with 2. The `--repeat=3` doc comment in `matrix.dart:7` does not match the default of 1. | Read CPU after open; randomise variant order per repeat; private port and a check of the server's exit code; drop the mobile RSS tables; align the repeat counts. |

### 6.3 Claims in the docs, checked against the data

Supported:

- Every table in `RESULTS.md`; Dart 3.13.7 / Rust 1.99; 3 and 2 runs per cell.
- D27 selected rows (all 28 numbers).
- "2× on unicode" (ROADMAP:36): 615.7 vs 318.5 µs main, 1.93×, from one case.
- "Android worker costs about 2× the CPU" (ROADMAP:39): 1.6–2.4× in stream; not in wss flood (~1.0×).
- "RSS grew to 500–900 MB without flow control" (ROADMAP:42): 371–889 MB.
- The Android anomaly numbers and the `c-worker-tape` 16 MB/s case (bimodal: 51.5, 15.4, 16.2).
- Micro ratios on a single re-run: `JsonUtf8Encoder` 1.6–1.9×, `ascii.encode` 2.1–2.3×, tape field access 17–44 ns vs 11 ns on a `Map`. Absolute values were 5–20% above the documented ones (loaded machine).

Not supported, or only with qualification:

- `README.md:21` "4–21× faster on phones" (B-4).
- `ROADMAP:35` "5–8× lower latency": 3.3–10.3× (Android) and 4.5–20× (iOS) against `dart:io`, 1.4–5.6× against the best Dart variant, and probe-affected (B-3).
- `ROADMAP:35` "~2× less CPU per message": 2.3–4.3× on Android, 1.4–1.9× on iOS.
- `ROADMAP:37` "`d-tape` … with half the CPU" of the worker that builds models: 0.22–0.28× on Android, 0.61–0.70× on iOS; neither is a half.
- `ROADMAP:38` "`Dart_CObject` … slowest variant everywhere": worst CPU in 10 of 10 JSON stream rows and worst throughput in 9 of 12 JSON flood/ping-pong rows, but best or second in the binary rows and 6th–10th of 11 on the main-isolate metric.
- `ROADMAP:36` / `DECISIONS:52` "20–50% faster than `dart:io`" (B-10, B-1).
- `DECISIONS:537` "nothing measurable … within run-to-run noise" (B-7).
- `DECISIONS:539` "RSS by up to half" (B-8).
- `RESULTS.md:193` "±1%" noise floor (B-13).
- `DECISIONS:79-89` and `:629-650`: validator v1/v2 and FFI call-cost numbers (B-14).
- The "longest stall" and stream p99/max tables for JSON of 14 KB and more (B-2).

`docs/ARCHITECTURE.md`, `CHANGELOG.md` and `native/README.md` contain no
performance numbers.
