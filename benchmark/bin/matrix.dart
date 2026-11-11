/// Runs the benchmark matrix: every case × every variant, each in a fresh
/// AOT process, and prints markdown tables.
///
/// Usage:
///   dart run bin/matrix.dart [--bench=build/cli/bundle/bin/bench] [--server=../server/target/release/ws_bench_server]
///       [--variants=a,b,c,c-worker,c-worker-raw] [--filter=substring]
///       [--repeat=3] [--out=build/results.jsonl] [--quick]
///
/// With `--server` the matrix starts (and stops) the server itself.
/// Each case is repeated `--repeat` times; the median run (by the case's key
/// metric) is reported.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

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

Future<void> main(List<String> arguments) async {
  final args = <String, String>{
    for (final a in arguments)
      if (a.startsWith('--'))
        a.substring(2).split('=').first:
            a.contains('=') ? a.substring(a.indexOf('=') + 1) : 'true',
  };
  final bench = args['bench'] ?? 'build/cli/bundle/bin/bench';
  final variants = (args['variants'] ??
          'a,b,c,c-worker,c-worker-raw,c-tape,c-worker-tape,c-worker-model,d-bytes,d-tape,d-cobject')
      .split(',');
  final filter = args['filter'];
  final repeat = int.parse(args['repeat'] ?? '1');
  final out = File(args['out'] ?? 'build/results.jsonl');
  final quick = args['quick'] == 'true';

  Process? server;
  if (args['server'] case final path?) {
    server = await Process.start(path, const []);
    server.stderr.transform(utf8.decoder).listen(stderr.write);
    await _waitForPort(9090);
  }

  final selected = cases(quick: quick)
      .where((c) => filter == null || '${c.scenario} ${c.name}'.contains(filter))
      .toList();
  final results = <String, Map<String, Map<String, Object?>>>{};
  out.parent.createSync(recursive: true);
  final sink = out.openWrite();
  try {
    for (final c in selected) {
      final key = '${c.scenario}: ${c.name}';
      for (final v in variants) {
        final runs = <Map<String, Object?>>[];
        for (var i = 0; i < repeat; i++) {
          stderr.write('\r\x1B[K$key — $v (${i + 1}/$repeat)');
          final r = await _run(bench, ['--variant=$v', '--scenario=${c.scenario}', ...c.args]);
          if (r == null) continue;
          r['case'] = c.name;
          sink.writeln(jsonEncode(r));
          runs.add(r);
        }
        if (runs.isEmpty) continue;
        final metric = keyMetric(c.scenario);
        runs.sort((x, y) => ((x[metric] as num?) ?? 0).compareTo((y[metric] as num?) ?? 0));
        (results[key] ??= {})[v] = runs[runs.length ~/ 2];
      }
    }
  } finally {
    await sink.close();
    stderr.writeln('\r\x1B[K');
    server?.kill();
  }

  _printTables(selected, variants, results);
  stdout.writeln('\nRaw results: ${out.path}');
}

Future<void> _waitForPort(int port) async {
  for (var i = 0; i < 50; i++) {
    try {
      final s = await Socket.connect('127.0.0.1', port);
      s.destroy();
      return;
    } on SocketException {
      await Future<void>.delayed(const Duration(milliseconds: 100));
    }
  }
  throw StateError('Server did not start on port $port');
}

Future<Map<String, Object?>?> _run(String bench, List<String> args) async {
  final result = await Process.run(bench, args).timeout(const Duration(minutes: 3));
  if (result.exitCode != 0) {
    stderr.writeln('\n$bench ${args.join(' ')} failed: ${result.stderr}');
    return null;
  }
  return jsonDecode((result.stdout as String).trim().split('\n').last) as Map<String, Object?>;
}

String _f(Object? v, [int digits = 1]) => switch (v) {
      null => '—',
      int i => '$i',
      num n => n.toStringAsFixed(digits),
      _ => '$v',
    };

void _printTables(
  List<Case> selected,
  List<String> variants,
  Map<String, Map<String, Map<String, Object?>>> results,
) {
  void table(String title, String scenario, String Function(Map<String, Object?>) cell) {
    final rows = selected.where((c) => c.scenario == scenario).toList();
    if (rows.isEmpty) return;
    stdout
      ..writeln('\n### $title\n')
      ..writeln('| case | ${variants.join(' | ')} |')
      ..writeln('|---|${variants.map((_) => '---:').join('|')}|');
    for (final c in rows) {
      final byVariant = results['${c.scenario}: ${c.name}'] ?? const {};
      final cells = variants.map((v) {
        final r = byVariant[v];
        if (r == null) return 'FAIL';
        final lost = r['received'] != r['expected'] ? ' ⚠${r['received']}/${r['expected']}' : '';
        return cell(r) + lost;
      });
      stdout.writeln('| ${c.name} | ${cells.join(' | ')} |');
    }
  }

  table('Stream — main isolate µs per message (lower is better)', 'stream',
      (r) => _f(r['mainUsPerMsg'], 2));
  table('Stream — longest main isolate stall, µs', 'stream', (r) => _f(r['stallMaxUs']));
  table('Stream — delivery latency p50 / p99, µs', 'stream',
      (r) => '${_f(r['latP50Us'])} / ${_f(r['latP99Us'])}');
  table('Stream — process CPU µs per message (all threads, probe excluded)', 'stream',
      (r) => _f(r['cpuUsPerMsg'], 1));
  table('Flood — throughput MB/s (higher is better)', 'flood', (r) => _f(r['mbPerSec']));
  table('Flood — process CPU µs per message', 'flood', (r) => _f(r['cpuUsPerMsg']));
  table('Flood — peak RSS, MB', 'flood', (r) => _f(r['rssMb']));
  table('Ping-pong — msg/s (MB/s), higher is better', 'pingpong',
      (r) => '${_f(r['msgsPerSec'], 0)} (${_f(r['mbPerSec'])})');
  table('Ping-pong — RTT p50 / p99, µs', 'pingpong',
      (r) => '${_f(r['rttP50Us'])} / ${_f(r['rttP99Us'])}');
  table('Ping-pong — process CPU µs per message', 'pingpong', (r) => _f(r['cpuUsPerMsg']));
  table('Echo — RTT p50 / p99, µs', 'echo', (r) => '${_f(r['rttP50Us'])} / ${_f(r['rttP99Us'])}');
  table('Echo — process CPU µs per message', 'echo', (r) => _f(r['cpuUsPerMsg']));
}
