@TestOn('vm && browser')

import 'package:test/test.dart';
import 'package:ws/interface.dart';
import 'package:ws/src/platform/platform.dart';

void main() {
  group('WebSocketReadyState', () {
    test('fromCode() returns the correct state for each code', () {
      expect(WebSocketReadyState.fromCode(0), WebSocketReadyState.connecting);
      expect(WebSocketReadyState.fromCode(1), WebSocketReadyState.open);
      expect(WebSocketReadyState.fromCode(2), WebSocketReadyState.closing);
      expect(WebSocketReadyState.fromCode(3), WebSocketReadyState.closed);
    });

    test('fromCode() throws ArgumentError for invalid code', () {
      expect(() => WebSocketReadyState.fromCode(4), throwsArgumentError);
    });
  });

  group('IWebSocketPlatformTransport', () {
    late IWebSocketPlatformTransport transport;

    setUp(() {
      transport = $getWebSocketTransport('ws://example.com');
    });

    test('url is set correctly', () {
      expect(transport.url, 'ws://example.com');
    });

    test('readyState is initially closed', () {
      expect(transport.readyState, WebSocketReadyState.closed);
    });
  });
}
