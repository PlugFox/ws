# ws_benchmark

Benchmarks of WebSocket client transports for the `ws` package, run against
the Rust server in [`../server`](../server).

```bash
make bench        # from the repo root: full matrix, 3 repeats (~25 min)
make bench-quick  # shorter runs, 1 repeat
```

Single case:

```bash
dart compile exe bin/bench.dart -o build/bench
build/bench --variant=c --scenario=pingpong --size=14336 --count=10000 [--tls]
```

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
