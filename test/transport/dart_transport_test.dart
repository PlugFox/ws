@TestOn('vm')
library;

import 'package:test/test.dart';
import 'package:ws/src/transport/dart/dart_transport.dart';

import '../support/test_server.dart';
import 'conformance.dart';
import 'server_suite.dart';

void main() {
  transportConformance(() => const DartTransport());
  group(
    'dart',
    () => serverConformance(() => const DartTransport()),
    skip: TestServer.skip,
  );
}
