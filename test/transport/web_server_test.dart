@TestOn('browser')
library;

import 'package:test/test.dart';

import '../support/browser_server.dart';
import 'server_shared.dart';

/// The shared server suite on the browser's WebSocket (T-3): the Rust
/// test server runs in a hybrid isolate (server_hybrid.dart).
void main() {
  group('web against the Rust test server', () {
    BrowserServerStart? start;
    setUpAll(() async => start = await BrowserServer.start());
    sharedServerConformance(() => start!.server!, (
      protocolPing: false,
      protocolCloseCodes: false,
    ), unavailable: () => start?.skip);
  });
}
