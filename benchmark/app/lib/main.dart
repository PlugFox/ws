// ignore_for_file: avoid_print
// Runs the mobile benchmark matrix on a phone or tablet against the server
// embedded in this process, prints every result as a `BENCH {json}` line and
// appends it to `<systemTemp>/results.jsonl`.
//
// Build in release mode: flutter run --release -d <device>
// Options (--dart-define): REPEAT=3, FILTER=<substring>, VARIANTS=a,c,...
import 'dart:convert';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:ws_benchmark/ws_benchmark.dart';

const _repeat = int.fromEnvironment('REPEAT', defaultValue: 3);
const _filter = String.fromEnvironment('FILTER');
const _variants = String.fromEnvironment('VARIANTS');

void main() => runApp(const BenchApp());

class BenchApp extends StatefulWidget {
  const BenchApp({super.key});

  @override
  State<BenchApp> createState() => _BenchAppState();
}

class _BenchAppState extends State<BenchApp> {
  String _status = 'Starting…';
  final List<String> _log = [];

  @override
  void initState() {
    super.initState();
    // Let the first frame render before the benchmarks take over.
    Future<void>.delayed(const Duration(seconds: 2), _run);
  }

  void _show(String status) {
    if (mounted) setState(() => _status = status);
  }

  Future<void> _run() async {
    if (!startEmbeddedServer()) {
      _show('Embedded server failed to start');
      return;
    }
    final out = File('${Directory.systemTemp.path}/results.jsonl');
    if (out.existsSync()) out.deleteSync();
    final variants = _variants.isEmpty ? mobileVariants : _variants.split(',');
    final selected = mobileCases()
        .where((c) => _filter.isEmpty || '${c.scenario} ${c.name}'.contains(_filter))
        .toList();
    final total = _repeat * selected.length * variants.length;
    var done = 0;
    var failed = 0;
    print('BENCH_START ${out.path} runs=$total');
    // Repeat the whole matrix rather than each case so thermal throttling
    // spreads evenly over the variants.
    for (var i = 0; i < _repeat; i++) {
      for (final c in selected) {
        for (final v in variants) {
          done++;
          _show('[$done/$total] ${c.scenario}: ${c.name} — $v');
          await Future<void>.delayed(const Duration(milliseconds: 300));
          try {
            final r = await runBenchCase(
              parseBenchArgs(['--variant=$v', '--scenario=${c.scenario}', ...c.args]),
            )
              ..['case'] = c.name
              ..['device'] = Platform.operatingSystem;
            final line = jsonEncode(r);
            out.writeAsStringSync('$line\n', mode: FileMode.append, flush: true);
            print('BENCH $line');
          } on Object catch (e) {
            failed++;
            print('BENCH_FAIL ${c.scenario}: ${c.name} — $v: $e');
            _log.add('FAIL ${c.name} $v: $e');
          }
        }
      }
    }
    print('BENCH_DONE runs=$done failed=$failed');
    _show('Done: $done runs, $failed failed\n${out.path}');
  }

  @override
  Widget build(BuildContext context) => MaterialApp(
        home: Scaffold(
          body: SafeArea(
            child: Padding(
              padding: const EdgeInsets.all(16),
              child: Text([_status, ..._log].join('\n')),
            ),
          ),
        ),
      );
}
