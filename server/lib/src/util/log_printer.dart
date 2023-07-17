import 'dart:convert';
import 'dart:isolate';

import 'package:l/l.dart';

final Converter<Map<String, Object?>, String> _jsonEncoder =
    const JsonEncoder().cast<Map<String, Object?>, String>();

String? Function(
  Object message,
  LogLevel logLevel,
  DateTime dateTime,
) $logPrinter(String environment, int verboseLevel) => switch (environment) {
      'local' => (message, logLevel, dateTime) => verboseLevel < logLevel.level
          ? null
          : '[${logLevel.prefix}] '
              '${dateTime.hour.toString().padLeft(2, '0')}:'
              '${dateTime.minute.toString().padLeft(2, '0')}:'
              '${dateTime.second.toString().padLeft(2, '0')} | '
              '${switch (message.toString()) {
              String msg when msg.length > 100 =>
                '${msg.substring(100 - 4)} ...',
              _ => message
            }}',
      _ => (message, logLevel, dateTime) => verboseLevel < logLevel.level
          ? null
          : _jsonEncoder.convert(<String, Object?>{
              /* 'prefix': logLevel.prefix, */
              /* 'timestamp': dateTime.toUtc().toIso8601String(), */
              'level': logLevel.level,
              'unixtime': dateTime.microsecondsSinceEpoch,
              'message': message.toString(),
            }),
    };

Null Function(
  Object message,
  LogLevel logLevel,
  DateTime dateTime,
) $sendToPort(SendPort port, int verboseLevel) =>
    (message, logLevel, dateTime) {
      if (verboseLevel >= logLevel.level) {
        port.send(LogMessage(
          date: dateTime,
          level: logLevel,
          message: message.toString(),
        ));
      }
      return;
    };

extension LogMessageCallableX on LogMessage {
  /// Call the logger
  void call() => level.when<void>(
        v: () => l.v(message),
        debug: () => l.d(message),
        info: () => l.i(message),
        warning: () => l.w(message),
        error: () => l.e(message),
        vv: () => l.v2(message),
        vvv: () => l.v3(message),
        vvvv: () => l.v4(message),
        vvvvv: () => l.v5(message),
        vvvvvv: () => l.v6(message),
        shout: () => l.s(message),
      );
}
