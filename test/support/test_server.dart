// Starts the Rust test server (server/, `--ready-json`) for VM tests.
// Build it with `mise run server:build`; tests that need it are skipped
// when the binary is missing.
import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:ws/src/util/sha256.dart';

/// The running `ws_bench_server` and its ports (server/README.md).
final class TestServer {
  TestServer._(this._process, Map<String, Object?> ready)
    : ws = ready['ws']! as int,
      wss = ready['wss']! as int,
      wssBadHost = ready['wss_badhost']! as int,
      wssExpired = ready['wss_expired']! as int,
      ws6 = ready['ws6'] as int?,
      proxy = ready['proxy']! as int,
      proxyAuth = ready['proxy_auth']! as int,
      caPem = ready['ca_pem']! as String,
      wssSpki = _hex(ready['wss_spki']! as String);

  final Process _process;
  final int ws;
  final int wss;
  final int wssBadHost;
  final int wssExpired;
  final int? ws6;
  final int proxy;
  final int proxyAuth;
  final String caPem;

  /// DER SubjectPublicKeyInfo of the `wss` certificate, as rcgen encoded
  /// it (independent of the code under test).
  final Uint8List wssSpki;

  /// The pin of the `wss` certificate: base64 SHA-256 of [wssSpki].
  String get wssPin => base64.encode(sha256(wssSpki));

  static Uint8List _hex(String hex) => Uint8List.fromList([
    for (var i = 0; i < hex.length; i += 2)
      int.parse(hex.substring(i, i + 2), radix: 16),
  ]);

  /// Path of the server binary: `$WS_TEST_SERVER` or the release build.
  static String get binary =>
      Platform.environment['WS_TEST_SERVER'] ??
      'server/target/release/ws_bench_server'
          '${Platform.isWindows ? '.exe' : ''}';

  /// Why tests are skipped, or `false` when the binary exists.
  static Object get skip {
    if (File(binary).existsSync()) return false;
    // On CI a missing server must fail, not silently skip the suite.
    if (Platform.environment['CI'] == 'true') {
      throw StateError('test server not built: $binary');
    }
    return 'build the test server first: mise run server:build ($binary)';
  }

  static Future<TestServer> start() async {
    final process = await Process.start(binary, [
      '--host',
      '127.0.0.1',
      '--port',
      '0',
      '--tls-port',
      '0',
      '--ready-json',
    ]);
    process.stderr.drain<void>().ignore();
    final line = await process.stdout
        .transform(utf8.decoder)
        .transform(const LineSplitter())
        .first
        .timeout(const Duration(seconds: 20));
    return TestServer._(process, jsonDecode(line) as Map<String, Object?>);
  }

  /// Stops the server (it exits when stdin closes).
  Future<void> stop() async {
    await _process.stdin.close();
    await _process.exitCode.timeout(
      const Duration(seconds: 5),
      onTimeout: () {
        _process.kill();
        return -1;
      },
    );
  }

  String url(String path) => 'ws://127.0.0.1:$ws$path';
  String tls(String path, {String host = 'localhost'}) =>
      'wss://$host:$wss$path';

  Future<Object?> _get(String path) async {
    final client = HttpClient();
    try {
      final request = await client.get('127.0.0.1', ws, path);
      final response = await request.close();
      final body = await response.transform(utf8.decoder).join();
      return response.statusCode == 200 ? jsonDecode(body) : null;
    } finally {
      client.close(force: true);
    }
  }

  /// What connection [id] sent, once the server is done with it.
  Future<Map<String, Object?>> log(String id) async {
    for (var i = 0; i < 200; i++) {
      final entry = await _get('/log?id=$id') as Map<String, Object?>?;
      if (entry != null && entry['done'] == true) return entry;
      await Future<void>.delayed(const Duration(milliseconds: 25));
    }
    throw TimeoutException('no finished log entry for $id');
  }

  /// Every CONNECT target the proxies saw.
  Future<List<String>> proxyLog() async {
    final log = (await _get('/proxy-log'))! as Map<String, Object?>;
    return [for (final t in log['tunnels']! as List<Object?>) t! as String];
  }
}
