/// Runs a single benchmark case and prints one JSON line with the results.
///
/// Usage:
///   bench --variant=a|b|c|c-worker|c-worker-raw
///         --scenario=stream|flood|echo
///         [--format=json|binary] [--size=1024] [--rate=1000]
///         [--duration=5000] [--count=N] [--tls] [--unicode]
///         [--consume=header|full]
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

import 'dart:convert';
import 'dart:io';

import 'package:ws_benchmark/ws_benchmark.dart';

Future<void> main(List<String> arguments) async {
  final result = await runBenchCase(parseBenchArgs(arguments));
  stdout.writeln(jsonEncode(result));
  exit(0);
}
