import 'package:test/test.dart';
import 'package:ws/src/util/logger.dart';

void main() {
  group('Logger', () {
    test('log', () {
      fine('FINE');
      config('CONF');
      info('INFO');
      warning(Exception('WARN'), null, 'REASON');
      severe(Error(), StackTrace.empty, 'ERR!');
    });
  });
}
