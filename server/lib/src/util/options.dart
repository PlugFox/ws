import 'dart:io' as io;
import 'dart:math' as math;

import 'package:args/args.dart';

({int port, int isolates}) $extractOptions(List<String> arguments) {
  final argResult = (ArgParser()
        ..addOption(
          'port',
          abbr: 'p',
          valueHelp: '8080',
          help: 'Port to listen on.',
          defaultsTo:
              io.Platform.environment['PORT'] ?? String.fromEnvironment('PORT'),
        )
        ..addOption(
          'isolates',
          abbr: 'i',
          valueHelp: '6',
          help: 'Count of isolates to spawn.',
          defaultsTo: io.Platform.environment['ISOLATES'] ??
              String.fromEnvironment('ISOLATES'),
        ))
      .parse(arguments);

  int? getInt(String key) {
    final value = argResult[key];
    if (value is String && value.isNotEmpty) return int.tryParse(value);
    return null;
  }

  return (
    port: getInt('port') ?? 8080,
    isolates: getInt('isolates') ?? math.max(io.Platform.numberOfProcessors, 2),
  );
}
