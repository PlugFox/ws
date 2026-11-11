/// Runs the benchmark matrix: every case × every variant, each in a fresh
/// AOT process, and prints markdown tables.
///
/// Usage:
///   dart run bin/matrix.dart [--bench=build/cli/bundle/bin/bench] [--server=../server/target/release/ws_bench_server]
///       [--variants=a,b,c,c-worker,c-worker-raw] [--filter=substring]
///       [--repeat=3] [--out=build/results.jsonl] [--quick]
///   dart run bin/matrix.dart --from=results.jsonl [--mobile]
///
/// `--from` only renders tables from earlier raw results (e.g. pulled from the
/// mobile runner app in `app/`); `--mobile` selects the mobile case list.
///
/// With `--server` the matrix starts (and stops) the server itself.
/// Each case is repeated `--repeat` times; the median run (by the case's key
/// metric) is reported.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:ws_benchmark/src/cases.dart';

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

  if (args['from'] case final from?) {
    final mobile = args['mobile'] == 'true';
    final selected = mobile ? mobileCases() : cases(quick: quick);
    final shown = args['variants'] == null && mobile ? mobileVariants : variants;
    final grouped = <String, Map<String, List<Map<String, Object?>>>>{};
    for (final line in File(from).readAsLinesSync()) {
      if (line.trim().isEmpty) continue;
      final r = jsonDecode(line) as Map<String, Object?>;
      ((grouped['${r['scenario']}: ${r['case']}'] ??= {})[r['variant']! as String] ??= []).add(r);
    }
    _printTables(selected, shown, {
      for (final MapEntry(:key, :value) in grouped.entries)
        key: {
          for (final MapEntry(key: v, value: runs) in value.entries)
            v: _median(runs, keyMetric(runs.first['scenario']! as String)),
        },
    });
    return;
  }

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
        (results[key] ??= {})[v] = _median(runs, keyMetric(c.scenario));
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

Map<String, Object?> _median(List<Map<String, Object?>> runs, String metric) {
  runs.sort((x, y) => ((x[metric] as num?) ?? 0).compareTo((y[metric] as num?) ?? 0));
  return runs[runs.length ~/ 2];
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
