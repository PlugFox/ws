// Browser runner: runs a suite in this page against the benchmark server and
// posts every result to the page's origin (`bin/web_bench.dart`).
//
// Query: ?suite=compare-web|grid&variants=ws,wsc&repeat=3&tlsPort=9443
import 'dart:convert';
import 'dart:js_interop';

import 'package:web/web.dart' as web;
import 'package:web_socket_channel/web_socket_channel.dart';
import 'package:ws_benchmark/ws_benchmark_web.dart';

const _wasm = bool.fromEnvironment('dart.tool.dart2wasm');

Future<BenchClient> _connect(
  String variant,
  Uri uri, {
  required void Function(Object message) onMessage,
  required String format,
}) =>
    switch (variant) {
      'ws' => LibraryClient.connect(uri, onMessage, format),
      'wsc' => WscClient.connect(WebSocketChannel.connect(uri), onMessage),
      _ => throw ArgumentError.value(variant, 'variant'),
    };

const _platform = BenchPlatform(connect: _connect, probe: false);

Future<void> _post(String path, String body) async {
  await web.window
      .fetch(path.toJS, web.RequestInit(method: 'POST', body: body.toJS))
      .toDart;
}

void _status(String text) {
  web.document.body?.textContent = text;
  _post('/log', text).ignore();
}

Future<void> main() async {
  final q = Uri.parse(web.window.location.href).queryParameters;
  final suite = q['suite'] ?? 'compare-web';
  final variants = (q['variants'] ?? compareWebVariants.join(',')).split(',');
  final repeat = int.parse(q['repeat'] ?? '3');
  final filter = q['filter'] ?? '';
  final tlsPort = q['tlsPort'] ?? '9443';
  final compiler = _wasm ? 'dart2wasm' : 'dart2js';
  final selected = suiteCases(suite)
      .where((c) => filter.isEmpty || '${c.scenario} ${c.name}'.contains(filter))
      .toList();
  final total = repeat * selected.length * variants.length;
  var done = 0, failed = 0;
  for (var i = 0; i < repeat; i++) {
    for (final c in selected) {
      for (final v in variants) {
        done++;
        _status('[$done/$total] $compiler ${c.scenario}: ${c.name} — $v');
        await Future<void>.delayed(const Duration(milliseconds: 300));
        try {
          final r = await runBenchCase(
            parseBenchArgs([
              '--variant=$v',
              '--scenario=${c.scenario}',
              '--tls-port=$tlsPort',
              ...c.args,
            ]),
            _platform,
          )
            ..['case'] = c.name
            ..['run'] = i
            ..['compiler'] = compiler
            ..['device'] = 'chrome';
          await _post('/result', jsonEncode(r));
        } on Object catch (e) {
          failed++;
          _status('FAIL ${c.scenario}: ${c.name} — $v: $e');
        }
      }
    }
  }
  await _post('/done', jsonEncode({'runs': done, 'failed': failed}));
}
