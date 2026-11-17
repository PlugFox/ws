/// Renders the library vs `web_socket_channel` comparison and the ping-pong
/// grid from raw JSON lines of several platforms as markdown.
///
/// Usage:
///   dart run bin/report.dart "macOS=results/compare-macos.jsonl" \
///       "iPad=results/compare-ipad.jsonl" ... > COMPARISON.md
///
/// A file with a `compiler` field (browser runs) splits into one platform
/// per compiler: `<label> dart2js`, `<label> dart2wasm`. Each cell is the
/// median of the repeats by the case's key metric.
library;

import 'dart:convert';
import 'dart:io';

import 'package:ws_benchmark/src/cases.dart';

typedef Run = Map<String, Object?>;

void main(List<String> arguments) {
  // platform -> "scenario: case" -> variant -> runs
  final data = <String, Map<String, Map<String, List<Run>>>>{};
  for (final arg in arguments) {
    final [label, path] = arg.split('=');
    for (final line in File(path).readAsLinesSync()) {
      if (line.trim().isEmpty) continue;
      final r = jsonDecode(line) as Run;
      final compiler = r['compiler'];
      final platform = compiler is String ? '$label $compiler' : label;
      final variant = r['variant']! as String;
      (((data[platform] ??= {})['${r['scenario']}: ${r['case']}'] ??= {})[variant] ??= [])
          .add(r);
    }
  }
  final platforms = data.keys.toList();

  Run? cell(String platform, String scenario, String name, String variant) {
    final runs = data[platform]?['$scenario: $name']?[variant];
    if (runs == null || runs.isEmpty) return null;
    final metric = keyMetric(scenario);
    final sorted = [...runs]
      ..sort((a, b) => ((a[metric] as num?) ?? 0).compareTo((b[metric] as num?) ?? 0));
    return sorted[sorted.length ~/ 2];
  }

  bool isWeb(String platform) => platform.contains('dart2');
  // Mobile runs keep every case in one process next to an embedded server.
  bool embedded(String platform) => data[platform]!.values
      .expand((v) => v.values)
      .expand((runs) => runs)
      .any((r) => r.containsKey('serverCpuMs'));
  // Columns: the library's best transport per platform, then the rest.
  List<(String, String)> columns(String platform) => isWeb(platform)
      ? const [('ws', 'ws'), ('wsc', 'web_socket_channel')]
      : const [
          ('ws-dart', 'ws (Dart)'),
          ('ws-native', 'ws + ws_native'),
          ('wsc', 'web_socket_channel'),
        ];

  // [floor]: values below it are noise; the ratio uses the floor instead
  // and is shown as a lower bound.
  String ratio(Run? best, Run? base, String metric,
      {bool lowerIsBetter = false, double floor = 0}) {
    var a = (best?[metric] as num?)?.toDouble(), b = (base?[metric] as num?)?.toDouble();
    if (a == null || b == null) return '—';
    final bound = lowerIsBetter ? a < floor : b < floor;
    if (lowerIsBetter) {
      a = a < floor ? floor : a;
    } else {
      b = b < floor ? floor : b;
    }
    if (a <= 0 || b <= 0) return '—';
    final x = lowerIsBetter ? b / a : a / b;
    return '${bound ? '≥' : ''}×${x.toStringAsFixed(x >= 10 ? 0 : 2)}';
  }

  List<Run> runsOf(String platform, String scenario, String name, String variant) =>
      data[platform]?['$scenario: $name']?[variant] ?? const [];

  // Best library variant for a row by the metric.
  Run? bestWs(String platform, String scenario, String name, String metric,
      {bool lowerIsBetter = false}) {
    final candidates = [
      for (final (v, _) in columns(platform))
        if (v != 'wsc') ?cell(platform, scenario, name, v),
    ].where((r) => r[metric] is num).toList();
    if (candidates.isEmpty) return null;
    candidates.sort((a, b) => (a[metric]! as num).compareTo(b[metric]! as num));
    return lowerIsBetter ? candidates.first : candidates.last;
  }

  // Noise band of the main-isolate probe, in % of main-isolate time: the
  // largest negative reading of the platform (the probe's calibration
  // drift), at least 0.1 %.
  final noiseBand = {
    for (final p in platforms)
      p: [
        0.1,
        for (final byVariant in data[p]!.values)
          for (final runs in byVariant.values)
            for (final r in runs)
              if (r['mainBusyPct'] case final num b when b < 0) -b.toDouble(),
      ].reduce((a, b) => a > b ? a : b),
  };
  double mainFloor(String p, Run r) =>
      noiseBand[p]! / 100 * 1e6 / ((r['msgsPerSec'] as num?) ?? 1).toDouble();

  final out = StringBuffer();
  void table(
    String title,
    String scenario,
    List<Case> rows,
    String metric,
    String Function(Run) format, {
    bool lowerIsBetter = false,
    bool vmOnly = false,
    bool processPerCase = false,
    double floor = 0,
    double Function(String platform, Run run)? floorOf,
    String Function(List<Run>)? formatRuns,
    String? note,
  }) {
    out.writeln('\n### $title\n');
    if (note != null) out.writeln('$note\n');
    out
      ..writeln('| platform | case | ws (Dart / browser) | ws + ws_native | web_socket_channel '
          '| best ws vs wsc |')
      ..writeln('|---|---|---:|---:|---:|---:|');
    for (final p in platforms) {
      if (vmOnly && isWeb(p)) continue;
      if (processPerCase && embedded(p)) continue;
      for (final c in rows.where((c) => c.scenario == scenario)) {
        final wsDart = cell(p, scenario, c.name, isWeb(p) ? 'ws' : 'ws-dart');
        final wsNative = isWeb(p) ? null : cell(p, scenario, c.name, 'ws-native');
        final wsc = cell(p, scenario, c.name, 'wsc');
        if (wsDart == null && wsNative == null && wsc == null) continue;
        String f(Run? r) => r == null || r[metric] == null
            ? '—'
            : formatRuns != null
                ? formatRuns(runsOf(p, scenario, c.name, r['variant']! as String))
                : floorOf != null && (r[metric]! as num) < floorOf(p, r)
                    ? '≈0 (${_f(r['mainBusyPct'], 1)} %)'
                    : format(r);
        final best = bestWs(p, scenario, c.name, metric, lowerIsBetter: lowerIsBetter);
        out.writeln('| $p | ${c.name} | ${f(wsDart)} | ${f(wsNative)} | ${f(wsc)} '
            '| ${ratio(best, wsc, metric, lowerIsBetter: lowerIsBetter, floor: best != null && floorOf != null ? floorOf(p, best) : floor)} |');
      }
    }
  }

  final compare = [...compareCases(), ...compareModelCases()];
  out.writeln('## Library vs web_socket_channel (wss)');
  table('Flood — throughput, MB/s (higher is better)', 'flood', compare, 'mbPerSec',
      (r) => _f(r['mbPerSec'], 1));
  table('Flood — messages per second (higher is better)', 'flood', compare, 'msgsPerSec',
      (r) => _f(r['msgsPerSec'], 0));
  table('Flood — process CPU per message, µs (lower is better)', 'flood', compare,
      'cpuUsPerMsg', (r) => _f(r['cpuUsPerMsg'], 1),
      lowerIsBetter: true, vmOnly: true,
      note: 'All threads of the process (Rust transport threads included); '
          'on mobile the embedded server\'s threads are subtracted.');
  table('Flood — peak RSS, MB (min–max over the runs)', 'flood', compare, 'rssMb',
      (r) => _f(r['rssMb'], 0),
      formatRuns: (runs) {
        final xs = runs.map((r) => (r['rssMb']! as num).toDouble()).toList()..sort();
        return '${_f(xs.first, 0)}–${_f(xs.last, 0)}';
      },
      lowerIsBetter: true, vmOnly: true, processPerCase: true,
      note: 'Only where every run is a fresh process (not on mobile, where all '
          'cases share one). The ratio compares the median runs.');
  table('Stream — main isolate time per message, µs (share of main-isolate time)', 'stream',
      compare, 'mainUsPerMsg',
      (r) => '${_f(r['mainUsPerMsg'], 1)} (${_f(r['mainBusyPct'], 1)} %)',
      lowerIsBetter: true, vmOnly: true, floorOf: mainFloor,
      note: 'The time the UI isolate loses to receiving and decoding: what costs '
          'frames in a Flutter app. Readings inside the probe\'s noise band '
          '(${[for (final p in platforms) if (!isWeb(p)) '$p ±${noiseBand[p]!.toStringAsFixed(1)} %'].join(', ')} '
          'of main-isolate time, the largest negative reading per platform) show '
          'as ≈0, and ratios against them are lower bounds (≥).');
  table('Stream — longest main isolate stall, µs', 'stream', compare, 'stallMaxUs',
      (r) => _f(r['stallMaxUs'], 0),
      lowerIsBetter: true, vmOnly: true);
  table('Stream — delivery latency p50 / p99, µs', 'stream', compare, 'latP50Us',
      (r) => '${_f(r['latP50Us'], 0)} / ${_f(r['latP99Us'], 0)}',
      lowerIsBetter: true, vmOnly: true);
  table('Stream — process CPU per message, µs', 'stream', compare, 'cpuUsPerMsg',
      (r) => _f(r['cpuUsPerMsg'], 1),
      lowerIsBetter: true, vmOnly: true);

  table('Ping-pong ×10000 → model — round trips per second', 'pingpong', compare,
      'msgsPerSec', (r) => '${_f(r['msgsPerSec'], 0)} (${_f(r['mbPerSec'], 1)} MB/s)');
  table('Ping-pong ×10000 → model — RTT p50 / p99, µs', 'pingpong', compare, 'rttP50Us',
      (r) => '${_f(r['rttP50Us'], 0)} / ${_f(r['rttP99Us'], 0)}',
      lowerIsBetter: true);

  // Ping-pong grid.
  out
    ..writeln('\n## Ping-pong grid: message size × message count (wss)')
    ..writeln('\nRound trips per second (one-way MB/s). Spinify counts sent and '
        'received messages, so its msg/s and MB/s are twice these for the same run.');
  for (final format in ['json', 'binary']) {
    out.writeln('\n### $format, ×10000 (spinify layout)\n');
    for (final p in platforms) {
      final cols = columns(p);
      out
        ..writeln('\n**$p**\n')
        ..writeln('| size | ${cols.map((c) => c.$2).join(' | ')} |')
        ..writeln('|---|${cols.map((_) => '---:').join('|')}|');
      for (final size in gridSizes) {
        final name = '$format ${sizeLabel(size)} ×10000';
        final cells = cols.map((c) {
          final r = cell(p, 'pingpong', name, c.$1);
          return r == null ? '—' : '${_f(r['msgsPerSec'], 0)} (${_f(r['mbPerSec'], 1)})';
        });
        out.writeln('| ${sizeLabel(size)} | ${cells.join(' | ')} |');
      }
    }

    out
      ..writeln('\n### $format — effect of the message count, MB/s')
      ..writeln('\nEach cell: ×100 / ×1 000 / ×10 000 round trips.\n');
    for (final p in platforms) {
      final cols = columns(p);
      out
        ..writeln('\n**$p**\n')
        ..writeln('| size | ${cols.map((c) => c.$2).join(' | ')} |')
        ..writeln('|---|${cols.map((_) => '---:').join('|')}|');
      for (final size in gridSizes) {
        final cells = [
          for (final c in cols)
            [
              for (final n in gridCounts)
                switch (cell(p, 'pingpong', '$format ${sizeLabel(size)} ×$n', c.$1)) {
                  null => '—',
                  final r => _f(r['mbPerSec'], 1),
                },
            ].join(' / '),
        ];
        out.writeln('| ${sizeLabel(size)} | ${cells.join(' | ')} |');
      }
    }
  }

  out.writeln('\n### Ping-pong RTT p50 / p99, µs (×10000)\n');
  for (final format in ['json', 'binary']) {
    for (final p in platforms) {
      final cols = columns(p);
      out
        ..writeln('\n**$p, $format**\n')
        ..writeln('| size | ${cols.map((c) => c.$2).join(' | ')} |')
        ..writeln('|---|${cols.map((_) => '---:').join('|')}|');
      for (final size in gridSizes) {
        final cells = cols.map((c) {
          final r = cell(p, 'pingpong', '$format ${sizeLabel(size)} ×10000', c.$1);
          return r == null ? '—' : '${_f(r['rttP50Us'], 0)} / ${_f(r['rttP99Us'], 0)}';
        });
        out.writeln('| ${sizeLabel(size)} | ${cells.join(' | ')} |');
      }
    }
  }

  // Run-to-run spread of the key metric, to judge which differences are real.
  // Grouped: short ×100 ping-pongs and main-isolate probe readings (often
  // near zero) spread far more than the rest.
  String group(Run r) => switch (r['scenario']) {
        'stream' => 'stream',
        _ when (r['case']! as String).endsWith('×100') => 'x100',
        _ => 'rest',
      };
  out
    ..writeln('\n## Run-to-run spread\n')
    ..writeln('(max − min) / median of the key metric over the repeats of a cell: '
        'median / p90 / max over the cells.\n')
    ..writeln('| platform | flood and ping-pong ×1 000, ×10 000 | ping-pong ×100 '
        '| stream (main-isolate time) |')
    ..writeln('|---|---:|---:|---:|');
  for (final p in platforms) {
    final spreads = <String, List<double>>{};
    for (final byVariant in data[p]!.values) {
      for (final runs in byVariant.values) {
        if (runs.length < 2) continue;
        final metric = keyMetric(runs.first['scenario']! as String);
        final xs = runs.map((r) => (r[metric] as num?)?.toDouble() ?? 0).toList()..sort();
        final med = xs[xs.length ~/ 2];
        if (med == 0) continue;
        (spreads[group(runs.first)] ??= []).add((xs.last - xs.first).abs() / med.abs());
      }
    }
    String summary(List<double>? xs) {
      if (xs == null || xs.isEmpty) return '—';
      xs.sort();
      String pct(double x) => '${(x * 100).toStringAsFixed(0)} %';
      return '${pct(xs[xs.length ~/ 2])} / '
          '${pct(xs[(xs.length * 0.9).floor().clamp(0, xs.length - 1)])} / ${pct(xs.last)}';
    }

    out.writeln('| $p | ${summary(spreads['rest'])} | ${summary(spreads['x100'])} '
        '| ${summary(spreads['stream'])} |');
  }

  stdout.write(out);
}

String _f(Object? v, int digits) => switch (v) {
      null => '—',
      num n => n.toStringAsFixed(digits),
      _ => '$v',
    };
