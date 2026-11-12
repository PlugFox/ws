@TestOn('vm')
library;

import 'package:test/test.dart';
import 'package:ws/src/transport/dart/dart_transport.dart';

import 'conformance.dart';

void main() => transportConformance(() => const DartTransport());
