import 'package:test/test.dart';

import 'client/client_test.dart' as client_test;
import 'platform/platform_test.dart' as platform_test;

void main() {
  group('WS tests', () {
    platform_test.main();
    client_test.main();
  });
}
