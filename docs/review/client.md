# Review: public API and client core

Scope: `lib/ws.dart`, `lib/transport.dart`, `lib/testing.dart`,
`lib/src/client/**`, `lib/src/transport/transport.dart`,
`transport_registry.dart`, `fake/`, `lib/src/util/backoff.dart`,
`test/client/**`.

Method: full read of the code, then experiments against
`FakeWebSocketServer` under `fake_async`, one run against a `dart:io` echo
server, and one run in Chrome (`dart test -p chrome`). `dart test
test/client` passes (64 tests) on the reviewed tree. Line numbers refer to
the working tree at review time (`client.dart` 1045 lines); other agents
were editing concurrently, so they may drift.

"Verified" means an experiment reproduced the failure; the experiment id
(E*n*, W*n*) is given. The experiments are in
`/private/tmp/claude-501/-Users-fox-git-ws/f3c8aee7-6bf3-4ef1-a912-0087569c4cb8/scratchpad/clientreview/test/`
(`a_test.dart`, `b_test.dart`, `web_test.dart`); they print observations
rather than assert, and are worth porting into `test/client` as
regression tests once the fixes land.

## Summary

The session-identity design (`identical(_session, this)`) is sound and
holds up against stale transport events. The defects cluster in four
places:

1. **Synchronous user callbacks in the middle of a transition.** State
   events, `onError` and `shouldReconnect` run before the transition's
   bookkeeping is finished. D22 made re-entry not throw; it did not make it
   correct. Result: leaked live connections, a state stuck on
   `reconnecting`, futures completed with the wrong outcome (F3, F5, F9,
   F10).
2. **`close()` and `disconnect()` futures that never complete** or leave a
   half-closed client (F1, F2, F4).
3. **Reconnect policy holes** that let a client hammer a server (F7, F8,
   F16) and one heartbeat configuration that kills healthy connections
   (F6).
4. **RPC**: duplicate ids hang forever, and the stock JSON handler eats
   ordinary server messages (F11, F12).

## Findings

Severity: **Critical** = data loss, leak or hang on a plausible path;
**High** = wrong behaviour users will hit; **Medium** = wrong in an edge
or a misleading contract; **Low** = polish.

| # | Sev | Where | Failure scenario | Status | Suggested fix |
|---|---|---|---|---|---|
| F1 | Critical | `client.dart:297`, `:303-318`; `web_transport.dart` `close()` | **`reconnect()` breaks the client in the browser.** `_dropSession(1001, 'Reconnecting')` passes 1001 to `WebSocket.close`, which only accepts 1000 or 3000–4999 and throws `InvalidAccessError`. The throw escapes `reconnect()` synchronously after `_session = null` and before `_start()`: the old socket stays open on the server, `state` still says `open`, `push` fails with "Not connected", nothing reconnects. `reconnect()` is the documented call for "app returned to foreground". | Verified in Chrome (W1): `reconnect() threw InvalidAccessError`, `state=open`, server still has 1 open socket, `push` → `WebSocketNotConnectedException` | Use 1000 (or a 4xxx code) for internal drops; make `_dropSession` exception-proof (`try { close } catch { abort }`) and always reach `_start()`. The web transport's `close()` should also catch and fall back to `socket.close()`. Add a browser test for `reconnect()` and for URL switching. |
| F2 | Critical | `client.dart:539-548`, `:484-503` | **`close()` with an invalid code leaves a zombie.** `_closed = true` is set before `disconnect()` validates the arguments. `close(1001)` fails with `ArgumentError`, but the client is now marked closed while the connection stays open: a second `close()` returns at once and does nothing, `connect()` fails with "The client is closed", `push` still sends, streams never close, the socket is never released. | Verified (E1): after `close(1001)` then `close()`: `state=open`, server connection open, `push` delivered | Validate first: call `_checkClose` at the top of `close()` before `_closed = true` (or validate, then set the flag). |
| F3 | Critical | `client.dart:437-455` (`_onSessionClosed`), `:320-325` (`_start`) | **Re-entry from `onError` or a `states` listener orphans a live connection.** In the retry branch `onError` is called (`:437`) and `reconnecting` is emitted (`:449`) before `_reconnectTimer` is assigned (`:452`). If the callback calls `reconnect()` or `connect(otherUrl)`, a session starts; control returns, the timer is armed anyway, fires, and `_start()` overwrites `_session` without dropping the first one. The first connection stays open forever with its events ignored. If the callback calls `disconnect()` instead, the state ends as `reconnecting` permanently although nothing will reconnect. | Verified (E4a/E4b): 3 connections, 2 of them open; (E4c): `disconnect()` inside `onError` → final state `reconnecting(attempt 1 …)` forever | Finish all bookkeeping, then notify: compute the delay, arm the timer, and only then call `onError` and `_setState`. Afterwards re-check `_session == null && _wanted` before trusting the timer. Make `_start()` defensive: `_dropSession` (or assert `_session == null`) at its top. Apply the same "notify last" rule to every transition (see F5, F9, F10). |
| F4 | Critical | `client.dart:546-547` | **`await client.close()` never completes when any `states` or `messages` subscription is paused.** `close()` awaits `StreamController.close()`, whose future waits for every listener to receive `done`; a paused subscription never does. The common shape is `await for (final m in client.messages) { if (...) await client.close(); }`: the loop body pauses the subscription, so `close()` waits for the loop and the loop waits for `close()`. | Verified (E23): paused `states` listener → hang; paused `messages` listener → hang. (E23b): `await for` body calling `close()` still pending after 5 s with `state=closed` | Do not await the controllers: `unawaited(_states.close()); unawaited(_messages.close());`. |
| F5 | High | `client.dart:363-391` (`_onTransportOpen`) | **`disconnect()`/`close()` during the `onOpen` hook is undone.** After the awaited hook the code checks only session identity. A session that the user disconnected meanwhile is still the current session (it is waiting for the close handshake), so the client emits `open` after `disconnecting`, counts a connect, arms the stable timer and starts the heartbeat on a closing connection. | Verified (E3): states `connecting → disconnecting → open → disconnected`; `state.isOpen == true` while `push` fails | After the hook: `if (!identical(_session, session) \|\| session.byUser \|\| session.failure != null) return;`. Put the same guard at the top of `_Session.onOpen` so a late `onOpen` after `abort()` is ignored (the transport contract does not forbid it). |
| F6 | High | `client.dart:893-912` (`_onHeartbeat`) | **Heartbeat with `timeout >= interval` kills healthy connections.** Liveness is tested with `_sinceInbound.elapsed >= mark`, where `mark ≈ interval`. A pong resets the stopwatch, but by the time the check runs it reads `timeout` again, which is ≥ `mark`. Example: `interval: 5 s, timeout: 10 s`, server answers every ping at once → "Heartbeat timed out" every 15 s. | Verified (E7): one ping, pong answered, then `reconnecting(… Heartbeat timed out)` | Track inbound activity with a counter or flag (`_inboundSeq`), not elapsed time: remember the value when the ping is sent, fail only if it is unchanged. Better: when the pong arrives, cancel the timeout timer and reschedule the next ping (also fixes the cadence, which is now `interval + timeout`, 40 s for the defaults; E7b: 7 pings in 301 s). |
| F7 | High | `client.dart:715`, `options.dart` `stableAfter` doc, D14 | **Any received message resets the backoff, so "greet then close" servers are hammered and `maxAttempts` never triggers.** A server that accepts, sends one frame (an error object, a hello) and closes with e.g. 4401 is the typical auth-failure shape. Each cycle resets `_failures`, so the delay never grows beyond the first step and the attempt number is always 1. | Verified (E14): 408 connections in 5 minutes with defaults, attempt always 1, `maxAttempts: 3` never reached | Remove the message-based reset; keep only `stableAfter`. If a fast reset is wanted, tie it to something the server cannot produce by accident (a completed `onOpen` hook plus N seconds). |
| F8 | High | `backoff.dart:13-20` | **`minDelay: Duration.zero` gives a zero delay on every attempt**: the jitter ceiling is `min * 2^attempt = 0`. The client reconnects in a tight loop with no backoff at all. | Verified (E12): delays `[0,0,0,0,0,0,0,0]`; 41 560 attempts in 200 ms of real time against a 503 server (and `fake_async.elapse` never returns) | Use a non-zero base for the exponential part (`max(min, 100 ms) * 2^attempt`) or assert `minDelay > Duration.zero` in `WebSocketReconnect`. |
| F9 | High | `client.dart:502-536`, `:303-318` | **`disconnect()` followed by `connect()` leaves the `disconnect()` future pending forever.** `disconnect()` returns `session.closed.future`. A `connect()` during the closing handshake (`_wanted` is false, so it restarts) drops that session via `_dropSession`, which never completes `closed`; the session's later `onClosed` is ignored as stale. Happens with `client.disconnect(); client.connect(url);` and with a user tapping disconnect/connect within `disconnectTimeout`. | Verified (E2, E2b): future still pending after 10 minutes of fake time, both with a silent server and with an echoing server in the same tick | Complete `session.closed` in `_dropSession` (and in every path that discards a session). Simplest: complete it inside `_Session.dispose()` if not yet completed. |
| F10 | High | `client.dart:427-435`, `:385-386` | **Waiters are completed after the state event, so a listener's `connect()` gets the wrong result.** (a) Terminal failure: a listener that reacts to `disconnected(error: …)` by calling `connect()` adds a new waiter, which the following `_completeWaiters(cause)` fails with the old error, although the new attempt goes on and opens. (b) On `open`: a listener that calls `connect(otherUrl)` gets its future completed successfully while the new session is still `connecting`. | Verified (E5): inner `connect()` fails with the previous "Connection lost" while the state becomes `open`. (E6): `connect('ws://other')` completed while `state=connecting(ws://other)` | Take the waiter list before notifying: `final waiters = _takeWaiters(); _setState(...); complete(waiters)`. Same rule as F3. |
| F11 | High | `client.dart:605-616` | **A duplicate RPC id makes the earlier call hang forever.** `_calls[id] = call` overwrites silently. The first call is no longer in the map, so neither a reply, the timeout scan, a disconnect nor `close()` ever completes it. Reachable with any custom `WebSocketRpc` that takes the id from the payload. | Verified (E8): first call still pending after 1 h and after `close()` | Reject before sending: if `_calls.containsKey(id)`, fail the new call with `ArgumentError`/`WebSocketRpcException` (or fail the old one explicitly). |
| F12 | High | `rpc.dart:42-46`, `client.dart:734-739` | **`WebSocketJsonRpc` swallows ordinary server messages that contain a numeric `id`.** `match` treats every object with a numeric `idKey` as a reply; with no pending call it is counted in `droppedReplies` and never reaches `messages`. `id` is the most common key in domain payloads (`{"type":"user.updated","id":42}`), and JSON-RPC 2.0 server requests also carry `id`. The loss is silent. | Verified (E9): two of three server messages dropped, `droppedReplies=2` | Deliver a matched message with no pending call as a normal message unless the id is one this client issued recently (ids are `1.._seq`, so `id > _seq` can never be a late reply). Also let `match` require the absence of a `method` key for JSON-RPC 2.0, and document the hazard on `idKey`. |
| F13 | Medium | `client.dart:766-775` | **Invalid UTF-8: the client sends close 1007 and then waits forever.** No close timer is armed and the state stays `open`. If the server does not answer the close, the client sits in `open` with every `push` failing; with no heartbeat nothing ends it. Messages also keep being delivered after the client sent its close frame. | Verified (E11): after 1 h `state=open`, `push` → "The connection is closing", no pending timer | Route through one "close by us with failure" helper: set `failure`, `closingByUs`, stop delivery, call `close(1007)`, arm `closeTimer` with `disconnectTimeout` → `abort()`. |
| F14 | Medium | `client.dart:510-535`, `:677-693`, `:824-827` | **Pending RPC calls wait out the closing handshake after `disconnect()`.** Replies are dropped once `byUser` is set (D37), but `_failCalls` runs only when the transport reports the close. A call whose reply arrives during the handshake fails up to `disconnectTimeout` later instead of completing or failing at once. | Verified (E10): reply sent during the handshake; call pending at 2.9 s, fails `disconnected` at 3.1 s | Call `_failCalls(disconnected)` (and fail drain/ping waiters) in `disconnect()` at the moment delivery stops. |
| F15 | Medium | `client.dart:543`, `:600-604`, `:618-631`, `exceptions.dart:129-142` | **RPC failure reasons do not match the documentation.** (a) `WebSocketRpcFailure.closed` is unreachable: `close()` awaits `disconnect()`, which already failed every call with `disconnected`. The test `pending calls fail with closed on close()` accepts either value, which hides this. (b) `rpc()` while not connected or after `close()` fails with `WebSocketNotConnectedException`, not the documented `WebSocketRpcException`. (c) A send refused by `sendBufferLimit` or an encode error is reported as `failure: disconnected`. | Verified (E10): `close()` → `disconnected`; `rpc` after close → `WebSocketNotConnectedException`; full buffer → `failure=disconnected, cause=WebSocketBufferFullException` | In `close()` call `_failCalls(closed)` before `disconnect()`. Pick one contract for "not connected" (recommended: always `WebSocketRpcException(failure: disconnected/closed)`). Rethrow non-connection send errors unchanged or add `WebSocketRpcFailure.notSent`. Tighten the test. |
| F16 | Medium | `client.dart:440-448`, `:470-477` | **No `Retry-After`, and 1013 has no jitter.** 429 and 503 responses are retried with the normal backoff even when the server sent `Retry-After` (the header is available in `WebSocketHandshakeException.headers`; TEST_PLAN T6 only says "exposed"). Close 1013 waits exactly `maxDelay`, so every client an overloaded server sheds comes back in the same instant. D14 also promises "TLS errors at max backoff", which the code does not do. | By inspection; 1013 behaviour is asserted by the existing test `close 1013 waits maxDelay` | Honour `Retry-After` (delta-seconds and HTTP-date, clamped to a sane upper bound, plus jitter). For 1013 use `maxDelay` plus jitter (e.g. `maxDelay * (1 + random(0..0.5))`). Either implement the TLS rule or remove it from D14. |
| F17 | Medium | `client.dart:326-360` | **A transport that throws from `connect()` produces an unhandled zone error and a false "Connect timed out".** The microtask is not guarded; `connectTimer` is assigned before the throwing call, so 10 s later the session fails as a timeout and the client retries forever, raising an unhandled error each time. Any synchronous validation in a transport (bad host, unsupported option) triggers it. | Verified (E17): zone received `ArgumentError` twice; states `connecting → reconnecting(Connect timed out) → connecting` | Wrap `_transport.connect` in `try/catch` and end the session with `WebSocketConnectionException(cause: e)`; treat `ArgumentError`/`UnsupportedError` as terminal. |
| F18 | Medium | `client.dart:326-343`, `:363-380` | **The `request` provider and the `onOpen` hook have no time limit.** `connectTimer` starts after the provider returns and is cancelled before the hook runs; the heartbeat starts after the hook. A token refresh or an auth exchange that never completes keeps the client in `connecting` forever, on a connection nobody is watching. | Verified (E22): both cases still `connecting` after 1 h | Start `connectTimer` before the provider and cancel it only when the state becomes `open` (one budget for the whole attempt), or add a separate `onOpenTimeout`. |
| F19 | Medium | `client.dart:570-586`; `dart_transport.dart:329-343` | **`metrics.bytesSent` is always 0 on the pure-Dart transport** (the default on the VM). The client derives it from the growth of `bufferedAmount`, but the transport writes straight to the socket when its queue is empty, so the amount does not grow. The fake never grows it either, so no client test can see it. | Verified (E16): 3 × 1000-byte messages over `dart:io` echo → `sent=3 bytesSent=0`, `bytesReceived=3000` | Count the encoded size in the codec (`send` returns the byte length) instead of sampling the queue. Add a conformance assertion per transport. |
| F20 | Medium | `fake_transport.dart:135-137`, `:199-203` | **The fake loses messages sent right before `close()`/`drop()`.** Deliveries are microtasks that check `_ended` when they run; `close()` sets `_ended` synchronously. `conn..sendText('bye')..close()` delivers nothing, whereas a real connection delivers frames that precede the close frame. Tests written against the fake can pass for the wrong reason or teach the wrong behaviour. | Verified (E13): `got=[]`, state `disconnected(1000)` | Keep one ordered event queue per connection; `close`/`fail` enqueue the terminal event after what is already queued. |
| F21 | Medium | `client.dart:218-227`, `options.dart` `onError` doc | **Without `onError`, exceptions thrown by user callbacks vanish.** A bug in `onMessage` or `onStateChange` leaves no trace: nothing in the zone, nothing in `metrics.lastError` (only `WebSocketException`s are stored). Swallowing is reasonable for transient network errors, which are visible in `state`; it is not for programming errors, which Dart reports to the zone everywhere else. Decode failures are also silent apart from a counter, i.e. dropped data. | By inspection; the test `without onError, a throwing listener is contained (C25)` pins the behaviour | Split the two kinds. Errors raised by user code (callbacks, `isPong`, RPC handler, `shouldReconnect`): if `onError` is null, `Zone.current.handleUncaughtError`. Connection failures that are retried: `onError` only. Decode failures: `onError`, and consider an `errors` stream so they are observable without a constructor argument. |
| F22 | Medium | `client.dart:540` | **A second `close()` returns before the first has finished.** `if (_closed) return;` — a caller that awaits it continues while the state is still `disconnecting` and the socket is open. | Verified (E15): second future completed at `state=disconnecting(1000)` | Store the future of the first call and return it. |
| F23 | Low | `client.dart:635-650` | **RPC timer scheduling is O(pending) per call and per reply**, so a burst is O(n²). | Verified (E20): 2 000 calls 28 ms to issue; 8 000 → 182 ms; 32 000 → 2 530 ms (answering costs the same again) | Calls with the default timeout are already ordered by deadline: keep a queue, or only recompute when the removed call was the earliest; or track `min` incrementally. |
| F24 | Low | `client.dart:332-341`, `:371-377` | **One failure is reported to `onError` twice** when `request` or `onOpen` throws: once raw, once wrapped as `WebSocketConnectionException` by the retry path. | Verified (E21): `[Bad state: auth failed, WebSocketConnectionException: onOpen callback failed (cause: …)]` | Report only the wrapped exception (it carries the cause). |
| F25 | Low | `client.dart:314` | **A 3 s abort timer outlives the client** after a URL switch or `reconnect()` followed by `close()`; it keeps a CLI isolate alive and breaks "no pending timers" checks. | Verified (E25): `state=closed`, one pending 3 s timer | Keep the timer on the dropped session and cancel it when that connection reports `onClosed` (needs the stale session to still receive its own `onClosed`), or track drop timers in a set cancelled by `close()`. |
| F26 | Low | `client.dart:760-763` | A message is still added to `messages` after `onMessage` called `disconnect()` for it. | Verified (E24): listener saw the message at `state=disconnecting` | Re-check `session._delivering` between `onMessage` and the stream. |
| F27 | Low | `client.dart:265-283`, `:502-503` | `connect()` and `disconnect()` throw `ArgumentError`/`FormatException` synchronously for bad arguments but return a failed future for a closed client. A caller using `.catchError` or `unawaited` misses the synchronous ones. | Verified (E30) | Decide one rule. Dart practice for `Future`-returning methods is to report everything through the future; argument errors may stay synchronous if documented. |
| F28 | Low | `client.dart:614`, `:648` | RPC deadlines use wall-clock `clock.now()`. A clock step (NTP, manual change) expires calls early or late. | Suspected (not reproduced) | Use a monotonic `clock.stopwatch()` started with the client. |
| F29 | Low | `client.dart:385` | `WebSocketState.open.url` is the URL given to `connect()`, not the one the `request` provider returned and the transport used. | By inspection | Keep the effective URL on the session. |
| F30 | Low | `client.dart:917-929`, `:947-956` | The heartbeat ping and manual `ping()` share one stopwatch, so `lastRoundTrip` can measure from the wrong ping; a manual `ping()` whose send throws waits for the full timeout instead of failing. | By inspection | Fail the waiter when the send throws; keep the send time per outstanding ping. |
| F31 | Low | `fake_transport.dart:54`, `:103` | The fake cannot model a browser (`supportsPing` is fixed to `true`), does not grow `bufferedAmount` on send, does not enforce `maxMessageSize`, and has no latency control. The no-ping heartbeat path and the `WebSocketUnsupportedException` path of `ping()` are therefore untested in `test/client`. | By inspection | `FakeWebSocketServer({bool supportsPing = true})`; optional automatic queue accounting. |

Checked and found correct (no finding): stale-session events after URL
switch and `reconnect()`; `connect`/`disconnect`/`close` in the same
microtask before the transport exists; `disconnect()` during the reconnect
wait; drain and ping waiters on session loss (always completed); a late
RPC reply after a timeout; a throwing `match`/`decode`/`isPong`; the
`.ignore()` on the `connect()` future (D21); the D37 claim about browsers
dropping messages when not OPEN (matches the WHATWG text).

## API design notes

**Error model.** `onError` as the only sink is thin for a library whose
goal is reliability (F21). Three audiences are mixed in one callback:
transient connection failures (already in `state.reconnecting.cause`),
dropped data (decode errors), and bugs in user callbacks. Suggested shape:
user-code errors go to the zone when `onError` is absent; add a
`Stream<WebSocketException> errors` or put decode errors there. Note also
that a terminal failure is *not* sent to `onError` (only to `state` and
`metrics.lastError`); the doc comment says so, but "the last failure, the
one that matters, skips the error callback" will surprise people.

**Unhandled futures.** `connect()` is safe to fire and forget (D21);
`push()` is not: `client.push(x);` while disconnected raises an unhandled
asynchronous error (E18), which terminates a CLI isolate. Fire-and-forget
sends are the norm in UI code. Options: document it loudly; add a
synchronous `bool trySend(T)`/`void add(T)` that reports through
`onError`; or keep `push` strict and rely on the `unawaited_futures` lint.
The current asymmetry between `connect` and `push` should at least be
stated in the `push` doc.

**Names that collide.** `WebSocketException` also exists in `dart:io`
(a `package:` declaration silently shadows a `dart:` one, so
`on WebSocketException` in a file importing both changes meaning without a
diagnostic) and in `package:web_socket` (a real ambiguous-import error).
`WebSocket*` prefixes on every type make the collisions systematic.
Consider `WsException`/`WsState`… or accept it and document `import …
as ws`.

**Sealed hierarchies are a compatibility commitment.** `WebSocketException`
and `WebSocketState` are sealed and `WebSocketRpcFailure` is an enum:
adding a subtype or a value breaks every exhaustive `switch` downstream,
so each addition is a major version. `WebSocketBufferFullException` was
just added, which shows the set is not stable yet. Either freeze the set
before 2.0.0 or make the exception base non-sealed (`abstract base`/
`interface`) and keep only the state sealed.

**State type.** `WebSocketState$Disconnected` etc.: the `$` names are not
Dart style and show up in user code for every pattern match
(`case WebSocketState$Open(:final url)`). Plain `WebSocketDisconnected`,
`WebSocketOpen` read better. The classes have no `==`/`hashCode`, so
`states.distinct()` and `expect(state, WebSocketState.open(...))` do not
work. A `url` on `reconnecting` and a `willReconnect`/`isConnecting`
getter would save users a switch.

**`states` and `messages` are synchronous broadcast streams.** Fast, but
listeners run inside the client's transitions (source of F3/F5/F10), there
is no replay (a late `states` listener gets nothing until the next change;
messages that arrive with no listener are dropped silently, including
between `await connect()` and `listen`), and no backpressure. Document
"listen before connect". If the "notify last" discipline proves hard to
keep, deliver state events asynchronously and leave `onStateChange` as the
synchronous hook.

**`push` semantics.** The name differs from `dart:io` (`add`),
`package:web_socket` (`sendText`/`sendBytes`) and browsers (`send`); it is
the maintainer's choice, fine, but worth one line in the README mapping.
"Completes when the transport accepted it" plus "fails when the connection
is lost while waiting for drain" means a failed `push` may still have been
sent; say so. Backpressure waits for the queue to reach zero, not to fall
below the high-water mark, which is conservative but halves throughput for
a steady producer; a low-water mark would be the usual design. There is no
assert that `sendHighWaterMark <= sendBufferLimit`.

**`rpc`.** `assert` plus `StateError` for a missing handler behaves
differently in debug and release; pick one (a failed future with
`StateError`). `WebSocketRpc.decode` takes and returns `T`, so the reply
type cannot differ from the message type; acceptable for v1. Users cannot
compose the default reconnect decision: `shouldReconnect` replaces the
whole policy (terminal codes and 4xx rules included). Expose the default
as a static function so a custom predicate can delegate to it. A throwing
`shouldReconnect` currently means "stop forever"; "keep the default" is
the safer fallback for a reliability library.

**Reconnect defaults.** A clean close 1000 from the server reconnects by
default; many protocols use it for "we are done". Application codes
4000–4999 always reconnect; servers commonly use them for auth failures
(with F7 this is the hammering case). Consider making 1000 terminal by
default or documenting both. `maxAttempts` counts reconnects, so the total
number of tries is `maxAttempts + 1`; the doc says "failed attempts in a
row". `WebSocketReconnect.disabled()` could simply be `maxAttempts: 0`.

**Heartbeat in the browser.** Configuring `WebSocketHeartbeat` without
`ping` on the web silently does nothing. An `assert` or a one-time
`onError` report would prevent false confidence.

**Options.** `WebSocketOptions.transport` has type `WebSocketTransport`,
which `lib/ws.dart` does not export (nor does `lib/testing.dart`): a user
who writes a transport wrapper for tests must discover
`package:ws/transport.dart` (my first experiment failed to compile for
this reason). Either export the type from `ws.dart` or have `testing.dart`
re-export `transport.dart`. `WebSocketClientInterface` is an awkward name
for "the sender half"; `WebSocketSender`/`WebSocketSink` describes it.
`WebSocketJsOptions` is empty; shipping an empty public class commits to
it. No validation of durations or sizes anywhere (negative values,
`minDelay > maxDelay`, zero `interval`).

**No public codec.** Only three `T`s are possible. Typed clients
(`WebSocketClient<MyEvent>`) are an obvious request; keeping
`MessageCodec` private for 2.0 is fine, but the generic parameter on
options and callbacks already promises more than is offered.

**Documentation that does not match the code.**

- D14: "TLS errors (at max backoff)" — not implemented.
- D14: 1013 "waits at least the max delay" — it waits exactly `maxDelay`.
- D16: "`.vm(...)`/`.js(...)` are sugar" and "SPKI pins" — neither exists.
- D13 and the `rpc` doc: failure `closed` on `close()` — never produced
  (F15).
- `WebSocketMetrics.messagesReceived`: "delivered to the application" —
  it also counts application pongs and messages dropped for lack of a
  listener. `bytesReceived` is UTF-16 length for browser text and 0 for
  natively decoded JSON; `bytesSent` is 0 on the Dart transport (F19).
- `WebSocketClient.push`: "fails … when not open" — it works in state
  `connecting` once the transport is up (needed for `onOpen`, but anyone
  can call it then). `WebSocketState.isOpen` "whether messages can be sent
  now" is therefore not the same condition.
- ARCHITECTURE "The backoff resets only when a connection proved stable
  … so accept-then-close servers are not hammered" — true only for
  servers that send nothing (F7).

## Future risks to plan for

1. **Re-entrancy is handled case by case.** F3, F5, F10 and F26 are one
   bug class: user code runs synchronously while the client is between two
   consistent states. A structural rule is needed, not four patches:
   every transition mutates all fields first, collects "effects" (state
   event, waiter completions, `onError`), and runs them last; or all user
   notifications are deferred to a microtask. The randomized
   operation-sequence test planned in ROADMAP §6 should include callbacks
   that call `connect`/`reconnect`/`disconnect`/`close` from inside every
   notification, with invariants checked after each step: at most one live
   connection in the fake, `state` consistent with `_session`, no pending
   future after `close()`, no pending timer after `close()`.
2. **The client trusts the transport contract completely.** A missing
   `onClosed` after `abort()` hangs `disconnect()` and `close()`; a
   synchronous throw from any `TransportConnection` method escapes into
   user code or the zone (F1, F17). Third-party transports (`ws_native`
   lives in another package) will get this wrong at some point. Add a
   last-resort timer after `abort()` and guard every transport call. A
   reusable transport conformance suite exported from `testing.dart`
   would let transport authors check themselves.
3. **Close codes differ by platform.** The browser rule (1000, 3000–4999)
   is enforced for user calls but not for the client's own drops (F1).
   Keep one table of codes the client itself sends and test it in Chrome.
4. **Server protection is only as good as the weakest reset.** F7, F8 and
   F16 each let a fleet of clients synchronise or spin. For a library
   whose main promise is reconnecting, add a hard floor (no two attempts
   closer than ~100 ms regardless of configuration), jitter on every
   delay, and a test that counts connections per minute for the common
   bad-server shapes (accept-then-close, greet-then-close, 503, 1013,
   stalled handshake).
5. **Suspended apps and timers.** The heartbeat handles a late timer; the
   RPC deadline queue (wall clock, F28), `stableAfter` and
   `disconnectTimeout` have not been examined under suspension. On mobile
   a resumed app will see all timers fire at once on a dead socket;
   the order in which they fire decides whether the user sees one
   reconnect or a burst of RPC timeouts first. Worth a scripted
   `fake_async` scenario with a large jump.
6. **Metrics are derived, not measured** (F19): anything inferred from
   `bufferedAmount` depends on each transport's queueing. Byte counters
   should come from the codec/transport as explicit numbers, or be removed
   before they become API people chart.
7. **Public surface before 2.0.0.** Sealed exceptions, `$` state names,
   the `WebSocket*` prefix, `WebSocketClientInterface`, the empty
   `WebSocketJsOptions` and the `WebSocketRpc` shape are all cheap to
   change now and breaking afterwards. Decide them deliberately and record
   the decision in DECISIONS.md.
8. **The fake is part of the public API.** Users will write their tests
   against `FakeWebSocketServer`; where it differs from a real network
   (F20, F31) their tests encode wrong assumptions, and fixing the fake
   later breaks them. Fix ordering semantics before release.
