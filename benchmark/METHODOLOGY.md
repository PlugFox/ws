# Benchmark methodology: `ws` vs `web_socket_channel`

How the numbers in [COMPARISON.md](COMPARISON.md) were produced: what was
measured, on what, with which code paths, how each metric is computed, and
what the numbers do not tell you. Everything here can be re-run with the
commands at the end.

## 1. Questions

1. How fast is `package:ws` 2.0 in its two typed modes,
   `WebSocketClient.binary` and `WebSocketClient.json`, on macOS, iPadOS,
   Android and in the browser (dart2js and dart2wasm), over `wss`?
2. How does it compare with `package:web_socket_channel`, the most used
   WebSocket package on pub.dev (publisher dart.dev), on the same benchmark?
3. How do message size and message count affect throughput: the layout of
   spinify's benchmark (sequential request/response), extended with the
   message count as a second axis?

## 2. What is compared

| id | package | transport underneath | JSON path | delivered to the callback |
|---|---|---|---|---|
| `ws-dart` | `ws` 2.0.0-dev.1 | own RFC 6455 codec over `dart:io` `SecureSocket` (BoringSSL) | fused UTF-8 → JSON decoder on the bytes, main isolate | `Map<String, Object?>` / `Uint8List` |
| `ws-native` | `ws` + `ws_native` 2.0.0-dev.1 | Rust: tokio + tungstenite + rustls (ring) via FFI, own thread | simd-json in Rust into a tape; Dart gets lazy `Map` views | `Map<String, Object?>` (lazy) / `Uint8List` (external typed data) |
| `ws` (browser) | `ws` 2.0.0-dev.1 | browser `WebSocket`, `binaryType = 'arraybuffer'` | `jsonDecode` of the JS string | `Map<String, Object?>` / `Uint8List` |
| `wsc` | `web_socket_channel` 3.0.3 | VM: `IOWebSocketChannel` (`dart:io` `WebSocket`); browser: `WebSocketChannel.connect` (`package:web_socket`) | `jsonDecode(String)` in the stream listener, `jsonEncode` before `sink.add` | `Map<String, Object?>` / `Uint8List` |

Each client is used the way its documentation shows, through the fastest
public path:

- `ws`: `WebSocketClient.json(...)` / `.binary(...)` with
  `WebSocketOptions.onMessage` (synchronous callback, no `Stream`),
  `client.push(map)` / `client.push(bytes)` to send. Reconnect disabled,
  `maxMessageSize` raised to 256 MB.
- `wsc`: `channel.stream.listen(...)`, `channel.sink.add(jsonEncode(map))` /
  `sink.add(bytes)`. JSON is decoded in the listener because
  `web_socket_channel` has no JSON mode; an app has to do the same.
- Compression (permessage-deflate) is off for all clients; the server does
  not negotiate extensions.
- TLS certificate verification: the server's CA is generated at startup, so
  the VM clients skip verification (`WebSocketTls(allowInsecure: true)` for
  `ws`, `HttpClient.badCertificateCallback` for `wsc`), and Chrome runs with
  `--ignore-certificate-errors`. Verification happens once per connection,
  before any timed section.

Code: `lib/src/library.dart` (`ws`), `lib/src/wsc.dart` (`wsc`),
`lib/src/clients.dart` (`connectVm`), `web/main.dart` (browser connector).

`ws-dart` passes `transport: WebSocketTransports.platform` explicitly.
`WsNative.install()` makes the native transport the default for every
client created afterwards in the process. On macOS every run is a fresh
process, but the mobile app runs all variants in one process, so without
the explicit transport every "`ws-dart`" run after the first `ws-native`
run silently used Rust. The first mobile pass had exactly that bug; its
results were discarded and both devices were re-run.

## 3. Platforms

| platform | hardware | OS | runtime |
|---|---|---|---|
| macOS | MacBook Pro, Apple M3 Max (14 cores), 36 GB | macOS 27.0.1 | Dart 3.14.0, AOT (`dart build cli`) |
| iPad | iPad Pro 11" (4th gen, iPad14,3), Apple M2 | iPadOS 26.6.1 | Flutter 3.47.9, release (AOT) |
| Android | Samsung Galaxy S22 Ultra (SM-S908B), Exynos 2200 | Android 16 | Flutter 3.47.9, release (AOT), arm64 |
| Chrome dart2js | the same Mac | macOS 27.0.1 | Chrome 157.0.8103.52 headless (`--headless=new`), `dart compile js -O4` |
| Chrome dart2wasm | the same Mac | macOS 27.0.1 | Chrome 157, `dart compile wasm -O2` |

Rust 1.100.0 (release, thin LTO) for the server and both native libraries.
The devices were connected to the Mac by USB and charged from it, screen on
(the app keeps it awake), no other apps in the foreground; Android reported
thermal status 0 (none) at the start. The Mac ran on AC power with no other
heavy processes. Device runs (which use the Mac only for USB logging)
overlapped in time with the macOS and browser runs.

## 4. Topology

```
macOS / Chrome:   client process ──loopback, TLS──▶ ws_bench_server (separate process, tokio multi-thread)
iPad / Android:   app process: Dart client ──loopback, TLS──▶ embedded ws_bench_server (2 tokio threads)
```

- Everything runs over `wss` on `127.0.0.1`. Loopback takes the network
  out of the measurement: the numbers are the cost of the client stack
  (TLS, framing, masking, copies, UTF-8, JSON, event-loop delivery), which
  is what differs between the libraries. On a real network the wire adds
  latency to every round trip and caps throughput; the client costs
  measured here remain and add to it.
- The server is the Rust `ws_bench_server` (`../server`): tokio,
  tungstenite, rustls with the ring provider, TLS 1.3 by default. One
  server binary serves every platform, so the server side is identical for
  all clients.
- Phones and tablets cannot run a separate server process, so the app
  starts the same server inside its own process
  (`ws_bench_server::start_embedded`, 2 worker threads). That also avoids
  Wi-Fi noise and the iOS local network prompt. The server's threads report
  their own CPU time (`CLOCK_THREAD_CPUTIME_ID`), which is subtracted from
  the process CPU time.
- On macOS the server competes with the client for the same CPU, but with
  14 cores neither is starved.

## 5. Payloads

**JSON** (server `/stream` and client ping-pong use the same shape):

```json
{"seq":17,"ts":1763312345678901,"items":[
  {"id":100000,"name":"item-0","price":0.99,"qty":0,"active":true,
   "tags":["alpha","beta","gamma"],"meta":{"score":1.5,"rank":0,"note":null}},
  ...]}
```

Items are appended until the message reaches the target size (± one item,
about 100 bytes). `seq` is the sequence number, `ts` the send time in µs
since the epoch. A 14 KB message has about 130 items: thousands of map
entries, strings and numbers, close to a real API response, unlike a
payload of random bytes.

**Binary**: exactly `size` bytes; bytes 0–7 are `seq` and bytes 8–15 `ts`
(little-endian u64, written as two u32 halves because
`ByteData.setUint64` throws under dart2js), the rest is zero filler.

**Sizes**: 1, 5, 10, 14, 30 and 60 KB (KB = 1024 bytes): the union of
spinify's VM table (1/14/30/60 KB) and browser table (1/5/10/14 KB).
Centrifugo rejects messages over 64 KB, which is why spinify stops at 60.

## 6. Scenarios and metrics

All times come from `Stopwatch` (monotonic; µs on the VM,
`performance.now()` in browsers). Time stamps carried inside messages use
`anchor + stopwatch`, where the anchor is `DateTime.now()` taken at the
start of every case, so they are monotonic within a case and have the
stopwatch's resolution (`DateTime.now()` has only 1 ms in browsers).
Server time stamps (stream latency) come from the system clock of the
same device. The anchor is re-taken per case because in a long-lived
process (the mobile app runs for up to 80 minutes) the system clock and
the monotonic clock drift apart by milliseconds: anchored once per
process, the first mobile pass showed negative stream latencies on the
iPad. Those stream cases were re-run on both devices with the per-case
anchor; ping-pong RTTs never mix clocks and were not affected.

### 6.1 Flood (server → client throughput)

The server sends `count` messages as fast as the socket accepts them
(flushing every 64 messages), then a close frame `1000 done`.

- Timed from the first message delivered to the callback until the close
  frame arrives.
- `msgs/s = received / wall`, `MB/s = received × messageBytes / wall_µs`
  (MB = 10⁶ bytes; `messageBytes` is the payload size, without WebSocket
  and TLS framing).
- `CPU µs/msg`: process CPU time (user + system, all threads, from
  `getrusage(RUSAGE_SELF)`) over the run, divided by the messages. It
  includes the Rust transport threads of `ws-native` and the
  `dart:io` IO threads of the others.
- Peak RSS (`ru_maxrss`), only where each run is a fresh process (macOS).
- Counts: 50 000 × 1 KB JSON, 10 000 × 14 KB JSON, 3 000 × 60 KB JSON,
  100 000 × 1 KB binary, 20 000 × 60 KB binary. The same counts run on
  every platform so the numbers line up; they are sized so that the slowest
  platform still runs for about a second or more.
- The client checks the count and order (`seq`) of what it received; a run
  that lost or reordered messages would be flagged in the tables.

### 6.2 Stream (fixed rate, main-isolate cost)

The server sends at a fixed rate (1 KB JSON at 2 000/s, 14 KB JSON at
500/s, 60 KB binary at 200/s) for 3 s. Fixed rates show what a steady
real-time feed costs the UI thread rather than the maximum.

- **Main-isolate time per message**: a probe on the main isolate spins in
  100 µs chunks scheduled with `Timer.run`. The time it does *not* get to
  spin was taken by other events: socket reads, decoding, callbacks. The
  probe is calibrated for 1 s on an idle isolate first and its idle
  overhead is subtracted:
  `mainUsPerMsg = (busy − idleBusy) × wall / received`. This is the cost a
  Flutter app pays on its UI thread, the one that drops frames.
- **Longest stall**: the largest gap between two probe chunks, i.e. the
  longest time the event loop was blocked at once (a frame is 16.7 ms at
  60 Hz, 8.3 ms at 120 Hz).
- **Delivery latency p50/p99**: `receiveTime − ts` per message.
- **CPU µs/msg**: as in flood, minus the CPU the probe itself burned
  spinning.
- Not run in browsers: browsers clamp nested `setTimeout(0)` to 4 ms, so
  the probe would measure the clamp, not the transport.

### 6.3 Ping-pong (sequential request/response, spinify's method)

The client sends a message to `/echo`, waits for the echo, then sends the
next one, `count` times. The connection is opened before the stopwatch
starts. Each request is built fresh (`{...template, 'seq': n, 'ts': now}`
encoded by the library, or a fresh `Uint8List` copy for binary), so the
send path (encoding, masking, copying) is measured as well as the receive
path.

- `round trips/s = count / wall`, `MB/s = count × messageBytes / wall_µs`
  (one direction).
- RTT p50/p99 per round trip. Chrome coarsens `performance.now()` to
  100 µs on pages that are not cross-origin isolated, so browser RTTs are
  quantized to 0.1 ms.
- **Spinify's numbers are not directly comparable**: spinify counts sent
  plus received messages and bytes, i.e. twice our msg/s and MB/s for the
  same run, and goes through Centrifugo (Go) with its protocol envelope.
  The layout of the tables follows spinify; the units are defined above.

### 6.4 The grid: size × count

Ping-pong for every combination of 6 sizes × 3 counts (100, 1 000,
10 000 round trips) × 2 formats. The count axis shows the start-up cost:

- ×100 is dominated by one-off costs: first-use code paths (JIT tiering in
  browsers, lazy initialization on the VM), the first TLS records, socket
  buffers growing, the first GCs.
- ×10 000 is the steady state and the column comparable with spinify's
  tables (which use 10 000).

The ×10 000 column of every size is also presented in spinify's layout.

### 6.5 JSON consumption: header vs full model

By default the client reads only `seq` and `ts` from each decoded message.
For the eager decoders (`ws-dart`, `ws` in browsers, `wsc`) that changes
nothing: the whole document is already materialized. `ws-native` hands out
lazy views over a tape that simd-json built off the Dart thread, so it
skips materializing the fields that are not read. That is a real property
of the library, but an app usually reads everything. The `→ model` cases
therefore map every message to typed models (`Order.fromJson` over all
fields, `lib/src/model.dart`) for every variant: flood 1/14/60 KB, stream
14 KB @ 500/s and ping-pong 1/14/60 KB × 10 000. Read the `→ model` rows
when the app maps every field, the plain rows when it reads a few.

## 7. Procedure

- **Repeats**: 3 per cell. The reported value is the median run, chosen by
  the scenario's key metric (flood: MB/s; ping-pong: msg/s; stream:
  main-isolate µs/msg); the other metrics in that row come from the same
  run. The report ends with the run-to-run spread
  ((max − min) / median) per platform, grouped into long runs, ×100
  ping-pongs and main-isolate readings, to tell real differences from
  noise: for long runs the median spread is 1–3 %, so differences under
  about 5 % are not meaningful.
- **Probe noise band**: the main-isolate probe's calibration drifts, so a
  transport that costs nothing can read slightly negative. The largest
  negative reading per platform (macOS 0.1 %, iPad 1.4 %, Android 0.6 % of
  main-isolate time) is taken as the noise band; readings inside it are
  shown as ≈0 and ratios against them as lower bounds. On Android the same
  case also varies by several points between runs (the probe cannot tell
  work on the main thread from the thread being descheduled on a
  big.LITTLE CPU), so only large differences there mean something.
- **Interleaving**: within each repeat, the variants of a case run one
  after another (`ws-dart`, `ws-native`, `wsc`), and on mobile and in the
  browser the whole matrix is repeated rather than each case, so thermal
  throttling and background activity spread over all variants instead of
  hitting one.
- **macOS**: every run is a fresh AOT process (`bin/matrix.dart` →
  `build/cli/bundle/bin/bench`), so no state carries over: no warm caches,
  a fresh heap, fresh RSS.
- **iPad / Android**: one release app runs the whole matrix in one process
  (`app/lib/main.dart`), 300 ms pause between runs. Heap and code warmth
  carry over between cases; peak RSS is therefore not reported.
- **Browsers**: one page per compiler runs the whole matrix
  (`web/main.dart`), 300 ms pause between runs, results posted to the
  runner (`bin/web_bench.dart`), which serves the page, starts the server
  and Chrome (fresh profile, `--headless=new`,
  `--disable-background-timer-throttling`,
  `--disable-renderer-backgrounding`,
  `--disable-backgrounding-occluded-windows`, `--disable-extensions`).
  Browsers expose no process CPU or RSS, so those columns are empty.
- **No warm-up run** is discarded: the ×100 column of the grid is meant to
  show the cold start. In the other cases, the run lengths (≥ 1 s) make
  the start-up share small.

## 8. Limits and threats to validity

- **Loopback** hides network latency and bandwidth. Over a real link,
  per-message client costs still matter for CPU, battery and UI jank, but
  round-trip throughput is bounded by the RTT long before the client.
- **Server on the same device**: on mobile the server shares the CPU (2
  threads) with the client; its CPU time is subtracted from the CPU metric
  but it still competes for cores and memory bandwidth, which lowers the
  absolute numbers of all variants alike.
- **Different TLS stacks**: `ws-dart` and `wsc` on the VM use BoringSSL
  through `dart:io`; `ws-native` uses rustls with ring; browsers use
  Chrome's BoringSSL. Part of every difference on `wss` is the TLS
  implementation and how many copies sit around it, which is a real
  property of each option.
- **One device per platform**, one Chrome build, headless. Other SoCs,
  thermal states or Safari/Firefox will give other absolute numbers.
- **Synthetic messages**: one JSON shape. Payloads with longer strings
  (or non-ASCII text) shift costs toward UTF-8 decoding; the earlier
  prototype study (`RESULTS.md`) covers the unicode case.
- **MB/s** counts payload bytes only; TLS and WebSocket framing add
  2–14 bytes per frame plus about 22 bytes per TLS record.

## 9. Reproducing

```bash
# Server and the AOT bench binary (build hooks compile the Rust crates)
cd server && cargo build --release && cd ../benchmark
dart build cli -t bin/bench.dart -o build/cli
SRV=../server/target/release/ws_bench_server

# macOS: fresh process per run
dart run bin/matrix.dart --suite=compare       --repeat=3 --server=$SRV --out=results/compare-macos.jsonl
dart run bin/matrix.dart --suite=compare-model --repeat=3 --server=$SRV --out=results/compare-model-macos.jsonl
dart run bin/matrix.dart --suite=grid          --repeat=3 --server=$SRV --out=results/grid-macos.jsonl

# Chrome, dart2js and dart2wasm
dart run bin/web_bench.dart --suite=compare-web       --out=results/compare-web.jsonl
dart run bin/web_bench.dart --suite=compare-model-web --out=results/compare-model-web.jsonl
dart run bin/web_bench.dart --suite=grid              --out=results/grid-web.jsonl

# Android
cd app
flutter build apk --release --target-platform android-arm64 \
  --dart-define=SUITE=compare,grid,compare-model --dart-define=REPEAT=3
adb install -r build/app/outputs/flutter-apk/app-release.apk
adb shell am start -n dev.wsbench.ws_bench_app/.MainActivity
# when logcat shows BENCH_DONE (about 95 minutes on the S22 Ultra):
adb logcat -d -s flutter | grep 'BENCH {' | sed 's/.*BENCH //' > ../results/android.jsonl

# iPad / iPhone
flutter build ios --release --dart-define=SUITE=compare,grid,compare-model --dart-define=REPEAT=3
xcrun devicectl device install app --device <udid> build/ios/iphoneos/Runner.app
xcrun devicectl device process launch --device <udid> dev.wsbench.wsBenchApp
# when the screen says "Done":
xcrun devicectl device copy from --device <udid> --domain-type appDataContainer \
  --domain-identifier dev.wsbench.wsBenchApp --source tmp/results.jsonl \
  --destination ../results/ipad.jsonl
cd ..

# Tables (COMPARISON.md = hand-written summary + this output)
dart run bin/report.dart \
  "macOS=results/compare-macos.jsonl" "macOS=results/compare-model-macos.jsonl" \
  "macOS=results/grid-macos.jsonl" "iPad=results/ipad.jsonl" "Android=results/android.jsonl" \
  "Chrome=results/compare-web.jsonl" "Chrome=results/compare-model-web.jsonl" \
  "Chrome=results/grid-web.jsonl"
```

`--dart-define=FILTER=stream` (a substring of `"<scenario> <case>"`)
re-runs a subset; that is how the mobile stream cases were re-run after the
clock fix (section 6).
