@TestOn('vm')
library;

import 'package:test/test.dart';
import 'package:ws_native/ws_native.dart';

// The shared conformance suite of package:ws (same tests as the pure-Dart
// transport).
import '../../test/transport/conformance.dart';

void main() {
  setUpAll(() {
    expect(WsNative.isAvailable, isTrue, reason: 'native library not built');
    expect(WsNative.install(), isTrue);
  });

  transportConformance(() => WsNative.library!.transport());
}
