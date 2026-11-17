/// Runs a suite in Chrome, compiled with dart2js and dart2wasm, against the
/// benchmark server, and writes the results as JSON lines.
///
/// Usage (from `benchmark/`):
///   dart run bin/web_bench.dart [--suite=compare-web|grid] [--repeat=3]
///       [--compilers=dart2js,dart2wasm] [--variants=ws,wsc] [--filter=...]
///       [--server=../server/target/release/ws_bench_server]
///       [--chrome=/Applications/Google Chrome.app/Contents/MacOS/Google Chrome]
///       [--headed] [--out=results/web-<suite>.jsonl]
///
/// Every compiler gets a fresh Chrome profile and process. The page loads
/// from http://127.0.0.1 and connects to `wss://127.0.0.1:9443`; Chrome runs
/// with `--ignore-certificate-errors` because the server's CA is generated
/// at startup. The page posts results to `/result` on its own origin.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';

Future<void> main(List<String> arguments) async {
  final args = <String, String>{
    for (final a in arguments)
      if (a.startsWith('--'))
        a.substring(2).split('=').first:
            a.contains('=') ? a.substring(a.indexOf('=') + 1) : 'true',
  };
  final suite = args['suite'] ?? 'compare-web';
  final repeat = args['repeat'] ?? '3';
  final compilers = (args['compilers'] ?? 'dart2js,dart2wasm').split(',');
  final serverPath = args['server'] ?? '../server/target/release/ws_bench_server';
  final chrome =
      args['chrome'] ?? '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome';
  final out = File(args['out'] ?? 'results/web-$suite.jsonl');
  final query = {
    'suite': suite,
    'repeat': repeat,
    'variants': ?args['variants'],
    'filter': ?args['filter'],
    'tlsPort': '9443',
  };

  final root = Directory('build/web');
  for (final compiler in compilers) {
    await _compile(compiler, Directory('${root.path}/$compiler'));
  }

  final server = await Process.start(serverPath, const ['--port', '9090', '--tls-port', '9443']);
  server.stderr.drain<void>().ignore();
  await _waitForPort(9443);

  out.parent.createSync(recursive: true);
  final sink = out.openWrite();
  late Completer<void> finished;
  final http = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  http.listen((request) async {
    final path = request.uri.path;
    if (request.method == 'POST') {
      final body = await utf8.decodeStream(request);
      switch (path) {
        case '/result':
          sink.writeln(body);
          final r = jsonDecode(body) as Map<String, Object?>;
          stderr.writeln('  ${r['compiler']} ${r['scenario']} ${r['case']} '
              '${r['variant']}: ${r['msgsPerSec'] is num ? (r['msgsPerSec']! as num).round() : '-'} msg/s, '
              '${r['mbPerSec'] is num ? (r['mbPerSec']! as num).toStringAsFixed(1) : '-'} MB/s');
        case '/log':
          stderr.writeln(body);
        case '/done':
          stderr.writeln('done: $body');
          if (!finished.isCompleted) finished.complete();
      }
      request.response.statusCode = 204;
      await request.response.close();
      return;
    }
    final file = File('${root.path}${path == '/' ? '/index.html' : path}');
    if (!file.existsSync()) {
      request.response.statusCode = 404;
      await request.response.close();
      return;
    }
    request.response.headers.contentType = switch (file.path.split('.').last) {
      'html' => ContentType.html,
      'js' || 'mjs' => ContentType('text', 'javascript', charset: 'utf-8'),
      'wasm' => ContentType('application', 'wasm'),
      _ => ContentType.binary,
    };
    await request.response.addStream(file.openRead());
    await request.response.close();
  });

  try {
    for (final compiler in compilers) {
      finished = Completer<void>();
      final profile = Directory.systemTemp.createTempSync('ws-bench-chrome-');
      final url = Uri.http('127.0.0.1:${http.port}', '/$compiler/index.html', query);
      stderr.writeln('$compiler: $url');
      final browser = await Process.start(chrome, [
        if (args['headed'] != 'true') '--headless=new',
        '--ignore-certificate-errors',
        '--user-data-dir=${profile.path}',
        '--no-first-run',
        '--no-default-browser-check',
        '--disable-extensions',
        '--disable-background-timer-throttling',
        '--disable-renderer-backgrounding',
        '--disable-backgrounding-occluded-windows',
        url.toString(),
      ]);
      browser.stdout.drain<void>().ignore();
      browser.stderr.drain<void>().ignore();
      await finished.future.timeout(const Duration(hours: 2));
      browser.kill();
      await browser.exitCode;
      profile.deleteSync(recursive: true);
    }
  } finally {
    await sink.close();
    await http.close(force: true);
    server.kill();
  }
  stdout.writeln('Raw results: ${out.path}');
}

Future<void> _compile(String compiler, Directory dir) async {
  dir.createSync(recursive: true);
  stderr.writeln('Compiling with $compiler...');
  final (cmd, loader) = switch (compiler) {
    'dart2js' => (
        ['compile', 'js', '-O4', 'web/main.dart', '-o', '${dir.path}/main.dart.js'],
        '<script defer src="main.dart.js"></script>',
      ),
    'dart2wasm' => (
        ['compile', 'wasm', '-O2', 'web/main.dart', '-o', '${dir.path}/main.wasm'],
        '<script type="module">\n'
            'import {compileStreaming} from "./main.mjs";\n'
            'const app = await compileStreaming(fetch("main.wasm"));\n'
            'const instance = await app.instantiate({});\n'
            'instance.invokeMain();\n'
            '</script>',
      ),
    _ => throw ArgumentError.value(compiler, 'compiler'),
  };
  final r = await Process.run(Platform.resolvedExecutable, cmd);
  if (r.exitCode != 0) throw StateError('$compiler failed:\n${r.stdout}\n${r.stderr}');
  File('${dir.path}/index.html').writeAsStringSync(
    '<!doctype html><html><head><meta charset="utf-8"><title>ws bench</title>\n'
    '$loader</head><body>loading</body></html>\n',
  );
}

Future<void> _waitForPort(int port) async {
  for (var i = 0; i < 50; i++) {
    try {
      (await Socket.connect('127.0.0.1', port)).destroy();
      return;
    } on SocketException {
      await Future<void>.delayed(const Duration(milliseconds: 100));
    }
  }
  throw StateError('Server did not start on port $port');
}
