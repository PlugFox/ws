# ws_benchmark

Benchmarks of WebSocket client transports for the `ws` package, run against
the Rust server in [`../server`](../server).

```bash
mise run bench        # from anywhere in the repo: full matrix, 3 repeats (~25 min)
mise run bench:quick  # shorter runs, 1 repeat
```

Single case. `dart build cli` runs the build hook, which compiles the Rust
crate in `native/` and bundles it next to the executable. `dart compile exe`
does not run hooks, so the `d-*` variants would fail with it.

```bash
dart build cli -t bin/bench.dart -o build/cli
build/cli/bundle/bin/bench --variant=d-tape --scenario=pingpong --size=14336 --count=10000 [--tls] [--consume=full]
```

## Variants

| id | transport | where JSON is parsed | what main receives |
|---|---|---|---|
| `a` | `dart:io` WebSocket, main | main, `jsonDecode(String)` | `Map` |
| `b` | `dart:io` WebSocket, worker | worker | `Map` (copied by `SendPort`) |
| `c` | own codec (`lib/src/codec.dart`), main | main, fused UTF-8 + JSON | `Map` |
| `c-worker` | own codec, worker | worker | `Map` |
| `c-worker-raw` | own codec, worker | main | bytes (`TransferableTypedData`) |
| `c-tape` | own codec, main | main, into a tape | lazy `TapeMap` views |
| `c-worker-tape` | own codec, worker | worker, into a tape | tape bytes → lazy views |
| `c-worker-model` | own codec, worker | worker, plus `Order.fromJson` | typed models |
| `d-bytes` | Rust (tokio + tungstenite) | main | external typed data |
| `d-tape` | Rust | Rust, simd-json into a tape | external tape → lazy views |
| `d-cobject` | Rust | Rust, simd-json into a `Dart_CObject` tree | VM-rebuilt `List` tree → views |
| `ws-dart` | `package:ws`, pure-Dart transport | main, fused UTF-8 + JSON | `Map` |
| `ws-native` | `package:ws` + `ws_native` | Rust, simd-json into a tape | lazy `Map` views |
| `wsc` | `package:web_socket_channel` (`IOWebSocketChannel`) | main, `jsonDecode(String)` | `Map` |

`--consume=full` maps every message to typed models (`Order.fromJson`, all
fields), as an app would. The default reads only `seq` and `ts`.

The tape format is described in `lib/src/tape.dart`. The Dart builder and
the Rust builder (`native/src/tape.rs`) produce the same layout.

## Scenarios

- `stream`: the server pushes `rate` msg/s. A probe on the main isolate
  spins in 100 µs `Timer.run` chunks, so the time it *didn't* get is the
  main-isolate cost of the transport plus decoding. This is the metric that
  matters for Flutter UI threads. Gaps between chunks give the longest
  event-loop stall, a proxy for jank.
- `flood`: the server pushes `count` messages as fast as possible. Measures
  throughput, process CPU per message across all threads, and peak RSS.
- `pingpong`: `count` sequential request/response pairs, as in spinify's
  benchmark (sizes 1/14/30/60 KB). Measures msg/s and RTT.
- `echo`: the client sends at a fixed rate. Measures RTT under steady load,
  including the send path (encoding and masking).

Payload sizes follow typical limits: Centrifugo rejects messages over 64 KB.
The 1 MB cases are stress tests.

Results: [RESULTS.md](RESULTS.md) (prototypes, the design study) and
[COMPARISON.md](COMPARISON.md) (the library vs `web_socket_channel` on
macOS, iPad, Android and Chrome; method in [METHODOLOGY.md](METHODOLOGY.md)).

## Suites

`bin/matrix.dart --suite=…`, the app's `--dart-define=SUITE=…` (comma
separated) and `bin/web_bench.dart --suite=…` pick the case list from
`lib/src/cases.dart`:

| suite | what |
|---|---|
| `default` | the prototype matrix of `RESULTS.md` |
| `mobile` | the prototype matrix for phones and tablets |
| `compare` / `compare-web` | `ws` vs `web_socket_channel`: flood and stream, json and binary, wss |
| `compare-model` / `compare-model-web` | the same with every JSON message mapped to typed models |
| `grid` | ping-pong over wss: 6 sizes × 100/1 000/10 000 messages × json/binary |

## Browsers

`web/main.dart` runs a suite in a page; `bin/web_bench.dart` compiles it
with dart2js and dart2wasm, serves it, starts the server and a headless
Chrome, and collects the results:

```bash
dart run bin/web_bench.dart --suite=grid --repeat=3 --out=results/grid-web.jsonl
dart run bin/report.dart "macOS=results/grid-macos.jsonl" "Chrome=results/grid-web.jsonl"
```

## Mobile devices

`app/` is a Flutter runner for phones and tablets. A separate server process
cannot run there, so the app starts the Rust server inside its own process
(`ws_bench_server::start_embedded`, 2 threads, 127.0.0.1). That also avoids
Wi-Fi noise and the iOS local network prompt. The CPU time of the server's
threads is excluded from `cpuUsPerMsg`: each server thread reports its
`CLOCK_THREAD_CPUTIME_ID` when it parks. The case list is `mobileCases()` in
`lib/src/cases.dart`.

```sh
cd app
flutter build apk --release --target-platform android-arm64 --dart-define=REPEAT=2
adb install -r build/app/outputs/flutter-apk/app-release.apk
adb shell am start -n dev.wsbench.ws_bench_app/.MainActivity
adb logcat -s flutter | grep 'BENCH '        # one JSON line per run

flutter build ios --release --dart-define=REPEAT=2
xcrun devicectl device install app --device <udid> build/ios/iphoneos/Runner.app
xcrun devicectl device process launch --device <udid> dev.wsbench.wsBenchApp
xcrun devicectl device copy from --device <udid> --domain-type appDataContainer \
  --domain-identifier dev.wsbench.wsBenchApp --source tmp/results.jsonl --destination ios.jsonl

dart run bin/matrix.dart --from=ios.jsonl --mobile   # markdown tables
```

Other options are `--dart-define=FILTER=<substring>` and `VARIANTS=a,c,...`.
Peak RSS is not meaningful here, because every case runs in one process.

The build hook builds the Rust crate for Android with the NDK clang that
Flutter passes to it. Release `print` output on iOS goes to os_log, not to
the console, so read the results file from the app container.

## Profiling on Android

The app's manifest has `<profileable android:shell="true"/>`, so the
system `simpleperf` can record the release build without root, and the app
logs `BENCH_CASE <scenario>: <case> — <variant>` when a case starts.

```sh
# symbols for Dart AOT code; one case and variant keep the timing simple
flutter build apk --release --target-platform android-arm64 \
  --split-debug-info=/tmp/dartsyms --dart-define=SUITE=grid \
  "--dart-define=FILTER=json 14KB ×10000" --dart-define=VARIANTS=ws-native --dart-define=REPEAT=1
adb install -r build/app/outputs/flutter-apk/app-release.apk
adb logcat -c && adb shell am start -n dev.wsbench.ws_bench_app/.MainActivity
until adb logcat -d -s flutter | grep -q BENCH_CASE; do sleep 0.3; done; sleep 2
# cpu-clock sees every core (cpu-cycles is not counted on all cores of a
# big.LITTLE SoC); -g unwinds with DWARF
adb shell simpleperf record --app dev.wsbench.ws_bench_app -e cpu-clock -f 4000 -g \
  --duration 6 -o /data/local/tmp/perf.data
adb pull /data/local/tmp/perf.data
NDK_SP=$ANDROID_HOME/ndk/<version>/simpleperf
mkdir -p syms && cp /tmp/dartsyms/app.android-arm64.symbols syms/
python3 $NDK_SP/binary_cache_builder.py -i perf.data -lib syms
$NDK_SP/bin/darwin/x86_64/simpleperf report -i perf.data --symfs binary_cache \
  --sort comm,dso,symbol
```

Gradle strips the `.so` files it packages, and `ws_native` is built with
`strip = true` and without a build id, so its frames show as
`libws_native.so[+addr]`. Build once with `strip = false` in
`native/rust/Cargo.toml`, take the unstripped library from
`app/.dart_tool/hooks_runner/shared/ws_native/build/*/libws_native.so`,
report with `--sort dso,vaddr_in_file` and look the addresses up in
`llvm-nm -n -C` of that file (the addresses are virtual addresses, no
offset). `simpleperf dump` gives the CPU of every sample, for which cluster
a thread ran on.

## Library variants and Autobahn

- `--variant=ws-dart` / `ws-native`: the `ws` package itself through its
  public API (pure-Dart transport / `ws_native`).
- Protocol conformance with the Autobahn TestSuite:

  ```bash
  mkdir -p build/autobahn
  docker run -d --rm -p 9001:9001 -v $PWD/autobahn:/config \
    -v $PWD/build/autobahn:/reports --name fuzzingserver \
    crossbario/autobahn-testsuite \
    wstest -m fuzzingserver -s /config/fuzzingserver.json
  dart run bin/autobahn.dart --transport=dart     # or native
  python3 autobahn/summary.py                     # build/autobahn/index.html
  docker stop fuzzingserver
  ```
