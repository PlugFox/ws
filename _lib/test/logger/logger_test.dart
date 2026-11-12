import 'dart:async';

import 'package:test/test.dart';
import 'package:ws/src/util/logger.dart';

void main() {
  group('Logger', () {
    test('log', () {
      runZoned(
        () {
          fine('FINE');
          config('CONF');
          info('INFO');
          warning(Exception('WARN'), null, 'REASON');
          severe(Error(), StackTrace.empty, 'ERR!');
        },
        zoneValues: <Object?, Object?>{
          #dev.plugfox.ws.debug: true,
        },
      );
    });
  });
}
