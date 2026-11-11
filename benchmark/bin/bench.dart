/// Runs a single benchmark case and prints one JSON line with the results.
///
/// Usage:
///   bench --variant=a|b|c|c-worker|c-worker-raw
///         --scenario=stream|flood|echo
///         [--format=json|binary] [--size=1024] [--rate=1000]
///         [--duration=5000] [--count=N] [--tls] [--unicode]
///         [--host=127.0.0.1] [--port=9090] [--tls-port=9443]
///
/// Scenarios:
///   stream — server pushes `rate` msg/s for `duration` ms; measures main
///            isolate time per message, event-loop stalls and latency.
///   flood  — server pushes `count` messages as fast as possible; measures
///            throughput and process CPU per message.
///   echo   — client sends `rate` msg/s, server echoes; measures round trip.
///   pingpong — `count` sequential request/response pairs (next request only
///            after the response), as in spinify's benchmark; msg/s and RTT.
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:ws_benchmark/ws_benchmark.dart';

Future<void> main(List<String> arguments) async {
  final args = <String, String>{
    for (final a in arguments)
      if (a.startsWith('--'))
        a.substring(2).split('=').first:
            a.contains('=') ? a.substring(a.indexOf('=') + 1) : 'true',
  };
  final variant = Variant.parse(args['variant'] ?? 'a');
  final scenario = args['scenario'] ?? 'stream';
  final format = args['format'] ?? 'json';
  final size = int.parse(args['size'] ?? '1024');
  final rate = int.parse(args['rate'] ?? '1000');
  final duration = int.parse(args['duration'] ?? '5000');
  final tls = args['tls'] == 'true';
  final unicode = args['unicode'] == 'true';
  final host = args['host'] ?? '127.0.0.1';
  final port = int.parse(tls ? args['tls-port'] ?? '9443' : args['port'] ?? '9090');
  final scheme = tls ? 'wss' : 'ws';

  final result = <String, Object?>{
    'variant': variant.id,
    'scenario': scenario,
    'format': format,
    'tls': tls,
    'size': size,
  };

  switch (scenario) {
    case 'stream' || 'flood':
      final flood = scenario == 'flood';
      final count = int.parse(args['count'] ?? '${rate * duration ~/ 1000}');
      final uri = Uri.parse(
        '$scheme://$host:$port/stream?format=$format&size=$size'
        '&rate=${flood ? 0 : rate}&count=$count&unicode=${unicode ? 1 : 0}',
      );
      result.addAll(await _stream(variant, uri, count, probe: !flood));
    case 'echo':
      final uri = Uri.parse('$scheme://$host:$port/echo');
      result.addAll(await _echo(variant, uri, format, size, rate, duration, unicode));
    case 'pingpong':
      final uri = Uri.parse('$scheme://$host:$port/echo');
      final count = int.parse(args['count'] ?? '10000');
      result.addAll(await _pingPong(variant, uri, format, size, count, unicode));
    default:
      throw ArgumentError.value(scenario, 'scenario');
  }
  stdout.writeln(jsonEncode(result));
  exit(0);
}

int _nowMicros() => DateTime.now().microsecondsSinceEpoch;

/// Extracts (seq, ts) from a delivered message.
(int, int) _header(Object message) => switch (message) {
      Map<String, Object?> m => (m['seq']! as int, m['ts']! as int),
      Uint8List b => (
          ByteData.sublistView(b).getUint64(0, Endian.little),
          ByteData.sublistView(b).getUint64(8, Endian.little),
        ),
      _ => throw StateError('Unexpected message ${message.runtimeType}'),
    };

int _messageBytes(Object message) => switch (message) {
      Uint8List b => b.length,
      _ => utf8.encode(jsonEncode(message)).length,
    };

Future<Map<String, Object?>> _stream(
  Variant variant,
  Uri uri,
  int expected, {
  required bool probe,
}) async {
  final mainProbe = MainIsolateProbe();
  final idle = probe ? await mainProbe.calibrate(const Duration(seconds: 1)) : null;

  final latencies = <int>[];
  var received = 0;
  var lastSeq = -1;
  var outOfOrder = 0;
  var messageBytes = 0;
  final sw = Stopwatch();

  final usage0 = ResourceUsage.now();
  final client = await BenchClient.connect(
    variant,
    uri,
    onMessage: (message) {
      final (seq, ts) = _header(message);
      latencies.add(_nowMicros() - ts);
      if (received == 0) {
        sw.start();
        messageBytes = _messageBytes(message);
      }
      if (seq != lastSeq + 1) outOfOrder++;
      lastSeq = seq;
      received++;
    },
  );
  if (probe) mainProbe.start();
  await client.done.timeout(Duration(seconds: 60 + expected ~/ 1000));
  sw.stop();
  final busy = probe ? mainProbe.stop() : null;
  final usage1 = ResourceUsage.now();

  final wallUs = sw.elapsedMicroseconds;
  // Exclude the CPU burnt by the probe's spin loop.
  final cpuUs = usage1.cpuMicros - usage0.cpuMicros - (busy?.spunMicros ?? 0);
  final lat = Percentiles.of(latencies);
  return {
    'received': received,
    'expected': expected,
    'outOfOrder': outOfOrder,
    'closeCode': client.closeCode,
    'msgBytes': messageBytes,
    'wallMs': wallUs / 1000,
    'msgsPerSec': wallUs == 0 ? 0 : received * 1e6 / wallUs,
    'mbPerSec': wallUs == 0 ? 0 : received * messageBytes / wallUs,
    'cpuUsPerMsg': received == 0 ? 0 : cpuUs / received,
    'cpuCores': wallUs == 0 ? 0 : cpuUs / wallUs,
    'rssMb': usage1.maxRssBytes / (1 << 20),
    'latP50Us': lat.p50,
    'latP99Us': lat.p99,
    'latMaxUs': lat.max,
    if (busy != null && idle != null) ...{
      'mainBusyPct': 100 * (busy.busyFraction - idle.busyFraction),
      'mainUsPerMsg': received == 0
          ? 0
          : (busy.busyFraction - idle.busyFraction) * busy.wallMicros / received,
      'stallP99Us': busy.gaps.p99,
      'stallMaxUs': busy.gaps.max,
    },
  };
}

/// JSON payload of roughly [size] bytes, same shape as the server's.
Map<String, Object?> _jsonPayload(int size, bool unicode) {
  final items = <Object?>[];
  var length = 1;
  for (var i = 0; length + 48 < size; i++) {
    final item = <String, Object?>{
      'id': 100000 + i,
      'name': unicode ? 'Товар №$i — ünïcødé ✓' : 'item-$i',
      'price': i * 1.37 + 0.99,
      'qty': i % 17,
      'active': i.isEven,
      'tags': ['alpha', 'beta', 'gamma'],
      'meta': {'score': 1.5, 'rank': i % 100, 'note': null},
    };
    items.add(item);
    length += utf8.encode(jsonEncode(item)).length + 1;
  }
  return {'seq': 0, 'ts': 0, 'items': items};
}

Future<Map<String, Object?>> _echo(
  Variant variant,
  Uri uri,
  String format,
  int size,
  int rate,
  int duration,
  bool unicode,
) async {
  final count = rate * duration ~/ 1000;
  final template = _jsonPayload(size, unicode);
  final binary = Uint8List(size < 16 ? 16 : size);
  final rtts = <int>[];
  final allReceived = Completer<void>();

  final usage0 = ResourceUsage.now();
  final client = await BenchClient.connect(
    variant,
    uri,
    onMessage: (message) {
      final (_, ts) = _header(message);
      rtts.add(_nowMicros() - ts);
      if (rtts.length == count && !allReceived.isCompleted) allReceived.complete();
    },
  );

  final sw = Stopwatch()..start();
  var sent = 0;
  final interval = Duration(microseconds: (1e6 / rate).round().clamp(1, 1000000));
  final timer = Timer.periodic(
    interval < const Duration(milliseconds: 1) ? const Duration(milliseconds: 1) : interval,
    (timer) {
      final due = (sw.elapsedMicroseconds * rate ~/ 1000000 + 1).clamp(0, count);
      for (; sent < due; sent++) {
        if (format == 'binary') {
          ByteData.sublistView(binary)
            ..setUint64(0, sent, Endian.little)
            ..setUint64(8, _nowMicros(), Endian.little);
          client.send(Uint8List.fromList(binary));
        } else {
          client.send({...template, 'seq': sent, 'ts': _nowMicros()});
        }
      }
      if (sent >= count) timer.cancel();
    },
  );
  await allReceived.future.timeout(Duration(milliseconds: duration + 10000), onTimeout: () {});
  timer.cancel();
  final wallUs = sw.elapsedMicroseconds;
  final usage1 = ResourceUsage.now();
  await client.close();

  final rtt = Percentiles.of(rtts);
  final cpuUs = usage1.cpuMicros - usage0.cpuMicros;
  return {
    'received': rtts.length,
    'expected': count,
    'wallMs': wallUs / 1000,
    'cpuUsPerMsg': rtts.isEmpty ? 0 : cpuUs / rtts.length,
    'rssMb': usage1.maxRssBytes / (1 << 20),
    'rttP50Us': rtt.p50,
    'rttP99Us': rtt.p99,
    'rttMaxUs': rtt.max,
  };
}

Future<Map<String, Object?>> _pingPong(
  Variant variant,
  Uri uri,
  String format,
  int size,
  int count,
  bool unicode,
) async {
  final template = _jsonPayload(size, unicode);
  final binary = Uint8List(size < 16 ? 16 : size);
  final messageBytes =
      format == 'binary' ? binary.length : utf8.encode(jsonEncode(template)).length;
  final rtts = <int>[];
  final finished = Completer<void>();
  late final BenchClient client;

  void sendNext(int seq) {
    if (format == 'binary') {
      ByteData.sublistView(binary)
        ..setUint64(0, seq, Endian.little)
        ..setUint64(8, _nowMicros(), Endian.little);
      client.send(Uint8List.fromList(binary));
    } else {
      client.send({...template, 'seq': seq, 'ts': _nowMicros()});
    }
  }

  final usage0 = ResourceUsage.now();
  client = await BenchClient.connect(
    variant,
    uri,
    onMessage: (message) {
      final (seq, ts) = _header(message);
      rtts.add(_nowMicros() - ts);
      if (rtts.length < count) {
        sendNext(seq + 1);
      } else if (!finished.isCompleted) {
        finished.complete();
      }
    },
  );
  final sw = Stopwatch()..start();
  sendNext(0);
  await finished.future.timeout(const Duration(minutes: 2), onTimeout: () {});
  sw.stop();
  final usage1 = ResourceUsage.now();
  await client.close();

  final wallUs = sw.elapsedMicroseconds;
  final rtt = Percentiles.of(rtts);
  final cpuUs = usage1.cpuMicros - usage0.cpuMicros;
  return {
    'received': rtts.length,
    'expected': count,
    'msgBytes': messageBytes,
    'wallMs': wallUs / 1000,
    'msgsPerSec': wallUs == 0 ? 0 : rtts.length * 1e6 / wallUs,
    'mbPerSec': wallUs == 0 ? 0 : rtts.length * messageBytes / wallUs,
    'cpuUsPerMsg': rtts.isEmpty ? 0 : cpuUs / rtts.length,
    'rssMb': usage1.maxRssBytes / (1 << 20),
    'rttP50Us': rtt.p50,
    'rttP99Us': rtt.p99,
    'rttMaxUs': rtt.max,
  };
}
