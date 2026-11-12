# Review of the rewrite, 2026-11-12: triage

Six independent reviews (Fable) of everything on branch `rewrite`, plus
what was done about each finding. The detailed reports are next to this
file; finding ids below refer to them.

| Report | Scope | Findings |
|---|---|---|
| [client.md](client.md) | public API, client core, fake | 31 (4 critical, 8 high) |
| [protocol.md](protocol.md) | RFC 6455 codec, Dart and web transports | 24 (5 high) |
| [native.md](native.md) | Rust library, FFI | 30 (3 high, none critical) |
| [distribution.md](distribution.md) | packaging, hook, installer, CI, release | 30 (8 high) |
| [tests_docs.md](tests_docs.md) | tests, docs, benchmarks | ranked missing tests, doc fixes |
| [future_risks.md](future_risks.md) | platforms, servers, ecosystem | 15 roadmap errors, Top 20 |

## Fixed (with tests)

- **client** F1–F14, F15, F16, F17, F18, F19, F20, F21, F22, F24, F25,
  F26, F30 (send failure), F31 (`supportsPing`). DECISIONS.md D38;
  regression tests in `test/client/client_review_test.dart`.
- **protocol** F1, F2, F13 (fragments copied into one bounded buffer,
  fail fast on the header), F3, F4, F5 (subprotocol and user-info
  validation, never a synchronous `onClosed`), F6 (one pending pong),
  F7, F8 (close result kept, transport close timer), F9 (no sends before
  open).
- **native** 1, 2 (own client handshake, D39), 3 (per-message and pong
  credits), 4 (strict config, ABI 3), 11 (user info inside the `try`).
  Also: repeated `Accept`/`Protocol` and unoffered extensions rejected.
- **distribution** F1, F2, F3, F4, F5, F6, F7 (generator side), F9,
  F10, F11, F12, F13 (argument validation, I/O errors), F16 (Windows in
  the native CI), F17, F18. DECISIONS.md D40.
- **tests_docs** T-1 (skip override), T-2 (server built in CI).

## Open, in planned order

1. ~~**Release pipeline**~~ done, DECISIONS.md D43 (distribution F6,
   F7, F8, F14, F15, F19, F20, F29; future_risks Top 20 #7).
2. ~~**Liveness by default**~~ done, DECISIONS.md D41 (R3, R4, 6.7,
   10.1–10.3).
3. ~~**Lifecycle API**~~ done, DECISIONS.md D44 (future_risks 2.1,
   2.3, 2.5, 3.2, 3.3, 5.1; Top 20 #2). Field test on devices open.
4. ~~**Error hierarchy before the API freeze**~~ done, DECISIONS.md D42
   (typed DNS/TCP/TLS/proxy reasons, frozen sealed set, names kept).
5. ~~**Android trust model**~~ done, DECISIONS.md D45 (R2, 3.5, 3.7 pins;
   Top 20 #5; native "custom roots rebuilding the TLS config"), verified
   on Android 16 and iPadOS 26.6.1.
6. **Tests** (tests_docs §4): ~~the browser on the shared suites
   (T-3)~~ done, DECISIONS.md D46 (also D-3); ~~transport contract
   checker (T28) and the session-identity test (T-4)~~ done, D47;
   ~~reconnect policy on real sockets~~ done, D48; ~~randomized
   re-entrancy sequences, exact content checks (T-5, T-6)~~ done, D49.
   Left from §4 for item 7: exact backpressure (T-7, `sendBufferLimit`),
   calls after close (T-12), close variants, tape equivalence, stalls,
   native lifecycle.
7. **Remaining medium/low items**: native 5–10, 12–30 (pinned read
   chunks, tape memory, OS verifier blocking `abort()`, custom roots
   rebuilding the TLS config), protocol F10–F12, F14–F18, client F23,
   F27–F29, distribution F21–F30, doc corrections in tests_docs.md.
