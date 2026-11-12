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

Results: [RESULTS.md](RESULTS.md).

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
