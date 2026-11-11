/// Benchmark cases shared by `bin/matrix.dart` and the mobile runner app.
library;

final class Case {
  const Case(this.name, this.scenario, this.args);

  final String name;
  final String scenario;
  final List<String> args;
}

// Sizes follow spinify's benchmark (1 / 14 / 30 / 60 KB; Centrifugo rejects
// messages over 64 KB). 1 MB cases are stress tests beyond typical limits.
const kb1 = 1024, kb14 = 14336, kb30 = 30720, kb60 = 61440, mb1 = 1048576;

List<Case> cases({required bool quick}) {
  final d = quick ? 1500 : 3000;
  final f = quick ? 4 : 1;
  const full = '--consume=full';
  return [
    // Fixed-rate stream: main isolate cost per message, stalls, latency.
    // "→ model" cases map every message to typed models (Order.fromJson).
    Case('json ~50B @20k/s', 'stream', ['--size=0', '--rate=20000', '--duration=$d']),
    Case('json 1KB @5k/s', 'stream', ['--size=$kb1', '--rate=5000', '--duration=$d']),
    Case('json 1KB @5k/s → model', 'stream', ['--size=$kb1', '--rate=5000', '--duration=$d', full]),
    Case('json 14KB @1k/s', 'stream', ['--size=$kb14', '--rate=1000', '--duration=$d']),
    Case('json 14KB @1k/s → model', 'stream', ['--size=$kb14', '--rate=1000', '--duration=$d', full]),
    Case('json 60KB @500/s', 'stream', ['--size=$kb60', '--rate=500', '--duration=$d']),
    Case('json 60KB @500/s → model', 'stream', ['--size=$kb60', '--rate=500', '--duration=$d', full]),
    Case('json 60KB unicode @500/s → model', 'stream', ['--size=$kb60', '--rate=500', '--duration=$d', '--unicode', full]),
    Case('binary 60KB @500/s', 'stream', ['--format=binary', '--size=$kb60', '--rate=500', '--duration=$d']),
    Case('wss json 14KB @1k/s → model', 'stream', ['--tls', '--size=$kb14', '--rate=1000', '--duration=$d', full]),
    Case('json 1MB @20/s → model (stress)', 'stream', ['--size=$mb1', '--rate=20', '--duration=$d', full]),
    // Flood: max throughput and process CPU per message.
    Case('json 1KB', 'flood', ['--size=$kb1', '--count=${200000 ~/ f}']),
    Case('json 14KB → model', 'flood', ['--size=$kb14', '--count=${30000 ~/ f}', full]),
    Case('json 60KB', 'flood', ['--size=$kb60', '--count=${8000 ~/ f}']),
    Case('json 60KB → model', 'flood', ['--size=$kb60', '--count=${8000 ~/ f}', full]),
    Case('binary 60KB', 'flood', ['--format=binary', '--size=$kb60', '--count=${30000 ~/ f}']),
    Case('wss json 14KB → model', 'flood', ['--tls', '--size=$kb14', '--count=${30000 ~/ f}', full]),
    Case('wss binary 60KB', 'flood', ['--tls', '--format=binary', '--size=$kb60', '--count=${30000 ~/ f}']),
    // Sequential request/response, as in spinify's benchmark.
    Case('json 1KB', 'pingpong', ['--size=$kb1', '--count=${10000 ~/ f}']),
    Case('json 14KB', 'pingpong', ['--size=$kb14', '--count=${10000 ~/ f}']),
    Case('json 14KB → model', 'pingpong', ['--size=$kb14', '--count=${10000 ~/ f}', full]),
    Case('json 30KB', 'pingpong', ['--size=$kb30', '--count=${10000 ~/ f}']),
    Case('json 60KB', 'pingpong', ['--size=$kb60', '--count=${10000 ~/ f}']),
    Case('json 60KB → model', 'pingpong', ['--size=$kb60', '--count=${10000 ~/ f}', full]),
    Case('binary 60KB', 'pingpong', ['--format=binary', '--size=$kb60', '--count=${10000 ~/ f}']),
    Case('wss json 14KB → model', 'pingpong', ['--tls', '--size=$kb14', '--count=${10000 ~/ f}', full]),
    // Steady-rate echo: round trip under load incl. the send path.
    Case('json 1KB @1k/s', 'echo', ['--size=$kb1', '--rate=1000', '--duration=$d']),
    Case('binary 60KB @200/s', 'echo', ['--format=binary', '--size=$kb60', '--rate=200', '--duration=$d']),
  ];
}

/// Key metric used to pick the median run.
String keyMetric(String scenario) => switch (scenario) {
      'stream' => 'mainUsPerMsg',
      'flood' => 'mbPerSec',
      'pingpong' => 'msgsPerSec',
      _ => 'rttP50Us',
    };

/// Cases for phones and tablets: TLS (as in production), message sizes from
/// spinify's limits, rates scaled down for mobile CPUs. Run against the
/// server embedded in the app process (127.0.0.1).
List<Case> mobileCases() {
  const d = 2000;
  const full = '--consume=full';
  return [
    Case('wss json 1KB @2k/s → model', 'stream', ['--tls', '--size=$kb1', '--rate=2000', '--duration=$d', full]),
    Case('wss json 14KB @500/s → model', 'stream', ['--tls', '--size=$kb14', '--rate=500', '--duration=$d', full]),
    Case('wss json 60KB @100/s → model', 'stream', ['--tls', '--size=$kb60', '--rate=100', '--duration=$d', full]),
    Case('wss binary 60KB @200/s', 'stream', ['--tls', '--format=binary', '--size=$kb60', '--rate=200', '--duration=$d']),
    Case('ws json 14KB → model', 'flood', ['--size=$kb14', '--count=5000', full]),
    Case('wss json 14KB → model', 'flood', ['--tls', '--size=$kb14', '--count=5000', full]),
    Case('wss json 60KB → model', 'flood', ['--tls', '--size=$kb60', '--count=2000', full]),
    Case('wss binary 60KB', 'flood', ['--tls', '--format=binary', '--size=$kb60', '--count=10000']),
    Case('wss json 1KB → model', 'pingpong', ['--tls', '--size=$kb1', '--count=5000', full]),
    Case('wss json 14KB → model', 'pingpong', ['--tls', '--size=$kb14', '--count=3000', full]),
    Case('wss json 60KB → model', 'pingpong', ['--tls', '--size=$kb60', '--count=1500', full]),
  ];
}

/// Variants measured on mobile devices.
const mobileVariants = ['a', 'c', 'c-worker', 'c-worker-model', 'd-bytes', 'd-tape'];
