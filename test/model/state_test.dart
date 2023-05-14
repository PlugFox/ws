import 'package:test/test.dart';
import 'package:ws/ws.dart';

void main() {
  group('WebSocketClientState tests', () {
    test(r'WebSocketClientState$Connecting properties', () {
      var state = const WebSocketClientState$Connecting();
      expect(state.readyState, equals(WebSocketReadyState.connecting));
      expect(state.hashCode, equals(WebSocketReadyState.connecting.code));
      expect(state, equals(const WebSocketClientState$Connecting()));
      expect(state.toString(), equals('WebSocketClientState.connecting'));
    });

    test(r'WebSocketClientState$Open properties', () {
      var state = const WebSocketClientState$Open(url: 'ws://test');
      expect(state.readyState, equals(WebSocketReadyState.open));
      expect(state.url, equals('ws://test'));
      expect(state.hashCode,
          equals(WebSocketReadyState.open.code ^ state.url.hashCode));
      expect(state, equals(const WebSocketClientState$Open(url: 'ws://test')));
      expect(state.toString(), equals('WebSocketClientState.open'));
    });

    test(r'WebSocketClientState$Closing properties', () {
      var state = const WebSocketClientState$Closing(
          closeCode: 1000, closeReason: 'test');
      expect(state.readyState, equals(WebSocketReadyState.closing));
      expect(state.closeCode, equals(1000));
      expect(state.closeReason, equals('test'));
      expect(
          state,
          equals(const WebSocketClientState$Closing(
              closeCode: 1000, closeReason: 'test')));
      expect(state.toString(), equals('WebSocketClientState.closing'));
    });

    test(r'WebSocketClientState$Closed properties', () {
      var state = const WebSocketClientState$Closed(
          closeCode: 1000, closeReason: 'test');
      expect(state.readyState, equals(WebSocketReadyState.closed));
      expect(state.closeCode, equals(1000));
      expect(state.closeReason, equals('test'));
      expect(
          state,
          equals(const WebSocketClientState$Closed(
              closeCode: 1000, closeReason: 'test')));
      expect(state.toString(), equals('WebSocketClientState.closed'));
    });
  });
}
