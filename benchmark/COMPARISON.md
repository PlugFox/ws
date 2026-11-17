# `ws` 2.0 vs `web_socket_channel`: benchmark results

- Date: 2026-11-16/17. Branch `rewrite`, `ws` / `ws_native` 2.0.0-dev.1,
  `web_socket_channel` 3.0.3.
- Platforms: MacBook Pro M3 Max (macOS 27.0.1, Dart 3.14.0 AOT); iPad Pro
  11" M2 (iPadOS 26.6.1) and Galaxy S22 Ultra, Exynos 2200 (Android 16),
  both Flutter 3.47.9 release; Chrome 157 headless on the same Mac,
  dart2js `-O4` and dart2wasm `-O2`.
- Everything over **wss** (TLS 1.3) on loopback against the Rust
  `ws_bench_server`; on the iPad and the phone the server runs inside the
  app.
- Each cell: median of 3 runs. Methodology, metric definitions and limits:
  [METHODOLOGY.md](METHODOLOGY.md). Raw runs: `results/*.jsonl`
  (`compare-*-macos`, `grid-macos`, `ipad`, `android`, `*-web`).

Columns: **ws (Dart / browser)**: `WebSocketClient.json` / `.binary` with the
built-in transport (pure-Dart RFC 6455 on the VM, the browser `WebSocket` on
the web); **ws + ws_native**: the same client with the Rust transport
(`WsNative.install()`); **web_socket_channel**: `IOWebSocketChannel` on the
VM, `WebSocketChannel.connect` in browsers, JSON via
`jsonEncode`/`jsonDecode`. **best ws vs wsc**: the faster `ws` column
divided by `web_socket_channel` (higher is better for `ws`; for "lower is
better" metrics the inverse).

## Summary

**Native platforms, receive throughput (flood, wss).** `ws + ws_native`
moves 4–13× more data than `web_socket_channel` on macOS (JSON 14 KB: 581
vs 109 MB/s; binary 60 KB: 2 963 vs 227 MB/s), 6–19× on the iPad (JSON
14 KB: 487 vs 59; binary 60 KB: 1 756 vs 93) and 6–32× on Android (JSON
14 KB: 144 vs 14; binary 60 KB: 531 vs 17). With every JSON message mapped
to typed models the gap stays 4.5–10×. Process CPU per message is 2–12×
lower on macOS (all threads counted, Rust ones included).

**`ws` without `ws_native` ≈ `web_socket_channel`.** On the VM the pure-Dart
transport is 0–13 % faster (macOS JSON 1 KB: 119 vs 105 MB/s), never slower
beyond noise. Both sit on `dart:io`'s `SecureSocket`, and on wss TLS
dominates: on Android every Dart client is capped at ~15 MB/s whatever the
message size or format (and the earlier prototype runs measured the same
Dart clients at 2.5–4× that over plain `ws://` on both mobile devices,
`results/mobile-*.jsonl`). `ws_native` does TLS in Rust and does not hit
that ceiling.

**UI isolate.** At a fixed message rate the Dart transports cost the main
isolate 11–185 µs per message on macOS/iPad (5–8 % of its time at
500 msg/s of 14 KB JSON) and 48–1 030 µs on Android (up to 24 %); `ws +
ws_native` is within the probe's noise on macOS and iPad (decoding happens
in Rust; the lazy views cost nothing until read) and 1.5–9 % on Android.
When every message is mapped to models the cost moves to the mapping:
30 vs 124 µs/msg on macOS, 172 vs 483 µs/msg on Android. Delivery latency
p50 at a fixed rate: 152 vs 814 µs (macOS, JSON 14 KB), 180 vs 2 954 µs
(iPad, binary 60 KB), 889 vs 6 192 µs (Android, binary 60 KB).

**Browsers.** Both libraries use the browser's `WebSocket`, so they are
within ±4 % for messages ≥ 14 KB; `ws` is 6–24 % faster on 1 KB messages
(flood) thanks to its synchronous `onMessage` instead of a `Stream`.
dart2wasm decodes JSON slower than dart2js (flood JSON 14 KB: 209 vs
307 MB/s) and is equal on binary; in JSON ping-pong under dart2wasm `ws` is
4–11 % ahead of `web_socket_channel`, under dart2js they are within ±4 %.

**Size × count (ping-pong, spinify's method).**

- *Size*: round trips per second fall and MB/s rises with size until a
  ceiling. On macOS JSON: 22 → 94 MB/s (1 → 60 KB) for `ws_native`, 13 →
  51 MB/s for the Dart transport, 12 → 42 MB/s for `web_socket_channel`;
  binary reaches 514 / 122 / 123 MB/s. Pure-Dart clients flatten from about
  30 KB (TLS and decoding bound); `ws_native` keeps climbing.
- *Count*: on macOS the first 100 round trips run at 50–70 % of the
  steady state for 1 KB messages (JSON, Dart transport: 8.8 vs 12.7 MB/s)
  and at 80–100 % from 5 KB up. On the iPad ×100 reaches 29–85 % of
  ×10 000 for `ws_native` (the smaller the message, the lower). Browsers pay
  most: 28–80 % of the steady state (Chrome dart2js JSON 1 KB: 5.3 vs
  15.1 MB/s) while the JIT warms up, and the ×100 cells vary between runs
  by up to their own median.
- On macOS Chrome's native `JSON.parse` makes browser JSON ping-pong faster
  than the VM's pure-Dart path (14 KB: 4 869 vs 3 123 round trips/s), but
  `ws_native` is ahead of both (5 577).
- Spinify counts sent + received messages, so its msg/s and MB/s are twice
  these numbers for the same run (and it goes through Centrifugo).

**Memory.** In flood, `ws + ws_native` peaks at 31–295 MB RSS against
22–29 MB for the Dart clients (macOS, fresh process per run). The read-ahead
of the Rust transport is bounded by its 8 MB receive window, so the rest is
most likely tapes and payloads held as external typed data until the Dart
GC finalizes them; the 14 KB JSON case varied 116–295 MB between runs at the
same throughput. Not investigated further here.

## Open findings

1. **Android, JSON ping-pong: no gain from `ws_native`. Explained by a
   `simpleperf` profile (2026-11-17).** Sequential JSON round trips run at
   the same rate for all three clients (14 KB ×10 000: 418 / 440 / 392
   round trips/s for `ws_native` / Dart / `wsc`), while binary ping-pong is
   4.6× faster with `ws_native` (2 351 vs 510) and JSON flood is 10× faster.
   Re-run alone (`ws_native` only, JSON then binary 14 KB ×10 000) it
   reproduced: 742 vs 3 229 round trips/s, 1.2 vs 0.25 ms of CPU per round
   trip. Profiled with `simpleperf record -e cpu-clock` (all cores) on the
   release build made `profileable`:

   | per round trip (≈) | JSON | binary |
   |---|---:|---:|
   | Dart main thread | 750 µs | 97 µs |
   | – of it: `JsonUtf8Encoder` encoding the 14 KB request | 450 µs (60 %) | — |
   | – of it: reading the tape views of the reply | 3 µs | — |
   | `ws_native` threads | 550 µs | 145 µs |
   | – of it: simd-json + tape (`post_batch`) | ~100 µs (18 %) | — |
   | embedded server threads | 160 µs | 52 µs |
   | cores the Rust and server threads ran on | 100 % A710 | 67 % X2 |

   - The bottleneck is **encoding each request** on the Dart main isolate:
     `WebSocketClient.json` turns the `Map` into JSON there, whatever the
     transport, and so do `ws` with the Dart transport and
     `web_socket_channel` (`jsonEncode`). `ws_native` only speeds up the
     receive side, and that side is cheap here (~100 µs in Rust, 3 µs on
     main). The flood and stream scenarios do not send, which is why they
     show the full gain. On the M2 the same encoding is several times
     faster, so the iPad does not hit this limit (4 498 vs 1 321 round
     trips/s).
   - Secondary: in the JSON case the scheduler kept every thread on the
     three A710 cores, next to the busy main thread; in the binary case the
     Rust and server threads ran mostly on the X2. The same Rust work
     (syscalls, AES-GCM, posting to Dart) therefore costs more per round
     trip in the JSON case; only ~100 µs of the extra 400 µs is parsing.
   - Not a `ws_native` defect. What would help JSON-heavy request traffic
     on slow Android cores: encoding off the main isolate, or a way to push
     JSON that the app already encoded (e.g. in a worker). `JsonCodec.send`
     has no such path today; that is an API decision, not taken here.
2. **Peak RSS of `ws_native` under flood** (see Memory above).

The tables below are generated by `bin/report.dart` from the raw runs.

## Library vs web_socket_channel (wss)

### Flood — throughput, MB/s (higher is better)

| platform | case | ws (Dart / browser) | ws + ws_native | web_socket_channel | best ws vs wsc |
|---|---|---:|---:|---:|---:|
| macOS | json 1KB | 118.9 | 414.1 | 105.0 | ×3.94 |
| macOS | json 14KB | 122.8 | 581.0 | 108.6 | ×5.35 |
| macOS | json 60KB | 124.0 | 637.6 | 111.4 | ×5.73 |
| macOS | binary 1KB | 214.8 | 1432.1 | 201.1 | ×7.12 |
| macOS | binary 60KB | 231.4 | 2962.7 | 227.4 | ×13 |
| macOS | json 1KB → model | 102.5 | 417.9 | 93.1 | ×4.49 |
| macOS | json 14KB → model | 108.9 | 449.8 | 97.0 | ×4.64 |
| macOS | json 60KB → model | 106.0 | 468.0 | 98.3 | ×4.76 |
| iPad | json 1KB | 58.2 | 311.6 | 53.6 | ×5.81 |
| iPad | json 14KB | 63.1 | 486.9 | 58.6 | ×8.31 |
| iPad | json 60KB | 42.9 | 494.7 | 41.6 | ×12 |
| iPad | binary 1KB | 87.4 | 989.0 | 84.2 | ×12 |
| iPad | binary 60KB | 94.5 | 1755.9 | 93.4 | ×19 |
| iPad | json 1KB → model | 54.5 | 300.5 | 50.4 | ×5.96 |
| iPad | json 14KB → model | 58.4 | 400.6 | 54.6 | ×7.34 |
| iPad | json 60KB → model | 41.1 | 402.9 | 39.5 | ×10 |
| Android | json 1KB | 13.6 | 73.4 | 12.6 | ×5.84 |
| Android | json 14KB | 14.6 | 144.1 | 14.0 | ×10 |
| Android | json 60KB | 14.6 | 140.3 | 14.2 | ×9.89 |
| Android | binary 1KB | 15.0 | 162.3 | 15.2 | ×11 |
| Android | binary 60KB | 16.4 | 531.1 | 16.8 | ×32 |
| Android | json 1KB → model | 13.1 | 76.0 | 12.2 | ×6.25 |
| Android | json 14KB → model | 14.1 | 113.8 | 13.7 | ×8.30 |
| Android | json 60KB → model | 14.3 | 100.1 | 13.7 | ×7.31 |
| Chrome dart2js | json 1KB | 138.7 | — | 114.8 | ×1.21 |
| Chrome dart2js | json 14KB | 306.7 | — | 299.4 | ×1.02 |
| Chrome dart2js | json 60KB | 347.3 | — | 351.4 | ×0.99 |
| Chrome dart2js | binary 1KB | 244.0 | — | 196.9 | ×1.24 |
| Chrome dart2js | binary 60KB | 2230.1 | — | 2308.5 | ×0.97 |
| Chrome dart2js | json 1KB → model | 92.7 | — | 81.7 | ×1.14 |
| Chrome dart2js | json 14KB → model | 169.8 | — | 168.3 | ×1.01 |
| Chrome dart2js | json 60KB → model | 185.0 | — | 184.5 | ×1.00 |
| Chrome dart2wasm | json 1KB | 99.7 | — | 93.9 | ×1.06 |
| Chrome dart2wasm | json 14KB | 209.4 | — | 206.8 | ×1.01 |
| Chrome dart2wasm | json 60KB | 232.7 | — | 233.1 | ×1.00 |
| Chrome dart2wasm | binary 1KB | 228.1 | — | 192.0 | ×1.19 |
| Chrome dart2wasm | binary 60KB | 2214.1 | — | 2302.4 | ×0.96 |
| Chrome dart2wasm | json 1KB → model | 87.2 | — | 82.0 | ×1.06 |
| Chrome dart2wasm | json 14KB → model | 173.1 | — | 171.4 | ×1.01 |
| Chrome dart2wasm | json 60KB → model | 190.5 | — | 191.1 | ×1.00 |

### Flood — messages per second (higher is better)

| platform | case | ws (Dart / browser) | ws + ws_native | web_socket_channel | best ws vs wsc |
|---|---|---:|---:|---:|---:|
| macOS | json 1KB | 114962 | 400532 | 101536 | ×3.94 |
| macOS | json 14KB | 8585 | 40623 | 7594 | ×5.35 |
| macOS | json 60KB | 2021 | 10390 | 1815 | ×5.73 |
| macOS | binary 1KB | 209774 | 1398504 | 196381 | ×7.12 |
| macOS | binary 60KB | 3766 | 48221 | 3701 | ×13 |
| macOS | json 1KB → model | 99100 | 404112 | 90075 | ×4.49 |
| macOS | json 14KB → model | 7616 | 31449 | 6783 | ×4.64 |
| macOS | json 60KB → model | 1727 | 7626 | 1603 | ×4.76 |
| iPad | json 1KB | 56299 | 301345 | 51864 | ×5.81 |
| iPad | json 14KB | 4408 | 34042 | 4097 | ×8.31 |
| iPad | json 60KB | 700 | 8061 | 677 | ×12 |
| iPad | binary 1KB | 85384 | 965782 | 82199 | ×12 |
| iPad | binary 60KB | 1538 | 28580 | 1521 | ×19 |
| iPad | json 1KB → model | 52669 | 290633 | 48787 | ×5.96 |
| iPad | json 14KB → model | 4083 | 28009 | 3815 | ×7.34 |
| iPad | json 60KB → model | 669 | 6565 | 644 | ×10 |
| Android | json 1KB | 13172 | 71016 | 12163 | ×5.84 |
| Android | json 14KB | 1019 | 10077 | 976 | ×10 |
| Android | json 60KB | 237 | 2286 | 231 | ×9.89 |
| Android | binary 1KB | 14612 | 158503 | 14868 | ×11 |
| Android | binary 60KB | 267 | 8643 | 273 | ×32 |
| Android | json 1KB → model | 12653 | 73537 | 11764 | ×6.25 |
| Android | json 14KB → model | 983 | 7957 | 958 | ×8.30 |
| Android | json 60KB → model | 232 | 1632 | 223 | ×7.31 |
| Chrome dart2js | json 1KB | 134445 | — | 111235 | ×1.21 |
| Chrome dart2js | json 14KB | 21455 | — | 20947 | ×1.02 |
| Chrome dart2js | json 60KB | 5661 | — | 5727 | ×0.99 |
| Chrome dart2js | binary 1KB | 238266 | — | 192271 | ×1.24 |
| Chrome dart2js | binary 60KB | 36298 | — | 37573 | ×0.97 |
| Chrome dart2js | json 1KB → model | 89864 | — | 79139 | ×1.14 |
| Chrome dart2js | json 14KB → model | 11882 | — | 11773 | ×1.01 |
| Chrome dart2js | json 60KB → model | 3016 | — | 3008 | ×1.00 |
| Chrome dart2wasm | json 1KB | 96469 | — | 90794 | ×1.06 |
| Chrome dart2wasm | json 14KB | 14643 | — | 14461 | ×1.01 |
| Chrome dart2wasm | json 60KB | 3792 | — | 3798 | ×1.00 |
| Chrome dart2wasm | binary 1KB | 222717 | — | 187512 | ×1.19 |
| Chrome dart2wasm | binary 60KB | 36036 | — | 37474 | ×0.96 |
| Chrome dart2wasm | json 1KB → model | 84374 | — | 79290 | ×1.06 |
| Chrome dart2wasm | json 14KB → model | 12102 | — | 11985 | ×1.01 |
| Chrome dart2wasm | json 60KB → model | 3105 | — | 3114 | ×1.00 |

### Flood — process CPU per message, µs (lower is better)

All threads of the process (Rust transport threads included); on mobile the embedded server's threads are subtracted.

| platform | case | ws (Dart / browser) | ws + ws_native | web_socket_channel | best ws vs wsc |
|---|---|---:|---:|---:|---:|
| macOS | json 1KB | 9.7 | 3.2 | 10.8 | ×3.37 |
| macOS | json 14KB | 125.7 | 36.0 | 140.5 | ×3.90 |
| macOS | json 60KB | 519.0 | 135.3 | 575.1 | ×4.25 |
| macOS | binary 1KB | 5.2 | 1.2 | 5.7 | ×4.65 |
| macOS | binary 60KB | 287.9 | 23.9 | 293.2 | ×12 |
| macOS | json 1KB → model | 11.0 | 5.3 | 12.1 | ×2.30 |
| macOS | json 14KB → model | 140.1 | 70.2 | 156.8 | ×2.23 |
| macOS | json 60KB → model | 606.1 | 279.4 | 651.5 | ×2.33 |
| iPad | json 1KB | 20.3 | 4.7 | 21.7 | ×4.60 |
| iPad | json 14KB | 259.4 | 57.6 | 276.2 | ×4.80 |
| iPad | json 60KB | 1101.3 | 222.2 | 1154.4 | ×5.20 |
| iPad | binary 1KB | 14.2 | 1.8 | 14.6 | ×8.11 |
| iPad | binary 60KB | 794.1 | 40.7 | 800.3 | ×20 |
| iPad | json 1KB → model | 21.5 | 7.2 | 22.9 | ×3.18 |
| iPad | json 14KB → model | 277.9 | 78.4 | 293.0 | ×3.74 |
| iPad | json 60KB → model | 1175.3 | 330.4 | 1224.8 | ×3.71 |
| Android | json 1KB | 70.4 | 20.2 | 76.9 | ×3.81 |
| Android | json 14KB | 907.4 | 198.5 | 949.8 | ×4.79 |
| Android | json 60KB | 3887.5 | 819.1 | 4013.8 | ×4.90 |
| Android | binary 1KB | 59.2 | 9.8 | 59.2 | ×6.04 |
| Android | binary 60KB | 3214.7 | 192.2 | 3173.9 | ×17 |
| Android | json 1KB → model | 74.2 | 26.9 | 80.1 | ×2.98 |
| Android | json 14KB → model | 944.1 | 246.0 | 978.7 | ×3.98 |
| Android | json 60KB → model | 3986.4 | 1218.1 | 4182.6 | ×3.43 |

### Flood — peak RSS, MB (min–max over the runs)

Only where every run is a fresh process (not on mobile, where all cases share one). The ratio compares the median runs.

| platform | case | ws (Dart / browser) | ws + ws_native | web_socket_channel | best ws vs wsc |
|---|---|---:|---:|---:|---:|
| macOS | json 1KB | 23–23 | 31–32 | 23–23 | ×0.99 |
| macOS | json 14KB | 29–29 | 116–295 | 22–28 | ×0.76 |
| macOS | json 60KB | 29–29 | 87–128 | 28–28 | ×0.99 |
| macOS | binary 1KB | 25–25 | 32–33 | 24–25 | ×0.98 |
| macOS | binary 60KB | 23–23 | 60–68 | 22–23 | ×0.98 |
| macOS | json 1KB → model | 23–23 | 29–31 | 23–23 | ×1.00 |
| macOS | json 14KB → model | 29–29 | 83–88 | 22–28 | ×0.76 |
| macOS | json 60KB → model | 22–22 | 76–83 | 22–22 | ×1.00 |

### Stream — main isolate time per message, µs (share of main-isolate time)

The time the UI isolate loses to receiving and decoding: what costs frames in a Flutter app. Readings inside the probe's noise band (macOS ±0.1 %, iPad ±1.4 %, Android ±0.6 % of main-isolate time, the largest negative reading per platform) show as ≈0, and ratios against them are lower bounds (≥).

| platform | case | ws (Dart / browser) | ws + ws_native | web_socket_channel | best ws vs wsc |
|---|---|---:|---:|---:|---:|
| macOS | json 1KB @2k/s | 11.4 (2.3 %) | ≈0 (0.1 %) | 12.6 (2.5 %) | ≥×25 |
| macOS | json 14KB @500/s | 95.2 (4.8 %) | ≈0 (0.0 %) | 110.2 (5.5 %) | ≥×55 |
| macOS | binary 60KB @200/s | 114.1 (2.3 %) | ≈0 (-0.0 %) | 112.8 (2.3 %) | ≥×23 |
| macOS | json 14KB @500/s → model | 106.6 (5.3 %) | 29.9 (1.5 %) | 124.4 (6.2 %) | ×4.16 |
| iPad | json 1KB @2k/s | 14.8 (3.0 %) | ≈0 (-0.4 %) | 16.5 (3.3 %) | ≥×2.35 |
| iPad | json 14KB @500/s | 119.1 (6.0 %) | ≈0 (-0.6 %) | 134.9 (6.7 %) | ≥×4.80 |
| iPad | binary 60KB @200/s | 185.1 (3.7 %) | ≈0 (-1.0 %) | 176.6 (3.5 %) | ≥×2.51 |
| iPad | json 14KB @500/s → model | 144.6 (7.2 %) | ≈0 (1.1 %) | 156.7 (7.8 %) | ≥×5.57 |
| Android | json 1KB @2k/s | 48.4 (9.7 %) | 43.9 (8.8 %) | 57.5 (11.5 %) | ×1.31 |
| Android | json 14KB @500/s | 431.7 (21.6 %) | 126.0 (6.3 %) | 427.8 (21.4 %) | ×3.40 |
| Android | binary 60KB @200/s | 1011.7 (20.0 %) | 76.4 (1.5 %) | 1028.0 (20.6 %) | ×13 |
| Android | json 14KB @500/s → model | 448.6 (22.4 %) | 171.8 (8.6 %) | 483.0 (24.2 %) | ×2.81 |

### Stream — longest main isolate stall, µs

| platform | case | ws (Dart / browser) | ws + ws_native | web_socket_channel | best ws vs wsc |
|---|---|---:|---:|---:|---:|
| macOS | json 1KB @2k/s | 179 | 208 | 174 | ×0.97 |
| macOS | json 14KB @500/s | 272 | 232 | 297 | ×1.28 |
| macOS | binary 60KB @200/s | 105 | 113 | 97 | ×0.92 |
| macOS | json 14KB @500/s → model | 291 | 258 | 312 | ×1.21 |
| iPad | json 1KB @2k/s | 159 | 312 | 477 | ×3.00 |
| iPad | json 14KB @500/s | 347 | 263 | 357 | ×1.36 |
| iPad | binary 60KB @200/s | 200 | 168 | 147 | ×0.88 |
| iPad | json 14KB @500/s → model | 428 | 339 | 359 | ×1.06 |
| Android | json 1KB @2k/s | 1788 | 1711 | 587 | ×0.34 |
| Android | json 14KB @500/s | 1635 | 1025 | 1460 | ×1.42 |
| Android | binary 60KB @200/s | 1034 | 584 | 535 | ×0.92 |
| Android | json 14KB @500/s → model | 1294 | 1575 | 948 | ×0.73 |

### Stream — delivery latency p50 / p99, µs

| platform | case | ws (Dart / browser) | ws + ws_native | web_socket_channel | best ws vs wsc |
|---|---|---:|---:|---:|---:|
| macOS | json 1KB @2k/s | 429 / 508 | 119 / 179 | 429 / 518 | ×3.61 |
| macOS | json 14KB @500/s | 788 / 1525 | 152 / 242 | 814 / 1573 | ×5.36 |
| macOS | binary 60KB @200/s | 2620 / 2758 | 158 / 222 | 2631 / 2749 | ×17 |
| macOS | json 14KB @500/s → model | 824 / 1592 | 177 / 283 | 831 / 1622 | ×4.69 |
| iPad | json 1KB @2k/s | 496 / 571 | 135 / 216 | 504 / 588 | ×3.73 |
| iPad | json 14KB @500/s | 897 / 1073 | 170 / 278 | 930 / 1836 | ×5.47 |
| iPad | binary 60KB @200/s | 2947 / 3105 | 180 / 264 | 2954 / 3117 | ×16 |
| iPad | json 14KB @500/s → model | 932 / 1036 | 214 / 362 | 954 / 1073 | ×4.46 |
| Android | json 1KB @2k/s | 904 / 1842 | 589 / 1098 | 939 / 1386 | ×1.59 |
| Android | json 14KB @500/s | 1875 / 4324 | 828 / 1385 | 2082 / 4606 | ×2.51 |
| Android | binary 60KB @200/s | 7415 / 65909 | 889 / 1794 | 6192 / 13154 | ×6.97 |
| Android | json 14KB @500/s → model | 1988 / 4175 | 834 / 1550 | 2052 / 4181 | ×2.46 |

### Stream — process CPU per message, µs

| platform | case | ws (Dart / browser) | ws + ws_native | web_socket_channel | best ws vs wsc |
|---|---|---:|---:|---:|---:|
| macOS | json 1KB @2k/s | 20.3 | 7.7 | 21.6 | ×2.79 |
| macOS | json 14KB @500/s | 143.5 | 45.0 | 158.4 | ×3.52 |
| macOS | binary 60KB @200/s | 261.4 | 44.0 | 265.4 | ×6.03 |
| macOS | json 14KB @500/s → model | 152.0 | 74.2 | 169.9 | ×2.29 |
| iPad | json 1KB @2k/s | 64.4 | 42.5 | 65.5 | ×1.54 |
| iPad | json 14KB @500/s | 314.0 | 184.6 | 327.8 | ×1.78 |
| iPad | binary 60KB @200/s | 712.3 | 374.9 | 721.7 | ×1.93 |
| iPad | json 14KB @500/s → model | 353.5 | 216.8 | 371.1 | ×1.71 |
| Android | json 1KB @2k/s | 211.5 | 182.1 | 217.8 | ×1.20 |
| Android | json 14KB @500/s | 1107.2 | 823.1 | 1164.9 | ×1.42 |
| Android | binary 60KB @200/s | 2813.1 | 1394.2 | 2766.3 | ×1.98 |
| Android | json 14KB @500/s → model | 1164.9 | 874.1 | 1179.7 | ×1.35 |

### Ping-pong ×10000 → model — round trips per second

| platform | case | ws (Dart / browser) | ws + ws_native | web_socket_channel | best ws vs wsc |
|---|---|---:|---:|---:|---:|
| macOS | json 1KB ×10000 → model | 11529 (12.3 MB/s) | 19833 (21.2 MB/s) | 10597 (11.3 MB/s) | ×1.87 |
| macOS | json 14KB ×10000 → model | 3021 (43.3 MB/s) | 4865 (69.7 MB/s) | 2428 (34.8 MB/s) | ×2.00 |
| macOS | json 60KB ×10000 → model | 798 (49.0 MB/s) | 1301 (80.0 MB/s) | 654 (40.2 MB/s) | ×1.99 |
| iPad | json 1KB ×10000 → model | 5191 (5.6 MB/s) | 13347 (14.3 MB/s) | 4937 (5.3 MB/s) | ×2.70 |
| iPad | json 14KB ×10000 → model | 1463 (21.0 MB/s) | 3950 (56.6 MB/s) | 1297 (18.6 MB/s) | ×3.05 |
| iPad | json 60KB ×10000 → model | 374 (23.0 MB/s) | 895 (55.0 MB/s) | 334 (20.5 MB/s) | ×2.68 |
| Android | json 1KB ×10000 → model | 969 (1.0 MB/s) | 1921 (2.1 MB/s) | 995 (1.1 MB/s) | ×1.93 |
| Android | json 14KB ×10000 → model | 427 (6.1 MB/s) | 374 (5.4 MB/s) | 391 (5.6 MB/s) | ×1.09 |
| Android | json 60KB ×10000 → model | 114 (7.0 MB/s) | 110 (6.7 MB/s) | 104 (6.4 MB/s) | ×1.09 |
| Chrome dart2js | json 1KB ×10000 → model | 13385 (14.3 MB/s) | — | 13390 (14.3 MB/s) | ×1.00 |
| Chrome dart2js | json 14KB ×10000 → model | 3725 (53.4 MB/s) | — | 3767 (54.0 MB/s) | ×0.99 |
| Chrome dart2js | json 60KB ×10000 → model | 1196 (73.5 MB/s) | — | 1208 (74.3 MB/s) | ×0.99 |
| Chrome dart2wasm | json 1KB ×10000 → model | 13398 (14.3 MB/s) | — | 12663 (13.5 MB/s) | ×1.06 |
| Chrome dart2wasm | json 14KB ×10000 → model | 3640 (52.1 MB/s) | — | 3382 (48.5 MB/s) | ×1.08 |
| Chrome dart2wasm | json 60KB ×10000 → model | 1132 (69.6 MB/s) | — | 1032 (63.4 MB/s) | ×1.10 |

### Ping-pong ×10000 → model — RTT p50 / p99, µs

| platform | case | ws (Dart / browser) | ws + ws_native | web_socket_channel | best ws vs wsc |
|---|---|---:|---:|---:|---:|
| macOS | json 1KB ×10000 → model | 85 / 128 | 50 / 70 | 93 / 128 | ×1.86 |
| macOS | json 14KB ×10000 → model | 326 / 405 | 200 / 262 | 406 / 507 | ×2.03 |
| macOS | json 60KB ×10000 → model | 1246 / 1482 | 766 / 871 | 1509 / 1833 | ×1.97 |
| iPad | json 1KB ×10000 → model | 190 / 259 | 73 / 172 | 200 / 303 | ×2.74 |
| iPad | json 14KB ×10000 → model | 662 / 1007 | 246 / 466 | 750 / 1059 | ×3.05 |
| iPad | json 60KB ×10000 → model | 2645 / 3171 | 1125 / 1404 | 2961 / 3747 | ×2.63 |
| Android | json 1KB ×10000 → model | 1000 / 2137 | 460 / 1545 | 1009 / 1623 | ×2.19 |
| Android | json 14KB ×10000 → model | 2245 / 3697 | 2622 / 5038 | 2497 / 3472 | ×1.11 |
| Android | json 60KB ×10000 → model | 8489 / 11242 | 9834 / 13353 | 9322 / 12172 | ×1.10 |
| Chrome dart2js | json 1KB ×10000 → model | 100 / 201 | — | 100 / 201 | ×1.00 |
| Chrome dart2js | json 14KB ×10000 → model | 299 / 401 | — | 299 / 401 | ×1.00 |
| Chrome dart2js | json 60KB ×10000 → model | 801 / 1801 | — | 800 / 1599 | ×1.00 |
| Chrome dart2wasm | json 1KB ×10000 → model | 100 / 200 | — | 100 / 200 | ×1.00 |
| Chrome dart2wasm | json 14KB ×10000 → model | 300 / 500 | — | 300 / 500 | ×1.00 |
| Chrome dart2wasm | json 60KB ×10000 → model | 900 / 2300 | — | 1000 / 1700 | ×1.11 |

## Ping-pong grid: message size × message count (wss)

Round trips per second (one-way MB/s). Spinify counts sent and received messages, so its msg/s and MB/s are twice these for the same run.

### json, ×10000 (spinify layout)


**macOS**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 11894 (12.7) | 20604 (22.0) | 10706 (11.5) |
| 5KB | 6974 (36.4) | 10906 (56.9) | 5781 (30.2) |
| 10KB | 3768 (39.0) | 7037 (72.9) | 3066 (31.8) |
| 14KB | 3123 (44.7) | 5577 (79.9) | 2504 (35.9) |
| 30KB | 1620 (49.9) | 2746 (84.6) | 1294 (39.9) |
| 60KB | 826 (50.8) | 1527 (93.9) | 683 (42.0) |

**iPad**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 5215 (5.6) | 13780 (14.7) | 4992 (5.3) |
| 5KB | 3975 (20.7) | 8587 (44.8) | 3506 (18.3) |
| 10KB | 2251 (23.3) | 5691 (58.9) | 1520 (15.7) |
| 14KB | 1494 (21.4) | 4498 (64.4) | 1321 (18.9) |
| 30KB | 764 (23.5) | 1767 (54.4) | 678 (20.9) |
| 60KB | 382 (23.5) | 1036 (63.7) | 341 (21.0) |

**Android**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 981 (1.0) | 2256 (2.4) | 1005 (1.1) |
| 5KB | 814 (4.2) | 788 (4.1) | 796 (4.2) |
| 10KB | 492 (5.1) | 513 (5.3) | 451 (4.7) |
| 14KB | 440 (6.3) | 418 (6.0) | 392 (5.6) |
| 30KB | 227 (7.0) | 254 (7.8) | 205 (6.3) |
| 60KB | 117 (7.2) | 119 (7.3) | 107 (6.6) |

**Chrome dart2js**

| size | ws | web_socket_channel |
|---|---:|---:|
| 1KB | 14108 (15.1) | 13902 (14.9) |
| 5KB | 8855 (46.2) | 8864 (46.3) |
| 10KB | 6051 (62.7) | 6025 (62.4) |
| 14KB | 4869 (69.8) | 4865 (69.7) |
| 30KB | 2600 (80.1) | 2718 (83.8) |
| 60KB | 1487 (91.4) | 1499 (92.1) |

**Chrome dart2wasm**

| size | ws | web_socket_channel |
|---|---:|---:|
| 1KB | 13428 (14.4) | 12922 (13.8) |
| 5KB | 8030 (41.9) | 7507 (39.2) |
| 10KB | 5329 (55.2) | 4809 (49.8) |
| 14KB | 3862 (55.3) | 3629 (52.0) |
| 30KB | 2207 (68.0) | 2052 (63.2) |
| 60KB | 1203 (73.9) | 1101 (67.7) |

### json — effect of the message count, MB/s

Each cell: ×100 / ×1 000 / ×10 000 round trips.


**macOS**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 8.8 / 11.7 / 12.7 | 12.4 / 21.0 / 22.0 | 8.2 / 11.3 / 11.5 |
| 5KB | 33.4 / 36.2 / 36.4 | 50.4 / 55.7 / 56.9 | 27.8 / 30.2 / 30.2 |
| 10KB | 39.7 / 41.8 / 39.0 | 68.6 / 71.9 / 72.9 | 33.4 / 32.6 / 31.8 |
| 14KB | 41.7 / 44.6 / 44.7 | 64.1 / 76.8 / 79.9 | 34.8 / 37.7 / 35.9 |
| 30KB | 48.1 / 49.5 / 49.9 | 79.9 / 83.3 / 84.6 | 39.0 / 39.9 / 39.9 |
| 60KB | 50.2 / 50.6 / 50.8 | 92.1 / 93.2 / 93.9 | 41.3 / 41.7 / 42.0 |

**iPad**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 2.8 / 5.2 / 5.6 | 5.0 / 11.7 / 14.7 | 2.9 / 5.0 / 5.3 |
| 5KB | 10.3 / 18.9 / 20.7 | 14.5 / 37.1 / 44.8 | 8.6 / 16.2 / 18.3 |
| 10KB | 13.0 / 21.3 / 23.3 | 22.9 / 49.5 / 58.9 | 11.8 / 15.3 / 15.7 |
| 14KB | 16.4 / 21.0 / 21.4 | 25.4 / 54.4 / 64.4 | 15.4 / 18.2 / 18.9 |
| 30KB | 20.1 / 23.2 / 23.5 | 37.9 / 53.1 / 54.4 | 18.0 / 20.6 / 20.9 |
| 60KB | 21.6 / 23.4 / 23.5 | 53.8 / 61.2 / 63.7 | 19.3 / 20.8 / 21.0 |

**Android**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 0.6 / 0.9 / 1.0 | 1.0 / 1.7 / 2.4 | 0.5 / 1.0 / 1.1 |
| 5KB | 2.3 / 3.8 / 4.2 | 3.9 / 3.9 / 4.1 | 2.1 / 3.6 / 4.2 |
| 10KB | 3.1 / 4.7 / 5.1 | 4.7 / 5.2 / 5.3 | 3.1 / 4.4 / 4.7 |
| 14KB | 3.8 / 5.9 / 6.3 | 5.7 / 6.2 / 6.0 | 3.9 / 5.4 / 5.6 |
| 30KB | 4.6 / 6.7 / 7.0 | 6.8 / 7.6 / 7.8 | 5.0 / 6.3 / 6.3 |
| 60KB | 6.0 / 7.1 / 7.2 | 6.6 / 7.4 / 7.3 | 5.7 / 6.5 / 6.6 |

**Chrome dart2js**

| size | ws | web_socket_channel |
|---|---:|---:|
| 1KB | 5.3 / 11.8 / 15.1 | 5.1 / 10.8 / 14.9 |
| 5KB | 18.5 / 39.1 / 46.2 | 17.3 / 37.5 / 46.3 |
| 10KB | 28.8 / 52.5 / 62.7 | 27.9 / 53.0 / 62.4 |
| 14KB | 40.5 / 58.6 / 69.8 | 34.9 / 60.7 / 69.7 |
| 30KB | 54.6 / 76.3 / 80.1 | 51.0 / 79.1 / 83.8 |
| 60KB | 69.3 / 88.3 / 91.4 | 68.7 / 91.4 / 92.1 |

**Chrome dart2wasm**

| size | ws | web_socket_channel |
|---|---:|---:|
| 1KB | 4.9 / 11.0 / 14.4 | 5.6 / 11.1 / 13.8 |
| 5KB | 16.9 / 33.8 / 41.9 | 15.0 / 32.1 / 39.2 |
| 10KB | 38.2 / 48.5 / 55.2 | 24.2 / 43.1 / 49.8 |
| 14KB | 34.6 / 51.1 / 55.3 | 29.1 / 47.3 / 52.0 |
| 30KB | 46.3 / 67.3 / 68.0 | 42.1 / 60.8 / 63.2 |
| 60KB | 59.3 / 72.5 / 73.9 | 54.4 / 66.3 / 67.7 |

### binary, ×10000 (spinify layout)


**macOS**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 12936 (13.2) | 26453 (27.1) | 12599 (12.9) |
| 5KB | 11701 (59.9) | 23682 (121.3) | 11613 (59.5) |
| 10KB | 7259 (74.3) | 19830 (203.1) | 7398 (75.8) |
| 14KB | 7064 (101.3) | 18515 (265.4) | 7057 (101.2) |
| 30KB | 3787 (116.3) | 12635 (388.1) | 3780 (116.1) |
| 60KB | 1988 (122.1) | 8361 (513.7) | 1996 (122.7) |

**iPad**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 5724 (5.9) | 17266 (17.7) | 5683 (5.8) |
| 5KB | 5480 (28.1) | 15941 (81.6) | 5435 (27.8) |
| 10KB | 3244 (33.2) | 13349 (136.7) | 3256 (33.3) |
| 14KB | 3097 (44.4) | 12329 (176.7) | 3093 (44.3) |
| 30KB | 1689 (51.9) | 8870 (272.5) | 1692 (52.0) |
| 60KB | 869 (53.4) | 5959 (366.1) | 873 (53.6) |

**Android**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 1039 (1.1) | 2927 (3.0) | 1030 (1.1) |
| 5KB | 948 (4.9) | 2704 (13.8) | 930 (4.8) |
| 10KB | 534 (5.5) | 2489 (25.5) | 531 (5.4) |
| 14KB | 512 (7.3) | 2351 (33.7) | 510 (7.3) |
| 30KB | 267 (8.2) | 1940 (59.6) | 268 (8.2) |
| 60KB | 138 (8.5) | 1519 (93.3) | 138 (8.4) |

**Chrome dart2js**

| size | ws | web_socket_channel |
|---|---:|---:|
| 1KB | 17030 (17.4) | 16609 (17.0) |
| 5KB | 15263 (78.1) | 14804 (75.8) |
| 10KB | 14017 (143.5) | 13667 (139.9) |
| 14KB | 12728 (182.5) | 12520 (179.5) |
| 30KB | 10007 (307.4) | 9759 (299.8) |
| 60KB | 7320 (449.7) | 7148 (439.2) |

**Chrome dart2wasm**

| size | ws | web_socket_channel |
|---|---:|---:|
| 1KB | 16215 (16.6) | 16026 (16.4) |
| 5KB | 13530 (69.3) | 13499 (69.1) |
| 10KB | 11667 (119.5) | 11530 (118.1) |
| 14KB | 10392 (149.0) | 10201 (146.2) |
| 30KB | 7252 (222.8) | 7231 (222.1) |
| 60KB | 4866 (299.0) | 4857 (298.4) |

### binary — effect of the message count, MB/s

Each cell: ×100 / ×1 000 / ×10 000 round trips.


**macOS**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 8.7 / 13.3 / 13.2 | 13.9 / 26.3 / 27.1 | 8.3 / 12.6 / 12.9 |
| 5KB | 53.3 / 56.5 / 59.9 | 106.4 / 99.8 / 121.3 | 55.6 / 49.2 / 59.5 |
| 10KB | 67.6 / 73.1 / 74.3 | 170.5 / 195.2 / 203.1 | 71.3 / 75.0 / 75.8 |
| 14KB | 93.1 / 99.3 / 101.3 | 229.6 / 253.8 / 265.4 | 96.8 / 101.5 / 101.2 |
| 30KB | 105.0 / 116.5 / 116.3 | 340.1 / 378.9 / 388.1 | 106.9 / 114.7 / 116.1 |
| 60KB | 116.0 / 120.4 / 122.1 | 462.5 / 507.1 / 513.7 | 116.9 / 121.2 / 122.7 |

**iPad**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 3.0 / 5.4 / 5.9 | 5.6 / 13.4 / 17.7 | 3.1 / 5.4 / 5.8 |
| 5KB | 14.5 / 26.0 / 28.1 | 23.9 / 55.0 / 81.6 | 14.5 / 26.1 / 27.8 |
| 10KB | 21.4 / 31.8 / 33.2 | 41.4 / 99.9 / 136.7 | 21.0 / 32.0 / 33.3 |
| 14KB | 28.9 / 43.1 / 44.4 | 57.3 / 132.3 / 176.7 | 27.1 / 42.8 / 44.3 |
| 30KB | 41.4 / 50.7 / 51.9 | 90.0 / 221.7 / 272.5 | 44.0 / 50.7 / 52.0 |
| 60KB | 49.2 / 52.9 / 53.4 | 174.5 / 312.5 / 366.1 | 49.4 / 52.9 / 53.6 |

**Android**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 0.5 / 0.9 / 1.1 | 1.2 / 2.1 / 3.0 | 0.5 / 0.9 / 1.1 |
| 5KB | 2.3 / 4.3 / 4.9 | 5.6 / 9.8 / 13.8 | 2.2 / 4.2 / 4.8 |
| 10KB | 3.4 / 5.1 / 5.5 | 9.5 / 18.4 / 25.5 | 3.1 / 5.1 / 5.4 |
| 14KB | 4.2 / 6.8 / 7.3 | 11.6 / 24.0 / 33.7 | 4.1 / 7.1 / 7.3 |
| 30KB | 5.9 / 8.0 / 8.2 | 23.1 / 45.9 / 59.6 | 5.7 / 8.0 / 8.2 |
| 60KB | 7.1 / 8.3 / 8.5 | 35.7 / 75.7 / 93.3 | 6.8 / 8.1 / 8.4 |

**Chrome dart2js**

| size | ws | web_socket_channel |
|---|---:|---:|
| 1KB | 5.0 / 12.5 / 17.4 | 4.9 / 12.1 / 17.0 |
| 5KB | 24.9 / 53.6 / 78.1 | 26.0 / 54.8 / 75.8 |
| 10KB | 47.2 / 110.5 / 143.5 | 46.3 / 106.3 / 139.9 |
| 14KB | 68.9 / 146.0 / 182.5 | 60.5 / 140.3 / 179.5 |
| 30KB | 124.4 / 246.2 / 307.4 | 120.9 / 243.2 / 299.8 |
| 60KB | 195.0 / 399.2 / 449.7 | 178.1 / 379.0 / 439.2 |

**Chrome dart2wasm**

| size | ws | web_socket_channel |
|---|---:|---:|
| 1KB | 5.9 / 11.6 / 16.6 | 5.0 / 12.0 / 16.4 |
| 5KB | 22.0 / 49.6 / 69.3 | 21.3 / 53.8 / 69.1 |
| 10KB | 37.1 / 94.8 / 119.5 | 43.9 / 92.8 / 118.1 |
| 14KB | 62.1 / 117.1 / 149.0 | 66.1 / 119.5 / 146.2 |
| 30KB | 96.3 / 193.1 / 222.8 | 95.7 / 197.6 / 222.1 |
| 60KB | 151.7 / 263.0 / 299.0 | 147.3 / 265.6 / 298.4 |

### Ping-pong RTT p50 / p99, µs (×10000)


**macOS, json**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 83 / 116 | 47 / 67 | 91 / 138 |
| 5KB | 140 / 193 | 91 / 120 | 170 / 211 |
| 10KB | 262 / 325 | 139 / 181 | 320 / 397 |
| 14KB | 317 / 379 | 176 / 220 | 396 / 482 |
| 30KB | 613 / 711 | 364 / 409 | 762 / 911 |
| 60KB | 1194 / 1419 | 653 / 730 | 1447 / 1735 |

**iPad, json**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 189 / 238 | 70 / 169 | 198 / 262 |
| 5KB | 248 / 416 | 111 / 280 | 279 / 471 |
| 10KB | 437 / 669 | 171 / 349 | 641 / 947 |
| 14KB | 648 / 960 | 217 / 417 | 735 / 1063 |
| 30KB | 1301 / 1673 | 519 / 882 | 1464 / 1841 |
| 60KB | 2585 / 3337 | 992 / 1238 | 2903 / 3486 |

**Android, json**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 993 / 1799 | 392 / 1399 | 994 / 1783 |
| 5KB | 1186 / 2430 | 1324 / 2559 | 1206 / 2185 |
| 10KB | 1947 / 3044 | 1937 / 3269 | 2128 / 3315 |
| 14KB | 2184 / 3472 | 2346 / 3898 | 2448 / 3947 |
| 30KB | 4272 / 6022 | 3969 / 6890 | 4764 / 6161 |
| 60KB | 8269 / 10934 | 8864 / 11742 | 9091 / 11688 |

**Chrome dart2js, json**

| size | ws | web_socket_channel |
|---|---:|---:|
| 1KB | 100 / 201 | 100 / 201 |
| 5KB | 100 / 299 | 100 / 299 |
| 10KB | 199 / 301 | 199 / 301 |
| 14KB | 200 / 400 | 200 / 400 |
| 30KB | 400 / 701 | 399 / 601 |
| 60KB | 699 / 1701 | 699 / 1500 |

**Chrome dart2wasm, json**

| size | ws | web_socket_channel |
|---|---:|---:|
| 1KB | 100 / 200 | 100 / 200 |
| 5KB | 100 / 300 | 100 / 300 |
| 10KB | 200 / 400 | 200 / 400 |
| 14KB | 300 / 400 | 300 / 400 |
| 30KB | 400 / 800 | 500 / 900 |
| 60KB | 800 / 2200 | 900 / 1600 |

**macOS, binary**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 76 / 107 | 37 / 59 | 78 / 105 |
| 5KB | 85 / 113 | 41 / 62 | 84 / 137 |
| 10KB | 134 / 204 | 49 / 79 | 133 / 176 |
| 14KB | 139 / 190 | 53 / 73 | 138 / 212 |
| 30KB | 260 / 357 | 78 / 105 | 260 / 356 |
| 60KB | 497 / 625 | 118 / 163 | 496 / 674 |

**iPad, binary**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 173 / 219 | 56 / 156 | 174 / 222 |
| 5KB | 181 / 246 | 59 / 171 | 182 / 259 |
| 10KB | 305 / 388 | 71 / 162 | 303 / 370 |
| 14KB | 320 / 440 | 77 / 210 | 320 / 469 |
| 30KB | 588 / 744 | 109 / 220 | 587 / 696 |
| 60KB | 1148 / 1272 | 164 / 234 | 1142 / 1296 |

**Android, binary**

| size | ws (Dart) | ws + ws_native | web_socket_channel |
|---|---:|---:|---:|
| 1KB | 930 / 1756 | 296 / 1170 | 943 / 1722 |
| 5KB | 1029 / 2051 | 316 / 1207 | 1038 / 2147 |
| 10KB | 1836 / 4013 | 346 / 1375 | 1852 / 3617 |
| 14KB | 1913 / 4046 | 369 / 1434 | 1926 / 3909 |
| 30KB | 3672 / 6154 | 447 / 1621 | 3682 / 5885 |
| 60KB | 7144 / 10188 | 560 / 2048 | 7178 / 10669 |

**Chrome dart2js, binary**

| size | ws | web_socket_channel |
|---|---:|---:|
| 1KB | 99 / 201 | 99 / 201 |
| 5KB | 99 / 201 | 99 / 201 |
| 10KB | 100 / 201 | 100 / 201 |
| 14KB | 100 / 201 | 100 / 201 |
| 30KB | 100 / 201 | 100 / 201 |
| 60KB | 100 / 301 | 100 / 301 |

**Chrome dart2wasm, binary**

| size | ws | web_socket_channel |
|---|---:|---:|
| 1KB | 100 / 200 | 100 / 200 |
| 5KB | 100 / 200 | 100 / 200 |
| 10KB | 100 / 200 | 100 / 200 |
| 14KB | 100 / 200 | 100 / 200 |
| 30KB | 100 / 300 | 100 / 300 |
| 60KB | 200 / 300 | 200 / 300 |

## Run-to-run spread

(max − min) / median of the key metric over the repeats of a cell: median / p90 / max over the cells.

| platform | flood and ping-pong ×1 000, ×10 000 | ping-pong ×100 | stream (main-isolate time) |
|---|---:|---:|---:|
| macOS | 1 % / 7 % / 22 % | 5 % / 12 % / 21 % | 4 % / 36 % / 239 % |
| iPad | 1 % / 5 % / 20 % | 5 % / 21 % / 31 % | 15 % / 77 % / 136 % |
| Android | 3 % / 14 % / 36 % | 11 % / 26 % / 64 % | 41 % / 82 % / 173 % |
| Chrome dart2js | 3 % / 9 % / 16 % | 18 % / 83 % / 109 % | — |
| Chrome dart2wasm | 3 % / 8 % / 21 % | 23 % / 58 % / 83 % | — |
