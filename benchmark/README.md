# ws_benchmark

Benchmarks of WebSocket client transports for the `ws` package, run against
the Rust server in [`../server`](../server).

```bash
make bench        # from the repo root: full matrix, 3 repeats (~25 min)
make bench-quick  # shorter runs, 1 repeat
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
